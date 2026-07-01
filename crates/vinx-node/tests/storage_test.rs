//! Tests for the storage persistence layer.

use vinx_core::amount::Amount;
use vinx_crypto::{Address, KeyPair};
use vinx_node::{chain::Chain, config::NodeConfig, storage::Storage, Node};
use vinx_state::{create_genesis_state, GenesisConfig};

fn make_addr() -> (KeyPair, Address) {
    let kp = KeyPair::generate();
    let addr = Address::from_public_key(&kp.public_key());
    (kp, addr)
}

/// Creates a temp directory that is cleaned up when dropped.
struct TmpDir(std::path::PathBuf);
impl TmpDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "vinx_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn test_storage_roundtrip() {
    let tmp = TmpDir::new();
    let (_, admin) = make_addr();
    let (_, validator) = make_addr();

    let mut state = create_genesis_state(&GenesisConfig {
        chain_id: vinx_core::CHAIN_ID_DEVNET,
        admin_address: admin.clone(),
        validator_address: validator.clone(),
    });
    let (chain, _) = Chain::new_with_genesis(validator.clone(), 0);

    let storage = Storage::new(tmp.path());
    assert!(!storage.exists());

    storage.save(&mut state, &chain).unwrap();
    assert!(storage.exists());

    let (loaded_state, loaded_chain) = storage.load().expect("should load");
    assert_eq!(loaded_state.block_height, state.block_height);
    assert_eq!(
        loaded_state.account_balance(&admin),
        Amount::from_vinx(21_000_000)
    );
    assert_eq!(loaded_chain.tip_height(), chain.tip_height());
    assert_eq!(loaded_chain.tip_hash(), chain.tip_hash());
}

#[test]
fn test_incremental_persist_writes_only_dirty_rows() {
    let tmp = TmpDir::new();
    let (_, admin) = make_addr();
    let (_, validator) = make_addr();

    let mut state = create_genesis_state(&GenesisConfig {
        chain_id: vinx_core::CHAIN_ID_DEVNET,
        admin_address: admin.clone(),
        validator_address: validator.clone(),
    });
    let (chain, _) = Chain::new_with_genesis(validator.clone(), 0);
    let storage = Storage::new(tmp.path());

    // Initial full save flushes every account and clears the dirty set.
    storage.save(&mut state, &chain).unwrap();

    // Touch a single new account, then persist incrementally.
    let (_, bob) = make_addr();
    state.credit_for_test(bob.clone(), Amount::from_vinx(500));
    let write = Storage::serialize_incremental(&mut state, &chain).unwrap();
    // Only the changed account is written — not the whole account set.
    assert_eq!(write.account_rows.len(), 1);
    assert!(!write.replace_accounts);
    storage.write_state(write).unwrap();

    // Both the untouched genesis account and the new one survive a reload.
    let (loaded, _) = storage.load().expect("should load");
    assert_eq!(
        loaded.account_balance(&admin),
        Amount::from_vinx(21_000_000)
    );
    assert_eq!(loaded.account_balance(&bob), Amount::from_vinx(500));
}

#[test]
fn test_storage_load_returns_none_when_missing() {
    let tmp = TmpDir::new();
    let storage = Storage::new(tmp.path());
    assert!(storage.load().is_none());
}

#[tokio::test]
async fn test_state_persists_across_node_restarts() {
    let tmp = TmpDir::new();
    let (_, admin_addr) = make_addr();
    let (validator_kp, validator_addr) = make_addr();

    // ── First run: produce 5 blocks ──────────────────────────────────────────
    {
        let state = create_genesis_state(&GenesisConfig {
            chain_id: vinx_core::CHAIN_ID_DEVNET,
            admin_address: admin_addr.clone(),
            validator_address: validator_addr.clone(),
        });
        let (chain, _) = Chain::new_with_genesis(validator_addr.clone(), 0);
        let config = NodeConfig::new(validator_kp.clone()).with_data_dir(tmp.path());
        let node = Node::new(state, chain, config);

        for _ in 0..5 {
            node.tick().await.unwrap();
        }
        // Persist via the node's own storage (avoids opening a second redb instance)
        node.persist().await;
        assert_eq!(node.chain.read().await.tip_height(), 5);
    }

    // ── Second run: resume from disk ─────────────────────────────────────────
    {
        let storage = Storage::new(tmp.path());
        let (state, chain) = storage.load().expect("persisted data must exist");

        // Height and state should be exactly where we left off
        assert_eq!(chain.tip_height(), 5);
        assert_eq!(state.block_height, 5);
        assert_eq!(
            state.account_balance(&admin_addr),
            Amount::from_vinx(21_000_000)
        );
    }
}
