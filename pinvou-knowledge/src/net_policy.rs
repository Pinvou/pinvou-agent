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
pub fn is_tailnet(address: std::net::Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

/// IPv6 ULA 段（fc00::/7）。
pub fn is_ula(address: std::net::Ipv6Addr) -> bool {
    (address.segments()[0] & 0xfe00) == 0xfc00
}

/// Tailscale 保留 IPv6 段（fd7a:115c:a1e0::/48）。
pub(crate) fn is_tailscale_ipv6(address: std::net::Ipv6Addr) -> bool {
    let segments = address.segments();
    segments[0] == 0xfd7a && segments[1] == 0x115c && segments[2] == 0xa1e0
}

/// 组合私网判定：IPv4 为 RFC1918 / 链路本地 / Tailscale，IPv6 为 ULA /
/// fe80::/10 链路本地（IPv4-mapped 地址先归一化为 v4 再判定）。
///
/// `allow_loopback` 决定是否接受回环地址：加入（join）请求校验的是直连来源，
/// 宿主本机回环同样是合法来源；分享端点则必须拒绝回环，避免端点指向调用方
/// 自身。server/service 与桌面端共用本判定，避免各自手写同一组合。
pub fn is_private_network_ip(address: std::net::IpAddr, allow_loopback: bool) -> bool {
    match address {
        std::net::IpAddr::V4(address) => {
            address.is_private()
                || address.is_link_local()
                || is_tailnet(address)
                || (allow_loopback && address.is_loopback())
        }
        std::net::IpAddr::V6(address) => {
            if let Some(address) = address.to_ipv4_mapped() {
                return is_private_network_ip(std::net::IpAddr::V4(address), allow_loopback);
            }
            is_ula(address)
                || (address.segments()[0] & 0xffc0) == 0xfe80
                || (allow_loopback && address.is_loopback())
        }
    }
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

    #[test]
    fn private_network_classification_respects_the_loopback_flag() {
        use std::net::IpAddr;
        // 回环地址仅在接受回环的语义（server 加入来源）下为私网。
        for address in ["127.0.0.1", "::1"] {
            let address: IpAddr = address.parse().unwrap();
            assert!(is_private_network_ip(address, true), "{address}");
            assert!(!is_private_network_ip(address, false), "{address}");
        }
        // IPv4-mapped 地址两种语义下都按映射后的 v4 判定。
        let mapped: IpAddr = "::ffff:192.168.1.10".parse().unwrap();
        assert!(is_private_network_ip(mapped, true));
        assert!(is_private_network_ip(mapped, false));
        assert!(!is_private_network_ip(
            "::ffff:8.8.8.8".parse::<IpAddr>().unwrap(),
            true
        ));
    }
}
