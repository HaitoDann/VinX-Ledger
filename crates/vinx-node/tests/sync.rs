//! Élagage, nœud archive et synchronisation par l'état, sur un vrai réseau local.

// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

use libp2p::{Multiaddr, PeerId};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use vinx_core::{AdminAction, AdminTransaction, SignedAdminTransaction, UNITS_PER_VINX};
use vinx_crypto::{Address, KeyPair};
use vinx_node::network::{self, NetConfig};
use vinx_node::node::{self, SharedRef};
use vinx_state::Genesis;

const CHAIN: &str = "vinx-devnet-sync";

struct Spec {
    dir: PathBuf,
    port: u16,
    peer: PeerId,
}

fn spec(root: &Path, name: &str) -> Spec {
    let dir = root.join(name);
    let peer = network::load_or_create_identity(&dir)
        .unwrap()
        .public()
        .to_peer_id();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    Spec { dir, port, peer }
}

struct Running {
    shared: SharedRef,
    stop: watch::Sender<bool>,
    tasks: Vec<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

impl Running {
    fn height(&self) -> u64 {
        node::lock(&self.shared).chain.state.height
    }

    async fn stop(self) {
        self.stop.send(true).unwrap();
        for t in self.tasks {
            t.await.unwrap().unwrap();
        }
    }
}

/// Démarre un nœud. `retention_ms = None` : nœud archive.
fn start(
    genesis: &Genesis,
    me: &Spec,
    peers: &[&Spec],
    allowed: &[&Spec],
    key: Option<KeyPair>,
    retention_ms: Option<u64>,
) -> Running {
    let shared = node::open(genesis.clone(), &me.dir).unwrap();
    let identity = network::load_or_create_identity(&me.dir).unwrap();
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    let (in_tx, in_rx) = mpsc::unbounded_channel();
    let (req_tx, req_rx) = mpsc::unbounded_channel();
    {
        let mut s = node::lock(&shared);
        s.outbound = Some(out_tx);
        s.requests = Some(req_tx);
        s.retention_ms = retention_ms;
    }
    let (stop, stop_rx) = watch::channel(false);
    let config = NetConfig {
        listen: format!("/ip4/127.0.0.1/tcp/{}", me.port).parse().unwrap(),
        peers: peers
            .iter()
            .map(|p| -> Multiaddr {
                format!("/ip4/127.0.0.1/tcp/{}/p2p/{}", p.port, p.peer)
                    .parse()
                    .unwrap()
            })
            .collect(),
        allowed: allowed.iter().map(|a| a.peer).collect(),
    };
    let net = tokio::spawn(network::run(
        identity,
        config,
        shared.clone(),
        out_rx,
        req_rx,
        in_tx,
        stop_rx.clone(),
    ));
    let consensus = tokio::spawn(node::run(shared.clone(), key, in_rx, stop_rx));
    Running {
        shared,
        stop,
        tasks: vec![net, consensus],
    }
}

async fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..600 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("délai dépassé : {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_nodes_join_by_snapshot_or_from_an_archive() {
    let root = tempfile::tempdir().unwrap();
    let admins = [
        KeyPair::generate(),
        KeyPair::generate(),
        KeyPair::generate(),
    ];
    let validator = KeyPair::generate();
    let mut genesis = Genesis::standard(
        CHAIN,
        node::now_ms() / 1000 - 1,
        [
            admins[0].public_key(),
            admins[1].public_key(),
            admins[2].public_key(),
        ],
        vec![validator.public_key()],
    )
    .unwrap()
    .with_faucet(1_000 * UNITS_PER_VINX)
    .unwrap();
    genesis.params.min_block_interval_ms = 100;
    // Époques courtes : l'instantané d'état transporte aussi l'émission.
    genesis.epoch_blocks = 4;

    let v = spec(root.path(), "validateur");
    let archive = spec(root.path(), "archive");
    let fresh = spec(root.path(), "nouveau");
    let from_archive = spec(root.path(), "depuis-archive");

    // Validateur et nœud archive. Le validateur ne commencera à élaguer qu'une
    // fois l'archive à jour : sinon, elle n'aurait plus de quoi rejouer la chaîne.
    let validator_node = start(
        &genesis,
        &v,
        &[],
        &[&archive, &fresh],
        Some(validator),
        None,
    );
    let archive_node = start(&genesis, &archive, &[&v], &[&from_archive], None, None);
    wait_until("archive connectée", || archive_node.height() >= 3).await;
    // Élague tout bloc de plus de 500 ms.
    node::lock(&validator_node.shared).retention_ms = Some(500);

    // Un paiement, pour que l'état diffère de la genèse.
    let alice = Address::from_public_key(&KeyPair::generate().public_key());
    let tx = AdminTransaction {
        chain_id: CHAIN.into(),
        nonce: 1,
        action: AdminAction::TreasuryTransfer {
            to: alice,
            amount: 9 * UNITS_PER_VINX,
            memo: None,
        },
    };
    let tx = SignedAdminTransaction {
        approvals: vec![tx.approve(0, &admins[0]), tx.approve(2, &admins[2])],
        tx,
    };
    node::lock(&validator_node.shared)
        .mempool
        .add_admin(tx)
        .unwrap();
    wait_until("chaîne avancée", || validator_node.height() >= 30).await;

    // Le validateur a élagué ; le nœud archive a tout gardé.
    let lowest = node::lock(&validator_node.shared)
        .store
        .lowest_height()
        .unwrap()
        .unwrap();
    assert!(
        lowest > 10,
        "le validateur aurait dû élaguer (plus bas : {lowest})"
    );
    wait_until("archive à jour", || archive_node.height() >= 30).await;
    assert_eq!(
        node::lock(&archive_node.shared)
            .store
            .lowest_height()
            .unwrap(),
        Some(1)
    );

    // Un nouveau nœud, relié au seul validateur : les blocs manquants ont été
    // élagués, il doit passer par un instantané d'état.
    let fresh_node = start(&genesis, &fresh, &[&v], &[], None, Some(500));
    let target = validator_node.height() + 5;
    wait_until("synchronisation par l'état", || {
        fresh_node.height() >= target
    })
    .await;
    {
        let f = node::lock(&fresh_node.shared);
        assert_eq!(f.chain.state.balance(&alice), 9 * UNITS_PER_VINX);
        assert!(f.chain.state.check_invariant().is_ok());
        // Il n'a jamais reçu les premiers blocs.
        assert!(f.store.block(1).unwrap().is_none());
    }

    // Un autre nouveau nœud, relié à l'archive : il rejoue toute la chaîne.
    let replay_node = start(&genesis, &from_archive, &[&archive], &[], None, None);
    let target = validator_node.height() + 5;
    wait_until("rattrapage depuis l'archive", || {
        replay_node.height() >= target
    })
    .await;
    assert!(
        node::lock(&replay_node.shared)
            .store
            .block(1)
            .unwrap()
            .is_some()
    );

    // Tous les nœuds ont la même chaîne : le dernier bloc du nœud synchronisé par
    // l'état (qui a élagué les autres) est celui de l'archive à la même hauteur.
    let (h, last) = {
        let f = node::lock(&fresh_node.shared);
        (f.chain.state.height, f.chain.last.as_ref().unwrap().hash())
    };
    wait_until("archive à la même hauteur", || archive_node.height() >= h).await;
    let in_archive = |n: &Running| {
        node::lock(&n.shared)
            .store
            .block(h)
            .unwrap()
            .map(|b| b.hash())
    };
    assert_eq!(in_archive(&archive_node), Some(last));
    wait_until("rejeu à la même hauteur", || replay_node.height() >= h).await;
    assert_eq!(in_archive(&replay_node), Some(last));

    // Un nœud qui redémarre après une synchronisation par l'état repart de son disque.
    let fresh_height = fresh_node.height();
    let fresh_root = node::lock(&fresh_node.shared).chain.state.state_root();
    fresh_node.stop().await;
    let reopened = node::open(genesis.clone(), &fresh.dir).unwrap();
    {
        let r = node::lock(&reopened);
        assert!(r.chain.state.height >= fresh_height);
        if r.chain.state.height == fresh_height {
            assert_eq!(r.chain.state.state_root(), fresh_root);
        }
    }

    for n in [validator_node, archive_node, replay_node] {
        n.stop().await;
    }
}
