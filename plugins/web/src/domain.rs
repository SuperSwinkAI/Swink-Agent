use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};

use thiserror::Error;
use url::Url;

/// Errors from domain filtering.
#[non_exhaustive]
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
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct DomainFilter {
    pub allowlist: Vec<String>,
    pub denylist: Vec<String>,
    pub block_private_ips: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedHost {
    pub host: String,
    pub addr: SocketAddr,
}

impl DomainFilter {
    /// Construct a filter that preserves open-domain access while blocking
    /// private, loopback, link-local, and otherwise non-routable IP ranges.
    #[must_use]
    pub fn blocking_private_ips() -> Self {
        Self {
            block_private_ips: true,
            ..Self::default()
        }
    }

    /// Check whether the given URL is permitted by the filter.
    ///
    /// Steps:
    /// 1. Scheme must be `http` or `https`.
    /// 2. Host must be extractable.
    /// 3. If the allowlist is non-empty the host must appear in it.
    /// 4. The host must not appear in the denylist.
    /// 5. If `block_private_ips` is enabled, DNS-resolved addresses are checked
    ///    against private/loopback/link-local ranges (SSRF protection).
    ///
    /// Step 5 performs a *blocking* DNS lookup for domain hosts. Do not call
    /// this from an async executor thread; the crate's own async tool paths
    /// resolve on tokio's blocking pool instead.
    pub fn is_allowed(&self, url: &Url) -> Result<(), DomainFilterError> {
        if let Some((domain, port)) = self.check_without_dns(url)? {
            let addrs = resolve_blocking(&domain, port);
            first_public_addr(domain, addrs)?;
        }
        Ok(())
    }

    /// Run every check that needs no I/O: scheme, host, allow/deny lists,
    /// and private-IP classification of IP-literal hosts.
    ///
    /// Returns `Some((domain, port))` when the host is a domain name that
    /// still has to be DNS-resolved and checked for private addresses, i.e.
    /// the URL is *not* fully validated yet.
    pub(crate) fn check_without_dns(
        &self,
        url: &Url,
    ) -> Result<Option<(String, u16)>, DomainFilterError> {
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
        if !self.allowlist.is_empty() && !host_matches_any(&self.allowlist, host) {
            return Err(DomainFilterError::NotAllowlisted(host.to_string()));
        }

        // 4. Denylist check.
        if host_matches_any(&self.denylist, host) {
            return Err(DomainFilterError::DeniedDomain(host.to_string()));
        }

        // 5. Private IP / SSRF check.
        //
        // Use the typed `url.host()` enum rather than `host_str()`: IPv6
        // literals in `host_str()` keep their brackets ("[::1]"), which
        // neither `Ipv6Addr` parsing nor getaddrinfo accepts, so IP literals
        // are classified directly from the already-parsed address.
        if !self.block_private_ips {
            return Ok(None);
        }
        match url.host() {
            Some(url::Host::Ipv4(ip)) if is_private_ip(&IpAddr::V4(ip)) => {
                Err(DomainFilterError::PrivateIp(ip.to_string()))
            }
            Some(url::Host::Ipv6(ip)) if is_private_ip(&IpAddr::V6(ip)) => {
                Err(DomainFilterError::PrivateIp(ip.to_string()))
            }
            Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)) => Ok(None),
            Some(url::Host::Domain(domain)) => Ok(Some((
                domain.to_string(),
                url.port_or_known_default().unwrap_or(80),
            ))),
            None => Err(DomainFilterError::InvalidUrl("URL has no host".to_string())),
        }
    }

    /// Full validation for async paths. DNS resolution runs on tokio's
    /// blocking pool so a slow or wedged lookup never stalls the executor.
    ///
    /// Returns the pinned public address for domain hosts when
    /// `block_private_ips` is enabled.
    pub(crate) async fn validate_and_resolve(
        &self,
        url: &Url,
    ) -> Result<Option<ResolvedHost>, DomainFilterError> {
        self.validate_and_resolve_with(url, resolve_blocking).await
    }

    async fn validate_and_resolve_with<R>(
        &self,
        url: &Url,
        resolve: R,
    ) -> Result<Option<ResolvedHost>, DomainFilterError>
    where
        R: FnOnce(&str, u16) -> std::io::Result<Vec<SocketAddr>> + Send + 'static,
    {
        let Some((domain, port)) = self.check_without_dns(url)? else {
            return Ok(None);
        };
        let lookup_domain = domain.clone();
        let addrs = tokio::task::spawn_blocking(move || resolve(&lookup_domain, port))
            .await
            .map_err(|e| DomainFilterError::DnsError(domain.clone(), e.to_string()))?;
        first_public_addr(domain, addrs).map(Some)
    }
}

fn resolve_blocking(domain: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    (domain, port).to_socket_addrs().map(Iterator::collect)
}

/// Reject the host if *any* resolved address is private; otherwise pin the
/// first address.
fn first_public_addr(
    domain: String,
    addrs: std::io::Result<Vec<SocketAddr>>,
) -> Result<ResolvedHost, DomainFilterError> {
    let addrs = addrs.map_err(|e| DomainFilterError::DnsError(domain.clone(), e.to_string()))?;
    if let Some(private) = addrs.iter().find(|addr| is_private_ip(&addr.ip())) {
        return Err(DomainFilterError::PrivateIp(private.ip().to_string()));
    }
    let Some(&addr) = addrs.first() else {
        return Err(DomainFilterError::DnsError(
            domain,
            "no addresses found".to_string(),
        ));
    };
    Ok(ResolvedHost { host: domain, addr })
}

fn host_matches_any(entries: &[String], host: &str) -> bool {
    entries
        .iter()
        .any(|entry| host_matches_entry(host, entry.as_str()))
}

fn host_matches_entry(host: &str, entry: &str) -> bool {
    let host = normalize_domain(host);
    let entry = normalize_domain(entry);
    if host.is_empty() || entry.is_empty() {
        return false;
    }

    if let Some(suffix) = entry.strip_prefix("*.") {
        return !suffix.is_empty() && is_subdomain_of(&host, suffix);
    }

    host == entry || is_subdomain_of(&host, &entry)
}

fn normalize_domain(value: &str) -> String {
    value.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn is_subdomain_of(host: &str, suffix: &str) -> bool {
    host.len() > suffix.len()
        && host.ends_with(suffix)
        && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
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
    // 0.0.0.0/8 (current network)
    if octets[0] == 0 {
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
    // 100.64.0.0/10 (carrier-grade NAT)
    if octets[0] == 100 && (64..=127).contains(&octets[1]) {
        return true;
    }
    // 198.18.0.0/15 (benchmarking)
    if octets[0] == 198 && (18..=19).contains(&octets[1]) {
        return true;
    }
    // Documentation/test networks.
    if (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
        || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
        || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
    {
        return true;
    }
    // 224.0.0.0/4 (multicast) and 240.0.0.0/4 (reserved).
    if octets[0] >= 224 {
        return true;
    }
    false
}

fn is_private_ipv6(ip: &Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_private_ipv4(&mapped);
    }
    // :: (unspecified)
    if ip.is_unspecified() {
        return true;
    }
    // ::1 (loopback)
    if ip.is_loopback() {
        return true;
    }
    let segments = ip.segments();
    // fc00::/7 (unique local addresses)
    if segments[0] & 0xfe00 == 0xfc00 {
        return true;
    }
    // fe80::/10 (link-local unicast)
    if segments[0] & 0xffc0 == 0xfe80 {
        return true;
    }
    // ff00::/8 (multicast)
    if segments[0] & 0xff00 == 0xff00 {
        return true;
    }
    // 2001:db8::/32 (documentation)
    if segments[0] == 0x2001 && segments[1] == 0x0db8 {
        return true;
    }
    false
}

#[cfg(test)]
#[path = "domain_tests.rs"]
mod tests;
