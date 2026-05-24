use crate::{chain::Chain, config::NodeConfig, mempool::Mempool, NodeError};
use vinx_core::{amount::Amount, Block, BlockHeader, Transaction};
use vinx_state::WorldState;

/// Produces the next block: emits protocol tokens, applies mempool transactions,
/// commits to the chain, and updates the world state.
pub fn produce_block(
    state: &mut WorldState,
    chain: &mut Chain,
    mempool: &mut Mempool,
    config: &NodeConfig,
    timestamp: u64,
) -> Result<Block, NodeError> {
    let next_height = chain.tip_height() + 1;
    let prev_hash = chain.tip_hash();

    let mut block_txs: Vec<Transaction> = Vec::new();

    // Emit protocol tokens for this block (stops after 10-year emission period)
    let emit_amount = WorldState::emission_amount_for_block(next_height);
    if emit_amount > Amount::ZERO {
        let emission_tx = Transaction::new_emission(
            config.public_sale_pool.clone(),
            emit_amount,
            config.validator_address.clone(), // from field is unused for emission
        );
        match state.apply_transaction(&emission_tx) {
            Ok(()) => block_txs.push(emission_tx),
            Err(e) => tracing::warn!(height = next_height, error = %e, "Emission skipped"),
        }
    }

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

    let header = BlockHeader {
        height: next_height,
        prev_hash,
        timestamp,
        validator: config.validator_address.clone(),
        tx_count: block_txs.len() as u32,
        state_root: [0u8; 32], // Merkle root — placeholder until merkle module is added
    };
    let block = Block {
        header,
        transactions: block_txs,
    };

    chain.push(block.clone());
    state.block_height = next_height;

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
    use vinx_core::amount::{Amount, DEFAULT_FEE_FLOOR_ATOMS, EMISSION_PER_BLOCK_ATOMS};
    use vinx_crypto::{Address, KeyPair};
    use vinx_state::{create_genesis_state, GenesisConfig};

    fn setup() -> (WorldState, Chain, Mempool, NodeConfig) {
        let validator_kp = KeyPair::generate();
        let validator_addr = Address::from_public_key(&validator_kp.public_key());
        let pool_addr = Address::from_public_key(&KeyPair::generate().public_key());
        let admin_addr = Address::from_public_key(&KeyPair::generate().public_key());

        let state = create_genesis_state(&GenesisConfig {
            admin_address: admin_addr,
            reserve_address: validator_addr.clone(),
        });

        let (chain, _) = Chain::new_with_genesis(validator_addr.clone(), 0);
        let mempool = Mempool::default();
        let config = NodeConfig::new(validator_kp, pool_addr);

        (state, chain, mempool, config)
    }

    #[test]
    fn test_produce_first_block_height() {
        let (mut state, mut chain, mut mempool, config) = setup();
        let block = produce_block(&mut state, &mut chain, &mut mempool, &config, 1_000).unwrap();
        assert_eq!(block.header.height, 1);
    }

    #[test]
    fn test_first_block_contains_emission() {
        let (mut state, mut chain, mut mempool, config) = setup();
        let block = produce_block(&mut state, &mut chain, &mut mempool, &config, 1_000).unwrap();
        // Block 1 should contain exactly 1 emission transaction
        assert_eq!(block.transactions.len(), 1);
        assert_eq!(
            block.transactions[0].tx_type,
            vinx_core::TransactionType::Emission
        );
    }

    #[test]
    fn test_emission_credited_to_pool() {
        let (mut state, mut chain, mut mempool, config) = setup();
        let pool = config.public_sale_pool.clone();
        produce_block(&mut state, &mut chain, &mut mempool, &config, 1_000).unwrap();
        assert_eq!(
            state.account_balance(&pool).atoms(),
            EMISSION_PER_BLOCK_ATOMS
        );
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

        // Fund a sender (use admin address from genesis)
        let sender_kp = KeyPair::generate();
        let sender_addr = Address::from_public_key(&sender_kp.public_key());
        let receiver_addr = Address::from_public_key(&KeyPair::generate().public_key());

        // Manually credit the sender so they have funds
        state.credit_for_test(sender_addr.clone(), Amount::from_vinx(10_000));

        let amount = Amount::from_vinx(100);
        let fee = amount.calculate_fee(Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS));
        let tx = vinx_core::Transaction::new_transfer(&sender_kp, receiver_addr.clone(), amount, fee, 0);

        mempool.add(tx).unwrap();

        let block = produce_block(&mut state, &mut chain, &mut mempool, &config, 1_000).unwrap();

        // 1 emission + 1 transfer = 2 transactions
        assert_eq!(block.header.tx_count, 2);
        assert_eq!(state.account_balance(&receiver_addr), amount);
        assert_eq!(mempool.size(), 0);
    }

    #[test]
    fn test_invalid_mempool_tx_dropped() {
        let (mut state, mut chain, mut mempool, config) = setup();

        // Create a tx signed by an account with no balance (will fail on apply)
        let broke_kp = KeyPair::generate();
        let receiver = Address::from_public_key(&KeyPair::generate().public_key());
        let amount = Amount::from_vinx(9999);
        let fee = amount.calculate_fee(Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS));
        let bad_tx = vinx_core::Transaction::new_transfer(&broke_kp, receiver, amount, fee, 0);

        mempool.add(bad_tx).unwrap();

        let block = produce_block(&mut state, &mut chain, &mut mempool, &config, 1_000).unwrap();

        // Only emission tx — bad tx was silently dropped
        assert_eq!(block.header.tx_count, 1);
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
