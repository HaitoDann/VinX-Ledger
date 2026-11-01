//! Réseau P2P réel en local : 4 validateurs, démarrage décalé, panne, relais,
//! rattrapage, liste autorisée.

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
use vinx_node::network::{self, NetConfig, NetMessage};
use vinx_node::node::{self, SharedRef};
use vinx_state::Genesis;

const CHAIN: &str = "vinx-devnet-net";

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Running {
    shared: SharedRef,
    stop: watch::Sender<bool>,
    tasks: Vec<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

impl Running {
    async fn stop(self) {
        self.stop.send(true).unwrap();
        for t in self.tasks {
            t.await.unwrap().unwrap();
        }
    }

    /// Arrêt brutal, comme un `kill -9` : les tâches sont abandonnées en
    /// plein travail, sans arrêt propre. La base est relâchée une fois les
    /// tâches terminées.
    async fn crash(self) {
        for t in &self.tasks {
            t.abort();
        }
        for t in self.tasks {
            let _ = t.await;
        }
    }

    fn height(&self) -> u64 {
        node::lock(&self.shared).chain.state.height
    }

    fn block_hash(&self, h: u64) -> Option<[u8; 32]> {
        node::lock(&self.shared)
            .store
            .block(h)
            .unwrap()
            .map(|b| b.hash())
    }
}

struct Spec {
    dir: PathBuf,
    port: u16,
    peer: PeerId,
}

fn start(genesis: &Genesis, me: &Spec, others: &[&Spec], key: Option<KeyPair>) -> Running {
    start_delayed(genesis, me, others, key, 0)
}

fn start_delayed(
    genesis: &Genesis,
    me: &Spec,
    others: &[&Spec],
    key: Option<KeyPair>,
    delay_ms: u64,
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
    }
    let (stop, stop_rx) = watch::channel(false);
    let peers: Vec<Multiaddr> = others
        .iter()
        .map(|o| {
            format!("/ip4/127.0.0.1/tcp/{}/p2p/{}", o.port, o.peer)
                .parse()
                .unwrap()
        })
        .collect();
    let config = NetConfig {
        listen: format!("/ip4/127.0.0.1/tcp/{}", me.port).parse().unwrap(),
        peers,
        allowed: Vec::new(),
    };
    let shared2 = shared.clone();
    let stop2 = stop_rx.clone();
    let net = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        network::run(identity, config, shared2, out_rx, req_rx, in_tx, stop2).await
    });
    let consensus = tokio::spawn(node::run(shared.clone(), key, in_rx, stop_rx));
    Running {
        shared,
        stop,
        tasks: vec![net, consensus],
    }
}

fn spec(root: &Path, i: usize) -> Spec {
    let dir = root.join(format!("n{i}"));
    let peer = network::load_or_create_identity(&dir)
        .unwrap()
        .public()
        .to_peer_id();
    Spec {
        dir,
        port: free_port(),
        peer,
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
async fn four_validators_tolerate_a_crash_and_resync() {
    let root = tempfile::tempdir().unwrap();
    let admins = [
        KeyPair::generate(),
        KeyPair::generate(),
        KeyPair::generate(),
    ];
    let validators: Vec<KeyPair> = (0..4).map(|_| KeyPair::generate()).collect();
    let mut genesis = Genesis::standard(
        CHAIN,
        node::now_ms() / 1000 - 1,
        [
            admins[0].public_key(),
            admins[1].public_key(),
            admins[2].public_key(),
        ],
        validators.iter().map(KeyPair::public_key).collect(),
    )
    .unwrap()
    .with_faucet(1_000 * UNITS_PER_VINX)
    .unwrap();
    genesis.params.min_block_interval_ms = 200;
    genesis.params.timeout_propose_ms = 1_500;
    genesis.params.timeout_vote_ms = 800;
    // Époques courtes : les récompenses sont versées pendant le test, et les
    // quatre nœuds doivent s'accorder dessus (même hash de bloc).
    genesis.epoch_blocks = 2;

    let specs: Vec<Spec> = (0..4).map(|i| spec(root.path(), i)).collect();
    let others = |i: usize| -> Vec<&Spec> {
        specs
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, s)| s)
            .collect()
    };
    // Les nœuds 3 et 4 rejoignent le réseau 4 s après les autres : leurs premiers
    // messages sont perdus. Sans retransmission, la chaîne ne démarrait jamais.
    let mut nodes: Vec<Option<Running>> = (0..4)
        .map(|i| {
            Some(start_delayed(
                &genesis,
                &specs[i],
                &others(i),
                Some(validators[i].clone()),
                if i >= 2 { 4_000 } else { 0 },
            ))
        })
        .collect();

    // 1. Les quatre nœuds finalisent la même chaîne.
    wait_until("hauteur 3 partout", || {
        nodes.iter().flatten().all(|n| n.height() >= 3)
    })
    .await;
    let reference = nodes[0].as_ref().unwrap().block_hash(3);
    for n in nodes.iter().flatten() {
        assert_eq!(n.block_hash(3), reference);
    }
    // Récompenses de l'époque close à la hauteur 2 : des validateurs payés.
    {
        let n1 = nodes[0].as_ref().unwrap();
        let s = node::lock(&n1.shared);
        let paid: u64 = validators
            .iter()
            .map(|v| {
                s.chain
                    .state
                    .balance(&Address::from_public_key(&v.public_key()))
            })
            .sum();
        assert!(paid > 0, "aucune récompense versée");
        assert!(s.chain.state.emission.emitted > 0);
    }

    // 2. Panne du nœud 4 : les trois autres continuent.
    nodes[3].take().unwrap().stop().await;
    let h = nodes[0].as_ref().unwrap().height();
    wait_until("progression sans le nœud 4", || {
        nodes.iter().flatten().all(|n| n.height() >= h + 3)
    })
    .await;

    // 3. Une transaction soumise au nœud 1 est relayée et finalisée partout.
    let alice = Address::from_public_key(&KeyPair::generate().public_key());
    let tx = AdminTransaction {
        chain_id: CHAIN.into(),
        nonce: 1,
        action: AdminAction::TreasuryTransfer {
            to: alice,
            amount: 7 * UNITS_PER_VINX,
            memo: None,
        },
    };
    let tx = SignedAdminTransaction {
        approvals: vec![tx.approve(0, &admins[0]), tx.approve(1, &admins[1])],
        tx,
    };
    {
        let n1 = nodes[0].as_ref().unwrap();
        let mut s = node::lock(&n1.shared);
        s.mempool.add_admin(tx.clone()).unwrap();
        s.broadcast(NetMessage::AdminTx(tx));
    }
    wait_until("transaction finalisée partout", || {
        nodes
            .iter()
            .flatten()
            .all(|n| node::lock(&n.shared).chain.state.balance(&alice) == 7 * UNITS_PER_VINX)
    })
    .await;

    // 4. Retour du nœud 4 : il rattrape la chaîne par blocs certifiés.
    nodes[3] = Some(start(
        &genesis,
        &specs[3],
        &others(3),
        Some(validators[3].clone()),
    ));
    let target = nodes[0].as_ref().unwrap().height() + 2;
    wait_until("rattrapage du nœud 4", || {
        nodes[3].as_ref().unwrap().height() >= target
    })
    .await;
    let n4 = nodes[3].as_ref().unwrap();
    assert_eq!(
        n4.block_hash(target),
        nodes[0].as_ref().unwrap().block_hash(target)
    );
    assert_eq!(
        node::lock(&n4.shared).chain.state.balance(&alice),
        7 * UNITS_PER_VINX
    );

    // 5. Un intrus hors de la liste autorisée ne reçoit rien.
    let intruder = spec(root.path(), 9);
    let x = start(&genesis, &intruder, &[&specs[0]], None);
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(x.height(), 0);
    x.stop().await;

    for n in nodes.into_iter().flatten() {
        assert!(node::lock(&n.shared).chain.state.check_invariant().is_ok());
        n.stop().await;
    }
}

/// L'intervalle entre blocs (2 s) dépasse le délai de proposition (1 s) : le
/// consensus d'une hauteur ne doit démarrer qu'une fois l'intervalle écoulé,
/// sans quoi chaque bloc était finalisé au round 1 au lieu du round 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blocks_are_finalized_in_the_first_round() {
    let root = tempfile::tempdir().unwrap();
    let k = || KeyPair::generate().public_key();
    let validators: Vec<KeyPair> = (0..4).map(|_| KeyPair::generate()).collect();
    let mut genesis = Genesis::standard(
        CHAIN,
        node::now_ms() / 1000,
        [k(), k(), k()],
        validators.iter().map(KeyPair::public_key).collect(),
    )
    .unwrap();
    genesis.params.min_block_interval_ms = 2_000;
    genesis.params.timeout_propose_ms = 1_000;
    let specs: Vec<Spec> = (0..4).map(|i| spec(root.path(), i)).collect();
    let nodes: Vec<Running> = (0..4)
        .map(|i| {
            let others: Vec<&Spec> = specs
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, s)| s)
                .collect();
            start(&genesis, &specs[i], &others, Some(validators[i].clone()))
        })
        .collect();
    wait_until("hauteur 5", || nodes.iter().all(|n| n.height() >= 5)).await;
    for n in &nodes {
        let s = node::lock(&n.shared);
        // Le tout premier bloc peut attendre les connexions ; tous les suivants
        // doivent être finalisés au premier round.
        let later_rounds = s.metrics.blocks - s.metrics.blocks_first_round;
        assert!(
            later_rounds <= 1,
            "{later_rounds} blocs finalisés après le round 0"
        );
    }
    for n in nodes {
        n.stop().await;
    }
}

/// Scénario d'un testnet qui grandit : un validateur, puis trois nœuds démarrés
/// avec leur clé, ajoutés d'un coup par la multisig. Ils doivent se mettre à
/// voter sans redémarrage, et la chaîne doit ensuite tolérer une panne.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_validator_grows_to_four_without_restart() {
    let root = tempfile::tempdir().unwrap();
    let admins = [
        KeyPair::generate(),
        KeyPair::generate(),
        KeyPair::generate(),
    ];
    let validators: Vec<KeyPair> = (0..4).map(|_| KeyPair::generate()).collect();
    let mut genesis = Genesis::standard(
        CHAIN,
        node::now_ms() / 1000,
        [
            admins[0].public_key(),
            admins[1].public_key(),
            admins[2].public_key(),
        ],
        vec![validators[0].public_key()],
    )
    .unwrap();
    genesis.params.min_block_interval_ms = 300;
    genesis.params.timeout_propose_ms = 1_500;
    genesis.params.timeout_vote_ms = 800;
    let specs: Vec<Spec> = (0..4).map(|i| spec(root.path(), i)).collect();
    let others = |i: usize| -> Vec<&Spec> {
        specs
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, s)| s)
            .collect()
    };
    let mut nodes: Vec<Option<Running>> = (0..4)
        .map(|i| {
            Some(start(
                &genesis,
                &specs[i],
                &others(i),
                Some(validators[i].clone()),
            ))
        })
        .collect();

    // Les trois nouveaux suivent la chaîne sans voter.
    wait_until("nouveaux nœuds à jour", || {
        nodes.iter().flatten().all(|n| n.height() >= 3)
    })
    .await;
    for n in nodes.iter().flatten().skip(1) {
        assert!(node::lock(&n.shared).metrics.validator_index.is_none());
    }

    // La multisig ajoute les trois validateurs d'un coup.
    {
        let n0 = nodes[0].as_ref().unwrap();
        let mut s = node::lock(&n0.shared);
        for (i, v) in validators.iter().enumerate().skip(1) {
            let tx = AdminTransaction {
                chain_id: CHAIN.into(),
                nonce: i as u64,
                action: AdminAction::AddValidator {
                    key: v.public_key(),
                },
            };
            let tx = SignedAdminTransaction {
                approvals: vec![tx.approve(0, &admins[0]), tx.approve(1, &admins[1])],
                tx,
            };
            s.mempool.add_admin(tx).unwrap();
        }
    }
    wait_until("les quatre votent", || {
        nodes
            .iter()
            .flatten()
            .all(|n| node::lock(&n.shared).metrics.validator_index.is_some())
    })
    .await;
    let h = nodes[0].as_ref().unwrap().height();
    wait_until("la chaîne continue à quatre", || {
        nodes.iter().flatten().all(|n| n.height() >= h + 3)
    })
    .await;

    // À quatre, la panne d'un validateur est tolérée.
    nodes[3].take().unwrap().stop().await;
    let h = nodes[0].as_ref().unwrap().height();
    wait_until("progression avec trois validateurs sur quatre", || {
        nodes.iter().flatten().all(|n| n.height() >= h + 3)
    })
    .await;
    for n in nodes.into_iter().flatten() {
        n.stop().await;
    }
}

/// Exercices de panne n° 2 et 4 (docs/EXERCICES-DE-PANNE.md), rejoués à chaque
/// modification : deux validateurs sur quatre tombent, la chaîne s'arrête
/// (plus de quorum) puis repart à leur retour ; enfin les quatre s'arrêtent
/// brutalement ensemble et la chaîne repart depuis les disques, sans
/// équivoque.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn halts_without_quorum_then_survives_a_full_crash() {
    let root = tempfile::tempdir().unwrap();
    let admins = [
        KeyPair::generate(),
        KeyPair::generate(),
        KeyPair::generate(),
    ];
    let validators: Vec<KeyPair> = (0..4).map(|_| KeyPair::generate()).collect();
    let mut genesis = Genesis::standard(
        CHAIN,
        node::now_ms() / 1000 - 1,
        [
            admins[0].public_key(),
            admins[1].public_key(),
            admins[2].public_key(),
        ],
        validators.iter().map(KeyPair::public_key).collect(),
    )
    .unwrap();
    genesis.params.min_block_interval_ms = 200;
    genesis.params.timeout_propose_ms = 1_500;
    genesis.params.timeout_vote_ms = 800;

    let specs: Vec<Spec> = (0..4).map(|i| spec(root.path(), i)).collect();
    let others = |i: usize| -> Vec<&Spec> {
        specs
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, s)| s)
            .collect()
    };
    let launch = |i: usize| start(&genesis, &specs[i], &others(i), Some(validators[i].clone()));
    let mut nodes: Vec<Option<Running>> = (0..4).map(|i| Some(launch(i))).collect();
    let height = |nodes: &[Option<Running>], i: usize| nodes[i].as_ref().unwrap().height();
    wait_until("hauteur 3 partout", || {
        nodes.iter().flatten().all(|n| n.height() >= 3)
    })
    .await;

    // 1. Deux validateurs sur quatre tombent : plus de quorum (3 voix), la
    // chaîne s'arrête. Un bloc déjà en cours peut encore se finaliser.
    nodes[2].take().unwrap().crash().await;
    nodes[3].take().unwrap().crash().await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let stalled = height(&nodes, 0);
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(height(&nodes, 0), stalled, "la chaîne avance sans quorum");
    assert_eq!(height(&nodes, 1), stalled, "les deux survivants divergent");

    // 2. Leur retour rétablit le quorum : la chaîne repart d'elle-même.
    nodes[2] = Some(launch(2));
    nodes[3] = Some(launch(3));
    wait_until("reprise après le retour du quorum", || {
        nodes.iter().flatten().all(|n| n.height() >= stalled + 3)
    })
    .await;

    // 3. Les quatre s'arrêtent brutalement en même temps, puis redémarrent
    // depuis leurs disques.
    let before = (0..4).map(|i| height(&nodes, i)).min().unwrap();
    for n in nodes.iter_mut() {
        n.take().unwrap().crash().await;
    }
    for (i, n) in nodes.iter_mut().enumerate() {
        *n = Some(launch(i));
    }
    let target = before + 3;
    for _ in 0..1_200 {
        if nodes.iter().flatten().all(|n| n.height() >= target) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // En cas d'échec, l'état de chaque nœud.
    let report: Vec<String> = nodes
        .iter()
        .flatten()
        .map(|n| {
            let s = node::lock(&n.shared);
            format!(
                "hauteur {} (attendu {target}), round du dernier bloc {}",
                s.chain.state.height,
                s.metrics.last_round
            )
        })
        .collect();
    assert!(
        nodes.iter().flatten().all(|n| n.height() >= target),
        "pas de reprise après l'arrêt brutal de tous : {report:?}"
    );

    // Même chaîne partout, aucune preuve d'équivoque, invariants respectés.
    let reference = nodes[0].as_ref().unwrap().block_hash(target);
    for n in nodes.iter().flatten() {
        assert_eq!(n.block_hash(target), reference);
        let s = node::lock(&n.shared);
        assert!(s.store.evidence().unwrap().is_empty(), "équivoque détectée");
        assert!(s.chain.state.check_invariant().is_ok());
    }
    for n in nodes.into_iter().flatten() {
        n.stop().await;
    }
}
