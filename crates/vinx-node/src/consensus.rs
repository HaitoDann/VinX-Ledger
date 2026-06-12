use vinx_core::{Block, BlockSignature, ValidatorSet};
use vinx_crypto::{Address, KeyPair};

use crate::NodeError;

/// Validates that a block has the correct round-robin proposer and enough co-signatures.
/// Genesis blocks (height 0) are exempt from both checks.
pub fn validate_block(block: &Block, validator_set: &ValidatorSet) -> Result<(), NodeError> {
    if block.is_genesis() {
        return Ok(());
    }

    let expected = validator_set.leader_at(block.header.height);
    if expected != &block.header.validator {
        return Err(NodeError::Consensus(format!(
            "wrong proposer at height {}: expected {expected}, got {}",
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
            },
            transactions: vec![],
            signatures: vec![],
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
    fn test_wrong_proposer_rejected() {
        let validators = kps(2);
        let vs = ValidatorSet::new(validators.iter().map(addr_of).collect());

        // height 1 % 2 = 1 → validators[1] is the leader
        // But we declare validators[0] as proposer — wrong!
        let mut block = make_block(1, addr_of(&validators[0]));
        sign_block(&mut block, &validators[0], &vs).unwrap();
        sign_block(&mut block, &validators[1], &vs).unwrap();

        assert!(validate_block(&block, &vs).is_err());
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
}
