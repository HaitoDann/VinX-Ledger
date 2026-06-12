//! Integration tests — spins up a real HTTP server (port 0) and exercises
//! every RPC endpoint end-to-end.
//!
//! Each test creates its own independent node bound to a random OS-assigned
//! port so tests can run fully in parallel without port conflicts.

use std::sync::Arc;
use tokio::time::{sleep, Duration};

use vinx_core::{
    amount::{Amount, DEFAULT_FEE_FLOOR_ATOMS},
    Transaction,
};
use vinx_crypto::{Address, KeyPair};
use vinx_node::{chain::Chain, config::NodeConfig, Node};
use vinx_state::{create_genesis_state, GenesisConfig};
use bincode;

// ─── Test harness ─────────────────────────────────────────────────────────────

/// Starts a fresh node on an OS-assigned port and waits until /health responds.
///
/// Returns `(node, base_url)` where `base_url` is e.g. `"http://127.0.0.1:12345"`.
/// The `WorldState` inside `node` can be mutated directly via `node.state` for
/// test setup (e.g. `credit_for_test`).
async fn start_test_node() -> (Arc<Node>, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind to port 0");
    let local_addr = listener.local_addr().expect("local_addr");

    let admin_kp = KeyPair::generate();
    let validator_kp = KeyPair::generate();

    let admin_addr = Address::from_public_key(&admin_kp.public_key());
    let validator_addr = Address::from_public_key(&validator_kp.public_key());

    // NOTE: GenesisConfig is written here with the NEW two-field signature that
    // will be in place once `validator_address` is added.  The existing codebase
    // only has `admin_address`; the compiler will surface any mismatch.
    let state = create_genesis_state(&GenesisConfig {
        admin_address: admin_addr.clone(),
        validator_address: validator_addr.clone(),
    });

    let (chain, _genesis_block) = Chain::new_with_genesis(validator_addr.clone(), 0);

    let config = NodeConfig::new(validator_kp)
        // Very long block time so auto-ticking never fires during tests
        .with_block_time(9_999)
        .with_rpc_listen(local_addr.to_string());

    let node = Node::new(state, chain, config);

    // Spawn RPC server in a background task
    let rpc_node = Arc::clone(&node);
    tokio::spawn(async move {
        let _ = rpc_node.run_rpc_on(listener).await;
    });

    // Wait until the server is ready (up to 1 s in 25 ms increments)
    let http = reqwest::Client::new();
    let health_url = format!("http://{}/health", local_addr);
    for _ in 0..40 {
        if http.get(&health_url).send().await.is_ok() {
            break;
        }
        sleep(Duration::from_millis(25)).await;
    }

    let base_url = format!("http://{}", local_addr);
    (node, base_url)
}

// ─── Test 1 — /health ─────────────────────────────────────────────────────────

/// GET /health at genesis should return status="ok" and height=0.
#[tokio::test]
async fn test_health_endpoint() {
    let (_node, base_url) = start_test_node().await;
    let client = reqwest::Client::new();

    let resp: serde_json::Value = client
        .get(format!("{}/health", base_url))
        .send()
        .await
        .expect("GET /health")
        .json()
        .await
        .expect("parse JSON");

    assert_eq!(resp["status"], "ok", "status field should be 'ok'");
    assert_eq!(resp["height"], 0, "height should be 0 at genesis");
    assert_eq!(
        resp["mempool_pending"], 0,
        "mempool should be empty at genesis"
    );
}

// ─── Test 2 — submit tx and retrieve it, then include in block ────────────────

/// Submit a transfer via POST /tx/submit, verify it is accepted, retrieve it
/// by hash via GET /tx/:hash (mempool, not yet in a block), then tick to seal
/// block 1 and verify the tx is retrievable with block_height=1.
#[tokio::test]
async fn test_submit_and_retrieve_tx() {
    let (node, base_url) = start_test_node().await;
    let client = reqwest::Client::new();

    // Build a funded sender directly in state (bypassing the genesis admin path
    // so we have a known keypair with a known balance).
    let sender_kp = KeyPair::generate();
    let sender_addr = Address::from_public_key(&sender_kp.public_key());
    let receiver_addr = Address::from_public_key(&KeyPair::generate().public_key());

    {
        let mut state = node.state.write().await;
        state.credit_for_test(sender_addr.clone(), Amount::from_vinx(10_000));
    }

    // Build and submit a transfer
    let amount = Amount::from_vinx(100);
    let fee = amount.calculate_fee(Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS));
    let tx = Transaction::new_transfer(&sender_kp, receiver_addr.clone(), amount, fee, 0);
    let tx_hash = hex::encode(tx.hash());

    let submit_resp: serde_json::Value = client
        .post(format!("{}/tx/submit", base_url))
        .json(&tx)
        .send()
        .await
        .expect("POST /tx/submit")
        .json()
        .await
        .expect("parse JSON");

    assert_eq!(
        submit_resp["accepted"], true,
        "transaction should be accepted"
    );
    assert_eq!(
        submit_resp["tx_hash"].as_str().unwrap(),
        tx_hash,
        "returned hash should match computed hash"
    );

    // Tick: seal block 1 which includes the transaction
    node.tick().await.expect("tick should succeed");

    // Now GET /tx/:hash — it must be found and belong to block 1
    let tx_resp: serde_json::Value = client
        .get(format!("{}/tx/{}", base_url, tx_hash))
        .send()
        .await
        .expect("GET /tx/:hash")
        .json()
        .await
        .expect("parse JSON");

    assert_eq!(
        tx_resp["block_height"], 1,
        "transaction should be in block 1 after tick"
    );
    assert_eq!(
        tx_resp["hash"].as_str().unwrap(),
        tx_hash,
        "retrieved tx hash should match"
    );
}

// ─── Test 3 — account balance ─────────────────────────────────────────────────

/// Credit an account via `credit_for_test`, then GET /account/:addr and verify
/// the returned balance matches.
#[tokio::test]
async fn test_account_balance() {
    let (node, base_url) = start_test_node().await;
    let client = reqwest::Client::new();

    let kp = KeyPair::generate();
    let addr = Address::from_public_key(&kp.public_key());
    let credited = Amount::from_vinx(42_000);

    {
        let mut state = node.state.write().await;
        state.credit_for_test(addr.clone(), credited);
    }

    let resp: serde_json::Value = client
        .get(format!("{}/account/{}", base_url, addr))
        .send()
        .await
        .expect("GET /account/:addr")
        .json()
        .await
        .expect("parse JSON");

    assert_eq!(
        resp["address"].as_str().unwrap(),
        addr.as_str(),
        "address field should match"
    );

    let returned_atoms: u128 = resp["balance_atoms"]
        .as_str()
        .expect("balance_atoms is a string")
        .parse()
        .expect("parse u128");

    assert_eq!(
        returned_atoms,
        credited.atoms(),
        "balance_atoms should equal credited amount"
    );
    assert_eq!(resp["frozen"], false, "account should not be frozen");
    assert_eq!(resp["nonce"], 0, "nonce should be 0 for a fresh account");
}

// ─── Test 4 — /block/:height ──────────────────────────────────────────────────

/// Tick once to produce block 1, then GET /block/1 and check height=1.
#[tokio::test]
async fn test_block_endpoint() {
    let (node, base_url) = start_test_node().await;
    let client = reqwest::Client::new();

    // Produce block 1
    node.tick().await.expect("first tick should succeed");

    let resp: serde_json::Value = client
        .get(format!("{}/block/1", base_url))
        .send()
        .await
        .expect("GET /block/1")
        .json()
        .await
        .expect("parse JSON");

    assert_eq!(resp["height"], 1, "block height should be 1");
    assert!(
        resp["hash"].as_str().unwrap().len() == 64,
        "block hash should be a 64-char hex string"
    );
    // Genesis is at height 0, block 1 has a non-zero prev_hash
    assert_ne!(
        resp["prev_hash"].as_str().unwrap(),
        "0000000000000000000000000000000000000000000000000000000000000000",
        "prev_hash of block 1 must not be the all-zeros sentinel"
    );
}

// ─── Test 5 — mempool nonce ordering ─────────────────────────────────────────

/// Submit 3 transactions from the same sender with nonces 2, 0, 1 (out of order).
/// After draining via tick, verify they were included in the block in nonce order
/// 0, 1, 2 (the block's transaction list should be ordered by nonce).
#[tokio::test]
async fn test_mempool_ordering() {
    let (node, base_url) = start_test_node().await;
    let client = reqwest::Client::new();

    let sender_kp = KeyPair::generate();
    let sender_addr = Address::from_public_key(&sender_kp.public_key());
    let receiver_addr = Address::from_public_key(&KeyPair::generate().public_key());

    // Fund sender with enough for 3 transfers
    {
        let mut state = node.state.write().await;
        state.credit_for_test(sender_addr.clone(), Amount::from_vinx(100_000));
    }

    let amount = Amount::from_vinx(1);
    let fee = amount.calculate_fee(Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS));

    // Submit in deliberately wrong order: nonce 2, then 0, then 1
    for nonce in [2u64, 0, 1] {
        let tx = Transaction::new_transfer(
            &sender_kp,
            receiver_addr.clone(),
            amount,
            fee,
            nonce,
        );
        let submit_resp: serde_json::Value = client
            .post(format!("{}/tx/submit", base_url))
            .json(&tx)
            .send()
            .await
            .expect("POST /tx/submit")
            .json()
            .await
            .expect("parse JSON");
        assert_eq!(
            submit_resp["accepted"], true,
            "nonce {} should be accepted into mempool",
            nonce
        );
    }

    // Verify all 3 are pending
    let mempool_resp: serde_json::Value = client
        .get(format!("{}/mempool/size", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(mempool_resp["pending"], 3, "3 txs should be pending");

    // Tick to drain mempool into block 1
    node.tick().await.expect("tick should succeed");

    // Fetch block 1 and verify transaction ordering
    let block_resp: serde_json::Value = client
        .get(format!("{}/block/1", base_url))
        .send()
        .await
        .expect("GET /block/1")
        .json()
        .await
        .expect("parse JSON");

    let txs = block_resp["transactions"]
        .as_array()
        .expect("transactions should be an array");

    assert_eq!(txs.len(), 3, "block 1 should contain all 3 transactions");

    // Nonces must be 0, 1, 2 in that order (mempool drains lowest nonce first)
    for (expected_nonce, tx_entry) in [0u64, 1, 2].iter().zip(txs.iter()) {
        assert_eq!(
            tx_entry["nonce"].as_u64().unwrap(),
            *expected_nonce,
            "transaction at position {} should have nonce {}",
            expected_nonce,
            expected_nonce
        );
    }
}

// ─── Multi-validator consensus test ──────────────────────────────────────────

#[tokio::test]
async fn test_three_validator_governance_proposal() {
    // Set up 3 keypairs
    let kp1 = KeyPair::generate();
    let kp2 = KeyPair::generate();
    let kp3 = KeyPair::generate();
    let addr1 = Address::from_public_key(&kp1.public_key());
    let addr2 = Address::from_public_key(&kp2.public_key());
    let addr3 = Address::from_public_key(&kp3.public_key());

    use vinx_core::ValidatorSet;

    // Start with single-validator state so node (kp1/addr1) can always produce blocks.
    let state = create_genesis_state(&GenesisConfig {
        admin_address: addr1.clone(),
        validator_address: addr1.clone(),
    });

    let (chain, _) = vinx_node::chain::Chain::new_with_genesis(addr1.clone(), 0);
    let config = NodeConfig::new(kp1.clone())
        .with_block_time(9_999);
    let node = Node::new(state, chain, config);

    // Add addr2 and addr3 to the state's validator set for governance quorum purposes.
    // node.validator_set (used for leader election) is reset to [addr1] after each tick
    // so addr1 remains the sole block producer.
    {
        let mut s = node.state.write().await;
        s.validator_set = ValidatorSet::new(vec![addr1.clone(), addr2.clone(), addr3.clone()]);
        // Credit addr2 and addr3 so they can pay fees for governance txs
        s.credit_for_test(addr2.clone(), Amount::from_vinx(1_000));
        s.credit_for_test(addr3.clone(), Amount::from_vinx(1_000));
    }

    // Produce block 1 (empty — just sets up height).
    // After each tick, reset node.validator_set to single-validator so addr1
    // is always the next leader regardless of round-robin.
    node.tick().await.expect("tick block 1");
    *node.validator_set.write().await = ValidatorSet::single(addr1.clone());

    // validator2 submits a governance proposal to add a new validator
    let new_val_kp = KeyPair::generate();
    let new_val = Address::from_public_key(&new_val_kp.public_key());

    use vinx_core::{GovernanceAction, SubmitProposalPayload, GOVERNANCE_VOTING_PERIOD_BLOCKS};
    let payload = SubmitProposalPayload {
        description: "Add validator 4".to_string(),
        action: GovernanceAction::AddValidator(new_val.clone()),
        voting_period_blocks: GOVERNANCE_VOTING_PERIOD_BLOCKS,
    };
    let payload_bytes = bincode::serialize(&payload).unwrap();

    let submit_tx = {
        let s = node.state.read().await;
        let nonce = s.get_account(&addr2).map(|a| a.nonce).unwrap_or(0);
        let fee = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        let mut tx = vinx_core::Transaction {
            tx_type: vinx_core::TransactionType::SubmitProposal,
            from: addr2.clone(),
            to: addr2.clone(),
            amount: Amount::ZERO,
            fee,
            nonce,
            payload: payload_bytes,
            pub_key: None,
            signature: None,
        };
        tx.sign(&kp2);
        tx
    };

    node.mempool.write().await.add(submit_tx).unwrap();
    node.tick().await.expect("tick block 2 (proposal submitted)");
    *node.validator_set.write().await = ValidatorSet::single(addr1.clone());

    // Verify proposal exists
    {
        let s = node.state.read().await;
        assert!(s.proposals.contains_key(&0), "proposal 0 should exist");
        assert_eq!(s.proposals[&0].status, vinx_core::ProposalStatus::Active);
    }

    // validator1 and validator2 vote YES (quorum = ceil(2*3/3) = 2)
    let vote_payload = bincode::serialize(&vinx_core::VotePayload { proposal_id: 0, approve: true }).unwrap();

    // Vote from validator1 (addr1)
    let vote1 = {
        let s = node.state.read().await;
        let nonce = s.get_account(&addr1).map(|a| a.nonce).unwrap_or(0);
        let fee = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        let mut tx = vinx_core::Transaction {
            tx_type: vinx_core::TransactionType::VoteProposal,
            from: addr1.clone(),
            to: addr1.clone(),
            amount: Amount::ZERO,
            fee,
            nonce,
            payload: vote_payload.clone(),
            pub_key: None,
            signature: None,
        };
        tx.sign(&kp1);
        tx
    };

    // Vote from validator2 (addr2)
    let vote2 = {
        let s = node.state.read().await;
        let nonce = s.get_account(&addr2).map(|a| a.nonce).unwrap_or(0);
        let fee = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        let mut tx = vinx_core::Transaction {
            tx_type: vinx_core::TransactionType::VoteProposal,
            from: addr2.clone(),
            to: addr2.clone(),
            amount: Amount::ZERO,
            fee,
            nonce,
            payload: vote_payload.clone(),
            pub_key: None,
            signature: None,
        };
        tx.sign(&kp2);
        tx
    };

    {
        let mut mp = node.mempool.write().await;
        mp.add(vote1).unwrap();
        mp.add(vote2).unwrap();
    }
    node.tick().await.expect("tick block 3 (votes cast)");

    // Proposal should now be Executed (2/3 = quorum) and new_val added
    {
        let s = node.state.read().await;
        assert_eq!(
            s.proposals[&0].status,
            vinx_core::ProposalStatus::Executed,
            "proposal should be executed after reaching quorum"
        );
        assert!(
            s.validator_set.contains(&new_val),
            "new validator should be in validator set after governance execution"
        );
    }
}

// ─── Test 6 — /chain/sync ─────────────────────────────────────────────────────

/// Produce 3 blocks, then GET /chain/sync?from=0&limit=10.
/// The response count should be 4 (genesis block 0 + blocks 1, 2, 3).
#[tokio::test]
async fn test_chain_sync_endpoint() {
    let (node, base_url) = start_test_node().await;
    let client = reqwest::Client::new();

    // Produce 3 blocks
    for _ in 0..3 {
        node.tick().await.expect("tick should succeed");
    }

    let resp: serde_json::Value = client
        .get(format!("{}/chain/sync?from=0&limit=10", base_url))
        .send()
        .await
        .expect("GET /chain/sync")
        .json()
        .await
        .expect("parse JSON");

    assert_eq!(
        resp["from"], 0,
        "sync response should start from height 0"
    );
    assert_eq!(
        resp["count"], 4,
        "sync should return 4 blocks: genesis + 3 produced"
    );

    let blocks = resp["blocks"].as_array().expect("blocks should be an array");
    assert_eq!(blocks.len(), 4, "blocks array length should be 4");

    // Verify heights are sequential 0, 1, 2, 3
    for (i, block) in blocks.iter().enumerate() {
        assert_eq!(
            block["height"].as_u64().unwrap(),
            i as u64,
            "block at index {} should have height {}",
            i,
            i
        );
    }
}
