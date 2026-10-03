//! Peer discovery without a fixed server: entry points (seeds built into the binary),
//! a Kademlia routing table fed by every connection, and the peers remembered on disk
//! so a restart reconnects even if every entry point is gone.

use std::path::Path;

use libp2p::{multiaddr::Protocol, Multiaddr, PeerId};

/// Connections the node looks for; discovery stops dialing above this.
pub const TARGET_PEERS: usize = 25;
/// Hard cap: extra inbound connections are closed.
pub const MAX_PEERS: usize = 60;
/// Peers kept in `peers.json`.
const MAX_STORED_PEERS: usize = 200;
/// Default P2P port of a seed.
const SEED_P2P_PORT: u16 = 9001;

const TESTNET_SEEDS: &str = include_str!("../../../../seeds/testnet.txt");

/// Entry points built into the binary for `chain_id`, as multiaddrs.
pub fn builtin_seeds(chain_id: u32) -> Vec<String> {
    let list = match chain_id {
        vinx_core::chain_id::CHAIN_ID_TESTNET => TESTNET_SEEDS,
        _ => "",
    };
    parse_seeds(list)
}

/// One seed per line: `host` or `host:port` (DNS name or IPv4), `#` for comments.
pub fn parse_seeds(list: &str) -> Vec<String> {
    list.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (host, port) = match l.rsplit_once(':') {
                Some((h, p)) if p.parse::<u16>().is_ok() => (h, p.to_string()),
                _ => (l, SEED_P2P_PORT.to_string()),
            };
            let kind = if host.parse::<std::net::Ipv4Addr>().is_ok() {
                "ip4"
            } else {
                "dns4"
            };
            format!("/{kind}/{host}/tcp/{port}")
        })
        .collect()
}

/// Relays a node behind a NAT keeps a reservation on.
pub const WANTED_RELAYS: usize = 2;

/// NAT traversal state: which peers can relay for us, and which do.
#[derive(Default)]
pub struct Nat {
    /// AutoNAT says other nodes cannot dial us.
    pub private: bool,
    /// Peers we reached by dialing them directly, with that address.
    direct: std::collections::HashMap<PeerId, Multiaddr>,
    /// Peers that announced the relay (hop) protocol.
    hop: std::collections::HashSet<PeerId>,
    /// Relays we asked for a reservation.
    pub reserved: std::collections::HashSet<PeerId>,
}

impl Nat {
    pub fn reachable_at(&mut self, peer: PeerId, addr: Multiaddr) {
        // Keep the transport part only: the circuit address appends `/p2p/<relay>`.
        let addr: Multiaddr = addr
            .iter()
            .filter(|p| !matches!(p, Protocol::P2p(_)))
            .collect();
        self.direct.insert(peer, addr);
    }
    pub fn relay_capable(&mut self, peer: PeerId) {
        self.hop.insert(peer);
    }
    /// Forgets a relay whose connection closed. True if it was one of ours.
    pub fn drop_relay(&mut self, peer: &PeerId) -> bool {
        self.direct.remove(peer);
        self.reserved.remove(peer)
    }
    /// Reachable relays to ask, up to [`WANTED_RELAYS`] reservations in total.
    pub fn next_relays(&self) -> Vec<(PeerId, Multiaddr)> {
        let want = WANTED_RELAYS.saturating_sub(self.reserved.len());
        let mut c: Vec<(PeerId, Multiaddr)> = self
            .direct
            .iter()
            .filter(|(p, _)| self.hop.contains(*p) && !self.reserved.contains(*p))
            .map(|(p, a)| (*p, a.clone()))
            .collect();
        c.sort_by_key(|(p, _)| p.to_bytes());
        c.truncate(want);
        c
    }
}

/// An address that goes through a relay.
pub fn is_relayed(a: &Multiaddr) -> bool {
    a.iter().any(|p| matches!(p, Protocol::P2pCircuit))
}

pub fn is_loopback(a: &Multiaddr) -> bool {
    a.iter().any(|p| match p {
        Protocol::Ip4(ip) => ip.is_loopback() || ip.is_unspecified(),
        Protocol::Ip6(ip) => ip.is_loopback() || ip.is_unspecified(),
        _ => false,
    })
}

pub fn load_peers(path: &Path) -> Vec<(PeerId, Vec<Multiaddr>)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    let raw: Vec<(String, Vec<String>)> = serde_json::from_str(&text).unwrap_or_default();
    raw.into_iter()
        .filter_map(|(p, addrs)| {
            let peer = p.parse().ok()?;
            let addrs: Vec<Multiaddr> = addrs.iter().filter_map(|a| a.parse().ok()).collect();
            (!addrs.is_empty()).then_some((peer, addrs))
        })
        .take(MAX_STORED_PEERS)
        .collect()
}

pub fn save_peers(path: &Path, peers: &[(PeerId, Vec<Multiaddr>)]) {
    if peers.is_empty() {
        return; // never overwrite a useful list with an empty one while isolated
    }
    let raw: Vec<(String, Vec<String>)> = peers
        .iter()
        .take(MAX_STORED_PEERS)
        .map(|(p, a)| (p.to_string(), a.iter().map(|a| a.to_string()).collect()))
        .collect();
    if let Ok(json) = serde_json::to_string(&raw) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_parse_dns_ip_and_ports() {
        let s = parse_seeds("# comment\nseed.vinx.example\n1.2.3.4:9100\n\n  5.6.7.8 # x\n");
        assert_eq!(
            s,
            vec![
                "/dns4/seed.vinx.example/tcp/9001",
                "/ip4/1.2.3.4/tcp/9100",
                "/ip4/5.6.7.8/tcp/9001"
            ]
        );
        assert!(s.iter().all(|a| a.parse::<Multiaddr>().is_ok()));
    }

    #[test]
    fn peer_store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("vinx-peers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("peers.json");
        let p = PeerId::random();
        let a: Multiaddr = "/ip4/10.0.0.1/tcp/9001".parse().unwrap();
        save_peers(&path, &[(p, vec![a.clone()])]);
        assert_eq!(load_peers(&path), vec![(p, vec![a])]);
        save_peers(&path, &[]);
        assert_eq!(load_peers(&path).len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn relays_are_reachable_hop_peers_up_to_two() {
        let mut nat = Nat::default();
        let addr: Multiaddr = "/ip4/203.0.113.1/tcp/9001".parse().unwrap();
        let (a, b, c, d) = (
            PeerId::random(),
            PeerId::random(),
            PeerId::random(),
            PeerId::random(),
        );
        for p in [a, b, c] {
            nat.reachable_at(p, addr.clone());
        }
        for p in [a, b, c, d] {
            nat.relay_capable(p); // d speaks the protocol but was never reached directly
        }
        nat.reachable_at(a, format!("/ip4/203.0.113.1/tcp/9001/p2p/{a}").parse().unwrap());
        let first = nat.next_relays();
        assert!(first.iter().all(|(_, ad)| !ad.iter().any(|p| matches!(p, Protocol::P2p(_)))));
        assert_eq!(first.len(), 2);
        assert!(first.iter().all(|(p, _)| *p != d));
        nat.reserved.extend(first.iter().map(|(p, _)| *p));
        assert!(nat.next_relays().is_empty(), "two reservations are enough");
        let lost = first[0].0;
        assert!(nat.drop_relay(&lost));
        let refill = nat.next_relays();
        assert_eq!(refill.len(), 1);
        assert_ne!(
            refill[0].0, lost,
            "a lost relay is not retried until reached again"
        );
        let circuit: Multiaddr = format!("/ip4/1.2.3.4/tcp/1/p2p/{a}/p2p-circuit")
            .parse()
            .unwrap();
        assert!(is_relayed(&circuit));
        assert!(!is_relayed(&addr));
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback(&"/ip4/127.0.0.1/tcp/1".parse().unwrap()));
        assert!(!is_loopback(&"/ip4/192.168.1.2/tcp/1".parse().unwrap()));
    }
}
