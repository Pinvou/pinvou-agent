//! Link-local discovery for PINVOU shared knowledge hosts.
//!
//! mDNS is deliberately treated as an untrusted source of *addresses only*.
//! It never advertises a server id, the private service CA, an invitation
//! secret, or a device credential.  A desktop client must actively probe each
//! returned endpoint and ask the user to confirm the stable CA fingerprint
//! before it creates a join request.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use rand::RngCore;
use serde::{Deserialize, Serialize};

pub const SERVICE_TYPE: &str = "_pinvou-kb._tcp.local.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LanDiscoveryCandidate {
    pub endpoint: String,
    /// Display-only metadata from mDNS. It is not an authenticated server name.
    pub advertised_name: String,
}

/// Keeps the responder alive for the lifetime of the server process.
pub struct DiscoveryAdvertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Drop for DiscoveryAdvertisement {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

pub fn advertise(name: &str, port: u16) -> Result<DiscoveryAdvertisement, String> {
    let name = name.trim();
    if name.is_empty() || port == 0 {
        return Err("shared knowledge discovery metadata is unavailable".to_string());
    }
    let daemon = ServiceDaemon::new().map_err(|error| error.to_string())?;
    // A process-local random instance prevents mDNS metadata from becoming a
    // stable server-identity oracle. Identity comes only from the active probe.
    let mut instance_random = [0_u8; 6];
    rand::rng().fill_bytes(&mut instance_random);
    let suffix = instance_random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let instance = format!("PINVOU-{suffix}");
    let hostname = format!("pinvou-{suffix}.local.");
    let properties = HashMap::from([
        ("product".to_string(), "pinvou-knowledge".to_string()),
        ("protocol".to_string(), "2".to_string()),
        ("name".to_string(), name.chars().take(120).collect()),
    ]);
    let info = ServiceInfo::new(SERVICE_TYPE, &instance, &hostname, (), port, properties)
        .map_err(|error| error.to_string())?
        .enable_addr_auto();
    let fullname = info.get_fullname().to_string();
    daemon.register(info).map_err(|error| error.to_string())?;
    Ok(DiscoveryAdvertisement { daemon, fullname })
}

/// Returns untrusted LAN address candidates. The caller must actively probe
/// every endpoint; failed probes are discarded instead of allowing a virtual
/// interface address to take precedence over a reachable physical LAN address.
pub fn discover_lan_candidates(timeout: Duration) -> Result<Vec<LanDiscoveryCandidate>, String> {
    let daemon = ServiceDaemon::new().map_err(|error| error.to_string())?;
    let receiver = daemon
        .browse(SERVICE_TYPE)
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + timeout;
    let mut found = BTreeMap::<String, LanDiscoveryCandidate>::new();
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        if remaining.is_zero() {
            break;
        }
        let event = match receiver.recv_timeout(remaining) {
            Ok(event) => event,
            Err(_) => break,
        };
        let ServiceEvent::ServiceResolved(service) = event else {
            continue;
        };
        if service.get_property_val_str("product") != Some("pinvou-knowledge")
            || service
                .get_property_val_str("protocol")
                .and_then(|value| value.parse::<u32>().ok())
                .is_none_or(|version| version < 2)
        {
            continue;
        }
        let advertised_name = service
            .get_property_val_str("name")
            .unwrap_or("PINVOU Knowledge")
            .trim()
            .chars()
            .take(120)
            .collect::<String>();
        for address in service
            .get_addresses()
            .iter()
            .map(|address| address.to_ip_addr())
            .filter(|address| is_discoverable_lan_address(*address))
        {
            let endpoint = match address {
                IpAddr::V4(address) => format!("https://{address}:{}", service.get_port()),
                IpAddr::V6(address) => format!("https://[{address}]:{}", service.get_port()),
            };
            found
                .entry(endpoint.clone())
                .or_insert(LanDiscoveryCandidate {
                    endpoint,
                    advertised_name: advertised_name.clone(),
                });
        }
    }
    let _ = daemon.stop_browse(SERVICE_TYPE);
    let _ = daemon.shutdown();
    Ok(found.into_values().collect())
}

pub fn is_discoverable_lan_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_rfc1918(address),
        IpAddr::V6(address) => {
            let first = address.segments()[0];
            !address.is_loopback() && (first & 0xfe00) == 0xfc00 && !is_tailscale_ipv6(address)
        }
    }
}

fn is_rfc1918(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 10
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 168)
}

fn is_tailscale_ipv6(address: std::net::Ipv6Addr) -> bool {
    let segments = address.segments();
    segments[0] == 0xfd7a && segments[1] == 0x115c && segments[2] == 0xa1e0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearby_discovery_only_accepts_rfc1918_and_ula_addresses() {
        for address in ["192.168.1.20", "10.20.0.3", "172.31.2.8", "fd12::3"] {
            assert!(
                is_discoverable_lan_address(address.parse().unwrap()),
                "{address}"
            );
        }
        for address in [
            "127.0.0.1",
            "169.254.1.2",
            "172.32.0.1",
            "8.8.8.8",
            "100.64.12.34",
            "fd7a:115c:a1e0::1",
            "::1",
            "fe80::1",
        ] {
            assert!(
                !is_discoverable_lan_address(address.parse().unwrap()),
                "{address}"
            );
        }
    }
}
