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
    fn loopback_detection() {
        assert!(is_loopback(&"/ip4/127.0.0.1/tcp/1".parse().unwrap()));
        assert!(!is_loopback(&"/ip4/192.168.1.2/tcp/1".parse().unwrap()));
    }
}
