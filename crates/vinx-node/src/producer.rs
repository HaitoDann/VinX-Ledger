use crate::{chain::Chain, config::NodeConfig, mempool::Mempool, NodeError};
use vinx_core::{amount::Amount, Block, BlockHeader, BlockSignature};
use vinx_state::WorldState;

/// Produces the next block: applies mempool transactions, distributes staking rewards,
/// commits to the chain, and updates the world state.
///
/// The producing node must be the expected round-robin leader for `next_height`.
/// The block is signed by the proposer (counts as one co-signature toward quorum).
pub fn produce_block(
    state: &mut WorldState,
    chain: &mut Chain,
    mempool: &mut Mempool,
    config: &NodeConfig,
    timestamp: u64,
) -> Result<Block, NodeError> {
    let next_height = chain.tip_height() + 1;
    let prev_hash = chain.tip_hash();

    // Verify this node is the round-robin leader for the upcoming block
    let expected_leader = config.validator_set.leader_at(next_height);
    if expected_leader != &config.validator_address {
        return Err(NodeError::Consensus(format!(
            "not the leader for block {next_height}: expected {expected_leader}"
        )));
    }

    let mut block_txs: Vec<vinx_core::Transaction> = Vec::new();

    // Pull pending transactions from mempool and apply them
    let pending = mempool.drain(config.max_block_txs);
    let mut rejected = 0usize;
    for tx in pending {
        match state.apply_transaction(&tx) {
            Ok(()) => block_txs.push(tx),
            Err(e) => {
                tracing::debug!(error = %e, "Transaction rejected during block production");
                rejected += 1;
            }
        }
    }
    if rejected > 0 {
        tracing::warn!(rejected, "Transactions dropped from block");
    }

    // Advance block height before distributing so the interval check sees the new height
    state.block_height = next_height;

    // Auto-unfreeze accounts whose 12-month judicial freeze has expired
    state.check_auto_unfreeze();

    // Activate any pending protocol upgrade whose height has been reached
    state.check_upgrade_activation();

    // Distribute accumulated staking fees every STAKING_DISTRIBUTION_INTERVAL blocks
    let rewards = state.distribute_staking_rewards();
    if rewards > Amount::ZERO {
        tracing::debug!(rewards = %rewards, height = next_height, "Staking rewards distributed");
    }

    // Compute Merkle root over all account states after all mutations
    let state_root = state.compute_state_root();

    let header = BlockHeader {
        height: next_height,
        prev_hash,
        timestamp,
        validator: config.validator_address.clone(),
        tx_count: block_txs.len() as u32,
        state_root,
    };
    let mut block = Block {
        header,
        transactions: block_txs,
        signatures: Vec::new(),
    };

    // Proposer signs the block header hash (counts as one co-signature)
    let header_hash = block.hash();
    block.signatures.push(BlockSignature {
        validator: config.validator_address.clone(),
        pub_key: config.validator_keypair.public_key(),
        signature: config.validator_keypair.sign(&header_hash),
    });

    chain.push(block.clone());

    tracing::info!(
        height = next_height,
        txs = block.header.tx_count,
        "Block produced"
    );

    Ok(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::Chain;
    use crate::config::NodeConfig;
    use crate::mempool::Mempool;
    use vinx_core::amount::{Amount, DEFAULT_FEE_FLOOR_ATOMS};
    use vinx_crypto::{Address, KeyPair};
    use vinx_state::{create_genesis_state, GenesisConfig};

    fn setup() -> (WorldState, Chain, Mempool, NodeConfig) {
        let validator_kp = KeyPair::generate();
        let validator_addr = Address::from_public_key(&validator_kp.public_key());
        let admin_addr = Address::from_public_key(&KeyPair::generate().public_key());

        let state = create_genesis_state(&GenesisConfig {
            admin_address: admin_addr,
        });

        let (chain, _) = Chain::new_with_genesis(validator_addr.clone(), 0);
        let mempool = Mempool::default();
        let config = NodeConfig::new(validator_kp);

        (state, chain, mempool, config)
    }

    #[test]
    fn test_produce_first_block_height() {
        let (mut state, mut chain, mut mempool, config) = setup();
        let block = produce_block(&mut state, &mut chain, &mut mempool, &config, 1_000).unwrap();
        assert_eq!(block.header.height, 1);
    }

    #[test]
    fn test_first_block_has_no_transactions() {
        let (mut state, mut chain, mut mempool, config) = setup();
        let block = produce_block(&mut state, &mut chain, &mut mempool, &config, 1_000).unwrap();
        assert_eq!(block.transactions.len(), 0);
    }

    #[test]
    fn test_produce_consecutive_blocks() {
        let (mut state, mut chain, mut mempool, config) = setup();
        for i in 1..=5 {
            let block = produce_block(&mut state, &mut chain, &mut mempool, &config, i * 10).unwrap();
            assert_eq!(block.header.height, i);
        }
        assert_eq!(chain.tip_height(), 5);
        assert_eq!(state.block_height, 5);
    }

    #[test]
    fn test_mempool_tx_included_in_block() {
        let (mut state, mut chain, mut mempool, config) = setup();

        let sender_kp = KeyPair::generate();
        let sender_addr = Address::from_public_key(&sender_kp.public_key());
        let receiver_addr = Address::from_public_key(&KeyPair::generate().public_key());

        state.credit_for_test(sender_addr.clone(), Amount::from_vinx(10_000));

        let amount = Amount::from_vinx(100);
        let fee = amount.calculate_fee(Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS));
        let tx = vinx_core::Transaction::new_transfer(&sender_kp, receiver_addr.clone(), amount, fee, 0);

        mempool.add(tx).unwrap();

        let block = produce_block(&mut state, &mut chain, &mut mempool, &config, 1_000).unwrap();

        assert_eq!(block.header.tx_count, 1);
        assert_eq!(state.account_balance(&receiver_addr), amount);
        assert_eq!(mempool.size(), 0);
    }

    #[test]
    fn test_invalid_mempool_tx_dropped() {
        let (mut state, mut chain, mut mempool, config) = setup();

        let broke_kp = KeyPair::generate();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let amount = Amount::from_vinx(9999);
        let fee = amount.calculate_fee(Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS));
        let bad_tx = vinx_core::Transaction::new_transfer(&broke_kp, receiver, amount, fee, 0);

        mempool.add(bad_tx).unwrap();

        let block = produce_block(&mut state, &mut chain, &mut mempool, &config, 1_000).unwrap();

        // Bad tx dropped — empty block
        assert_eq!(block.header.tx_count, 0);
        assert_eq!(mempool.size(), 0);
    }

    #[test]
    fn test_prev_hash_links_blocks() {
        let (mut state, mut chain, mut mempool, config) = setup();
        let b1 = produce_block(&mut state, &mut chain, &mut mempool, &config, 10).unwrap();
        let b2 = produce_block(&mut state, &mut chain, &mut mempool, &config, 20).unwrap();
        assert_eq!(b2.header.prev_hash, b1.hash());
    }
}
