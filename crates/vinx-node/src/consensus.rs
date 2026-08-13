use vinx_core::{Block, BlockSignature, ValidatorSet};
use vinx_crypto::{Address, KeyPair};

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
        return Err(NodeError::Consensus(format!(
            "block {} needs {}/{} signatures, has {}",
            block.header.height,
            validator_set.quorum(),
            validator_set.len(),
            block.valid_signer_count(validator_set),
        )));
    }

    Ok(())
}

/// Signs `block` with `keypair` and appends the signature.
/// Returns an error if the keypair's address is not in the validator set.
pub fn sign_block(
    block: &mut Block,
    keypair: &KeyPair,
    validator_set: &ValidatorSet,
) -> Result<(), NodeError> {
    let addr = Address::from_public_key(&keypair.public_key());
    if !validator_set.contains(&addr) {
        return Err(NodeError::Consensus(
            "signer is not a registered validator".into(),
        ));
    }
    let header_hash = block.hash();
    block.signatures.push(BlockSignature {
        validator: addr,
        pub_key: keypair.public_key(),
        signature: keypair.sign(&header_hash),
    });
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

/// Renvoie le plus canonique de deux candidats (règles 3 → 4 → 5 de l'ADR 0031).
/// L'ordre induit est **total** → `reduce` est indépendant de l'ordre d'itération.
/// `pub(crate)` pour que `Chain` élise la tête parmi des références sans cloner les blocs.
pub(crate) fn more_canonical<'a>(a: &'a Block, b: &'a Block, vs: &ValidatorSet) -> &'a Block {
    // 3. Poids de co-signatures (plus = mieux).
    let (ca, cb) = (a.valid_signer_count(vs), b.valid_signer_count(vs));
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
            signatures: vec![],
            bls_aggregate: None,
            bls_cosigner_pks: vec![],
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

        // height 1 % 1 = 0 → kp is the leader
        let mut block = make_block(1, addr);
        sign_block(&mut block, &kp, &vs).unwrap();
        assert!(validate_block(&block, &vs).is_ok());
    }

    #[test]
    fn test_non_validator_proposer_rejected() {
        let validators = kps(2);
        let vs = ValidatorSet::new(validators.iter().map(addr_of).collect());

        // An outsider (not in the set) cannot propose a block
        let outsider = KeyPair::generate();
        let mut block = make_block(1, addr_of(&outsider));
        // Give it enough signatures from real validators (quorum met)
        sign_block(&mut block, &validators[0], &vs).unwrap();
        sign_block(&mut block, &validators[1], &vs).unwrap();

        assert!(validate_block(&block, &vs).is_err());
    }

    #[test]
    fn test_backup_proposer_accepted() {
        let validators = kps(3);
        let vs = ValidatorSet::new(validators.iter().map(addr_of).collect());

        // height 1 % 3 = 1 → validators[1] is the scheduled leader.
        // But in a slot-skip scenario, validators[2] may step in as backup.
        // As long as validators[2] is in the set and quorum is met, it's valid.
        let mut block = make_block(1, addr_of(&validators[2]));
        sign_block(&mut block, &validators[0], &vs).unwrap();
        sign_block(&mut block, &validators[2], &vs).unwrap(); // quorum = 2
        assert!(validate_block(&block, &vs).is_ok());
    }

    #[test]
    fn test_correct_leader_selected_by_height() {
        let validators = kps(3);
        let addrs: Vec<Address> = validators.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone());

        // height 3 % 3 = 0 → validators[0] is leader; quorum = 2
        let mut block = make_block(3, addrs[0].clone());
        sign_block(&mut block, &validators[0], &vs).unwrap();
        sign_block(&mut block, &validators[1], &vs).unwrap();

        assert!(validate_block(&block, &vs).is_ok());
    }

    #[test]
    fn test_insufficient_signatures_rejected() {
        let validators = kps(5);
        let addrs: Vec<Address> = validators.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone()); // quorum = 4

        // height 5 % 5 = 0 → validators[0] is leader
        let mut block = make_block(5, addrs[0].clone());
        // Only 3 signatures (need 4)
        for v in validators.iter().take(3) {
            sign_block(&mut block, v, &vs).unwrap();
        }
        assert!(validate_block(&block, &vs).is_err());
    }

    #[test]
    fn test_five_validators_quorum_met() {
        let validators = kps(5);
        let addrs: Vec<Address> = validators.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone()); // quorum = 4

        // height 5 % 5 = 0 → validators[0] is leader
        let mut block = make_block(5, addrs[0].clone());
        for v in validators.iter().take(4) {
            sign_block(&mut block, v, &vs).unwrap();
        }
        assert!(validate_block(&block, &vs).is_ok());
    }

    #[test]
    fn test_three_validators_one_offline() {
        // quorum for n=3 is 2 → 1 offline still OK
        let validators = kps(3);
        let addrs: Vec<Address> = validators.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone());
        assert_eq!(vs.quorum(), 2);

        // height 3 % 3 = 0 → validators[0] is leader
        let mut block = make_block(3, addrs[0].clone());
        sign_block(&mut block, &validators[0], &vs).unwrap();
        sign_block(&mut block, &validators[1], &vs).unwrap();
        // validators[2] is offline

        assert!(validate_block(&block, &vs).is_ok());
    }

    #[test]
    fn test_non_validator_cannot_sign() {
        let kp = KeyPair::generate();
        let vs = ValidatorSet::single(addr_of(&kp));
        let outsider = KeyPair::generate();

        let mut block = make_block(1, addr_of(&kp));
        assert!(sign_block(&mut block, &outsider, &vs).is_err());
    }

    #[test]
    fn test_nine_validators_three_offline() {
        // quorum for n=9 is 6 → 3 offline still OK
        let validators = kps(9);
        let addrs: Vec<Address> = validators.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(addrs.clone());
        assert_eq!(vs.quorum(), 6);

        // height 9 % 9 = 0 → validators[0] is leader
        let mut block = make_block(9, addrs[0].clone());
        for v in validators.iter().take(6) {
            sign_block(&mut block, v, &vs).unwrap();
        }
        assert!(validate_block(&block, &vs).is_ok());
    }

    // ─── Fork-choice (ADR 0031) ───────────────────────────────────────────────

    /// Construit un bloc à `height` proposé par `proposer`, co-signé par `signers`.
    fn contested_block(
        height: u64,
        proposer: Address,
        signers: &[&KeyPair],
        vs: &ValidatorSet,
    ) -> Block {
        let mut b = make_block(height, proposer);
        for kp in signers {
            sign_block(&mut b, kp, vs).unwrap();
        }
        b
    }

    #[test]
    fn test_fork_choice_leader_beats_backup_at_equal_weight() {
        let v = kps(3);
        let a: Vec<Address> = v.iter().map(addr_of).collect();
        let vs = ValidatorSet::new(a.clone());
        // height 1 → leader = a[1]. Backup = a[2]. Chacun 2 co-sigs (poids égal).
        let leader = contested_block(1, a[1].clone(), &[&v[0], &v[1]], &vs);
        let backup = contested_block(1, a[2].clone(), &[&v[0], &v[2]], &vs);

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
        // Leader (a[1]) avec 1 co-sig vs backup (a[2]) avec 2 co-sigs → le poids prime (règle 3).
        let leader = contested_block(1, a[1].clone(), &[&v[1]], &vs);
        let backup = contested_block(1, a[2].clone(), &[&v[0], &v[2]], &vs);

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
        // height 1 → leader = a[1]. Deux non-leaders (a[0], a[2]), poids égal → départage par hash.
        let x = contested_block(1, a[0].clone(), &[&v[0], &v[1]], &vs);
        let y = contested_block(1, a[2].clone(), &[&v[0], &v[2]], &vs);
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
        let only = contested_block(1, a[1].clone(), &[&v[0], &v[1]], &vs);
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
        let leader = contested_block(1, a[1].clone(), &[&v[0], &v[1]], &vs);
        let backup = contested_block(1, a[2].clone(), &[&v[0], &v[2]], &vs);

        let fwd = vec![leader.clone(), backup.clone()];
        let rev = vec![backup, leader];
        assert_eq!(
            canonical_head(&fwd, &vs).unwrap().hash(),
            canonical_head(&rev, &vs).unwrap().hash(),
            "l'élection est indépendante de l'ordre d'itération (ordre total)"
        );
    }
}
