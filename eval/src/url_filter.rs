//! URL filtering helpers for remote eval assets.

use std::net::IpAddr;

use url::{Host, Url};

/// Policy for deciding whether a remote URL is safe to fetch.
pub trait UrlFilter: Send + Sync {
    /// Returns `true` when the URL is allowed to be fetched.
    fn allows(&self, url: &Url) -> bool;
}

/// Default SSRF-oriented filter for remote eval assets.
///
/// This filter blocks loopback, RFC1918/private, link-local, unspecified, and
/// known cloud metadata endpoints. Public hostnames remain allowed here; later
/// attachment materialization layers can additionally require HTTPS.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultUrlFilter;

impl UrlFilter for DefaultUrlFilter {
    fn allows(&self, url: &Url) -> bool {
        let Some(host) = url.host() else {
            return false;
        };

        match host {
            Host::Ipv4(address) => is_public_ip(IpAddr::V4(address)),
            Host::Ipv6(address) => is_public_ip(IpAddr::V6(address)),
            Host::Domain(host) => !is_blocked_hostname(host),
        }
    }
}

fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            !(address.is_loopback()
                || address.is_private()
                || address.is_link_local()
                || address.is_broadcast()
                || address.is_unspecified()
                || is_azure_metadata_ipv4(address))
        }
        IpAddr::V6(address) => {
            !(address.is_loopback()
                || address.is_unspecified()
                || address.is_unique_local()
                || address.is_unicast_link_local())
        }
    }
}

fn is_blocked_hostname(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();

    host == "localhost"
        || host.ends_with(".localhost")
        || matches!(
            host.as_str(),
            "metadata.google.internal" | "instance-data.ec2.internal"
        )
}

fn is_azure_metadata_ipv4(address: std::net::Ipv4Addr) -> bool {
    address.octets() == [169, 254, 169, 254]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_public_hostnames() {
        let filter = DefaultUrlFilter;
        assert!(filter.allows(&Url::parse("https://example.com/image.png").unwrap()));
        assert!(filter.allows(&Url::parse("http://example.com/image.png").unwrap()));
    }

    #[test]
    fn blocks_loopback_and_private_ip_literals() {
        let filter = DefaultUrlFilter;

        assert!(!filter.allows(&Url::parse("https://127.0.0.1/test.png").unwrap()));
        assert!(!filter.allows(&Url::parse("https://10.0.0.5/test.png").unwrap()));
        assert!(!filter.allows(&Url::parse("https://192.168.1.20/test.png").unwrap()));
        assert!(!filter.allows(&Url::parse("https://[::1]/test.png").unwrap()));
    }

    #[test]
    fn blocks_known_metadata_hosts() {
        let filter = DefaultUrlFilter;

        assert!(!filter.allows(&Url::parse("https://169.254.169.254/latest/meta-data").unwrap()));
        assert!(
            !filter.allows(
                &Url::parse("https://metadata.google.internal/computeMetadata/v1").unwrap()
            )
        );
        assert!(
            !filter.allows(
                &Url::parse("https://instance-data.ec2.internal/latest/meta-data").unwrap()
            )
        );
        assert!(!filter.allows(&Url::parse("https://localhost/test.png").unwrap()));
    }
}
