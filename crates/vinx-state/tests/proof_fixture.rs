//! Golden account proofs shared with the TypeScript SDK (ADR 0083): the SDK test suite
//! verifies `sdk/vinx-sdk/src/__fixtures__/account_proof.json`, and this test fails if the
//! Rust side would produce anything else. Regenerate with `VINX_WRITE_FIXTURE=1`.

use vinx_core::Amount;
use vinx_crypto::Address;
use vinx_state::WorldState;

fn fixture() -> serde_json::Value {
    let mut s = WorldState::new();
    let addrs: Vec<Address> = (1u8..=5).map(|i| Address::from_bytes([i; 20])).collect();
    for (i, a) in addrs.iter().enumerate() {
        s.credit_for_test(*a, Amount::from_atoms(1_000_000 + i as u128));
    }
    let absent = Address::from_bytes([9; 20]);
    let root = s.compute_state_root();
    let mut cases = vec![];
    for a in [addrs[2], absent] {
        let p = s.account_proof(&a);
        let acc = s.get_account(&a).map(|acc| {
            serde_json::json!({
                "address": a.to_string(),
                "balance": "",
                "balance_atoms": acc.balance.atoms().to_string(),
                "nonce": acc.nonce,
                "staked": "",
                "staked_atoms": acc.staked.atoms().to_string(),
            })
        });
        cases.push(serde_json::json!({
            "account": acc,
            "proof": {
                "address": a.to_string(),
                "height": 0,
                "state_root": hex::encode(root),
                "accounts_root": hex::encode(p.accounts_root),
                "consensus_root": hex::encode(p.consensus_root),
                "key": hex::encode(p.key),
                "leaf_value": p.leaf_value.map(hex::encode),
                "proof_leaf": p.proof.leaf.map(|(k, v)| [hex::encode(k), hex::encode(v)]),
                "siblings": p.proof.siblings.iter().map(hex::encode).collect::<Vec<_>>(),
                "valid": true,
            }
        }));
    }
    serde_json::json!({ "state_root": hex::encode(root), "cases": cases })
}

#[test]
fn sdk_account_proof_fixture_is_current() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sdk/vinx-sdk/src/__fixtures__/account_proof.json"
    );
    let json = serde_json::to_string_pretty(&fixture()).unwrap() + "\n";
    if std::env::var("VINX_WRITE_FIXTURE").is_ok() {
        std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
        std::fs::write(path, &json).unwrap();
    }
    let on_disk = std::fs::read_to_string(path).expect("fixture missing: VINX_WRITE_FIXTURE=1");
    assert_eq!(
        on_disk, json,
        "SDK proof fixture is stale: rerun with VINX_WRITE_FIXTURE=1"
    );
}
