use vinx_core::{Block, Transaction, ValidatorSet};
use vinx_crypto::{BlsSecretKey, Hash32};
use vinx_state::WorldState;

use crate::NodeError;

/// Proves that `block` was really produced by `header.validator` (VINX-01 / VX-RED-002).
///
/// # Why this exists
///
/// `header.validator` is a plain public address, so set membership alone proves nothing:
/// any peer could name an honest validator as proposer and have the block applied. The
/// P2P ingestion path performed exactly that check and never looked at `bls_aggregate`,
/// so a non-validator could author blocks, order and censor transactions, and direct the
/// issuance reward at a validator of their choosing.
///
/// # What is checked
///
/// The proposer's bit must be set in `bls_bitmap`, and the aggregate must verify against
/// the registered on-chain BLS keys of exactly the bits that are set. Because a key only
/// enters the registry after a verified Proof-of-Possession, that aggregate cannot be
/// forged without the proposer's secret key.
///
/// This deliberately does **not** require quorum: a freshly gossiped block carries only
/// its producer's own co-signature and accumulates the rest afterwards. Requiring quorum
/// on ingestion would stall the chain. Quorum remains enforced where it belongs — in
/// [`validate_block_with_registry`] and in finality.
pub fn verify_proposer_authenticated(
    block: &Block,
    validator_set: &ValidatorSet,
    indexed_bls_pks: &[Option<[u8; 48]>],
) -> Result<(), NodeError> {
    if block.is_genesis() {
        return Ok(());
    }

    let Some(idx) = validator_set.index_of(&block.header.validator) else {
        return Err(NodeError::Consensus(format!(
            "block {} proposed by non-validator {}",
            block.header.height, block.header.validator
        )));
    };

    if !block.bls_bitmap_has(idx) {
        return Err(NodeError::Consensus(format!(
            "block {} carries no co-signature from its declared proposer {}",
            block.header.height, block.header.validator
        )));
    }

    // Verifies the aggregate against the registered keys of every set bit — including
    // the proposer's. Any tampering with the header changes `header.hash()` and fails here.
    block
        .bls_signer_count_from_bitmap(indexed_bls_pks)
        .map_err(|e| {
            NodeError::Consensus(format!(
                "block {} proposer authentication failed: {e}",
                block.header.height
            ))
        })?;

    Ok(())
}

/// Vérifie en parallèle les signatures de toutes les transactions d'un bloc (ADR 0015).
///
/// Déterministe et indépendant de l'ordre : `verify_tx_signature_pure` est le prédicat
/// canonique unique (VINX-03), et l'exécution d'état reste séquentielle ailleurs. Partagé
/// par le chemin P2P, la sync et le banc adversarial pour qu'ils ne puissent pas diverger
/// sur la définition de « bloc cryptographiquement valide » — la divergence entre deux
/// définitions est exactement ce qui a produit VINX-03.
pub fn verify_block_tx_signatures(txs: &[Transaction]) -> bool {
    use rayon::prelude::*;
    txs.par_iter()
        .all(|tx| WorldState::verify_tx_signature_pure(tx).is_ok())
}

/// Vue de la chaîne nécessaire pour valider un bloc entrant, extraite sous verrou par
/// l'appelant afin que cette fonction reste pure.
pub struct IncomingBlockCtx {
    /// Hash du tip courant — le parent que le bloc doit chaîner.
    pub tip_hash: Hash32,
    /// Timestamp du tip courant, pour le contrôle de monotonie (ADR 0005).
    pub tip_timestamp: u64,
    /// Horloge murale locale, pour la borne de dérive.
    pub now: u64,
}

/// Contrôles d'en-tête et d'autorité d'un bloc entrant, avant tout travail d'état.
///
/// C'est **la** définition de « ce bloc a le droit d'être appliqué au tip » : chaînage,
/// bornes de timestamp (ADR 0005), authentification cryptographique du proposeur contre le
/// registre BLS (ADR 0070), puis signatures de transactions (ADR 0015). Le chemin P2P, la
/// sync et le banc adversarial appellent tous celle-ci — un banc qui réimplémenterait la
/// séquence ne testerait que lui-même.
///
/// N'exige délibérément **pas** le quorum : un bloc fraîchement gossipé ne porte que la
/// co-signature de son producteur (voir `verify_proposer_authenticated`).
pub fn validate_incoming_block(
    block: &Block,
    ctx: &IncomingBlockCtx,
    validator_set: &ValidatorSet,
    indexed_bls_pks: &[Option<[u8; 48]>],
) -> Result<(), NodeError> {
    if block.header.prev_hash != ctx.tip_hash {
        return Err(NodeError::Consensus(format!(
            "block {} does not chain onto the current tip",
            block.header.height
        )));
    }
    // ADR 0005 : monotonie, puis borne de dérive. Le `state_root` ne rattrape pas ces
    // deux cas (les deux côtés utilisent le même timestamp de bloc), d'où un contrôle
    // explicite — cf. VINX-07, où son absence sur un seul chemin permettait de projeter
    // l'horloge protocole dans le futur et de frapper toute l'émission restante.
    if block.header.timestamp <= ctx.tip_timestamp {
        return Err(NodeError::Consensus(format!(
            "block {} timestamp is not monotonic",
            block.header.height
        )));
    }
    if block.header.timestamp
        > ctx
            .now
            .saturating_add(vinx_core::amount::MAX_CLOCK_DRIFT_SECS)
    {
        return Err(NodeError::Consensus(format!(
            "block {} timestamp is too far in the future",
            block.header.height
        )));
    }
    verify_proposer_authenticated(block, validator_set, indexed_bls_pks)?;
    if !verify_block_tx_signatures(&block.transactions) {
        return Err(NodeError::Consensus(format!(
            "block {} has invalid transaction signature(s)",
            block.header.height
        )));
    }
    Ok(())
}

/// Validates a block against the on-chain BLS key registry (ADR 0029 Phase 1).
///
/// Identical to `validate_block` for the proposer check, but uses
/// `Block::bls_signer_count_from_bitmap` for the finality check — each bit set in
/// `bls_bitmap` must correspond to a validator with a registered BLS key in
/// `indexed_bls_pks`. This closes the key-binding gap: arbitrary BLS keys not
/// enrolled in the validator pool cannot reach quorum.
///
/// `indexed_bls_pks[i]` is the registered G1 key (48 bytes) of the validator at
/// position `i` in the active `ValidatorSet`, or `None` when unregistered. Build
/// this slice with `WorldState::indexed_bls_keys(validator_set)`.
///
/// When `bls_bitmap` is empty (pre-ADR-0029 blocks), falls back to the
/// `bls_cosigner_pks` path exactly like `validate_block`.
pub fn validate_block_with_registry(
    block: &Block,
    validator_set: &ValidatorSet,
    indexed_bls_pks: &[Option<[u8; 48]>],
) -> Result<(), NodeError> {
    if block.is_genesis() {
        return Ok(());
    }

    if !validator_set.contains(&block.header.validator) {
        return Err(NodeError::Consensus(format!(
            "block {} proposed by non-validator {}",
            block.header.height, block.header.validator
        )));
    }

    let signer_count = block
        .bls_signer_count_from_bitmap(indexed_bls_pks)
        .map_err(|e| {
            NodeError::Consensus(format!(
                "block {} BLS aggregate invalid (registry check): {e}",
                block.header.height
            ))
        })?;

    if signer_count < validator_set.quorum() {
        return Err(NodeError::Consensus(format!(
            "block {} needs {}/{} BLS signatures from registered validators, has {}",
            block.header.height,
            validator_set.quorum(),
            validator_set.len(),
            signer_count,
        )));
    }

    Ok(())
}

/// Signs `block` with `bls_sk` at position `validator_idx` in the active ValidatorSet
/// (ADR 0046) and initializes or extends the BLS aggregate.
///
/// `validator_idx` is the signer's index in the current `ValidatorSet` — used to set
/// the corresponding bit in `bls_bitmap` and as the canonical double-sign guard. The
/// G1 public key is also appended to `bls_cosigner_pks` for backward compatibility.
///
/// Calling this multiple times with different `validator_idx` values extends the
/// aggregate correctly (BLS aggregation is associative). Returns `Err` when the
/// validator at `validator_idx` has already signed (bitmap guard), when the same G1
/// key appears twice, or when the aggregate bytes are malformed.
pub fn sign_block_bls(
    block: &mut Block,
    bls_sk: &BlsSecretKey,
    validator_idx: usize,
) -> Result<(), NodeError> {
    use vinx_crypto::{bls_aggregate, BlsSignature};

    // Canonical double-sign guard: bitmap prevents signing twice at the same index.
    if block.bls_bitmap_has(validator_idx) {
        return Err(NodeError::Consensus(
            "validator already co-signed this block (bitmap)".into(),
        ));
    }

    let header_hash = block.hash();
    let new_sig = bls_sk.sign(&header_hash);
    let new_pk = bls_sk.public_key().0.to_vec();

    // Belt-and-suspenders: also reject duplicate G1 keys from the cosigner list.
    if block.bls_cosigner_pks.iter().any(|pk| pk == &new_pk) {
        return Err(NodeError::Consensus(
            "BLS key already co-signed this block".into(),
        ));
    }

    let agg = if let Some(existing_bytes) = &block.bls_aggregate {
        let arr: [u8; 96] = existing_bytes
            .as_slice()
            .try_into()
            .map_err(|_| NodeError::Consensus("malformed existing BLS aggregate".into()))?;
        bls_aggregate(&[BlsSignature(arr), new_sig])
            .map_err(|e| NodeError::Consensus(format!("BLS aggregate failed: {e}")))?
    } else {
        bls_aggregate(&[new_sig])
            .map_err(|e| NodeError::Consensus(format!("BLS aggregate init failed: {e}")))?
    };

    block.bls_aggregate = Some(agg.0.to_vec());
    block.bls_cosigner_pks.push(new_pk);
    block.set_bls_bitmap_bit(validator_idx);
    Ok(())
}

/// Fork-choice déterministe (ADR 0031) — élit le bloc **canonique** parmi des candidats
/// valides **concurrents à la même hauteur contestée**, tous supposés étendre le préfixe
/// finalisé.
///
/// Fonction **pure et totale** : deux nœuds avec la même vue élisent la même tête, sans
/// dépendre de l'ordre d'arrivée réseau ni d'une horloge locale. Ordre de priorité :
///
/// 3. **Poids de co-signatures** — le bloc portant le plus de co-signatures valides gagne
///    (le plus proche du quorum, le plus soutenu par le set).
/// 4. **Priorité au leader prévu** — à poids égal, le bloc du leader round-robin
///    (`leader_at(H)`) l'emporte sur celui d'un backup (cohérent avec le slot-skip, ADR 0027).
/// 5. **Départage stable** — en dernier recours, le plus petit hash d'en-tête (ordre total).
///
/// Les règles 1–2 de l'ADR 0031 (ne jamais contredire la finalité ; préférer la finalité
/// justifiée la plus haute) sont des propriétés de **branche**, garanties par l'appelant :
/// tous les candidats doivent étendre `finalized_height` (réorg sous la finalité interdite).
pub fn canonical_head<'a>(
    candidates: &'a [Block],
    validator_set: &ValidatorSet,
    indexed_bls_pks: &[Option<[u8; 48]>],
) -> Option<&'a Block> {
    candidates
        .iter()
        .reduce(|acc, b| more_canonical(acc, b, validator_set, indexed_bls_pks))
}

/// Vrai si `block` est proposé par le leader round-robin prévu pour sa hauteur.
fn is_scheduled_leader(block: &Block, validator_set: &ValidatorSet) -> bool {
    block.header.validator == *validator_set.leader_at(block.header.height)
}

/// Co-signature weight for fork-choice rule 3 (ADR 0031).
///
/// VINX-05: this must count only co-signers whose BLS key is registered on-chain.
/// The previous implementation used `bls_signer_count()`, i.e. the block's own
/// `bls_cosigner_pks` list — an attacker attached 1 000 self-generated keys and won
/// every fork-choice race. Counting through the bitmap+registry bounds the weight by
/// `validator_set.len()` by construction.
fn signer_weight(block: &Block, indexed_bls_pks: &[Option<[u8; 48]>]) -> usize {
    block
        .bls_signer_count_from_bitmap(indexed_bls_pks)
        .unwrap_or(0)
}

/// Renvoie le plus canonique de deux candidats (règles 3 → 4 → 5 de l'ADR 0031).
/// L'ordre induit est **total** → `reduce` est indépendant de l'ordre d'itération.
/// `pub(crate)` pour que `Chain` élise la tête parmi des références sans cloner les blocs.
pub(crate) fn more_canonical<'a>(
    a: &'a Block,
    b: &'a Block,
    vs: &ValidatorSet,
    indexed_bls_pks: &[Option<[u8; 48]>],
) -> &'a Block {
    // 3. Poids de co-signatures (plus = mieux) — co-signataires **enregistrés** seulement.
    let (ca, cb) = (
        signer_weight(a, indexed_bls_pks),
        signer_weight(b, indexed_bls_pks),
    );
    if ca != cb {
        return if ca > cb { a } else { b };
    }
    // 4. Priorité au leader round-robin prévu (ADR 0063).
    let (la, lb) = (is_scheduled_leader(a, vs), is_scheduled_leader(b, vs));
    if la != lb {
        return if la { a } else { b };
    }
    // 5. Départage déterministe : plus petit hash d'en-tête.
    if a.hash() <= b.hash() {
        a
    } else {
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::{block::GENESIS_PREV_HASH, Block, BlockHeader};
    use vinx_crypto::{Address, KeyPair};

    fn addr_of(kp: &KeyPair) -> Address {
        Address::from_public_key(&kp.public_key())
    }

    fn kps(n: usize) -> Vec<KeyPair> {
        (0..n).map(|_| KeyPair::generate()).collect()
    }

    /// Deterministic BLS key for validator index `idx` — lets several blocks be verified
    /// against one shared registry, as they are on a real chain.
    fn test_bls_sk(idx: usize) -> BlsSecretKey {
        let mut seed = [0u8; 32];
        seed[0] = idx as u8 + 1;
        BlsSecretKey::from_bytes(&seed).expect("valid BLS scalar")
    }

    /// The registered BLS keys of the first `n` validator slots.
    fn test_registry(n: usize) -> Vec<Option<[u8; 48]>> {
        (0..n)
            .map(|i| Some(test_bls_sk(i).public_key().0))
            .collect()
    }

    /// Co-signs `block` at validator index `idx` with its registered BLS key, and records that
    /// key in `registry` so the registry-bound validation path can verify it. Mirrors what
    /// a real validator does: register the key on-chain, then sign.
    fn cosign_at(block: &mut Block, idx: usize, registry: &mut Vec<Option<[u8; 48]>>) {
        let sk = test_bls_sk(idx);
        if registry.len() <= idx {
            registry.resize(idx + 1, None);
        }
        registry[idx] = Some(sk.public_key().0);
        sign_block_bls(block, &sk, idx).unwrap();
    }

    fn make_block(height: u64, proposer: Address) -> Block {
        Block {
            header: BlockHeader {
                height,
                prev_hash: GENESIS_PREV_HASH,
                timestamp: 0,
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

    #[test]
    fn test_genesis_always_valid() {
        let kp = KeyPair::generate();
        let vs = ValidatorSet::single(addr_of(&kp));
        let genesis = make_block(0, addr_of(&kp));
        assert!(validate_block_with_registry(&genesis, &vs, &[]).is_ok());
    }

    #[test]
    fn test_single_validator_produces_and_finalizes_block() {
        let kp = KeyPair::generate();
        let addr = addr_of(&kp);
        let vs = ValidatorSet::single(addr);

        let mut block = make_block(1, addr);
        let mut reg_block: Vec<Option<[u8; 48]>> = Vec::new();
        cosign_at(&mut block, 0, &mut reg_block);
        assert!(validate_block_with_registry(&block, &vs, &reg_block).is_ok());
    }

    #[test]
    fn test_non_validator_proposer_rejected() {
        let validators = kps(2);
        let vs = ValidatorSet::new(validators.iter().map(addr_of).collect());

        let outsider = KeyPair::generate();
        let mut block = make_block(1, addr_of(&outsider));
        let mut reg_block: Vec<Option<[u8; 48]>> = Vec::new();
        cosign_at(&mut block, 0, &mut reg_block);
        cosign_at(&mut block, 1, &mut reg_block);

        assert!(validate_block_with_registry(&block, &vs, &reg_block).is_err());
    }

    #[test]
    fn test_backup_proposer_accepted() {
        let validators = kps(3);
        let vs = ValidatorSet::new(validators.iter().map(addr_of).collect());

        let mut block = make_block(1, addr_of(&validators[2]));
        let mut reg_block: Vec<Option<[u8; 48]>> = Vec::new();
        cosign_at(&mut block, 0, &mut reg_block);
        cosign_at(&mut block, 1, &mut reg_block);
        assert!(validate_block_with_registry(&block, &vs, &reg_block).is_ok());
    }

    #[test]
    fn test_correct_leader_selected_by_height() {
        let validators = kps(3);
        let addrs: Vec<Address> = validators.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone());

        let mut block = make_block(3, addrs[0]);
        let mut reg_block: Vec<Option<[u8; 48]>> = Vec::new();
        cosign_at(&mut block, 0, &mut reg_block);
        cosign_at(&mut block, 1, &mut reg_block);

        assert!(validate_block_with_registry(&block, &vs, &reg_block).is_ok());
    }

    #[test]
    fn test_insufficient_signatures_rejected() {
        let validators = kps(5);
        let addrs: Vec<Address> = validators.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone()); // quorum = 4

        let mut block = make_block(5, addrs[0]);
        // Only 3 BLS signers (need 4)
        let mut reg_block: Vec<Option<[u8; 48]>> = Vec::new();
        for idx in 0..3usize {
            cosign_at(&mut block, idx, &mut reg_block);
        }
        assert!(validate_block_with_registry(&block, &vs, &reg_block).is_err());
    }

    #[test]
    fn test_five_validators_quorum_met() {
        let validators = kps(5);
        let addrs: Vec<Address> = validators.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone()); // quorum = 4

        let mut block = make_block(5, addrs[0]);
        let mut reg_block: Vec<Option<[u8; 48]>> = Vec::new();
        for idx in 0..4usize {
            cosign_at(&mut block, idx, &mut reg_block);
        }
        assert!(validate_block_with_registry(&block, &vs, &reg_block).is_ok());
    }

    #[test]
    fn test_three_validators_one_offline() {
        let validators = kps(3);
        let addrs: Vec<Address> = validators.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone());
        assert_eq!(vs.quorum(), 2);

        let mut block = make_block(3, addrs[0]);
        let mut reg_block: Vec<Option<[u8; 48]>> = Vec::new();
        cosign_at(&mut block, 0, &mut reg_block);
        cosign_at(&mut block, 1, &mut reg_block);

        assert!(validate_block_with_registry(&block, &vs, &reg_block).is_ok());
    }

    #[test]
    fn test_nine_validators_three_offline() {
        let validators = kps(9);
        let addrs: Vec<Address> = validators.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone());
        assert_eq!(vs.quorum(), 6);

        let mut block = make_block(9, addrs[0]);
        let mut reg_block: Vec<Option<[u8; 48]>> = Vec::new();
        for idx in 0..6usize {
            cosign_at(&mut block, idx, &mut reg_block);
        }
        assert!(validate_block_with_registry(&block, &vs, &reg_block).is_ok());
    }

    // ─── BLS consensus (ADR 0046) ────────────────────────────────────────────

    #[test]
    fn test_sign_block_bls_initializes_aggregate() {
        let kp = KeyPair::generate();
        let addr = addr_of(&kp);
        let vs = ValidatorSet::single(addr);
        let mut block = make_block(1, addr);

        assert!(block.bls_aggregate.is_none());
        let mut reg_block: Vec<Option<[u8; 48]>> = Vec::new();
        cosign_at(&mut block, 0, &mut reg_block);
        assert!(block.bls_aggregate.is_some());
        assert_eq!(block.bls_cosigner_pks.len(), 1);
        assert_eq!(block.bls_aggregate.as_ref().unwrap().len(), 96);
        assert!(block.bls_bitmap_has(0), "bitmap bit 0 must be set");
        // Single BLS sig → quorum=1 met → validate_block accepts.
        assert!(validate_block_with_registry(&block, &vs, &reg_block).is_ok());
    }

    #[test]
    fn test_sign_block_bls_extends_aggregate() {
        let kp = KeyPair::generate();
        let addr = addr_of(&kp);
        let vs = ValidatorSet::new(vec![addr, addr_of(&KeyPair::generate())]);
        let mut block = make_block(1, addr);

        // Registered keys, so the registry-bound finality check below can verify them.
        sign_block_bls(&mut block, &test_bls_sk(0), 0).unwrap();
        sign_block_bls(&mut block, &test_bls_sk(1), 1).unwrap();
        assert_eq!(block.bls_cosigner_pks.len(), 2);
        assert!(
            block.bls_bitmap_has(0) && block.bls_bitmap_has(1),
            "both bitmap bits set"
        );
        // 2 BLS signers ≥ quorum=2 → block is finalized.
        assert_eq!(block.bls_signer_count_unverified().unwrap(), 2);
        assert!(block.is_finalized(&vs, &test_registry(vs.len())));
    }

    #[test]
    fn test_sign_block_bls_double_sign_rejected() {
        use vinx_crypto::BlsSecretKey;
        let kp = KeyPair::generate();
        let addr = addr_of(&kp);
        let mut block = make_block(1, addr);

        let bls_sk = BlsSecretKey::generate();
        sign_block_bls(&mut block, &bls_sk, 0).unwrap();
        // Same validator index signing twice must be rejected (bitmap guard).
        assert!(sign_block_bls(&mut block, &bls_sk, 0).is_err());
        // Different index with the same key is also rejected (cosigner_pks guard).
        assert!(sign_block_bls(&mut block, &bls_sk, 1).is_err());
    }

    #[test]
    fn test_validate_block_bls_finalized_accepted() {
        let kp = KeyPair::generate();
        let addr = addr_of(&kp);
        let vs = ValidatorSet::single(addr); // quorum = 1

        let mut block = make_block(1, addr);
        let mut reg_block: Vec<Option<[u8; 48]>> = Vec::new();
        cosign_at(&mut block, 0, &mut reg_block);
        assert!(validate_block_with_registry(&block, &vs, &reg_block).is_ok());
    }

    #[test]
    fn test_validate_block_bls_insufficient_rejected() {
        let kps3 = kps(3);
        let addrs: Vec<Address> = kps3.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone()); // quorum = 2

        let mut block = make_block(1, addrs[0]);
        // Only 1 BLS signer — below quorum of 2.
        let mut reg_block: Vec<Option<[u8; 48]>> = Vec::new();
        cosign_at(&mut block, 0, &mut reg_block);
        let err = validate_block_with_registry(&block, &vs, &reg_block).unwrap_err();
        // Error message should mention BLS, not Ed25519.
        assert!(
            err.to_string().contains("BLS"),
            "error should name the signature scheme: {err}"
        );
    }

    #[test]
    fn test_fork_choice_bls_vs_bls_by_count() {
        let v = kps(3);
        let a: Vec<Address> = v.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(a.clone()); // quorum = 2

        // block_a: 1 BLS sig; block_b: 2 BLS sigs.
        let mut block_a = make_block(1, a[0]);
        let mut reg_block_a: Vec<Option<[u8; 48]>> = Vec::new();
        cosign_at(&mut block_a, 0, &mut reg_block_a);

        let mut block_b = make_block(1, a[0]);
        block_b.header.state_root = [0xCCu8; 32];
        let mut reg_block_b: Vec<Option<[u8; 48]>> = Vec::new();
        cosign_at(&mut block_b, 0, &mut reg_block_b);
        cosign_at(&mut block_b, 1, &mut reg_block_b);

        let cands = [block_a.clone(), block_b.clone()];
        let head = canonical_head(&cands, &vs, &test_registry(vs.len())).unwrap();
        assert_eq!(head.hash(), block_b.hash(), "more BLS signers wins");
    }

    // ─── Fork-choice (ADR 0031) ───────────────────────────────────────────────

    /// Builds a block at `height` proposed by `proposer` with `n_sigs` BLS co-signatures.
    fn contested_block(height: u64, proposer: Address, n_sigs: usize) -> Block {
        let mut b = make_block(height, proposer);
        let mut reg_b: Vec<Option<[u8; 48]>> = Vec::new();
        for idx in 0..n_sigs {
            cosign_at(&mut b, idx, &mut reg_b);
        }
        b
    }

    #[test]
    fn test_fork_choice_leader_beats_backup_at_equal_weight() {
        let v = kps(3);
        let a: Vec<Address> = v.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(a.clone());
        // height 1 → leader = a[1]. Backup = a[2]. Each has 2 BLS co-sigs (equal weight).
        let leader = contested_block(1, a[1], 2);
        let backup = contested_block(1, a[2], 2);

        let cands = vec![backup, leader];
        let head = canonical_head(&cands, &vs, &test_registry(vs.len())).unwrap();
        assert_eq!(
            head.header.validator, a[1],
            "règle 4 : le leader prévu l'emporte"
        );
    }

    #[test]
    fn test_fork_choice_more_cosignatures_beats_leader_priority() {
        let v = kps(3);
        let a: Vec<Address> = v.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(a.clone());
        // Leader (a[1]) with 1 BLS sig vs backup (a[2]) with 2 BLS sigs → weight wins (rule 3).
        let leader = contested_block(1, a[1], 1);
        let backup = contested_block(1, a[2], 2);

        let cands = vec![leader, backup];
        let head = canonical_head(&cands, &vs, &test_registry(vs.len())).unwrap();
        assert_eq!(
            head.header.validator, a[2],
            "règle 3 > règle 4 : plus de co-signatures gagne"
        );
    }

    #[test]
    fn test_fork_choice_hash_tiebreak_between_non_leaders() {
        let v = kps(3);
        let a: Vec<Address> = v.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(a.clone());
        // height 1 → leader = a[1]. Two non-leaders (a[0], a[2]) with equal weight → hash tiebreak.
        let x = contested_block(1, a[0], 2);
        let y = contested_block(1, a[2], 2);
        let expected = if x.hash() <= y.hash() {
            x.hash()
        } else {
            y.hash()
        };

        let cands = vec![x, y];
        let head = canonical_head(&cands, &vs, &test_registry(vs.len())).unwrap();
        assert_eq!(head.hash(), expected, "règle 5 : plus petit hash d'en-tête");
    }

    #[test]
    fn test_fork_choice_single_and_empty() {
        let v = kps(3);
        let a: Vec<Address> = v.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(a.clone());
        let only = contested_block(1, a[1], 2);
        let only_hash = only.hash();

        let one = vec![only];
        assert_eq!(
            canonical_head(&one, &vs, &test_registry(vs.len()))
                .unwrap()
                .hash(),
            only_hash
        );
        let none: Vec<Block> = vec![];
        assert!(canonical_head(&none, &vs, &test_registry(vs.len())).is_none());
    }

    #[test]
    fn test_fork_choice_is_order_independent() {
        let v = kps(3);
        let a: Vec<Address> = v.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(a.clone());
        let leader = contested_block(1, a[1], 2);
        let backup = contested_block(1, a[2], 2);

        let fwd = vec![leader.clone(), backup.clone()];
        let rev = vec![backup, leader];
        assert_eq!(
            canonical_head(&fwd, &vs, &test_registry(vs.len()))
                .unwrap()
                .hash(),
            canonical_head(&rev, &vs, &test_registry(vs.len()))
                .unwrap()
                .hash(),
            "l'élection est indépendante de l'ordre d'itération (ordre total)"
        );
    }
}
