//! Defence-in-depth against server-side request forgery on user-supplied
//! URLs. The current attack surface is `POST /me/cbl-lists` (and the
//! subsequent refresh worker that re-fetches the stored URL): authenticated
//! callers can otherwise have the server probe internal network ranges,
//! cloud metadata endpoints (`169.254.169.254`), and the loopback interface.
//!
//! The strategy is two-layer:
//!
//! 1. **Pre-flight** — parse the URL, require `https://`, resolve the
//!    hostname (via `tokio::net::lookup_host`), and refuse if *any*
//!    resolved address is loopback / private / link-local / multicast /
//!    other internal-only range. A single A/AAAA record pointing into
//!    private space taints the request.
//!
//! 2. **Redirect policy** — outbound `reqwest::Client` rejects redirects
//!    whose host is a private-range IP *literal*. The pre-flight already
//!    covers DNS-resolved hosts, but a CDN-style redirect could otherwise
//!    push the connection to an internal IP literal between hops; this
//!    catches that edge.
//!
//! The DNS-rebind window (host resolves public on lookup-1, private on
//! lookup-2 during the actual connect) is acknowledged but not closed
//! here. Closing it requires a custom `Connector` that pins to the
//! pre-resolved address — useful follow-up; not blocking for v1.0.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use reqwest::redirect::Policy;

#[derive(Debug, thiserror::Error)]
pub enum SsrfError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("only https:// URLs are accepted")]
    SchemeNotHttps,
    #[error("URL must include a host")]
    MissingHost,
    #[error("hostname did not resolve: {0}")]
    Unresolvable(String),
    #[error("host resolves to a private or otherwise internal address ({0})")]
    PrivateAddress(IpAddr),
}

/// Parse + syntactic validation. Does not touch the network. Returns the
/// parsed URL on success so callers can pass the canonical form to reqwest.
pub fn validate_outbound_url(url: &str) -> Result<url::Url, SsrfError> {
    let parsed = url::Url::parse(url).map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;
    if parsed.scheme() != "https" {
        return Err(SsrfError::SchemeNotHttps);
    }
    // `url::Host` distinguishes domain / v4 / v6 in a typed way, which
    // matters because `host_str()` keeps `[::1]` as-is and that won't
    // round-trip through `IpAddr::from_str`.
    let host = parsed.host().ok_or(SsrfError::MissingHost)?;
    let maybe_ip: Option<IpAddr> = match host {
        url::Host::Ipv4(v4) => Some(IpAddr::V4(v4)),
        url::Host::Ipv6(v6) => Some(IpAddr::V6(v6)),
        url::Host::Domain(_) => None,
    };
    if let Some(ip) = maybe_ip
        && is_internal_ip(&ip)
    {
        return Err(SsrfError::PrivateAddress(ip));
    }
    Ok(parsed)
}

/// Resolve `host` (with the URL's port, defaulting to 443 for https) and
/// fail if *any* resolved address is in an internal range. Called after
/// [`validate_outbound_url`] and before the actual fetch.
pub async fn check_host_resolves_public(host: &str, port: u16) -> Result<(), SsrfError> {
    // Short-circuit IP-literal hosts — `validate_outbound_url` already
    // covered them, but the contract is that `check_host_resolves_public`
    // is safe to call standalone, so re-do it here.
    if let Ok(ip) = host.parse::<IpAddr>() {
        return if is_internal_ip(&ip) {
            Err(SsrfError::PrivateAddress(ip))
        } else {
            Ok(())
        };
    }
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| SsrfError::Unresolvable(e.to_string()))?;
    let mut count = 0usize;
    for addr in addrs {
        count += 1;
        let ip = addr.ip();
        if is_internal_ip(&ip) {
            return Err(SsrfError::PrivateAddress(ip));
        }
    }
    if count == 0 {
        return Err(SsrfError::Unresolvable(format!("no addresses for {host}")));
    }
    Ok(())
}

/// Build a redirect policy that limits hops to `max_hops` and rejects
/// any hop whose URL host is a private-range IP literal. Hostname
/// targets are *not* re-resolved here (sync callback, no async DNS);
/// the per-request pre-flight covers that.
pub fn outbound_redirect_policy(max_hops: usize) -> Policy {
    Policy::custom(move |attempt| {
        if attempt.previous().len() >= max_hops {
            return attempt.error(SsrfError::InvalidUrl(format!(
                "exceeded {max_hops} redirect hops"
            )));
        }
        if let Some(host) = attempt.url().host_str()
            && let Ok(ip) = host.parse::<IpAddr>()
            && is_internal_ip(&ip)
        {
            return attempt.error(SsrfError::PrivateAddress(ip));
        }
        attempt.follow()
    })
}

/// `true` if `ip` is in any range the server should refuse to talk to
/// from a user-supplied URL. Covers loopback, RFC-1918 private, link-local,
/// CGN (RFC-6598), multicast, broadcast, documentation, unspecified,
/// ULA (IPv6 fc00::/7), v4-mapped (and recurse on the inner v4).
pub fn is_internal_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_internal_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_internal_v4(&v4);
            }
            is_internal_v6(v6)
        }
    }
}

fn is_internal_v4(ip: &Ipv4Addr) -> bool {
    if ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
    {
        return true;
    }
    // RFC-6598 carrier-grade NAT 100.64.0.0/10 — std doesn't expose
    // a stable `is_shared` predicate.
    let octets = ip.octets();
    if octets[0] == 100 && (octets[1] & 0b1100_0000) == 0b0100_0000 {
        return true;
    }
    // 240.0.0.0/4 reserved (minus the broadcast already caught above).
    if octets[0] >= 240 {
        return true;
    }
    false
}

fn is_internal_v6(ip: &Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_multicast() || ip.is_unspecified() {
        return true;
    }
    let segments = ip.segments();
    // fc00::/7 — unique local addresses.
    if (segments[0] & 0xfe00) == 0xfc00 {
        return true;
    }
    // fe80::/10 — link-local.
    if (segments[0] & 0xffc0) == 0xfe80 {
        return true;
    }
    // 2001:db8::/32 — documentation.
    if segments[0] == 0x2001 && segments[1] == 0x0db8 {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_internal_v4_ranges() {
        for raw in [
            "127.0.0.1",
            "127.0.0.255",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254", // AWS / GCP / Azure metadata endpoint
            "100.64.0.1",      // CGN
            "100.127.255.255", // CGN upper
            "224.0.0.1",       // multicast
            "255.255.255.255", // broadcast
            "0.0.0.0",
            "192.0.2.1",    // TEST-NET-1
            "198.51.100.1", // TEST-NET-2
            "203.0.113.1",  // TEST-NET-3
            "240.0.0.1",    // reserved
        ] {
            let ip: IpAddr = raw.parse().unwrap();
            assert!(is_internal_ip(&ip), "expected {raw} to be internal");
        }
    }

    #[test]
    fn accepts_public_v4() {
        for raw in [
            "8.8.8.8",
            "1.1.1.1",
            "140.82.121.4",  // github.com
            "151.101.1.140", // fastly
        ] {
            let ip: IpAddr = raw.parse().unwrap();
            assert!(!is_internal_ip(&ip), "expected {raw} to be public");
        }
    }

    #[test]
    fn rejects_internal_v6_ranges() {
        for raw in [
            "::1",
            "fc00::1",
            "fd00::1",
            "fe80::1",
            "ff02::1",
            "::",
            "2001:db8::1",
            "::ffff:127.0.0.1", // v4-mapped loopback
            "::ffff:169.254.169.254",
        ] {
            let ip: IpAddr = raw.parse().unwrap();
            assert!(is_internal_ip(&ip), "expected {raw} to be internal");
        }
    }

    #[test]
    fn accepts_public_v6() {
        for raw in ["2606:4700:4700::1111", "2620:fe::fe"] {
            let ip: IpAddr = raw.parse().unwrap();
            assert!(!is_internal_ip(&ip), "expected {raw} to be public");
        }
    }

    #[test]
    fn validate_url_requires_https() {
        let err = validate_outbound_url("http://example.com/list.cbl").unwrap_err();
        assert!(matches!(err, SsrfError::SchemeNotHttps));
        let err = validate_outbound_url("ftp://example.com/list.cbl").unwrap_err();
        assert!(matches!(err, SsrfError::SchemeNotHttps));
    }

    #[test]
    fn validate_url_rejects_ip_literal_in_internal_range() {
        for url in [
            "https://127.0.0.1/list.cbl",
            "https://169.254.169.254/latest/meta-data/iam",
            "https://10.0.0.1/list.cbl",
            "https://[::1]/list.cbl",
            "https://[fc00::1]/list.cbl",
        ] {
            let err = validate_outbound_url(url).unwrap_err();
            assert!(
                matches!(err, SsrfError::PrivateAddress(_)),
                "expected PrivateAddress for {url}, got {err:?}"
            );
        }
    }

    #[test]
    fn validate_url_accepts_well_formed_https() {
        let url = validate_outbound_url("https://example.com/list.cbl").unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("example.com"));
    }
}
