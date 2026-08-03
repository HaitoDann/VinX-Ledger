use crate::{
    chain::Chain,
    config::NodeConfig,
    mempool::{is_future_nonce, Mempool},
    NodeError,
};
use vinx_core::{amount::Amount, Block, BlockHeader, BlockSignature, Transaction, ValidatorSet};
use vinx_crypto::sha256;
use vinx_state::WorldState;

/// Produces the next block: applies mempool transactions, rewards the producer
/// (collected fees + work emission), commits to the chain, and updates the world state.
///
/// The producing node must be the expected round-robin leader for `next_height`.
/// The block is signed by the proposer (counts as one co-signature toward quorum).
pub fn produce_block(
    state: &mut WorldState,
    chain: &mut Chain,
    mempool: &mut Mempool,
    config: &NodeConfig,
    validator_set: &ValidatorSet,
    timestamp: u64,
) -> Result<Block, NodeError> {
    let next_height = chain.tip_height() + 1;
    let prev_hash = chain.tip_hash();

    // Verify this node is the round-robin leader for the upcoming block
    let expected_leader = validator_set.leader_at(next_height);
    if expected_leader != &config.validator_address {
        return Err(NodeError::Consensus(format!(
            "not the leader for block {next_height}: expected {expected_leader}"
        )));
    }

    // Enforce timestamp monotonicity on the header, then derive the protocol clock
    // (ADR 0005): time-sensitive state transitions (emission, unbonding, upgrade
    // activation) run on the Median Time Past including this block — a single
    // producer cannot jump protocol time via its header timestamp.
    let prev_ts = chain
        .get_block(chain.tip_height())
        .map(|b| b.header.timestamp)
        .unwrap_or(0);
    let timestamp = timestamp.max(prev_ts.saturating_add(1));
    let protocol_ts = chain.median_time_past_with(timestamp);
    state.set_block_context(protocol_ts);

    // Flush staged (unverified) transactions via parallel sig verification pipeline
    let admitted = mempool.flush_staged();
    if admitted > 0 {
        tracing::debug!(
            admitted,
            "Staged transactions verified and admitted in parallel"
        );
    }

    // Update dynamic base fee from mempool pressure (EIP-1559-style surge pricing)
    state.update_base_fee(mempool.size(), config.max_block_txs);
    let current_base_fee = state.base_fee;
    if current_base_fee > state.fee_floor {
        tracing::info!(
            base_fee = %current_base_fee,
            mempool_size = mempool.size(),
            "Surge pricing active"
        );
    }

    let mut block_txs: Vec<vinx_core::Transaction> = Vec::new();

    // Pull pending transactions from mempool and apply them.
    // Signatures were already verified at mempool admission (verified `queues`
    // invariant), so use the trusted apply path — no redundant Ed25519 verify.
    let pending = mempool.drain(config.max_block_txs);
    let mut rejected = 0usize;
    let mut requeue_buf: Vec<vinx_core::Transaction> = Vec::new();
    for tx in pending {
        match state.apply_transaction_trusted(&tx) {
            Ok(()) => block_txs.push(tx),
            Err(e) => {
                tracing::debug!(error = %e, "Transaction rejected during block production");
                if is_future_nonce(&e) {
                    requeue_buf.push(tx);
                } else {
                    rejected += 1;
                }
            }
        }
    }
    if !requeue_buf.is_empty() {
        tracing::debug!(
            count = requeue_buf.len(),
            "Requeueing future-nonce transactions"
        );
        mempool.requeue(requeue_buf);
    }
    if rejected > 0 {
        tracing::warn!(rejected, "Transactions dropped from block");
    }

    // Advance block height before settling so height-based timers see the new height
    state.block_height = next_height;

    // Activate any pending protocol upgrade whose height has been reached
    state.check_upgrade_activation();

    // Reward the producer for its work: collected fees + work emission forged from the
    // Foundry. Also matures any unbonds due — both on the MTP protocol clock (ADR 0005).
    let (fees, emission) = state.settle_block(&config.validator_address, protocol_ts);
    if fees > Amount::ZERO || emission > Amount::ZERO {
        tracing::debug!(fees = %fees, emission = %emission, height = next_height, "Producer rewarded");
    }

    // ADR 0004: the founding invariant must hold. A violation here is a critical
    // internal bug — refuse to seal a block with a corrupted supply.
    if !state.supply_invariant_holds() {
        return Err(NodeError::Consensus(format!(
            "supply invariant violated producing block {next_height} — block not sealed"
        )));
    }

    // Compute Merkle root over all account states after all mutations
    let state_root = state.compute_state_root();
    let receipts_root = compute_receipts_root(&block_txs);

    let header = BlockHeader {
        height: next_height,
        prev_hash,
        timestamp,
        validator: config.validator_address,
        tx_count: block_txs.len() as u32,
        state_root,
        base_fee: current_base_fee.atoms() as u64,
        receipts_root,
    };
    let mut block = Block {
        header,
        transactions: block_txs,
        signatures: Vec::new(),
    };

    // Proposer signs the block header hash (counts as one co-signature)
    let header_hash = block.hash();
    block.signatures.push(BlockSignature {
        validator: config.validator_address,
        pub_key: config.validator_keypair.public_key(),
        signature: config.validator_keypair.sign(&header_hash),
    });

    chain.push(block.clone());

    tracing::info!(
        height = next_height,
        txs = block.header.tx_count,
        base_fee = %current_base_fee,
        "Block produced"
    );

    Ok(block)
}

/// Produces a block as a backup validator stepping in for an offline scheduled leader.
/// Skips the round-robin leader check — any registered validator may call this after
/// the slot timeout has expired.
pub fn produce_block_backup(
    state: &mut WorldState,
    chain: &mut Chain,
    mempool: &mut Mempool,
    config: &NodeConfig,
    validator_set: &ValidatorSet,
    timestamp: u64,
) -> Result<Block, NodeError> {
    // Only registered validators may step in
    if !validator_set.contains(&config.validator_address) {
        return Err(NodeError::Consensus(
            "backup producer is not a registered validator".into(),
        ));
    }
    produce_block_inner(state, chain, mempool, config, validator_set, timestamp)
}

fn produce_block_inner(
    state: &mut WorldState,
    chain: &mut Chain,
    mempool: &mut Mempool,
    config: &NodeConfig,
    _validator_set: &ValidatorSet,
    timestamp: u64,
) -> Result<Block, NodeError> {
    let next_height = chain.tip_height() + 1;
    let prev_hash = chain.tip_hash();

    let prev_ts = chain
        .get_block(chain.tip_height())
        .map(|b| b.header.timestamp)
        .unwrap_or(0);
    let timestamp = timestamp.max(prev_ts.saturating_add(1));
    // ADR 0005: protocol time is the MTP including this block, not the raw header.
    let protocol_ts = chain.median_time_past_with(timestamp);
    state.set_block_context(protocol_ts);

    let admitted = mempool.flush_staged();
    if admitted > 0 {
        tracing::debug!(admitted, "Staged transactions verified and admitted");
    }

    state.update_base_fee(mempool.size(), config.max_block_txs);
    let current_base_fee = state.base_fee;

    let mut block_txs: Vec<vinx_core::Transaction> = Vec::new();
    // Trusted apply: mempool queues hold only signature-verified transactions.
    let pending = mempool.drain(config.max_block_txs);
    let mut rejected = 0usize;
    let mut requeue_buf: Vec<vinx_core::Transaction> = Vec::new();
    for tx in pending {
        match state.apply_transaction_trusted(&tx) {
            Ok(()) => block_txs.push(tx),
            Err(e) => {
                tracing::debug!(error = %e, "Transaction rejected during block production");
                if is_future_nonce(&e) {
                    requeue_buf.push(tx);
                } else {
                    rejected += 1;
                }
            }
        }
    }
    if !requeue_buf.is_empty() {
        mempool.requeue(requeue_buf);
    }
    if rejected > 0 {
        tracing::warn!(rejected, "Transactions dropped from block");
    }

    state.block_height = next_height;
    state.check_upgrade_activation();

    let (fees, emission) = state.settle_block(&config.validator_address, protocol_ts);
    if !state.supply_invariant_holds() {
        return Err(NodeError::Consensus(format!(
            "supply invariant violated producing block {next_height} (backup) — block not sealed"
        )));
    }
    if fees > Amount::ZERO || emission > Amount::ZERO {
        tracing::debug!(fees = %fees, emission = %emission, "Producer rewarded (backup)");
    }

    let state_root = state.compute_state_root();
    let receipts_root = compute_receipts_root(&block_txs);
    let header = BlockHeader {
        height: next_height,
        prev_hash,
        timestamp,
        validator: config.validator_address,
        tx_count: block_txs.len() as u32,
        state_root,
        base_fee: current_base_fee.atoms() as u64,
        receipts_root,
    };
    let mut block = Block {
        header,
        transactions: block_txs,
        signatures: Vec::new(),
    };
    let header_hash = block.hash();
    block.signatures.push(BlockSignature {
        validator: config.validator_address,
        pub_key: config.validator_keypair.public_key(),
        signature: config.validator_keypair.sign(&header_hash),
    });
    chain.push(block.clone());

    tracing::info!(
        height = next_height,
        txs = block.header.tx_count,
        "Block produced (backup)"
    );
    Ok(block)
}

/// Computes a SHA-256 receipts root from the ordered list of included transactions.
/// Empty blocks get the zero hash.  Non-empty blocks: sha256(hash0 || hash1 || ...).
fn compute_receipts_root(txs: &[Transaction]) -> [u8; 32] {
    if txs.is_empty() {
        return [0u8; 32];
    }
    let mut bytes = Vec::with_capacity(txs.len() * 32);
    for tx in txs {
        bytes.extend_from_slice(&tx.hash());
    }
    sha256(&bytes)
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
            validator_address: validator_addr.clone(),
            chain_id: vinx_core::CHAIN_ID_DEVNET,
        });

        let (chain, _) = Chain::new_with_genesis(validator_addr.clone(), 0);
        let mempool = Mempool::default();
        let config = NodeConfig::new(validator_kp);

        (state, chain, mempool, config)
    }

    #[test]
    fn test_produce_first_block_height() {
        let (mut state, mut chain, mut mempool, config) = setup();
        let block = produce_block(
            &mut state,
            &mut chain,
            &mut mempool,
            &config,
            &config.validator_set,
            1_000,
        )
        .unwrap();
        assert_eq!(block.header.height, 1);
    }

    #[test]
    fn test_first_block_has_no_transactions() {
        let (mut state, mut chain, mut mempool, config) = setup();
        let block = produce_block(
            &mut state,
            &mut chain,
            &mut mempool,
            &config,
            &config.validator_set,
            1_000,
        )
        .unwrap();
        assert_eq!(block.transactions.len(), 0);
    }

    #[test]
    fn test_produce_consecutive_blocks() {
        let (mut state, mut chain, mut mempool, config) = setup();
        for i in 1..=5 {
            let block = produce_block(
                &mut state,
                &mut chain,
                &mut mempool,
                &config,
                &config.validator_set,
                i * 10,
            )
            .unwrap();
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
        let tx =
            vinx_core::Transaction::new_transfer(&sender_kp, receiver_addr.clone(), amount, fee, 0);

        mempool.add(tx).unwrap();

        let block = produce_block(
            &mut state,
            &mut chain,
            &mut mempool,
            &config,
            &config.validator_set,
            1_000,
        )
        .unwrap();

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

        let block = produce_block(
            &mut state,
            &mut chain,
            &mut mempool,
            &config,
            &config.validator_set,
            1_000,
        )
        .unwrap();

        assert_eq!(block.header.tx_count, 0);
        assert_eq!(mempool.size(), 0);
    }

    #[test]
    fn test_prev_hash_links_blocks() {
        let (mut state, mut chain, mut mempool, config) = setup();
        let b1 = produce_block(
            &mut state,
            &mut chain,
            &mut mempool,
            &config,
            &config.validator_set,
            10,
        )
        .unwrap();
        let b2 = produce_block(
            &mut state,
            &mut chain,
            &mut mempool,
            &config,
            &config.validator_set,
            20,
        )
        .unwrap();
        assert_eq!(b2.header.prev_hash, b1.hash());
    }

    #[test]
    fn test_fee_goes_to_producer_on_block() {
        let (mut state, mut chain, mut mempool, config) = setup();

        let sender_kp = KeyPair::generate();
        let sender_addr = Address::from_public_key(&sender_kp.public_key());
        state.credit_for_test(sender_addr.clone(), Amount::from_vinx(10_000));

        let amount = Amount::from_vinx(1_000);
        let fee = Amount::from_vinx(1); // explicit fee > floor
        let tx =
            vinx_core::Transaction::new_transfer(&sender_kp, sender_addr.clone(), amount, fee, 0);
        mempool.add(tx).unwrap();

        let foundry_before = state.foundry_balance();
        let producer = config.validator_address;
        let producer_before = state.account_balance(&producer);

        produce_block(
            &mut state,
            &mut chain,
            &mut mempool,
            &config,
            &config.validator_set,
            1_000,
        )
        .unwrap();

        // 100% of the fee goes to the block producer. The Foundry is untouched by fees;
        // the first block only establishes the emission epoch (forging nothing yet).
        assert_eq!(
            state
                .account_balance(&producer)
                .checked_sub(producer_before)
                .unwrap(),
            fee
        );
        assert_eq!(state.foundry_balance(), foundry_before);
    }

    #[test]
    fn test_base_fee_in_header() {
        let (mut state, mut chain, mut mempool, config) = setup();
        let block = produce_block(
            &mut state,
            &mut chain,
            &mut mempool,
            &config,
            &config.validator_set,
            1_000,
        )
        .unwrap();
        // At zero mempool load, base_fee == fee_floor
        assert_eq!(block.header.base_fee, DEFAULT_FEE_FLOOR_ATOMS as u64);
    }
}
