//! Link-local discovery for PINVOU shared knowledge hosts.
//!
//! Discovery only announces public service metadata. It never carries a share secret,
//! a device credential, collection names, or document data. Joining still creates a
//! normal approval request and all subsequent calls remain token authenticated.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};

pub const SERVICE_TYPE: &str = "_pinvou-kb._tcp.local.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredKnowledgeHost {
    pub server_id: String,
    pub identity: String,
    pub name: String,
    pub protocol_version: u32,
    pub tls_ca: String,
    pub endpoints: Vec<String>,
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

pub fn advertise(
    server_id: &str,
    identity: &str,
    name: &str,
    tls_ca: &str,
    port: u16,
) -> Result<DiscoveryAdvertisement, String> {
    if server_id.trim().is_empty() || identity.trim().is_empty() || tls_ca.trim().is_empty() {
        return Err("shared knowledge identity is unavailable".to_string());
    }
    let daemon = ServiceDaemon::new().map_err(|error| error.to_string())?;
    let suffix = identity
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(10)
        .collect::<String>();
    let suffix = if suffix.is_empty() { "host" } else { &suffix };
    let instance = format!("PINVOU-{suffix}");
    let hostname = format!("pinvou-{suffix}.local.");
    let mut properties = HashMap::from([
        ("id".to_string(), server_id.to_string()),
        ("identity".to_string(), identity.to_string()),
        ("name".to_string(), name.to_string()),
        ("protocol".to_string(), "2".to_string()),
    ]);
    let ca_chunks = tls_ca.as_bytes().chunks(180).collect::<Vec<_>>();
    if ca_chunks.len() > 8 {
        return Err("shared knowledge TLS identity is too large for LAN discovery".to_string());
    }
    properties.insert("ca_parts".to_string(), ca_chunks.len().to_string());
    for (index, chunk) in ca_chunks.into_iter().enumerate() {
        properties.insert(
            format!("ca{index}"),
            std::str::from_utf8(chunk)
                .map_err(|error| error.to_string())?
                .to_string(),
        );
    }
    let info = ServiceInfo::new(SERVICE_TYPE, &instance, &hostname, (), port, properties)
        .map_err(|error| error.to_string())?
        .enable_addr_auto();
    let fullname = info.get_fullname().to_string();
    daemon.register(info).map_err(|error| error.to_string())?;
    Ok(DiscoveryAdvertisement { daemon, fullname })
}

pub fn discover_nearby(timeout: Duration) -> Result<Vec<DiscoveredKnowledgeHost>, String> {
    let daemon = ServiceDaemon::new().map_err(|error| error.to_string())?;
    let receiver = daemon
        .browse(SERVICE_TYPE)
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + timeout;
    let mut found = BTreeMap::<String, DiscoveredKnowledgeHost>::new();
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
        let server_id = service
            .get_property_val_str("id")
            .unwrap_or_default()
            .trim()
            .to_string();
        let identity = service
            .get_property_val_str("identity")
            .unwrap_or_default()
            .trim()
            .to_string();
        let name = service
            .get_property_val_str("name")
            .unwrap_or("PINVOU Knowledge")
            .trim()
            .to_string();
        let protocol_version = service
            .get_property_val_str("protocol")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or_default();
        let tls_ca = service
            .get_property_val_str("ca_parts")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|parts| (1..=8).contains(parts))
            .and_then(|parts| {
                (0..parts)
                    .map(|index| service.get_property_val_str(&format!("ca{index}")))
                    .collect::<Option<Vec<_>>>()
                    .map(|chunks| chunks.concat())
            })
            .unwrap_or_default();
        if server_id.is_empty() || identity.is_empty() || tls_ca.is_empty() || protocol_version < 2
        {
            continue;
        }
        let mut endpoints = service
            .get_addresses()
            .iter()
            .map(|address| address.to_ip_addr())
            .filter(|address| is_discoverable_lan_address(*address))
            .map(|address| match address {
                IpAddr::V4(address) => format!("https://{address}:{}", service.get_port()),
                IpAddr::V6(address) => format!("https://[{address}]:{}", service.get_port()),
            })
            .collect::<Vec<_>>();
        endpoints.sort();
        endpoints.dedup();
        if endpoints.is_empty() {
            continue;
        }
        found
            .entry(identity.clone())
            .and_modify(|existing| {
                existing.endpoints.extend(endpoints.clone());
                existing.endpoints.sort();
                existing.endpoints.dedup();
            })
            .or_insert(DiscoveredKnowledgeHost {
                server_id,
                identity,
                name,
                protocol_version,
                tls_ca,
                endpoints,
            });
    }
    let _ = daemon.stop_browse(SERVICE_TYPE);
    let _ = daemon.shutdown();
    Ok(found.into_values().collect())
}

pub fn is_discoverable_lan_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            address.is_private() && !address.is_loopback() && !is_tailnet(address)
        }
        IpAddr::V6(address) => {
            let first = address.segments()[0];
            !address.is_loopback() && (first & 0xfe00) == 0xfc00
        }
    }
}

fn is_tailnet(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearby_discovery_excludes_public_loopback_link_local_and_tailnet_addresses() {
        for address in ["192.168.1.20", "10.20.0.3", "fd12::3"] {
            assert!(
                is_discoverable_lan_address(address.parse().unwrap()),
                "{address}"
            );
        }
        for address in [
            "127.0.0.1",
            "169.254.1.2",
            "8.8.8.8",
            "100.64.12.34",
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
