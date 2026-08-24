//! Banc n=3 — éprouve le consensus PoA Threshold à **trois validateurs**.
//!
//! Le doc d'état et l'index ADR posent ce banc comme **chemin critique** : rien du
//! backlog consensus (0002 finalité, 0027 jailing, 0031 fork-choice) n'a de valeur
//! tant que le multi-validateur n'est pas éprouvé. Ce test pilote le **vrai code**
//! (`consensus::sign_block_bls`, `consensus::validate_block`, `Chain::advance_finality`,
//! `ValidatorSet::{leader_at, quorum}`) sur un set de 3
//! et vérifie les propriétés que ces ADR tiennent pour acquises :
//!
//! 1. **Quorum** — n=3 ⇒ `quorum = ⌈2·3/3⌉ = 2`.
//! 2. **Round-robin** — le leader tourne sur les 3 validateurs.
//! 3. **Finalité (liveness)** — un bloc finalise dès 2/3 co-signatures ; tolère **1 panne**.
//! 4. **Sûreté** — sous le quorum (1/3, soit 2 pannes) **aucune finalité** unilatérale.
//! 5. **Finalité prefix-closed + reprise** — un trou non finalisé bloque l'avancée ;
//!    la co-signature tardive (chemin P2P réel) fait rattraper la finalité d'un coup.

use vinx_core::{Block, BlockHeader, ValidatorSet};
use vinx_crypto::{Address, BlsSecretKey, Hash32, KeyPair};
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
/// Retourne (kps Ed25519, bls_sks BLS, adresses, ValidatorSet).
fn three_validators() -> (Vec<KeyPair>, Vec<BlsSecretKey>, Vec<Address>, ValidatorSet) {
    let kps: Vec<KeyPair> = (0..3).map(|_| KeyPair::generate()).collect();
    let bls_sks: Vec<BlsSecretKey> = (0..3).map(|_| BlsSecretKey::generate()).collect();
    let addrs: Vec<Address> = kps.iter().map(addr).collect();
    let vs = ValidatorSet::new(addrs.clone());
    (kps, bls_sks, addrs, vs)
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
        bls_aggregate: None,
        bls_cosigner_pks: vec![],
        bls_bitmap: vec![],
    }
}

// ─── 1. Quorum ────────────────────────────────────────────────────────────────

#[test]
fn n3_quorum_is_two() {
    let (_, _, _, vs) = three_validators();
    assert_eq!(vs.len(), 3);
    assert_eq!(vs.quorum(), 2, "n=3 → quorum = ceil(2*3/3) = 2 (67%)");
}

// ─── 2 & 3. Round-robin + finalité, participation pleine (3/3) ─────────────────

#[test]
fn n3_round_robin_and_finality_full_participation() {
    let (_, bls_sks, addrs, vs) = three_validators();
    let (mut chain, _g) = Chain::new_with_genesis(addrs[0], 0);
    assert_eq!(chain.finalized_height(), 0, "genèse finale");

    let mut led = [0u32; 3];
    for h in 1..=6u64 {
        let leader = *vs.leader_at(h);
        let leader_idx = vs.leader_idx_at(h);
        led[leader_idx] += 1;

        let mut b = make_block(h, chain.tip_hash(), leader);
        bls_cosign(&mut b, &[0, 1, 2], &bls_sks, &addrs, &vs); // 3/3 co-signent
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
    let (_, bls_sks, addrs, vs) = three_validators();
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
        let present: Vec<usize> = (0..3).filter(|&i| i != OFFLINE).collect();
        bls_cosign(&mut b, &present, &bls_sks, &addrs, &vs);
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
    let (_, bls_sks, addrs, vs) = three_validators();
    let (mut chain, _g) = Chain::new_with_genesis(addrs[0], 0);

    // Un seul validateur signe (1/3) — 2 des 3 sont tombés.
    let leader_idx = vs.leader_idx_at(1);
    let leader = *vs.leader_at(1);
    let mut b = make_block(1, chain.tip_hash(), leader);
    bls_cosign(&mut b, &[leader_idx], &bls_sks, &addrs, &vs);

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
    let (_, bls_sks, addrs, vs) = three_validators();
    let (mut chain, _g) = Chain::new_with_genesis(addrs[0], 0);

    // h1 : 2/3 → final.
    let mut b1 = make_block(1, chain.tip_hash(), *vs.leader_at(1));
    bls_cosign(&mut b1, &[0, 1], &bls_sks, &addrs, &vs);
    chain.push(b1);
    chain.advance_finality(&vs);
    assert_eq!(chain.finalized_height(), 1);

    // h2 : 1/3 seulement → PAS final (un co-signataire manque).
    let mut b2 = make_block(2, chain.tip_hash(), *vs.leader_at(2));
    bls_cosign(&mut b2, &[0], &bls_sks, &addrs, &vs);
    chain.push(b2);

    // h3 : 2/3 → final en soi, mais un trou non finalisé le précède.
    let mut b3 = make_block(3, chain.tip_hash(), *vs.leader_at(3));
    bls_cosign(&mut b3, &[0, 1], &bls_sks, &addrs, &vs);
    chain.push(b3);

    chain.advance_finality(&vs);
    assert_eq!(
        chain.finalized_height(),
        1,
        "prefix-closed : h2 non final bloque la finalisation de h3"
    );

    // Reprise : re-signer h2 avec les deux validateurs (chemin P2P réel : le nœud
    // reçoit la co-signature tardive et reconstruit l'agrégat BLS complet).
    let b2_stored = chain.get_block(2).unwrap();
    let mut b2_updated = make_block(2, b2_stored.header.prev_hash, b2_stored.header.validator);
    bls_cosign(&mut b2_updated, &[0, 1], &bls_sks, &addrs, &vs);
    chain.set_block_bls(
        2,
        b2_updated.bls_aggregate.unwrap(),
        b2_updated.bls_cosigner_pks,
        b2_updated.bls_bitmap,
    );

    let now_final = chain
        .get_block(2)
        .unwrap()
        .bls_signer_count()
        .is_ok_and(|c| c >= vs.quorum());
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
    let (kps, _bls_sks, addrs, vs) = three_validators();

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

// ─── ADR 0029 Phase 1 — Agrégation BLS + bitmap ───────────────────────────────

/// Builds an `indexed_bls_pks` registry from a parallel list of BLS secret keys
/// and validator addresses. Index `i` corresponds to `ValidatorSet::index_of(addrs[i])`.
fn indexed_pks(bls_sks: &[BlsSecretKey], addrs: &[Address], vs: &ValidatorSet) -> Vec<Option<[u8; 48]>> {
    let mut pks = vec![None; vs.len()];
    for (i, bls_sk) in bls_sks.iter().enumerate() {
        if let Some(idx) = vs.index_of(&addrs[i]) {
            pks[idx] = Some(bls_sk.public_key().0);
        }
    }
    pks
}

/// Aggregates BLS signatures from a subset of validators onto `block`, then sets
/// the corresponding bitmap bits. Returns the `indexed_bls_pks` registry.
fn bls_cosign(
    block: &mut Block,
    signer_indices: &[usize],
    bls_sks: &[BlsSecretKey],
    addrs: &[Address],
    vs: &ValidatorSet,
) -> Vec<Option<[u8; 48]>> {
    for &i in signer_indices {
        vinx_node::consensus::sign_block_bls(block, &bls_sks[i])
            .expect("BLS sign must succeed");
        if let Some(vidx) = vs.index_of(&addrs[i]) {
            block.set_bls_bitmap_bit(vidx);
        }
    }
    indexed_pks(bls_sks, addrs, vs)
}

// ─── 7. n=1 : bloc produit avec BLS, bitmap bit 0 set ─────────────────────────

#[test]
fn n1_bls_produces_block_bitmap_bit_set() {
    let kp = KeyPair::generate();
    let addr = addr(&kp);
    let bls_sk = BlsSecretKey::generate();
    let vs = ValidatorSet::new(vec![addr]);

    let (chain, _) = Chain::new_with_genesis(addr, 0);
    let mut block = make_block(1, chain.tip_hash(), addr);

    let registry = bls_cosign(&mut block, &[0], &[bls_sk], &[addr], &vs);

    assert!(block.bls_aggregate.is_some(), "aggregate must be set");
    assert!(block.bls_bitmap_has(0), "bit 0 set for the sole validator");
    assert_eq!(block.bls_bitmap_popcount(), 1);

    let count = block
        .bls_signer_count_from_bitmap(&registry)
        .expect("bitmap verification must pass");
    assert_eq!(count, 1, "one signer counted from bitmap");
}

// ─── 8. n=3 : quorum BLS 2/3 atteint sans erreur crypto ──────────────────────

#[test]
fn n3_bls_quorum_two_signers_no_crypto_error() {
    let (_, _, addrs, vs) = three_validators();
    let bls_sks: Vec<BlsSecretKey> = (0..3).map(|_| BlsSecretKey::generate()).collect();
    let (chain, _) = Chain::new_with_genesis(addrs[0], 0);

    let mut block = make_block(1, chain.tip_hash(), *vs.leader_at(1));

    // Validators 0 and 1 sign (= quorum 2/3); validator 2 is offline.
    let registry = bls_cosign(&mut block, &[0, 1], &bls_sks, &addrs, &vs);

    assert_eq!(block.bls_bitmap_popcount(), 2, "two bits set");
    assert!(block.bls_bitmap_has(0));
    assert!(block.bls_bitmap_has(1));
    assert!(!block.bls_bitmap_has(2), "absent validator has no bit");

    let count = block
        .bls_signer_count_from_bitmap(&registry)
        .expect("aggregate must verify");
    assert_eq!(count, 2);
    assert!(
        count >= vs.quorum(),
        "2 signers ≥ quorum {} — block is finalizable",
        vs.quorum()
    );

    vinx_node::consensus::validate_block(&block, &vs).expect("block must be valid");
}

// ─── 9. popcount = nombre de co-signataires sur 6 blocs ───────────────────────

#[test]
fn n3_bls_bitmap_popcount_matches_signer_count() {
    let (_, bls_sks, addrs, vs) = three_validators();
    let registry = indexed_pks(&bls_sks, &addrs, &vs);
    let (mut chain, _) = Chain::new_with_genesis(addrs[0], 0);

    // Alternate between 2-signer and 3-signer rounds across 6 blocks.
    for h in 1..=6u64 {
        let leader = *vs.leader_at(h);
        let mut block = make_block(h, chain.tip_hash(), leader);

        // Odd heights: all 3 sign; even heights: only validators 0 and 1.
        let signers: &[usize] = if h % 2 == 1 { &[0, 1, 2] } else { &[0, 1] };
        let expected = signers.len();

        bls_cosign(&mut block, signers, &bls_sks, &addrs, &vs);

        assert_eq!(
            block.bls_bitmap_popcount(),
            expected,
            "h={h}: popcount must equal signer count"
        );
        let verified = block
            .bls_signer_count_from_bitmap(&registry)
            .expect("aggregate must verify");
        assert_eq!(verified, expected, "h={h}: verified count matches");

        chain.push(block);
    }
}

// ─── 10. Vecteur doré (ADR 0020) : bitmap déterministe pour un set donné ──────

#[test]
fn n3_bls_golden_vector_deterministic_bitmap() {
    let (_, bls_sks, addrs, vs) = three_validators();
    let (chain, _) = Chain::new_with_genesis(addrs[0], 0);
    let prev = chain.tip_hash();

    // Build the same block twice with the same signers — bitmap must be identical.
    let bitmap_a = {
        let mut b = make_block(1, prev, addrs[0]);
        bls_cosign(&mut b, &[0, 2], &bls_sks, &addrs, &vs);
        b.bls_bitmap.clone()
    };
    let bitmap_b = {
        let mut b = make_block(1, prev, addrs[0]);
        bls_cosign(&mut b, &[0, 2], &bls_sks, &addrs, &vs);
        b.bls_bitmap.clone()
    };

    assert_eq!(
        bitmap_a, bitmap_b,
        "same signers in canonical order → identical bitmap (ADR 0020 golden vector)"
    );
    assert!(
        bitmap_a[0] & 0b0000_0001 != 0,
        "bit 0 set (validator index 0 signed)"
    );
    assert!(
        bitmap_a[0] & 0b0000_0100 != 0,
        "bit 2 set (validator index 2 signed)"
    );
    assert!(
        bitmap_a[0] & 0b0000_0010 == 0,
        "bit 1 clear (validator index 1 did not sign)"
    );
}
