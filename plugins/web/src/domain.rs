use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

use thiserror::Error;
use url::Url;

/// Errors from domain filtering.
#[derive(Debug, Error)]
pub enum DomainFilterError {
    #[error("URL scheme '{0}' is not allowed; only http and https are permitted")]
    InvalidScheme(String),
    #[error("Domain '{0}' is on the deny list")]
    DeniedDomain(String),
    #[error("Domain '{0}' is not on the allow list")]
    NotAllowlisted(String),
    #[error("Address {0} is a private/internal IP and is blocked")]
    PrivateIp(String),
    #[error("Failed to parse URL: {0}")]
    InvalidUrl(String),
    #[error("DNS resolution failed for '{0}': {1}")]
    DnsError(String, String),
}

/// Domain allowlist/denylist with built-in SSRF protection.
#[derive(Debug, Clone, Default)]
pub struct DomainFilter {
    pub allowlist: Vec<String>,
    pub denylist: Vec<String>,
    pub block_private_ips: bool,
}

impl DomainFilter {
    /// Check whether the given URL is permitted by the filter.
    ///
    /// Steps:
    /// 1. Scheme must be `http` or `https`.
    /// 2. Host must be extractable.
    /// 3. If the allowlist is non-empty the host must appear in it.
    /// 4. The host must not appear in the denylist.
    /// 5. If `block_private_ips` is enabled, DNS-resolved addresses are checked
    ///    against private/loopback/link-local ranges (SSRF protection).
    pub fn is_allowed(&self, url: &Url) -> Result<(), DomainFilterError> {
        // 1. Scheme check.
        let scheme = url.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(DomainFilterError::InvalidScheme(scheme.to_string()));
        }

        // 2. Extract host.
        let host = url
            .host_str()
            .ok_or_else(|| DomainFilterError::InvalidUrl("URL has no host".to_string()))?;

        // 3. Allowlist check.
        if !self.allowlist.is_empty()
            && !self
                .allowlist
                .iter()
                .any(|a| a.eq_ignore_ascii_case(host))
        {
            return Err(DomainFilterError::NotAllowlisted(host.to_string()));
        }

        // 4. Denylist check.
        if self.denylist.iter().any(|d| d.eq_ignore_ascii_case(host)) {
            return Err(DomainFilterError::DeniedDomain(host.to_string()));
        }

        // 5. Private IP / SSRF check.
        if self.block_private_ips {
            let addrs = format!("{host}:80")
                .to_socket_addrs()
                .map_err(|e| DomainFilterError::DnsError(host.to_string(), e.to_string()))?;

            for addr in addrs {
                if is_private_ip(&addr.ip()) {
                    return Err(DomainFilterError::PrivateIp(addr.ip().to_string()));
                }
            }
        }

        Ok(())
    }
}

/// Returns `true` if the IP address belongs to a private, loopback,
/// link-local, or otherwise non-routable range.
fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_ipv4(v4),
        IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

fn is_private_ipv4(ip: &Ipv4Addr) -> bool {
    let octets = ip.octets();
    // 0.0.0.0
    if *ip == Ipv4Addr::UNSPECIFIED {
        return true;
    }
    // 127.0.0.0/8 (loopback)
    if octets[0] == 127 {
        return true;
    }
    // 10.0.0.0/8
    if octets[0] == 10 {
        return true;
    }
    // 172.16.0.0/12
    if octets[0] == 172 && (16..=31).contains(&octets[1]) {
        return true;
    }
    // 192.168.0.0/16
    if octets[0] == 192 && octets[1] == 168 {
        return true;
    }
    // 169.254.0.0/16 (link-local)
    if octets[0] == 169 && octets[1] == 254 {
        return true;
    }
    false
}

fn is_private_ipv6(ip: &Ipv6Addr) -> bool {
    // ::1 (loopback)
    if ip.is_loopback() {
        return true;
    }
    // fc00::/7 (unique local addresses)
    let segments = ip.segments();
    if segments[0] & 0xfe00 == 0xfc00 {
        return true;
    }
    false
}
