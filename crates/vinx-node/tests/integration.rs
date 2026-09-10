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
        chain_id: vinx_core::CHAIN_ID_DEVNET,
        admin_address: admin_addr.clone(),
        validator_address: validator_addr.clone(),
        validator_bls: None,
    });

    let (chain, _genesis_block) = Chain::new_with_genesis(validator_addr.clone(), 0);

    let config = NodeConfig::new(validator_kp)
        // Very long block time so auto-ticking never fires during tests
        .with_block_time(9_999)
        .with_rpc_listen(local_addr.to_string())
        // Admin routes are fail-closed: without a token they refuse everything,
        // so tests exercising them need one configured.
        .with_admin_token("test-admin-token");

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
        addr.to_bech32(),
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
        let tx = Transaction::new_transfer(&sender_kp, receiver_addr.clone(), amount, fee, nonce);
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

// ─── Test — faucet endpoint ───────────────────────────────────────────────────

/// POST /faucet/request drips tokens to the requested address and enforces
/// per-address cooldown (second request within cooldown window must return 400).
#[tokio::test]
async fn test_faucet_endpoint() {
    use vinx_node::chain::Chain;
    use vinx_node::config::NodeConfig;
    use vinx_node::Node;
    use vinx_state::{create_genesis_state, GenesisConfig};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();

    let admin_kp = KeyPair::generate();
    let validator_kp = KeyPair::generate();
    let faucet_kp = KeyPair::generate();

    let admin_addr = Address::from_public_key(&admin_kp.public_key());
    let validator_addr = Address::from_public_key(&validator_kp.public_key());
    let faucet_addr = Address::from_public_key(&faucet_kp.public_key());

    let recipient_kp = KeyPair::generate();
    let recipient_addr = Address::from_public_key(&recipient_kp.public_key());

    let state = create_genesis_state(&GenesisConfig {
        chain_id: vinx_core::CHAIN_ID_DEVNET,
        admin_address: admin_addr.clone(),
        validator_address: validator_addr.clone(),
        validator_bls: None,
    });
    let (chain, _) = Chain::new_with_genesis(validator_addr.clone(), 0);

    const FAUCET_ATOMS: u128 = 100 * 1_000_000_000_000_000_000; // 100 VinX

    let config = NodeConfig::new(validator_kp)
        .with_block_time(9_999)
        .with_rpc_listen(local_addr.to_string())
        .with_faucet(faucet_kp, FAUCET_ATOMS, 86_400);

    let node = Node::new(state, chain, config);

    // Fund the faucet account
    {
        let mut s = node.state.write().await;
        s.credit_for_test(faucet_addr.clone(), Amount::from_vinx(10_000));
    }

    let rpc_node = std::sync::Arc::clone(&node);
    tokio::spawn(async move {
        let _ = rpc_node.run_rpc_on(listener).await;
    });

    let http = reqwest::Client::new();
    let base = format!("http://{}", local_addr);
    for _ in 0..40 {
        if http.get(format!("{}/health", base)).send().await.is_ok() {
            break;
        }
        sleep(Duration::from_millis(25)).await;
    }

    // First request — must be accepted
    let resp: serde_json::Value = http
        .post(format!("{}/faucet/request", base))
        .json(&serde_json::json!({ "address": recipient_addr.to_string() }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(
        resp["accepted"], true,
        "faucet first request should be accepted"
    );
    assert!(
        !resp["tx_hash"].as_str().unwrap().is_empty(),
        "tx_hash must be non-empty"
    );
    assert_eq!(
        resp["amount_atoms"]
            .as_str()
            .unwrap()
            .parse::<u128>()
            .unwrap(),
        FAUCET_ATOMS,
        "dripped amount must match configured amount"
    );

    // Produce a block so the transaction is applied and we can check balances
    node.tick().await.unwrap();

    let account: serde_json::Value = http
        .get(format!("{}/account/{}", base, recipient_addr))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let balance: u128 = account["balance_atoms"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        balance, FAUCET_ATOMS,
        "recipient should have received exactly FAUCET_ATOMS"
    );

    // Second request within cooldown — must be rate-limited (400)
    let status = http
        .post(format!("{}/faucet/request", base))
        .json(&serde_json::json!({ "address": recipient_addr.to_string() }))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(
        status, 400,
        "second faucet request within cooldown must return 400"
    );

    // Request for a different address must still succeed (per-address cooldown)
    let other_addr = Address::from_public_key(&KeyPair::generate().public_key());
    let resp2: serde_json::Value = http
        .post(format!("{}/faucet/request", base))
        .json(&serde_json::json!({ "address": other_addr.to_string() }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        resp2["accepted"], true,
        "different address should not be rate-limited"
    );
}

// ─── Test — crash recovery ────────────────────────────────────────────────────

/// Simulates a node crash by dropping the node after persisting state, then
/// reloads from disk and verifies that block height, account balances and
/// circulating supply are fully preserved.
#[tokio::test]
async fn test_crash_recovery() {
    use vinx_node::chain::Chain;
    use vinx_node::config::NodeConfig;
    use vinx_node::storage::Storage;
    use vinx_node::Node;
    use vinx_state::{create_genesis_state, GenesisConfig};

    // Unique temp dir per test run (avoids collisions in parallel runs)
    let data_dir = std::env::temp_dir().join(format!(
        "vinx_crash_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&data_dir).unwrap();

    let validator_kp = KeyPair::generate();
    let admin_kp = KeyPair::generate();
    let sender_kp = KeyPair::generate();
    let receiver_kp = KeyPair::generate();

    let admin_addr = Address::from_public_key(&admin_kp.public_key());
    let validator_addr = Address::from_public_key(&validator_kp.public_key());
    let sender_addr = Address::from_public_key(&sender_kp.public_key());
    let receiver_addr = Address::from_public_key(&receiver_kp.public_key());

    const INITIAL_VINX: u64 = 10_000;
    const SEND_VINX: u64 = 100;
    const N_TXS: u64 = 3;
    const N_BLOCKS: u64 = 3;

    // ── Phase 1 : run, produce blocks, persist ────────────────────────────
    let (saved_height, saved_supply, saved_sender, saved_receiver) = {
        let state = create_genesis_state(&GenesisConfig {
            chain_id: vinx_core::CHAIN_ID_DEVNET,
            admin_address: admin_addr.clone(),
            validator_address: validator_addr.clone(),
            validator_bls: None,
        });
        let (chain, _) = Chain::new_with_genesis(validator_addr.clone(), 0);
        let config = NodeConfig::new(validator_kp.clone())
            .with_block_time(9_999)
            .with_data_dir(&data_dir);

        let node = Node::new(state, chain, config);

        {
            let mut s = node.state.write().await;
            // Mint tokens so the supply invariant holds (ADR 0004/ADR 0040): the
            // node produces blocks below, which enforce it in settle_block.
            s.credit_emit_for_test(sender_addr.clone(), Amount::from_vinx(INITIAL_VINX));
        }

        let amount = Amount::from_vinx(SEND_VINX);
        let fee = amount.calculate_fee(Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS));

        for nonce in 0..N_TXS {
            let tx =
                Transaction::new_transfer(&sender_kp, receiver_addr.clone(), amount, fee, nonce);
            node.mempool.write().await.add(tx).unwrap();
        }

        for _ in 0..N_BLOCKS {
            node.tick().await.unwrap();
        }

        node.persist().await;

        let s = node.chain.read().await;
        let st = node.state.read().await;
        (
            s.tip_height(),
            st.circulating_supply.atoms(),
            st.account_balance(&sender_addr).atoms(),
            st.account_balance(&receiver_addr).atoms(),
        )
        // Node dropped here — simulates crash
    };

    assert_eq!(
        saved_height, N_BLOCKS,
        "should have produced {N_BLOCKS} blocks"
    );

    // ── Phase 2 : reload from disk, verify nothing was lost ───────────────
    let storage = Storage::new(&data_dir);
    let (recovered_state, recovered_chain) = storage
        .load()
        .expect("persisted data must be loadable after crash");

    assert_eq!(
        recovered_chain.tip_height(),
        saved_height,
        "chain height must survive crash"
    );
    assert_eq!(
        recovered_state.circulating_supply.atoms(),
        saved_supply,
        "circulating_supply must survive crash"
    );
    assert_eq!(
        recovered_state.account_balance(&sender_addr).atoms(),
        saved_sender,
        "sender balance must survive crash"
    );
    assert_eq!(
        recovered_state.account_balance(&receiver_addr).atoms(),
        saved_receiver,
        "receiver balance must survive crash"
    );

    // Sanity: receiver actually received tokens (N_TXS × SEND_VINX atoms)
    let expected_receiver = N_TXS as u128 * Amount::from_vinx(SEND_VINX).atoms();
    assert_eq!(
        recovered_state.account_balance(&receiver_addr).atoms(),
        expected_receiver,
        "receiver should hold exactly N_TXS × SEND_VINX after recovery"
    );

    std::fs::remove_dir_all(&data_dir).ok();
}

// ─── AdminAction governance test ─────────────────────────────────────────────

#[tokio::test]
async fn test_admin_action_adds_validator() {
    let (node, _base_url) = start_test_node().await;

    // Generate admin and new validator keypairs
    let admin_kp = KeyPair::generate();
    let new_val_kp = KeyPair::generate();
    let admin_addr = Address::from_public_key(&admin_kp.public_key());
    let new_val_addr = Address::from_public_key(&new_val_kp.public_key());

    let bond = vinx_core::amount::MIN_VALIDATOR_BOND_ATOMS;

    // Override admin address, credit the admin, and fund the candidate so it can bond.
    {
        let mut s = node.state.write().await;
        s.admin_address = Some(admin_addr.clone());
        s.credit_for_test(admin_addr.clone(), Amount::from_vinx(1_000));
        s.credit_for_test(new_val_addr.clone(), Amount::from_atoms(bond));
    }

    // The candidate posts the minimum validator bond (required for admission). Since
    // ADR 0075 §3.1 the bond that enters the pool must carry the validator's BLS key.
    let chain_id = node.state.read().await.chain_id;
    let stake_tx = vinx_core::Transaction::new_stake_with_bls(
        &new_val_kp,
        Amount::from_atoms(bond),
        Amount::ZERO,
        0,
        &vinx_crypto::BlsSecretKey::generate(),
        chain_id,
    );
    node.mempool.write().await.add(stake_tx).unwrap();
    node.tick().await.expect("tick bond");

    // Build AdminAction tx to add new_val_addr as validator
    let action = vinx_core::GovernanceAction::AddValidator(new_val_addr.clone());
    let nonce = 0u64;
    let tx = vinx_core::Transaction::new_admin_action(&admin_kp, &action, nonce);

    node.mempool.write().await.add(tx).unwrap();
    node.tick().await.expect("tick");

    // Verify new validator was added
    let s = node.state.read().await;
    assert!(
        s.validator_set.contains(&new_val_addr),
        "new validator should be in set after AdminAction"
    );
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

    assert_eq!(resp["from"], 0, "sync response should start from height 0");
    assert_eq!(
        resp["count"], 4,
        "sync should return 4 blocks: genesis + 3 produced"
    );

    let blocks = resp["blocks"]
        .as_array()
        .expect("blocks should be an array");
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

// ─── Test 7 — Dynamic validator set: join request ────────────────────────────

/// A node can submit a validator join request via POST /validators/request.
/// An admin can list pending requests via GET /validators/pending.
#[tokio::test]
async fn test_validator_join_request() {
    let (node, base_url) = start_test_node().await;
    let client = reqwest::Client::new();

    let new_kp = KeyPair::generate();
    let new_addr = Address::from_public_key(&new_kp.public_key()).to_string();

    // Submit join request
    let resp = client
        .post(format!("{}/validators/request", base_url))
        .json(&serde_json::json!({
            "address": new_addr,
            "p2p_multiaddr": "/ip4/1.2.3.4/tcp/9000",
            "message": "Joining testnet as node B"
        }))
        .send()
        .await
        .expect("POST /validators/request");

    assert_eq!(resp.status(), 200, "join request should be accepted");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["accepted"], true);
    assert_eq!(body["address"], new_addr);

    // Admin lists pending requests
    let token = node.config.admin_token.clone().unwrap_or_default();
    let list: serde_json::Value = client
        .get(format!("{}/validators/pending", base_url))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .expect("GET /validators/pending")
        .json()
        .await
        .unwrap();

    assert_eq!(list["count"], 1, "one pending request");
    assert_eq!(list["requests"][0]["address"], new_addr);
    assert_eq!(
        list["requests"][0]["p2p_multiaddr"],
        "/ip4/1.2.3.4/tcp/9000"
    );
}

// ─── Test 8 — Dynamic validator set: liveness tracking ───────────────────────

/// After tick() is called, the /validators endpoint should reflect the node
/// as having been seen at the latest height (online).
#[tokio::test]
async fn test_validator_liveness_tracking() {
    let (node, base_url) = start_test_node().await;
    let client = reqwest::Client::new();

    let validator_addr = node.config.validator_address.to_string();

    // Before any block — node has never produced yet
    let before: serde_json::Value = client
        .get(format!("{}/validators", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let validators = before["validators"].as_array().unwrap();
    let me = validators
        .iter()
        .find(|v| v["address"] == validator_addr)
        .expect("our validator should be in the set");
    // Not yet online (no block produced on this node)
    assert_eq!(
        me["online"], false,
        "online should be false before first block"
    );

    // Produce 2 blocks
    node.tick().await.expect("tick 1");
    node.tick().await.expect("tick 2");

    // Now check liveness
    let after: serde_json::Value = client
        .get(format!("{}/validators", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let validators = after["validators"].as_array().unwrap();
    let me = validators
        .iter()
        .find(|v| v["address"] == validator_addr)
        .expect("our validator should still be in the set");

    assert_eq!(
        me["online"], true,
        "online should be true after producing blocks"
    );
    assert_eq!(
        me["last_seen_height"], 2,
        "last_seen_height should be 2 after two ticks"
    );
    assert_eq!(
        after["next_leader"], validator_addr,
        "single validator is always next leader"
    );
}

// ─── Test 9 — Dynamic validator set: add via AdminAction ─────────────────────

/// Admin adds a validator; the new set is reflected immediately in /validators.
/// Verify quorum is updated correctly (n=2 → quorum=2).
#[tokio::test]
async fn test_add_validator_updates_set() {
    let (node, base_url) = start_test_node().await;
    let client = reqwest::Client::new();

    // Get initial state
    let before: serde_json::Value = client
        .get(format!("{}/validators", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before["count"], 1, "should start with 1 validator");
    assert_eq!(before["quorum"], 1);

    // Admin adds a new validator
    let admin_kp = {
        // The admin key is the genesis admin — we need to retrieve it from the node setup.
        // In test setup `start_test_node` the admin kp is local; re-use the same approach:
        // write a tx directly to state bypassing the kp.
        // Instead: directly mutate the validator set for this test.
        let new_kp = KeyPair::generate();
        let new_addr = Address::from_public_key(&new_kp.public_key());
        node.state.write().await.validator_set.add(new_addr.clone());
        *node.validator_set.write().await = node.state.read().await.validator_set.clone();
        new_kp
    };
    let _ = admin_kp; // suppress unused warning

    // Check updated validator set
    let after: serde_json::Value = client
        .get(format!("{}/validators", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["count"], 2, "should now have 2 validators");
    assert_eq!(after["quorum"], 2, "quorum for n=2 is ceil(4/3)=2");
}

// ─── Stateful mempool admission (anti-spam) ──────────────────────────────────

/// An unfunded (nonexistent) account cannot park transactions in the mempool,
/// no matter how high a fee it claims — the fee-priority queue only admits
/// funded transactions.
#[tokio::test]
async fn test_admission_rejects_unfunded_sender() {
    let (node, base_url) = start_test_node().await;
    let client = reqwest::Client::new();

    let sender_kp = KeyPair::generate(); // never credited
    let receiver = Address::from_public_key(&KeyPair::generate().public_key());
    // Huge claimed fee: without stateful admission this would jump the queue.
    let tx = Transaction::new_transfer(
        &sender_kp,
        receiver,
        Amount::from_vinx(1),
        Amount::from_vinx(1_000_000),
        0,
    );

    let resp = client
        .post(format!("{}/tx/submit", base_url))
        .json(&tx)
        .send()
        .await
        .expect("POST /tx/submit");
    assert_eq!(resp.status(), 400, "unfunded sender must be rejected");
    assert_eq!(
        node.mempool.read().await.size(),
        0,
        "mempool must stay empty"
    );
}

/// A transaction signed for another network (wrong chain_id) is rejected at
/// admission instead of wasting a mempool slot until block production.
#[tokio::test]
async fn test_admission_rejects_wrong_chain_id() {
    let (node, base_url) = start_test_node().await;
    let client = reqwest::Client::new();

    let sender_kp = KeyPair::generate();
    let sender_addr = Address::from_public_key(&sender_kp.public_key());
    let receiver = Address::from_public_key(&KeyPair::generate().public_key());
    {
        let mut state = node.state.write().await;
        state.credit_for_test(sender_addr.clone(), Amount::from_vinx(1_000));
    }

    let amount = Amount::from_vinx(1);
    let fee = amount.calculate_fee(Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS));
    let mut tx = Transaction::new_transfer(&sender_kp, receiver, amount, fee, 0);
    tx.chain_id = 999; // not this network
    tx.sign(&sender_kp); // re-sign so the signature commits to the bogus chain id

    let resp = client
        .post(format!("{}/tx/submit", base_url))
        .json(&tx)
        .send()
        .await
        .expect("POST /tx/submit");
    assert_eq!(resp.status(), 400, "wrong chain_id must be rejected");
    assert_eq!(node.mempool.read().await.size(), 0);
}

/// A nonce absurdly far ahead of the account nonce is rejected (bounds
/// nonce-gap parking); a nonce within the window is accepted.
#[tokio::test]
async fn test_admission_bounds_nonce_window() {
    let (node, base_url) = start_test_node().await;
    let client = reqwest::Client::new();

    let sender_kp = KeyPair::generate();
    let sender_addr = Address::from_public_key(&sender_kp.public_key());
    let receiver = Address::from_public_key(&KeyPair::generate().public_key());
    {
        let mut state = node.state.write().await;
        state.credit_for_test(sender_addr.clone(), Amount::from_vinx(1_000));
    }

    let amount = Amount::from_vinx(1);
    let fee = amount.calculate_fee(Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS));

    // Way beyond MAX_NONCE_AHEAD (64) → rejected.
    let far = Transaction::new_transfer(&sender_kp, receiver.clone(), amount, fee, 1_000);
    let resp = client
        .post(format!("{}/tx/submit", base_url))
        .json(&far)
        .send()
        .await
        .expect("POST /tx/submit");
    assert_eq!(resp.status(), 400, "far-future nonce must be rejected");

    // A small gap (future nonce within the window) is fine — it waits in the
    // per-account queue for the missing nonce.
    let near = Transaction::new_transfer(&sender_kp, receiver, amount, fee, 3);
    let resp = client
        .post(format!("{}/tx/submit", base_url))
        .json(&near)
        .send()
        .await
        .expect("POST /tx/submit");
    assert_eq!(resp.status(), 200, "nonce within the window is admitted");
}

/// The cumulative funding rule: a sender cannot queue more total spend than its
/// balance, even if each transaction individually fits.
#[tokio::test]
async fn test_admission_enforces_cumulative_funding() {
    let (node, base_url) = start_test_node().await;
    let client = reqwest::Client::new();

    let sender_kp = KeyPair::generate();
    let sender_addr = Address::from_public_key(&sender_kp.public_key());
    let receiver = Address::from_public_key(&KeyPair::generate().public_key());

    let amount = Amount::from_vinx(60);
    let fee = amount.calculate_fee(Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS));
    // Fund exactly one transfer (amount + fee) — not two.
    {
        let mut state = node.state.write().await;
        state.credit_for_test(sender_addr.clone(), amount.checked_add(fee).unwrap());
    }

    let tx0 = Transaction::new_transfer(&sender_kp, receiver.clone(), amount, fee, 0);
    let resp = client
        .post(format!("{}/tx/submit", base_url))
        .json(&tx0)
        .send()
        .await
        .expect("POST /tx/submit");
    assert_eq!(resp.status(), 200, "first transfer fits the balance");

    let tx1 = Transaction::new_transfer(&sender_kp, receiver, amount, fee, 1);
    let resp = client
        .post(format!("{}/tx/submit", base_url))
        .json(&tx1)
        .send()
        .await
        .expect("POST /tx/submit");
    assert_eq!(
        resp.status(),
        400,
        "second transfer exceeds the balance once the queued one is counted"
    );
    assert_eq!(node.mempool.read().await.size(), 1);
}
