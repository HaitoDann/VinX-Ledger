//! Banc n=3 — éprouve le consensus PoA Threshold à **trois validateurs**.
//!
//! Le doc d'état et l'index ADR posent ce banc comme **chemin critique** : rien du
//! backlog consensus (0002 finalité, 0027 jailing, 0031 fork-choice) n'a de valeur
//! tant que le multi-validateur n'est pas éprouvé. Ce test pilote le **vrai code**
//! (`consensus::sign_block`, `consensus::validate_block`, `Chain::advance_finality`,
//! `Chain::add_co_signature`, `ValidatorSet::{leader_at, quorum}`) sur un set de 3
//! et vérifie les propriétés que ces ADR tiennent pour acquises :
//!
//! 1. **Quorum** — n=3 ⇒ `quorum = ⌈2·3/3⌉ = 2`.
//! 2. **Round-robin** — le leader tourne sur les 3 validateurs.
//! 3. **Finalité (liveness)** — un bloc finalise dès 2/3 co-signatures ; tolère **1 panne**.
//! 4. **Sûreté** — sous le quorum (1/3, soit 2 pannes) **aucune finalité** unilatérale.
//! 5. **Finalité prefix-closed + reprise** — un trou non finalisé bloque l'avancée ;
//!    la co-signature tardive (chemin P2P réel) fait rattraper la finalité d'un coup.

use vinx_core::{Block, BlockHeader, BlockSignature, ValidatorSet};
use vinx_crypto::{Address, Hash32, KeyPair};
use vinx_node::chain::Chain;
use vinx_node::config::NodeConfig;
use vinx_node::consensus;
use vinx_node::mempool::Mempool;
use vinx_node::producer::{produce_block, produce_block_backup};
use vinx_node::reorg::{consider_candidate, rebuild_canonical_state, ReorgOutcome};
use vinx_state::{create_genesis_state, GenesisConfig, WorldState};

fn addr(kp: &KeyPair) -> Address {
    Address::from_public_key(&kp.public_key())
}

/// Trois validateurs dans un ordre fixe (round-robin = ordre d'insertion).
fn three_validators() -> (Vec<KeyPair>, Vec<Address>, ValidatorSet) {
    let kps: Vec<KeyPair> = (0..3).map(|_| KeyPair::generate()).collect();
    let addrs: Vec<Address> = kps.iter().map(addr).collect();
    let vs = ValidatorSet::new(addrs.clone());
    (kps, addrs, vs)
}

/// Bloc non signé à `height`, chaîné sur `prev`, proposé par `proposer`.
fn make_block(height: u64, prev: Hash32, proposer: Address) -> Block {
    Block {
        header: BlockHeader {
            height,
            prev_hash: prev,
            timestamp: height,
            validator: proposer,
            tx_count: 0,
            state_root: [0u8; 32],
            base_fee: 0,
            receipts_root: [0u8; 32],
        },
        transactions: vec![],
        signatures: vec![],
        bls_aggregate: None,
        bls_cosigner_pks: vec![],
    }
}

/// Fait co-signer `block` par chaque validateur de `signers` via le vrai chemin
/// `consensus::sign_block` (qui refuse un signataire hors du set).
fn cosign(block: &mut Block, signers: &[&KeyPair], vs: &ValidatorSet) {
    for kp in signers {
        consensus::sign_block(block, kp, vs).expect("signataire dans le set");
    }
}

// ─── 1. Quorum ────────────────────────────────────────────────────────────────

#[test]
fn n3_quorum_is_two() {
    let (_, _, vs) = three_validators();
    assert_eq!(vs.len(), 3);
    assert_eq!(vs.quorum(), 2, "n=3 → quorum = ceil(2*3/3) = 2 (67%)");
}

// ─── 2 & 3. Round-robin + finalité, participation pleine (3/3) ─────────────────

#[test]
fn n3_round_robin_and_finality_full_participation() {
    let (kps, addrs, vs) = three_validators();
    let (mut chain, _g) = Chain::new_with_genesis(addrs[0], 0);
    assert_eq!(chain.finalized_height(), 0, "genèse finale");

    let mut led = [0u32; 3];
    for h in 1..=6u64 {
        let leader = *vs.leader_at(h);
        let leader_idx = vs.leader_idx_at(h);
        led[leader_idx] += 1;

        let mut b = make_block(h, chain.tip_hash(), leader);
        cosign(&mut b, &kps.iter().collect::<Vec<_>>(), &vs); // 3/3 co-signent
        consensus::validate_block(&b, &vs).expect("proposeur enregistré + quorum atteint");
        chain.push(b);
        chain.advance_finality(&vs);

        assert_eq!(
            chain.finalized_height(),
            h,
            "chaque hauteur finalise avec 3/3"
        );
        assert!(chain.is_final(h));
    }
    // Round-robin : sur 6 hauteurs, chacun des 3 a mené exactement 2 fois.
    assert_eq!(led, [2, 2, 2], "rotation round-robin du leader");
}

// ─── 3b. Liveness : tolère 1 validateur hors-ligne (2/3 reste le quorum) ───────

#[test]
fn n3_finality_survives_one_validator_down() {
    let (kps, addrs, vs) = three_validators();
    let (mut chain, _g) = Chain::new_with_genesis(addrs[0], 0);

    const OFFLINE: usize = 2; // le validateur d'index 2 est éteint tout du long

    for h in 1..=6u64 {
        // Si le leader prévu est hors-ligne, un backup propose (slot-skip : tout
        // membre peut proposer, cf. consensus::validate_block).
        let sched = vs.leader_idx_at(h);
        let proposer_idx = if sched == OFFLINE { 0 } else { sched };
        let leader = addrs[proposer_idx];

        let mut b = make_block(h, chain.tip_hash(), leader);
        // Les deux validateurs en ligne co-signent → 2/3 = quorum.
        let present: Vec<&KeyPair> = (0..3).filter(|&i| i != OFFLINE).map(|i| &kps[i]).collect();
        cosign(&mut b, &present, &vs);
        consensus::validate_block(&b, &vs).expect("2/3 atteint le quorum");
        chain.push(b);
        chain.advance_finality(&vs);

        assert_eq!(
            chain.finalized_height(),
            h,
            "finalité avance malgré 1 panne"
        );
    }
}

// ─── 4. Sûreté : sous le quorum (1/3, soit 2 pannes) aucune finalité ──────────

#[test]
fn n3_no_finality_below_quorum() {
    let (kps, addrs, vs) = three_validators();
    let (mut chain, _g) = Chain::new_with_genesis(addrs[0], 0);

    // Un seul validateur signe (1/3) — 2 des 3 sont tombés.
    let leader = *vs.leader_at(1);
    let leader_kp = kps.iter().find(|k| addr(k) == leader).unwrap();
    let mut b = make_block(1, chain.tip_hash(), leader);
    cosign(&mut b, &[leader_kp], &vs);

    assert!(
        consensus::validate_block(&b, &vs).is_err(),
        "1/3 est sous le quorum → bloc non finalisable"
    );
    chain.push(b);
    chain.advance_finality(&vs);
    assert_eq!(
        chain.finalized_height(),
        0,
        "sûreté : pas de finalité unilatérale sous le quorum"
    );
}

// ─── 5. Finalité prefix-closed + reprise via co-signature tardive ─────────────

#[test]
fn n3_prefix_closed_finality_and_recovery() {
    let (kps, addrs, vs) = three_validators();
    let (mut chain, _g) = Chain::new_with_genesis(addrs[0], 0);

    // h1 : 2/3 → final.
    let mut b1 = make_block(1, chain.tip_hash(), *vs.leader_at(1));
    cosign(&mut b1, &[&kps[0], &kps[1]], &vs);
    chain.push(b1);
    chain.advance_finality(&vs);
    assert_eq!(chain.finalized_height(), 1);

    // h2 : 1/3 seulement → PAS final (un co-signataire manque).
    let mut b2 = make_block(2, chain.tip_hash(), *vs.leader_at(2));
    cosign(&mut b2, &[&kps[0]], &vs);
    chain.push(b2);

    // h3 : 2/3 → final en soi, mais un trou non finalisé le précède.
    let mut b3 = make_block(3, chain.tip_hash(), *vs.leader_at(3));
    cosign(&mut b3, &[&kps[0], &kps[1]], &vs);
    chain.push(b3);

    chain.advance_finality(&vs);
    assert_eq!(
        chain.finalized_height(),
        1,
        "prefix-closed : h2 non final bloque la finalisation de h3"
    );

    // Reprise : un 2ᵉ validateur co-signe h2 en retard (chemin P2P réel).
    let hash2 = chain.get_block(2).unwrap().hash();
    let late = BlockSignature {
        validator: addr(&kps[1]),
        pub_key: kps[1].public_key(),
        signature: kps[1].sign(&hash2),
    };
    let now_final = chain.add_co_signature(2, late, &vs);
    assert!(now_final, "h2 atteint 2/3 après la co-signature tardive");

    chain.advance_finality(&vs);
    assert_eq!(
        chain.finalized_height(),
        3,
        "la finalité rattrape tout le préfixe contigu d'un coup"
    );
}

// ─── 6. Fork-choice (ADR 0031) : convergence indépendante de l'ordre d'arrivée ─────

/// Genèse mono-compte + set des 3 validateurs injecté dans l'état (round-robin réel).
fn genesis_state_n3(addrs: &[Address], vs: &ValidatorSet) -> WorldState {
    let mut s = create_genesis_state(&GenesisConfig {
        admin_address: addrs[0],
        validator_address: addrs[0],
        chain_id: vinx_core::CHAIN_ID_DEVNET,
    });
    s.validator_set = vs.clone();
    s
}

#[test]
fn n3_fork_choice_converges_regardless_of_arrival_order() {
    // Deux validateurs produisent chacun un bloc VALIDE à la même hauteur (leader vs backup :
    // le cas de collision que l'ADR 0031 doit résoudre). On vérifie que deux nœuds qui les
    // reçoivent dans des ordres OPPOSÉS convergent vers la même tête et le même état — via le
    // VRAI chemin de production, de fork-choice et de réorg (aucune simulation).
    let (kps, addrs, vs) = three_validators();

    let leader_addr = *vs.leader_at(1);
    let backup_addr = *addrs.iter().find(|a| **a != leader_addr).unwrap();
    let leader_kp = kps.iter().find(|k| addr(k) == leader_addr).unwrap().clone();
    let backup_kp = kps.iter().find(|k| addr(k) == backup_addr).unwrap().clone();
    let cfg_leader = NodeConfig::new(leader_kp);
    let cfg_backup = NodeConfig::new(backup_kp);

    // Bloc A — le leader prévu produit à h=1 (chemin leader normal).
    let block_a = {
        let mut s = genesis_state_n3(&addrs, &vs);
        let (mut c, _g) = Chain::new_with_genesis(addrs[0], 0);
        let mut mp = Mempool::default();
        produce_block(&mut s, &mut c, &mut mp, &cfg_leader, &vs, 100).expect("bloc leader valide")
    };
    // Bloc B — un backup produit à la MÊME hauteur depuis un état pré-bloc identique.
    let block_b = {
        let mut s = genesis_state_n3(&addrs, &vs);
        let (mut c, _g) = Chain::new_with_genesis(addrs[0], 0);
        let mut mp = Mempool::default();
        produce_block_backup(&mut s, &mut c, &mut mp, &cfg_backup, &vs, 100)
            .expect("bloc backup valide")
    };
    assert_ne!(
        block_a.hash(),
        block_b.hash(),
        "deux blocs concurrents distincts"
    );
    assert_eq!(block_a.header.height, 1);
    assert_eq!(block_b.header.height, 1);

    // Tête canonique selon la règle pure (identique sur tout nœud).
    let canonical = consensus::canonical_head(&[block_a.clone(), block_b.clone()], &vs)
        .expect("une tête canonique")
        .hash();

    // Un nœud qui applique `first`, puis reçoit `second` via le chemin de fork-choice réel.
    let run_node = |first: &Block, second: &Block| -> (Hash32, [u8; 32]) {
        let genesis = genesis_state_n3(&addrs, &vs);
        let (mut c, _g) = Chain::new_with_genesis(addrs[0], 0);
        // Applique `first` (état reconstruit depuis la genèse = snapshot finalisé à h=0).
        let mut st = rebuild_canonical_state(&genesis, 0, &c, 1, first).expect("first valide");
        c.push(first.clone());
        let mut vs_mut = vs.clone();
        // Reçoit `second` : MÊME orchestration que le handler P2P.
        let outcome = consider_candidate(&mut c, &mut st, &mut vs_mut, &genesis, 0, second.clone());
        // Selon l'ordre, ce sera Reorged (si `second` est canonique) ou NoChange.
        let expect_reorg = second.hash() == canonical && first.hash() != canonical;
        assert_eq!(
            outcome == ReorgOutcome::Reorged,
            expect_reorg,
            "réorg ssi le second reçu est le bloc canonique"
        );
        (c.tip_hash(), st.compute_state_root())
    };

    let (tip_ab, root_ab) = run_node(&block_a, &block_b); // reçoit A puis B
    let (tip_ba, root_ba) = run_node(&block_b, &block_a); // reçoit B puis A

    assert_eq!(
        tip_ab, tip_ba,
        "convergence : même tête quel que soit l'ordre d'arrivée"
    );
    assert_eq!(root_ab, root_ba, "convergence : même état (state_root)");
    assert_eq!(
        tip_ab, canonical,
        "la tête retenue est bien le bloc canonique"
    );
}
