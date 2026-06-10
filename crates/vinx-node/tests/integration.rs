//! Integration tests — spins up a real HTTP server (port 0) and exercises
//! every RPC endpoint end-to-end.

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

struct TestSetup {
    node: Arc<Node>,
    http: reqwest::Client,
    base_url: String,
    /// Has 21M VINX at genesis.
    admin_kp: KeyPair,
    admin_addr: Address,
}

impl TestSetup {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();

        let admin_kp = KeyPair::generate();
        let validator_kp = KeyPair::generate();

        let admin_addr = Address::from_public_key(&admin_kp.public_key());
        let validator_addr = Address::from_public_key(&validator_kp.public_key());

        let state = create_genesis_state(&GenesisConfig {
            admin_address: admin_addr.clone(),
        });
        let (chain, _) = Chain::new_with_genesis(validator_addr.clone(), 0);

        let config = NodeConfig::new(validator_kp)
            .with_block_time(9999)
            .with_rpc_listen(addr.to_string());

        let node = Node::new(state, chain, config);
        let rpc_node = Arc::clone(&node);
        tokio::spawn(async move {
            let _ = rpc_node.run_rpc_on(listener).await;
        });

        let http = reqwest::Client::new();
        let health_url = format!("http://{}/health", addr);
        for _ in 0..40 {
            if http.get(&health_url).send().await.is_ok() {
                break;
            }
            sleep(Duration::from_millis(25)).await;
        }

        Self {
            node,
            http,
            base_url: format!("http://{}", addr),
            admin_kp,
            admin_addr,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    async fn get_json(&self, path: &str) -> serde_json::Value {
        self.http
            .get(self.url(path))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn post_json(&self, path: &str, body: &impl serde::Serialize) -> reqwest::Response {
        self.http
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .unwrap()
    }

    async fn submit_transfer(
        &self,
        to: &Address,
        amount: Amount,
        nonce: u64,
    ) -> serde_json::Value {
        let fee = amount.calculate_fee(Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS));
        let tx = Transaction::new_transfer(&self.admin_kp, to.clone(), amount, fee, nonce);
        self.post_json("/tx/submit", &tx)
            .await
            .json()
            .await
            .unwrap()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_health_returns_ok() {
    let s = TestSetup::new().await;
    let resp = s.get_json("/health").await;
    assert_eq!(resp["status"], "ok");
    assert_eq!(resp["height"], 0);
    assert_eq!(resp["mempool_pending"], 0);
}

#[tokio::test]
async fn test_chain_height_at_genesis() {
    let s = TestSetup::new().await;
    let resp = s.get_json("/chain/height").await;
    assert_eq!(resp["height"], 0);
}

#[tokio::test]
async fn test_get_genesis_block() {
    let s = TestSetup::new().await;
    let resp = s.get_json("/block/0").await;
    assert_eq!(resp["height"], 0);
    assert!(resp["hash"].as_str().unwrap().len() == 64);
    assert_eq!(resp["tx_count"], 0);
}

#[tokio::test]
async fn test_get_nonexistent_block_returns_404() {
    let s = TestSetup::new().await;
    let resp = s
        .http
        .get(s.url("/block/9999"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_admin_account_has_21m_vinx() {
    let s = TestSetup::new().await;
    let resp = s
        .get_json(&format!("/account/{}", s.admin_addr))
        .await;
    assert_eq!(resp["address"].as_str().unwrap(), s.admin_addr.as_str());
    // 21M VinX = 21_000_000 * 10^18 atoms
    let expected: u128 = 21_000_000 * 1_000_000_000_000_000_000;
    assert_eq!(
        resp["balance_atoms"].as_str().unwrap(),
        expected.to_string()
    );
    assert_eq!(resp["frozen"], false);
    assert_eq!(resp["nonce"], 0);
}

#[tokio::test]
async fn test_get_unknown_account_returns_404() {
    let s = TestSetup::new().await;
    let unknown = Address::from_public_key(&KeyPair::generate().public_key());
    let resp = s
        .http
        .get(s.url(&format!("/account/{}", unknown)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_submit_valid_transfer_accepted() {
    let s = TestSetup::new().await;
    let receiver = Address::from_public_key(&KeyPair::generate().public_key());
    let resp = s.submit_transfer(&receiver, Amount::from_vinx(100), 0).await;
    assert_eq!(resp["accepted"], true);
    assert!(resp["tx_hash"].as_str().unwrap().len() == 64);
}

#[tokio::test]
async fn test_mempool_size_increments_on_submit() {
    let s = TestSetup::new().await;
    assert_eq!(s.get_json("/mempool/size").await["pending"], 0);

    let r = Address::from_public_key(&KeyPair::generate().public_key());
    s.submit_transfer(&r, Amount::from_vinx(1), 0).await;
    assert_eq!(s.get_json("/mempool/size").await["pending"], 1);
}

#[tokio::test]
async fn test_submit_missing_pubkey_rejected() {
    let s = TestSetup::new().await;
    let receiver = Address::from_public_key(&KeyPair::generate().public_key());
    let amount = Amount::from_vinx(1);
    let fee = amount.calculate_fee(Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS));

    let tx = Transaction {
        tx_type: vinx_core::TransactionType::Transfer,
        from: s.admin_addr.clone(),
        to: receiver,
        amount,
        fee,
        nonce: 0,
        pub_key: None,
        signature: None,
    };

    let resp = s.post_json("/tx/submit", &tx).await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_submit_tampered_amount_rejected() {
    let s = TestSetup::new().await;
    let receiver = Address::from_public_key(&KeyPair::generate().public_key());
    let amount = Amount::from_vinx(1);
    let fee = amount.calculate_fee(Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS));

    let mut tx = Transaction::new_transfer(&s.admin_kp, receiver, amount, fee, 0);
    tx.amount = Amount::from_vinx(999_999_999);

    let resp = s.post_json("/tx/submit", &tx).await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_submit_wrong_pubkey_rejected() {
    let s = TestSetup::new().await;
    let attacker_kp = KeyPair::generate();
    let receiver = Address::from_public_key(&KeyPair::generate().public_key());
    let amount = Amount::from_vinx(1);
    let fee = amount.calculate_fee(Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS));

    let mut tx = Transaction::new_transfer(&s.admin_kp, receiver, amount, fee, 0);
    tx.pub_key = Some(attacker_kp.public_key());

    let resp = s.post_json("/tx/submit", &tx).await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_block_production_increments_height() {
    let s = TestSetup::new().await;
    assert_eq!(s.get_json("/chain/height").await["height"], 0);

    s.node.tick().await.unwrap();
    assert_eq!(s.get_json("/chain/height").await["height"], 1);

    s.node.tick().await.unwrap();
    assert_eq!(s.get_json("/chain/height").await["height"], 2);
}

#[tokio::test]
async fn test_first_block_has_no_transactions() {
    let s = TestSetup::new().await;
    s.node.tick().await.unwrap();

    let block = s.get_json("/block/1").await;
    assert_eq!(block["tx_count"], 0);
}

#[tokio::test]
async fn test_transfer_included_in_block_and_balance_updated() {
    let s = TestSetup::new().await;
    let receiver_kp = KeyPair::generate();
    let receiver = Address::from_public_key(&receiver_kp.public_key());
    let amount = Amount::from_vinx(5_000);

    let submit_resp = s.submit_transfer(&receiver, amount, 0).await;
    assert_eq!(submit_resp["accepted"], true);

    let pre = s
        .http
        .get(s.url(&format!("/account/{}", receiver)))
        .send()
        .await
        .unwrap();
    assert_eq!(pre.status(), 404);

    s.node.tick().await.unwrap();

    assert_eq!(s.get_json("/mempool/size").await["pending"], 0);

    let acc = s
        .get_json(&format!("/account/{}", receiver))
        .await;
    let expected_atoms: u128 = 5_000 * 1_000_000_000_000_000_000;
    assert_eq!(
        acc["balance_atoms"].as_str().unwrap(),
        expected_atoms.to_string()
    );

    let admin = s
        .get_json(&format!("/account/{}", s.admin_addr))
        .await;
    assert_eq!(admin["nonce"], 1);
}

#[tokio::test]
async fn test_sequential_transfers_respect_nonces() {
    let s = TestSetup::new().await;
    let alice = Address::from_public_key(&KeyPair::generate().public_key());
    let bob = Address::from_public_key(&KeyPair::generate().public_key());

    s.submit_transfer(&alice, Amount::from_vinx(1_000), 0).await;
    s.submit_transfer(&bob, Amount::from_vinx(2_000), 1).await;

    s.node.tick().await.unwrap();

    let alice_acc = s.get_json(&format!("/account/{}", alice)).await;
    let bob_acc = s.get_json(&format!("/account/{}", bob)).await;

    let alice_atoms: u128 = 1_000 * 1_000_000_000_000_000_000;
    let bob_atoms: u128 = 2_000 * 1_000_000_000_000_000_000;
    assert_eq!(alice_acc["balance_atoms"].as_str().unwrap(), alice_atoms.to_string());
    assert_eq!(bob_acc["balance_atoms"].as_str().unwrap(), bob_atoms.to_string());
}

#[tokio::test]
async fn test_staking_rewards_distributed_at_block_100() {
    let s = TestSetup::new().await;

    let staker_kp = KeyPair::generate();
    let staker = Address::from_public_key(&staker_kp.public_key());

    // Block 1: fund staker from admin (nonce 0)
    s.submit_transfer(&staker, Amount::from_vinx(10_000), 0).await;
    s.node.tick().await.unwrap();

    // Block 2: staker stakes 5 000 VINX
    let stake_tx = Transaction::new_stake(
        &staker_kp,
        Amount::from_vinx(5_000),
        Amount::ZERO,
        0,
    );
    let stake_resp = s.post_json("/tx/submit", &stake_tx).await;
    assert_eq!(stake_resp.status(), 200);
    s.node.tick().await.unwrap();

    // Block 3: admin sends 10 000 VINX to generate a fee
    // Fee = 0.05% of 10 000 = 5 VINX; 80% = 4 VINX to staking pool
    let receiver = Address::from_public_key(&KeyPair::generate().public_key());
    s.submit_transfer(&receiver, Amount::from_vinx(10_000), 1).await;
    s.node.tick().await.unwrap();

    // Also the block 1 transfer generated a fee: 5 VINX, 80% = 4 VINX to pool
    // Pool total = 4 (block 1) + 4 (block 3) = 8 VINX

    // Snapshot balance before distribution window
    let before: u128 = s
        .get_json(&format!("/account/{}", staker))
        .await["balance_atoms"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // Blocks 4..=99 — no transactions, no distribution yet
    for _ in 4..=99 {
        s.node.tick().await.unwrap();
    }
    let mid: u128 = s
        .get_json(&format!("/account/{}", staker))
        .await["balance_atoms"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(mid, before, "No distribution should occur before block 100");

    // Block 100: distribution fires — staker owns 100% of stake → gets all 8 VINX
    s.node.tick().await.unwrap();

    let after: u128 = s
        .get_json(&format!("/account/{}", staker))
        .await["balance_atoms"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let expected_reward = Amount::from_vinx(8).atoms();
    assert_eq!(
        after - before,
        expected_reward,
        "Staker should receive 8 VINX at block 100 (before={before}, after={after})"
    );
}
