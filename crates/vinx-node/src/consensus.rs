use vinx_core::{Block, ValidatorSet};
use vinx_crypto::{Address, BlsSecretKey, KeyPair};

use crate::NodeError;

/// Validates that a block has a registered proposer and enough co-signatures.
/// Genesis blocks (height 0) are exempt from both checks.
///
/// # Proposer rule
/// Any registered validator may propose a block — the round-robin schedule is
/// advisory (normal) or pre-empted when the scheduled leader is offline (slot skip).
/// What is always enforced: the proposer must be a current member of the validator set.
pub fn validate_block(block: &Block, validator_set: &ValidatorSet) -> Result<(), NodeError> {
    if block.is_genesis() {
        return Ok(());
    }

    if !validator_set.contains(&block.header.validator) {
        return Err(NodeError::Consensus(format!(
            "block {} proposed by non-validator {}",
            block.header.height, block.header.validator
        )));
    }

    if !block.is_finalized(validator_set) {
        let have = block.bls_signer_count().unwrap_or(0);
        return Err(NodeError::Consensus(format!(
            "block {} needs {}/{} BLS signatures, has {}",
            block.header.height,
            validator_set.quorum(),
            validator_set.len(),
            have,
        )));
    }

    Ok(())
}

/// Signs `block` with `bls_sk` (ADR 0046) and initializes or extends the BLS aggregate.
///
/// The new G2 signature is aggregated with any existing `bls_aggregate`, and the
/// corresponding G1 public key is appended to `bls_cosigner_pks`. Calling this
/// multiple times with different keys extends the aggregate correctly (BLS aggregation
/// is associative). Returns `Err` when the key has already signed the block (double-sign
/// guard) or when the aggregate bytes are malformed.
pub fn sign_block_bls(block: &mut Block, bls_sk: &BlsSecretKey) -> Result<(), NodeError> {
    use vinx_crypto::{bls_aggregate, BlsSignature};
    let header_hash = block.hash();
    let new_sig = bls_sk.sign(&header_hash);
    let new_pk = bls_sk.public_key().0.to_vec();

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
) -> Option<&'a Block> {
    candidates
        .iter()
        .reduce(|acc, b| more_canonical(acc, b, validator_set))
}

/// Vrai si `block` est proposé par le leader round-robin prévu pour sa hauteur.
fn is_scheduled_leader(block: &Block, validator_set: &ValidatorSet) -> bool {
    block.header.validator == *validator_set.leader_at(block.header.height)
}

/// Co-signature weight for fork-choice rule 3 (ADR 0031).
fn signer_weight(block: &Block, _vs: &ValidatorSet) -> usize {
    block.bls_signer_count().unwrap_or(0)
}

/// Renvoie le plus canonique de deux candidats (règles 3 → 4 → 5 de l'ADR 0031).
/// L'ordre induit est **total** → `reduce` est indépendant de l'ordre d'itération.
/// `pub(crate)` pour que `Chain` élise la tête parmi des références sans cloner les blocs.
pub(crate) fn more_canonical<'a>(a: &'a Block, b: &'a Block, vs: &ValidatorSet) -> &'a Block {
    // 3. Poids de co-signatures (plus = mieux) — BLS count used when aggregate is present.
    let (ca, cb) = (signer_weight(a, vs), signer_weight(b, vs));
    if ca != cb {
        return if ca > cb { a } else { b };
    }
    // 4. Priorité au leader prévu.
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

    fn addr_of(kp: &KeyPair) -> Address {
        Address::from_public_key(&kp.public_key())
    }

    fn kps(n: usize) -> Vec<KeyPair> {
        (0..n).map(|_| KeyPair::generate()).collect()
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
        assert!(validate_block(&genesis, &vs).is_ok());
    }

    #[test]
    fn test_single_validator_produces_and_finalizes_block() {
        let kp = KeyPair::generate();
        let addr = addr_of(&kp);
        let vs = ValidatorSet::single(addr.clone());

        let mut block = make_block(1, addr);
        sign_block_bls(&mut block, &BlsSecretKey::generate()).unwrap();
        assert!(validate_block(&block, &vs).is_ok());
    }

    #[test]
    fn test_non_validator_proposer_rejected() {
        let validators = kps(2);
        let vs = ValidatorSet::new(validators.iter().map(addr_of).collect());

        let outsider = KeyPair::generate();
        let mut block = make_block(1, addr_of(&outsider));
        sign_block_bls(&mut block, &BlsSecretKey::generate()).unwrap();
        sign_block_bls(&mut block, &BlsSecretKey::generate()).unwrap();

        assert!(validate_block(&block, &vs).is_err());
    }

    #[test]
    fn test_backup_proposer_accepted() {
        let validators = kps(3);
        let vs = ValidatorSet::new(validators.iter().map(addr_of).collect());

        let mut block = make_block(1, addr_of(&validators[2]));
        sign_block_bls(&mut block, &BlsSecretKey::generate()).unwrap();
        sign_block_bls(&mut block, &BlsSecretKey::generate()).unwrap();
        assert!(validate_block(&block, &vs).is_ok());
    }

    #[test]
    fn test_correct_leader_selected_by_height() {
        let validators = kps(3);
        let addrs: Vec<Address> = validators.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone());

        let mut block = make_block(3, addrs[0].clone());
        sign_block_bls(&mut block, &BlsSecretKey::generate()).unwrap();
        sign_block_bls(&mut block, &BlsSecretKey::generate()).unwrap();

        assert!(validate_block(&block, &vs).is_ok());
    }

    #[test]
    fn test_insufficient_signatures_rejected() {
        let validators = kps(5);
        let addrs: Vec<Address> = validators.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone()); // quorum = 4

        let mut block = make_block(5, addrs[0].clone());
        // Only 3 BLS signers (need 4)
        for _ in 0..3 {
            sign_block_bls(&mut block, &BlsSecretKey::generate()).unwrap();
        }
        assert!(validate_block(&block, &vs).is_err());
    }

    #[test]
    fn test_five_validators_quorum_met() {
        let validators = kps(5);
        let addrs: Vec<Address> = validators.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone()); // quorum = 4

        let mut block = make_block(5, addrs[0].clone());
        for _ in 0..4 {
            sign_block_bls(&mut block, &BlsSecretKey::generate()).unwrap();
        }
        assert!(validate_block(&block, &vs).is_ok());
    }

    #[test]
    fn test_three_validators_one_offline() {
        let validators = kps(3);
        let addrs: Vec<Address> = validators.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone());
        assert_eq!(vs.quorum(), 2);

        let mut block = make_block(3, addrs[0].clone());
        sign_block_bls(&mut block, &BlsSecretKey::generate()).unwrap();
        sign_block_bls(&mut block, &BlsSecretKey::generate()).unwrap();

        assert!(validate_block(&block, &vs).is_ok());
    }

    #[test]
    fn test_nine_validators_three_offline() {
        let validators = kps(9);
        let addrs: Vec<Address> = validators.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone());
        assert_eq!(vs.quorum(), 6);

        let mut block = make_block(9, addrs[0].clone());
        for _ in 0..6 {
            sign_block_bls(&mut block, &BlsSecretKey::generate()).unwrap();
        }
        assert!(validate_block(&block, &vs).is_ok());
    }

    // ─── BLS consensus (ADR 0046) ────────────────────────────────────────────

    #[test]
    fn test_sign_block_bls_initializes_aggregate() {
        use vinx_crypto::BlsSecretKey;
        let kp = KeyPair::generate();
        let addr = addr_of(&kp);
        let vs = ValidatorSet::single(addr.clone());
        let mut block = make_block(1, addr);

        assert!(block.bls_aggregate.is_none());
        sign_block_bls(&mut block, &BlsSecretKey::generate()).unwrap();
        assert!(block.bls_aggregate.is_some());
        assert_eq!(block.bls_cosigner_pks.len(), 1);
        assert_eq!(block.bls_aggregate.as_ref().unwrap().len(), 96);
        // Single BLS sig → quorum=1 met → validate_block accepts.
        assert!(validate_block(&block, &vs).is_ok());
    }

    #[test]
    fn test_sign_block_bls_extends_aggregate() {
        use vinx_crypto::BlsSecretKey;
        let kp = KeyPair::generate();
        let addr = addr_of(&kp);
        let vs = ValidatorSet::new(vec![addr.clone(), addr_of(&KeyPair::generate())]);
        let mut block = make_block(1, addr);

        let sk1 = BlsSecretKey::generate();
        let sk2 = BlsSecretKey::generate();
        sign_block_bls(&mut block, &sk1).unwrap();
        sign_block_bls(&mut block, &sk2).unwrap();
        assert_eq!(block.bls_cosigner_pks.len(), 2);
        // 2 BLS signers ≥ quorum=2 → block is finalized.
        assert_eq!(block.bls_signer_count().unwrap(), 2);
        assert!(block.is_finalized(&vs));
    }

    #[test]
    fn test_sign_block_bls_double_sign_rejected() {
        use vinx_crypto::BlsSecretKey;
        let kp = KeyPair::generate();
        let addr = addr_of(&kp);
        let mut block = make_block(1, addr);

        let bls_sk = BlsSecretKey::generate();
        sign_block_bls(&mut block, &bls_sk).unwrap();
        // Same key signing twice must be rejected.
        assert!(sign_block_bls(&mut block, &bls_sk).is_err());
    }

    #[test]
    fn test_validate_block_bls_finalized_accepted() {
        use vinx_crypto::BlsSecretKey;
        let kp = KeyPair::generate();
        let addr = addr_of(&kp);
        let vs = ValidatorSet::single(addr.clone()); // quorum = 1

        let mut block = make_block(1, addr);
        sign_block_bls(&mut block, &BlsSecretKey::generate()).unwrap();
        assert!(validate_block(&block, &vs).is_ok());
    }

    #[test]
    fn test_validate_block_bls_insufficient_rejected() {
        use vinx_crypto::BlsSecretKey;
        let kps3 = kps(3);
        let addrs: Vec<Address> = kps3.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone()); // quorum = 2

        let mut block = make_block(1, addrs[0].clone());
        // Only 1 BLS signer — below quorum of 2.
        sign_block_bls(&mut block, &BlsSecretKey::generate()).unwrap();
        let err = validate_block(&block, &vs).unwrap_err();
        // Error message should mention BLS, not Ed25519.
        assert!(
            err.to_string().contains("BLS"),
            "error should name the signature scheme: {err}"
        );
    }

    #[test]
    fn test_fork_choice_bls_vs_bls_by_count() {
        use vinx_crypto::BlsSecretKey;
        let v = kps(3);
        let a: Vec<Address> = v.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(a.clone()); // quorum = 2

        // block_a: 1 BLS sig; block_b: 2 BLS sigs.
        let mut block_a = make_block(1, a[0].clone());
        sign_block_bls(&mut block_a, &BlsSecretKey::generate()).unwrap();

        let mut block_b = make_block(1, a[0].clone());
        block_b.header.state_root = [0xCCu8; 32];
        sign_block_bls(&mut block_b, &BlsSecretKey::generate()).unwrap();
        sign_block_bls(&mut block_b, &BlsSecretKey::generate()).unwrap();

        let cands = [block_a.clone(), block_b.clone()];
        let head = canonical_head(&cands, &vs).unwrap();
        assert_eq!(head.hash(), block_b.hash(), "more BLS signers wins");
    }

    // ─── Fork-choice (ADR 0031) ───────────────────────────────────────────────

    /// Builds a block at `height` proposed by `proposer` with `n_sigs` BLS co-signatures.
    fn contested_block(height: u64, proposer: Address, n_sigs: usize) -> Block {
        let mut b = make_block(height, proposer);
        for _ in 0..n_sigs {
            sign_block_bls(&mut b, &BlsSecretKey::generate()).unwrap();
        }
        b
    }

    #[test]
    fn test_fork_choice_leader_beats_backup_at_equal_weight() {
        let v = kps(3);
        let a: Vec<Address> = v.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(a.clone());
        // height 1 → leader = a[1]. Backup = a[2]. Each has 2 BLS co-sigs (equal weight).
        let leader = contested_block(1, a[1].clone(), 2);
        let backup = contested_block(1, a[2].clone(), 2);

        let cands = vec![backup, leader];
        let head = canonical_head(&cands, &vs).unwrap();
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
        let leader = contested_block(1, a[1].clone(), 1);
        let backup = contested_block(1, a[2].clone(), 2);

        let cands = vec![leader, backup];
        let head = canonical_head(&cands, &vs).unwrap();
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
        let x = contested_block(1, a[0].clone(), 2);
        let y = contested_block(1, a[2].clone(), 2);
        let expected = if x.hash() <= y.hash() {
            x.hash()
        } else {
            y.hash()
        };

        let cands = vec![x, y];
        let head = canonical_head(&cands, &vs).unwrap();
        assert_eq!(head.hash(), expected, "règle 5 : plus petit hash d'en-tête");
    }

    #[test]
    fn test_fork_choice_single_and_empty() {
        let v = kps(3);
        let a: Vec<Address> = v.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(a.clone());
        let only = contested_block(1, a[1].clone(), 2);
        let only_hash = only.hash();

        let one = vec![only];
        assert_eq!(canonical_head(&one, &vs).unwrap().hash(), only_hash);
        let none: Vec<Block> = vec![];
        assert!(canonical_head(&none, &vs).is_none());
    }

    #[test]
    fn test_fork_choice_is_order_independent() {
        let v = kps(3);
        let a: Vec<Address> = v.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(a.clone());
        let leader = contested_block(1, a[1].clone(), 2);
        let backup = contested_block(1, a[2].clone(), 2);

        let fwd = vec![leader.clone(), backup.clone()];
        let rev = vec![backup, leader];
        assert_eq!(
            canonical_head(&fwd, &vs).unwrap().hash(),
            canonical_head(&rev, &vs).unwrap().hash(),
            "l'élection est indépendante de l'ordre d'itération (ordre total)"
        );
    }
}
