//! Banc adversarial multi-nœuds — ADR 0080 §2.2.
//!
//! Tous les correctifs de consensus de l'audit de septembre 2026 étaient validés **dans un
//! seul processus**, alors que les propriétés qu'ils protègent sont des propriétés de
//! **réseau**. Ce banc monte plusieurs nœuds, chacun avec son propre `WorldState`, sa propre
//! `Chain` et son propre verrou de vote, les fait communiquer par un bus contrôlable, et
//! vérifie les invariants après chaque opération.
//!
//! **Il pilote le vrai code.** Les nœuds produisent avec `producer::produce_block`, valident
//! avec `consensus::validate_incoming_block` (la primitive que le chemin P2P appelle aussi)
//! et appliquent avec `reorg::replay_block`. Un banc qui réimplémenterait la séquence de
//! validation ne testerait que lui-même.
//!
//! Ce qui est simulé : **uniquement** la couche transport (libp2p). Le verrou de vote est le
//! vrai `Storage::claim_vote`, sur une base redb par nœud — sans quoi l'invariant 2 ne
//! testerait que la sémantique que le banc s'est lui-même donnée. La livraison est explicite,
//! ce qui permet les partitions et le réordonnancement de façon déterministe : pas de test
//! *flaky*, et un échec est reproductible.
//!
//! # Vérification par mutation
//!
//! Un banc qui reste vert quand on casse le correctif qu'il garde ne prouve rien. Chaque
//! correctif a donc été neutralisé et le banc ré-exécuté :
//!
//! | Mutation | Résultat |
//! |---|---|
//! | `verify_proposer_authenticated` neutralisée (VINX-01) | **2 scénarios échouent** ✓ |
//! | `Storage::claim_vote` accorde toujours (VX-RED-003) | **1 scénario échoue** ✓ |
//! | Repli du quorum BLS restauré (VINX-02) | *aucun échec* — voir ci-dessous |
//!
//! Le troisième cas n'est **pas** un trou de couverture mais une **défense en profondeur**
//! constatée : le repli ne se déclenche que sur un `bls_bitmap` vide, or
//! `verify_proposer_authenticated` exige que le bit du proposeur soit positionné et rejette
//! donc le bloc avant tout comptage de finalité. Les deux correctifs sont des barrières
//! indépendantes, chacune suffisante contre l'attaque réseau. La primitive elle-même est
//! couverte directement par `vinx-core` :
//! `block::tests::test_empty_bitmap_does_not_fall_back_to_cosigner_pks`, qui échoue bien
//! sous cette mutation. À retenir en revue : ce banc valide les chemins réseau, pas les
//! primitives isolées — les deux niveaux sont nécessaires.

use std::collections::HashMap;

use vinx_core::{Amount, Block, Transaction, ValidatorSet};
use vinx_crypto::{bls_aggregate, Address, BlsSecretKey, Hash32, KeyPair};
use vinx_node::chain::Chain;
use vinx_node::config::NodeConfig;
use vinx_node::consensus::{self, IncomingBlockCtx};
use vinx_node::mempool::Mempool;
use vinx_node::producer::{produce_block, produce_block_backup};
use vinx_node::reorg;
use vinx_node::storage::Storage;
use vinx_state::{create_genesis_state, GenesisBlsKey, GenesisConfig, WorldState};

const GENESIS_TS: u64 = 1_700_000_000;
/// Cadence utilisée par le banc : au-delà de `SLOT_TIMEOUT_SECS`, un bloc d'un non-leader
/// impute un manquement au leader prévu (ADR 0027). Les scénarios qui ne testent pas le
/// jailing produisent donc à cadence normale.
const BLOCK_TIME: u64 = 12;

// ─── Nœud simulé ──────────────────────────────────────────────────────────────

struct SimNode {
    idx: usize,
    addr: Address,
    kp: KeyPair,
    bls: BlsSecretKey,
    state: WorldState,
    chain: Chain,
    mempool: Mempool,
    config: NodeConfig,
    /// Le **vrai** verrou de vote (ADR 0071), sur une base redb dédiée à ce nœud.
    storage: Storage,
}

impl SimNode {
    fn indexed_pks(&self) -> Vec<Option<[u8; 48]>> {
        self.state.indexed_bls_keys(&self.state.validator_set)
    }

    /// Réclame le droit de co-signer à `height` via le vrai verrou durable. Échec fermé
    /// sur erreur d'E/S, comme le chemin P2P (ADR 0071 §2.4).
    fn claim_vote(&mut self, height: u64, hash: Hash32) -> bool {
        self.storage.claim_vote(height, hash).unwrap_or(false)
    }
}

// ─── Réseau ───────────────────────────────────────────────────────────────────

/// Répertoire temporaire nettoyé au Drop — un par réseau simulé.
struct TmpDir(std::path::PathBuf);
impl TmpDir {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("vinx_harness_{tag}_{nanos}"));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Network {
    /// Conservé pour que les bases redb vivent aussi longtemps que le réseau.
    _dir: TmpDir,
    nodes: Vec<SimNode>,
    /// Groupes de nœuds pouvant communiquer. `None` = réseau complet.
    partition: Option<Vec<Vec<usize>>>,
    /// Journal des votes observés, tous nœuds confondus : (validateur, hauteur) → hashes.
    /// Sert l'invariant 2 — un validateur ne signe jamais deux hashes à une même hauteur.
    vote_log: HashMap<(Address, u64), Vec<Hash32>>,
}

impl Network {
    fn new(n: usize) -> Self {
        Self::named(n, "net")
    }

    fn named(n: usize, tag: &str) -> Self {
        let dir = TmpDir::new(tag);
        let kps: Vec<KeyPair> = (0..n).map(|_| KeyPair::generate()).collect();
        let blss: Vec<BlsSecretKey> = (0..n).map(|_| BlsSecretKey::generate()).collect();
        let addrs: Vec<Address> = kps
            .iter()
            .map(|k| Address::from_public_key(&k.public_key()))
            .collect();
        let vs = ValidatorSet::new(addrs.clone());

        let nodes = (0..n)
            .map(|i| {
                // Genèse **identique** sur chaque nœud : même config, mêmes clés BLS
                // enregistrées, même timestamp — sans quoi les hash de genèse divergent et
                // la sync casse dès la hauteur 1 (ADR 0075 §2.2).
                let mut state = create_genesis_state(&GenesisConfig {
                    chain_id: vinx_core::CHAIN_ID_DEVNET,
                    admin_address: addrs[0],
                    validator_address: addrs[0],
                    validator_bls: Some(GenesisBlsKey {
                        pub_key: blss[0].public_key().0,
                        pop: blss[0].proof_of_possession().0,
                    }),
                });
                state.validator_set = vs.clone();
                // Tous les validateurs bondés avec leur clé BLS enregistrée. Le banc teste
                // les invariants de consensus, pas le chemin d'enrôlement (couvert par
                // audit_regression.rs) — mais le registre doit être peuplé, sinon aucun
                // bloc n'est authentifiable (ADR 0070).
                for (j, a) in addrs.iter().enumerate() {
                    let mut e = vinx_core::ValidatorPoolEntry::new(
                        vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS,
                        GENESIS_TS,
                    );
                    e.status = vinx_core::PoolStatus::Active;
                    e.bls_pub_key = Some(blss[j].public_key().0.to_vec());
                    e.bls_pop = Some(blss[j].proof_of_possession().0.to_vec());
                    state.validator_pool.insert(*a, e);
                }
                let (chain, _g) = Chain::new_with_genesis(addrs[0], GENESIS_TS);
                let config = NodeConfig::new(kps[i].clone()).with_bls_key(blss[i].clone());
                let node_dir = dir.0.join(format!("node{i}"));
                std::fs::create_dir_all(&node_dir).unwrap();
                let storage = Storage::open(&node_dir).expect("stockage du nœud");
                SimNode {
                    idx: i,
                    addr: addrs[i],
                    kp: kps[i].clone(),
                    bls: blss[i].clone(),
                    state,
                    chain,
                    mempool: Mempool::default(),
                    config,
                    storage,
                }
            })
            .collect();

        Self {
            _dir: dir,
            nodes,
            partition: None,
            vote_log: HashMap::new(),
        }
    }

    fn vs(&self) -> ValidatorSet {
        self.nodes[0].state.validator_set.clone()
    }

    /// Tous les nœuds ont une genèse identique — prérequis de tout le reste.
    fn assert_identical_genesis(&self) {
        let g = self.nodes[0].chain.tip_hash();
        for n in &self.nodes {
            assert_eq!(n.chain.tip_hash(), g, "genèse divergente au nœud {}", n.idx);
        }
    }

    fn can_reach(&self, a: usize, b: usize) -> bool {
        match &self.partition {
            None => true,
            Some(groups) => groups.iter().any(|g| g.contains(&a) && g.contains(&b)),
        }
    }

    /// Le nœud `i` produit un bloc au tip, via le vrai chemin de production.
    ///
    /// `produce_block` exige d'être le leader prévu ; un non-leader passe par
    /// `produce_block_backup`, qui est le chemin légitime de slot-skip (ADR 0027/0031).
    /// Le banc essaie le premier puis retombe sur le second, ce qui reproduit exactement
    /// ce que fait un nœud réel selon qu'il est leader ou remplaçant.
    fn produce(&mut self, i: usize, ts: u64) -> Result<Block, String> {
        let vs = self.vs();
        let n = &mut self.nodes[i];
        let block = match produce_block(
            &mut n.state,
            &mut n.chain,
            &mut n.mempool,
            &n.config,
            &vs,
            ts,
        ) {
            Ok(b) => b,
            Err(_) => produce_block_backup(
                &mut n.state,
                &mut n.chain,
                &mut n.mempool,
                &n.config,
                &vs,
                ts,
            )
            .map_err(|e| e.to_string())?,
        };
        // Produire est un vote (ADR 0071 §2.3) : le producteur co-signe son propre bloc.
        let h = block.header.height;
        let hash = block.hash();
        assert!(
            n.claim_vote(h, hash),
            "le producteur doit pouvoir réclamer son propre vote"
        );
        let addr = n.addr;
        self.vote_log.entry((addr, h)).or_default().push(hash);
        let pks = self.nodes[i].indexed_pks();
        self.nodes[i].chain.advance_finality(&vs, &pks);
        Ok(block)
    }

    /// Livre `block` au nœud `to` par le **vrai** chemin de validation puis d'application.
    /// Retourne `Ok(())` si le bloc a été accepté et appliqué.
    fn deliver(&mut self, block: &Block, to: usize, now: u64) -> Result<(), String> {
        let vs = self.vs();
        let n = &mut self.nodes[to];
        let ctx = IncomingBlockCtx {
            tip_hash: n.chain.tip_hash(),
            tip_timestamp: n.chain.tip_timestamp(),
            now,
        };
        let pks = n.state.indexed_bls_keys(&n.state.validator_set);
        // 1. En-tête + autorité + signatures de tx — primitive partagée avec le chemin P2P.
        consensus::validate_incoming_block(block, &ctx, &vs, &pks).map_err(|e| e.to_string())?;
        // 2. Transition d'état avec rollback, et contrôle du state_root.
        let snapshot = n.state.clone();
        if let Err(e) = reorg::replay_block(&mut n.state, &n.chain, block) {
            n.state = snapshot;
            return Err(e);
        }
        n.chain.push(block.clone());
        let pks = n.state.indexed_bls_keys(&n.state.validator_set);
        n.chain.advance_finality(&vs, &pks);
        Ok(())
    }

    /// Diffuse un bloc à tous les nœuds joignables depuis `from`.
    fn broadcast(&mut self, block: &Block, from: usize, now: u64) -> Vec<(usize, bool)> {
        let targets: Vec<usize> = (0..self.nodes.len())
            .filter(|&j| j != from && self.can_reach(from, j))
            .collect();
        targets
            .into_iter()
            .map(|j| (j, self.deliver(block, j, now).is_ok()))
            .collect()
    }

    /// Le nœud `i` co-signe `block` — sous verrou de vote (ADR 0071). Retourne `false`
    /// lorsque le verrou refuse, c'est-à-dire lorsqu'une équivocation est empêchée.
    fn cosign(&mut self, i: usize, block: &mut Block) -> bool {
        let hash = block.hash();
        let h = block.header.height;
        if !self.nodes[i].claim_vote(h, hash) {
            return false;
        }
        let addr = self.nodes[i].addr;
        self.vote_log.entry((addr, h)).or_default().push(hash);
        let vs = self.vs();
        let idx = vs.index_of(&addr).expect("validateur du set");
        consensus::sign_block_bls(block, &self.nodes[i].bls, idx).expect("co-signature");
        true
    }

    // ─── Invariants (ADR 0080 §2.2) ───────────────────────────────────────────

    /// INVARIANT 1 — deux blocs finalisés à la même hauteur ne peuvent jamais avoir des
    /// hashes différents. C'est la propriété de sûreté fondamentale.
    fn assert_no_conflicting_finality(&self) {
        let mut seen: HashMap<u64, (Hash32, usize)> = HashMap::new();
        for n in &self.nodes {
            for h in 1..=n.chain.finalized_height() {
                if let Some(b) = n.chain.get_block(h) {
                    match seen.get(&h) {
                        Some((hash, other)) => assert_eq!(
                            *hash,
                            b.hash(),
                            "INVARIANT 1 VIOLÉ : hauteur {h} finalisée avec deux hashes \
                             différents (nœuds {other} et {})",
                            n.idx
                        ),
                        None => {
                            seen.insert(h, (b.hash(), n.idx));
                        }
                    }
                }
            }
        }
    }

    /// INVARIANT 2 — un validateur ne signe jamais deux hashes différents à la même hauteur.
    fn assert_no_equivocation(&self) {
        for ((addr, h), hashes) in &self.vote_log {
            let mut uniq: Vec<&Hash32> = hashes.iter().collect();
            uniq.sort_unstable();
            uniq.dedup();
            assert_eq!(
                uniq.len(),
                1,
                "INVARIANT 2 VIOLÉ : le validateur {addr} a signé {} hashes distincts à la \
                 hauteur {h}",
                uniq.len()
            );
        }
    }

    /// INVARIANT 3 — un bloc ne devient jamais final sans signatures de clés de validateurs
    /// réellement enregistrées.
    fn assert_finality_backed_by_registry(&self) {
        for n in &self.nodes {
            let pks = n.state.indexed_bls_keys(&n.state.validator_set);
            let quorum = n.state.validator_set.quorum();
            for h in 1..=n.chain.finalized_height() {
                let Some(b) = n.chain.get_block(h) else {
                    continue;
                };
                let count = b.bls_signer_count_from_bitmap(&pks).unwrap_or_else(|e| {
                    panic!(
                        "INVARIANT 3 VIOLÉ : bloc final h={h} au nœud {} dont l'agrégat ne \
                         vérifie pas contre le registre ({e})",
                        n.idx
                    )
                });
                assert!(
                    count >= quorum,
                    "INVARIANT 3 VIOLÉ : bloc final h={h} au nœud {} avec {count} \
                     co-signataires enregistrés < quorum {quorum}",
                    n.idx
                );
            }
        }
    }

    /// INVARIANT 4 — tous les nœuds s'accordent sur la validité cryptographique des
    /// transactions d'un bloc. Une divergence ici est un fork induit à distance (VINX-03).
    fn assert_tx_validity_agreement(&self, block: &Block) {
        let verdicts: Vec<bool> = self
            .nodes
            .iter()
            .map(|_| consensus::verify_block_tx_signatures(&block.transactions))
            .collect();
        assert!(
            verdicts.windows(2).all(|w| w[0] == w[1]),
            "INVARIANT 4 VIOLÉ : désaccord entre nœuds sur la validité crypto d'un bloc"
        );
    }

    /// INVARIANT 5 — le fork-choice ne peut jamais remplacer un bloc finalisé.
    fn assert_finalized_prefix_stable(&self, before: &HashMap<usize, Vec<(u64, Hash32)>>) {
        for n in &self.nodes {
            let Some(prev) = before.get(&n.idx) else {
                continue;
            };
            for (h, hash) in prev {
                let now = n.chain.get_block(*h).map(|b| b.hash());
                assert_eq!(
                    now,
                    Some(*hash),
                    "INVARIANT 5 VIOLÉ : le bloc finalisé h={h} du nœud {} a changé",
                    n.idx
                );
            }
        }
    }

    fn finalized_snapshot(&self) -> HashMap<usize, Vec<(u64, Hash32)>> {
        self.nodes
            .iter()
            .map(|n| {
                let v = (1..=n.chain.finalized_height())
                    .filter_map(|h| n.chain.get_block(h).map(|b| (h, b.hash())))
                    .collect();
                (n.idx, v)
            })
            .collect()
    }

    /// Vérifie tous les invariants d'état (1, 2, 3) d'un coup.
    fn assert_all(&self) {
        self.assert_no_conflicting_finality();
        self.assert_no_equivocation();
        self.assert_finality_backed_by_registry();
    }
}

// ─── Outillage attaquant ──────────────────────────────────────────────────────

/// Fabrique un quorum BLS avec des clés que l'attaquant génère lui-même, bitmap vide —
/// l'attaque VINX-02 exactement.
fn forge_quorum(block: &mut Block, n: usize) {
    let sks: Vec<BlsSecretKey> = (0..n).map(|_| BlsSecretKey::generate()).collect();
    let msg = block.header.hash();
    let sigs: Vec<_> = sks.iter().map(|sk| sk.sign(&msg)).collect();
    let agg = bls_aggregate(&sigs).expect("agrégat");
    block.bls_aggregate = Some(agg.0.to_vec());
    block.bls_cosigner_pks = sks.iter().map(|sk| sk.public_key().0.to_vec()).collect();
    block.bls_bitmap = vec![];
}

// ─── Scénarios ────────────────────────────────────────────────────────────────

/// Chemin nominal : n=4, production en round-robin, co-signature au quorum, finalité.
/// Sert de contrôle — si celui-ci échoue, les scénarios d'attaque ne prouvent rien.
#[test]
fn nominal_four_validators_reach_finality() {
    let mut net = Network::named(4, "nominal");
    net.assert_identical_genesis();
    let vs = net.vs();
    assert_eq!(vs.quorum(), 3, "n=4 → quorum = ⌈8/3⌉ = 3");

    for h in 1..=6u64 {
        let leader = vs.leader_idx_at(h);
        let ts = GENESIS_TS + h * BLOCK_TIME;
        let mut block = net.produce(leader, ts).expect("le leader produit");

        // Deux autres validateurs co-signent → 3/4 = quorum.
        let others: Vec<usize> = (0..4).filter(|&i| i != leader).take(2).collect();
        for i in others {
            assert!(net.cosign(i, &mut block), "co-signature honnête accordée");
        }

        // Le producteur ré-enregistre le bloc co-signé, puis diffuse.
        net.nodes[leader].chain.set_block_bls(
            h,
            block.bls_aggregate.clone().unwrap(),
            block.bls_cosigner_pks.clone(),
            block.bls_bitmap.clone(),
        );
        let results = net.broadcast(&block, leader, ts);
        for (j, ok) in &results {
            assert!(ok, "le nœud {j} doit accepter un bloc honnête à h={h}");
        }
        net.assert_tx_validity_agreement(&block);
        net.assert_all();
    }

    for n in &net.nodes {
        assert_eq!(n.chain.tip_height(), 6, "nœud {} au tip 6", n.idx);
        assert!(
            n.chain.finalized_height() >= 5,
            "nœud {} doit avoir finalisé le préfixe (obtenu {})",
            n.idx,
            n.chain.finalized_height()
        );
    }
}

/// INVARIANT 2 — deux blocs concurrents à la même hauteur ne doivent pas faire co-signer
/// deux fois un validateur honnête. C'est VX-RED-003 au niveau réseau : la détection ne
/// suffisait pas, seul le verrou empêche.
#[test]
fn conflicting_blocks_cannot_induce_equivocation() {
    let mut net = Network::named(4, "equiv");
    let vs = net.vs();
    let ts = GENESIS_TS + BLOCK_TIME;

    // Deux producteurs différents bâtissent chacun un bloc valide à h=1 sur le même parent.
    let block_a = net.produce(0, ts).expect("A produit");
    let block_b = net.produce(1, ts + 1).expect("B produit");
    assert_ne!(
        block_a.hash(),
        block_b.hash(),
        "deux blocs concurrents distincts"
    );

    // Un validateur honnête (2) reçoit A puis B et est sollicité pour co-signer les deux.
    let mut a = block_a.clone();
    let mut b = block_b.clone();
    assert!(net.cosign(2, &mut a), "première co-signature accordée");
    assert!(
        !net.cosign(2, &mut b),
        "INVARIANT 2 : la seconde co-signature à la même hauteur doit être REFUSÉE"
    );

    // Et le refus tient quel que soit l'ordre d'arrivée, pour un autre validateur.
    let mut b2 = block_b.clone();
    let mut a2 = block_a.clone();
    assert!(net.cosign(3, &mut b2), "3 co-signe B d'abord");
    assert!(!net.cosign(3, &mut a2), "3 refuse A ensuite");

    net.assert_no_equivocation();
    let _ = vs;
}

/// INVARIANT 3 — un non-validateur ne peut pas faire finaliser un bloc, ni avec un quorum
/// forgé (VINX-02) ni en usurpant un proposeur (VINX-01).
#[test]
fn outsider_cannot_get_a_block_accepted_or_finalized() {
    let mut net = Network::named(4, "outsider");
    let vs = net.vs();
    let ts = GENESIS_TS + BLOCK_TIME;
    let honest_validator = *vs.validators().first().unwrap();

    // L'attaquant n'a aucune clé de validateur. Il bâtit un bloc cohérent côté état en
    // partant d'un nœud honnête, puis se déclare proposeur.
    let mut forged = net.produce(0, ts).expect("bloc de base");
    net.nodes[0].chain.reorg_replace(1, forged.clone()); // remet le nœud 0 à plat
    forged.header.validator = honest_validator;
    forged.bls_aggregate = None;
    forged.bls_cosigner_pks = vec![];
    forged.bls_bitmap = vec![];

    for j in 1..4 {
        assert!(
            net.deliver(&forged, j, ts).is_err(),
            "INVARIANT 3 : un bloc sans co-signature du proposeur doit être refusé (nœud {j})"
        );
    }

    // Même bloc, mais rembourré d'un quorum de clés que l'attaquant a générées.
    let mut forged_q = forged.clone();
    forge_quorum(&mut forged_q, vs.quorum());
    for j in 1..4 {
        assert!(
            net.deliver(&forged_q, j, ts).is_err(),
            "INVARIANT 3 : un quorum de clés non enregistrées doit être refusé (nœud {j})"
        );
    }

    net.assert_all();
    for n in &net.nodes[1..] {
        assert_eq!(
            n.chain.tip_height(),
            0,
            "le nœud {} n'a rien appliqué",
            n.idx
        );
    }
}

/// INVARIANT 6 — une entrée P2P malformée ne modifie jamais l'état de consensus.
#[test]
fn malformed_input_never_changes_consensus_state() {
    let mut net = Network::named(4, "malformed");
    let ts = GENESIS_TS + BLOCK_TIME;
    let good = net.produce(0, ts).expect("bloc honnête");

    let before: Vec<(u64, Hash32, Hash32)> = net.nodes[1..]
        .iter()
        .map(|n| {
            let mut st = n.state.clone();
            (
                n.chain.tip_height(),
                n.chain.tip_hash(),
                st.compute_state_root(),
            )
        })
        .collect();

    // Une batterie de blocs malformés, chacun dérivé d'un bloc par ailleurs valide.
    let mut cases: Vec<(&str, Block)> = Vec::new();
    let mut b = good.clone();
    b.header.state_root = [0xAA; 32];
    cases.push(("state_root falsifié", b));
    let mut b = good.clone();
    b.header.prev_hash = [0xBB; 32];
    cases.push(("prev_hash cassé", b));
    let mut b = good.clone();
    b.header.timestamp = GENESIS_TS - 1;
    cases.push(("timestamp non monotone", b));
    let mut b = good.clone();
    b.header.timestamp = ts + 10 * 365 * 24 * 3600;
    cases.push(("timestamp dans 10 ans", b));
    let mut b = good.clone();
    b.bls_bitmap = vec![0xFF; 64]; // bits bien au-delà du set
    cases.push(("bitmap surdimensionné", b));
    let mut b = good.clone();
    b.bls_aggregate = Some(vec![0u8; 96]);
    cases.push(("agrégat nul", b));
    let mut b = good.clone();
    b.bls_aggregate = Some(vec![0u8; 5]); // longueur invalide
    cases.push(("agrégat de longueur invalide", b));
    let mut b = good.clone();
    b.header.tx_count = u32::MAX;
    cases.push(("tx_count incohérent", b));

    for (name, bad) in cases {
        for j in 1..4 {
            let _ = net.deliver(&bad, j, ts); // peut échouer : c'est le but
            let mut st = net.nodes[j].state.clone();
            let (h0, hash0, root0) = before[j - 1];
            assert_eq!(
                net.nodes[j].chain.tip_height(),
                h0,
                "[{name}] tip du nœud {j}"
            );
            assert_eq!(
                net.nodes[j].chain.tip_hash(),
                hash0,
                "[{name}] hash du nœud {j}"
            );
            assert_eq!(
                st.compute_state_root(),
                root0,
                "INVARIANT 6 VIOLÉ : [{name}] a modifié l'état du nœud {j}"
            );
        }
    }
    net.assert_all();
}

/// INVARIANT 7 — un producteur ne peut pas valider localement ce qu'un validateur honnête
/// rejette. C'est la divergence VINX-03 / VX-RED-006, au niveau réseau : le producteur
/// appliquait avec `apply_transaction_trusted` une transaction que ses pairs refusaient.
#[test]
fn producer_cannot_commit_what_honest_peers_reject() {
    let mut net = Network::named(4, "producer");
    let ts = GENESIS_TS + BLOCK_TIME;

    // Un compte financé pour l'attaquant, une victime, sur **tous** les nœuds.
    let attacker = KeyPair::generate();
    let attacker_addr = Address::from_public_key(&attacker.public_key());
    let victim = Address::from_public_key(&KeyPair::generate().public_key());
    let victim_balance = Amount::from_vinx(5_000);
    for n in net.nodes.iter_mut() {
        n.state
            .credit_for_test(attacker_addr, Amount::from_vinx(1_000));
        n.state.credit_for_test(victim, victim_balance);
    }

    // La transaction de vol par sponsor : frais = solde entier de la victime, aucune
    // signature de sponsor.
    let mut theft = Transaction::new_transfer(
        &attacker,
        attacker_addr,
        Amount::from_atoms(0),
        victim_balance,
        0,
    );
    theft.sponsor = Some(victim);
    theft.sponsor_pub_key = None;
    theft.sponsor_signature = None;
    theft.sign(&attacker);

    // Le mempool du producteur doit la refuser à l'admission — c'est le correctif VINX-03.
    assert!(
        net.nodes[0].state.admission_check(&theft).is_err(),
        "le producteur doit refuser la transaction à l'admission"
    );

    // Même si elle était incluse de force dans un bloc, les pairs la rejettent — et
    // l'invariant 4 exige qu'ils soient tous d'accord.
    let mut block = net.produce(0, ts).expect("bloc");
    block.transactions.push(theft);
    block.header.tx_count = block.transactions.len() as u32;
    assert!(
        !consensus::verify_block_tx_signatures(&block.transactions),
        "un bloc portant la transaction volée est cryptographiquement invalide"
    );
    net.assert_tx_validity_agreement(&block);
    for j in 1..4 {
        assert!(
            net.deliver(&block, j, ts).is_err(),
            "INVARIANT 7 : le nœud {j} doit rejeter le bloc du producteur"
        );
    }

    // Et la victime n'a rien perdu, nulle part.
    for n in &net.nodes {
        assert_eq!(
            n.state.account_balance(&victim),
            victim_balance,
            "le solde de la victime doit être intact au nœud {}",
            n.idx
        );
    }
    net.assert_all();
}

/// INVARIANTS 1 & 5 sous **partition réseau** : deux groupes avancent séparément, puis la
/// partition guérit. Aucun préfixe finalisé ne doit changer, et aucune hauteur ne doit être
/// finalisée avec deux hashes différents.
#[test]
fn network_partition_never_produces_conflicting_finality() {
    let mut net = Network::named(7, "partition");
    let vs = net.vs();
    assert_eq!(vs.quorum(), 5, "n=7 → quorum 5");

    // Bloc 1 honnête, finalisé par tout le monde avant la partition.
    let ts1 = GENESIS_TS + BLOCK_TIME;
    let leader = vs.leader_idx_at(1);
    let mut b1 = net.produce(leader, ts1).expect("h=1");
    for i in (0..7).filter(|&i| i != leader).take(4) {
        assert!(net.cosign(i, &mut b1));
    }
    net.nodes[leader].chain.set_block_bls(
        1,
        b1.bls_aggregate.clone().unwrap(),
        b1.bls_cosigner_pks.clone(),
        b1.bls_bitmap.clone(),
    );
    for (j, ok) in net.broadcast(&b1, leader, ts1) {
        assert!(ok, "h=1 accepté par le nœud {j}");
    }
    net.assert_all();
    let finalized_before = net.finalized_snapshot();

    // Partition 4 │ 3 — aucun groupe ne peut atteindre le quorum de 5 à lui seul.
    net.partition = Some(vec![vec![0, 1, 2, 3], vec![4, 5, 6]]);

    let ts2 = ts1 + BLOCK_TIME;
    // Chaque groupe tente d'avancer.
    let mut ba = net.produce(0, ts2).expect("groupe A produit");
    for i in [1usize, 2, 3] {
        net.cosign(i, &mut ba);
    }
    let _ = net.broadcast(&ba, 0, ts2);

    let mut bb = net.produce(4, ts2 + 1).expect("groupe B produit");
    for i in [5usize, 6] {
        net.cosign(i, &mut bb);
    }
    let _ = net.broadcast(&bb, 4, ts2 + 1);

    // Ni l'un ni l'autre n'atteint le quorum de 5 : aucune finalité ne doit avancer à h=2.
    net.assert_all();
    net.assert_finalized_prefix_stable(&finalized_before);
    for n in &net.nodes {
        assert!(
            n.chain.finalized_height() <= 1,
            "INVARIANT 1 : aucun groupe minoritaire ne doit finaliser (nœud {} à {})",
            n.idx,
            n.chain.finalized_height()
        );
    }

    // Guérison : les invariants tiennent toujours, et le préfixe finalisé n'a pas bougé.
    net.partition = None;
    net.assert_all();
    net.assert_finalized_prefix_stable(&finalized_before);
}
