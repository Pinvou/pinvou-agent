//! 私网地址判定与私有 DNS 后缀的单一实现，供 discovery、client、server、
//! service 共用。各调用方在 loopback/link-local 上的组合语义保持各自原状，
//! 这里只收敛单个地址段与后缀列表的判定。

/// 仅在私网中可达的 DNS 后缀（mDNS/home 网关/Tailscale MagicDNS）。
#[cfg(feature = "server")]
pub(crate) const PRIVATE_HOST_SUFFIXES: [&str; 5] =
    [".local", ".lan", ".internal", ".home.arpa", ".ts.net"];

/// RFC1918 私有 IPv4 段（10/8、172.16/12、192.168/16）。
pub(crate) fn is_rfc1918(address: std::net::Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 10
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 168)
}

/// Tailscale IPv4 段（100.64/10）。
pub(crate) fn is_tailnet(address: std::net::Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

/// IPv6 ULA 段（fc00::/7）。
pub(crate) fn is_ula(address: std::net::Ipv6Addr) -> bool {
    (address.segments()[0] & 0xfe00) == 0xfc00
}

/// Tailscale 保留 IPv6 段（fd7a:115c:a1e0::/48）。
pub(crate) fn is_tailscale_ipv6(address: std::net::Ipv6Addr) -> bool {
    let segments = address.segments();
    segments[0] == 0xfd7a && segments[1] == 0x115c && segments[2] == 0xa1e0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tailnet_covers_the_reserved_100_64_10_range_only() {
        for address in ["100.64.0.1", "100.100.1.1", "100.127.255.254"] {
            assert!(is_tailnet(address.parse().unwrap()), "{address}");
        }
        for address in ["100.63.0.1", "100.128.0.1", "101.0.0.1"] {
            assert!(!is_tailnet(address.parse().unwrap()), "{address}");
        }
    }
}
