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
use vinx_node::consensus;

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
