//! Opens the RPC port on the home router (UPnP), like libp2p already does for the P2P
//! port. A validator at home then serves the genesis, the faucet and the API to the
//! nodes joining it, with nothing to configure on the box.
//!
//! Only for an RPC bound to the LAN (`--lan`, `--testnet`): a node listening on
//! 127.0.0.1 is private on purpose and never asks for a mapping. Best effort: no
//! router, UPnP disabled or carrier-grade NAT simply leave the port closed.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::RwLock;
use std::time::Duration;

use igd_next::{aio::tokio::search_gateway, PortMappingProtocol, SearchOptions};

/// Lease asked to the router; renewed well before it runs out.
const LEASE_SECS: u32 = 3600;
const RENEW_EVERY: Duration = Duration::from_secs(20 * 60);
const RETRY_EVERY: Duration = Duration::from_secs(5 * 60);

static PUBLIC_API: RwLock<Option<String>> = RwLock::new(None);

/// Public `ip:port` the router forwards to this node's RPC, if the mapping holds.
pub fn public_api() -> Option<String> {
    PUBLIC_API.read().ok()?.clone()
}

fn set_public(v: Option<String>) {
    if let Ok(mut g) = PUBLIC_API.write() {
        *g = v;
    }
}

/// Whether an RPC bound to `addr` is meant to be reachable from other machines.
pub fn wants_mapping(addr: &SocketAddr) -> bool {
    !addr.ip().is_loopback()
}

/// Local address used to reach `gateway` — the address the router must forward to.
fn local_ip_towards(gateway: SocketAddr) -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect(gateway).ok()?;
    Some(s.local_addr().ok()?.ip())
}

async fn map_once(port: u16) -> Result<String, String> {
    let gw = search_gateway(SearchOptions::default())
        .await
        .map_err(|e| format!("router search: {e}"))?;
    let local = local_ip_towards(gw.addr).ok_or("no route to the router")?;
    let target = SocketAddr::new(local, port);
    // Some boxes only accept permanent leases (UPnP error 725), and some answer a timed
    // lease with a malformed reply: fall back to a permanent mapping, still renewed.
    if let Err(first) = gw
        .add_port(
            PortMappingProtocol::TCP,
            port,
            target,
            LEASE_SECS,
            "VinX API",
        )
        .await
    {
        gw.add_port(PortMappingProtocol::TCP, port, target, 0, "VinX API")
            .await
            .map_err(|e| format!("router {} refused the mapping: {first} / {e}", gw.addr))?;
    }
    let ext = gw
        .get_external_ip()
        .await
        .map_err(|e| format!("router {} external address: {e}", gw.addr))?;
    if !is_public(&ext) {
        // Carrier-grade NAT: the box itself sits behind another NAT.
        return Err(format!(
            "router's external address {ext} is not public (CGNAT)"
        ));
    }
    Ok(SocketAddr::new(ext, port).to_string())
}

fn is_public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || (o[0] == 100 && (64..128).contains(&o[1]))) // 100.64.0.0/10, CGNAT
        }
        IpAddr::V6(v6) => !v6.is_loopback() && !v6.is_unspecified(),
    }
}

/// Keeps the RPC port open on the router for as long as the node runs.
pub async fn keep_open(port: u16) {
    let mut warned = false;
    loop {
        match map_once(port).await {
            Ok(public) => {
                if public_api().as_deref() != Some(public.as_str()) {
                    tracing::info!(address = %public, "API port opened on the router (UPnP)");
                }
                set_public(Some(public));
                warned = false;
                tokio::time::sleep(RENEW_EVERY).await;
            }
            Err(e) => {
                if !warned {
                    tracing::info!(reason = %e, "API port not opened on the router (UPnP) — open {port}/TCP by hand to serve other nodes");
                    warned = true;
                }
                set_public(None);
                tokio::time::sleep(RETRY_EVERY).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_only_rpc_reachable_from_the_lan() {
        assert!(!wants_mapping(&"127.0.0.1:8545".parse().unwrap()));
        assert!(wants_mapping(&"0.0.0.0:8545".parse().unwrap()));
        assert!(wants_mapping(&"192.168.1.231:8545".parse().unwrap()));
    }

    #[test]
    fn recognises_public_addresses() {
        for ip in [
            "192.168.1.1",
            "10.0.0.1",
            "100.72.1.2",
            "127.0.0.1",
            "0.0.0.0",
        ] {
            assert!(!is_public(&ip.parse().unwrap()), "{ip}");
        }
        assert!(is_public(&"82.64.12.34".parse().unwrap()));
    }
}
