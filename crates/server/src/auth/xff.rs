//! `X-Forwarded-For` walker (§17.7).
//!
//! Returns the first untrusted hop from the right of the `X-Forwarded-For`
//! chain. Hops listed in `trusted_proxies` (parsed from
//! `COMIC_TRUSTED_PROXIES`, a comma-separated list of CIDRs or bare IPs) are
//! skipped — those are our own reverse-proxies. The first hop that is *not*
//! trusted is the real client.
//!
//! Without `COMIC_TRUSTED_PROXIES` set, every request appears to come from the
//! reverse proxy. We fall back to the connection's peer address — which in
//! that misconfigured case is the reverse proxy itself. Rate limits and audit
//! IPs will then bucket all traffic into one address; operators must set the
//! env var for a useful deploy.

use axum::http::HeaderMap;
use ipnet::IpNet;
use std::net::IpAddr;

/// Parse the comma-separated `COMIC_TRUSTED_PROXIES` value into a list of
/// CIDR networks. Bare IPs are accepted (treated as a `/32` or `/128`).
/// Empty entries are skipped; parse errors are logged and dropped so a
/// typo doesn't crash boot.
pub fn parse_trusted_proxies(raw: &str) -> Vec<IpNet> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|tok| match tok.parse::<IpNet>() {
            Ok(net) => Some(net),
            Err(_) => match tok.parse::<IpAddr>() {
                Ok(addr) => Some(IpNet::from(addr)),
                Err(e) => {
                    tracing::warn!(token = %tok, error = %e, "ignoring malformed entry in COMIC_TRUSTED_PROXIES");
                    None
                }
            },
        })
        .collect()
}

/// Resolve the real client IP for this request.
///
/// Walks `X-Forwarded-For` right-to-left. Returns the first IP that is *not*
/// covered by any entry in `trusted`. If every hop is trusted (and there's at
/// least one), returns the leftmost hop — the original client per the header
/// contract. If the header is missing or unparseable, returns `peer`.
pub fn client_ip(headers: &HeaderMap, peer: IpAddr, trusted: &[IpNet]) -> IpAddr {
    let Some(raw) = headers
        .get(axum::http::header::FORWARDED.as_str())
        .or_else(|| headers.get("x-forwarded-for"))
        .and_then(|v| v.to_str().ok())
    else {
        return peer;
    };

    // Reject `Forwarded:` (RFC 7239) — we only support XFF for now. Calling
    // sites set XFF; the Forwarded header pick above is only for forward-compat
    // and would parse differently.
    if raw.contains("for=") {
        return peer;
    }

    // Parse each hop; trust connection peer if any parse fails.
    let hops: Vec<IpAddr> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<IpAddr>().ok())
        .collect();

    if hops.is_empty() {
        return peer;
    }

    // Walk right-to-left until we find an untrusted hop. Stops at the first
    // hop that is NOT in our trusted set — that's the real client.
    for hop in hops.iter().rev() {
        if !trusted.iter().any(|net| net.contains(hop)) {
            return *hop;
        }
    }

    // Every listed hop is trusted — the leftmost is the original client.
    hops[0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn h(vals: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in vals {
            h.insert(
                axum::http::HeaderName::from_static(k),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    fn peer() -> IpAddr {
        "127.0.0.1".parse().unwrap()
    }

    #[test]
    fn parse_handles_mixed_cidrs_and_ips() {
        let nets = parse_trusted_proxies("10.0.0.0/8, 192.168.1.1 , ::1, garbage, ");
        assert_eq!(nets.len(), 3);
    }

    #[test]
    fn no_header_returns_peer() {
        let h = HeaderMap::new();
        assert_eq!(client_ip(&h, peer(), &[]), peer());
    }

    #[test]
    fn single_untrusted_hop_returned() {
        let h = h(&[("x-forwarded-for", "203.0.113.5")]);
        assert_eq!(
            client_ip(&h, peer(), &[]),
            "203.0.113.5".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn walks_right_to_left_through_trusted_proxies() {
        let trusted = parse_trusted_proxies("10.0.0.0/8");
        // client → trusted lb → trusted ingress. We trust the last two; client
        // is 203.0.113.5.
        let h = h(&[("x-forwarded-for", "203.0.113.5, 10.0.0.5, 10.0.0.6")]);
        assert_eq!(
            client_ip(&h, peer(), &trusted),
            "203.0.113.5".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn all_trusted_returns_leftmost() {
        let trusted = parse_trusted_proxies("10.0.0.0/8");
        let h = h(&[("x-forwarded-for", "10.0.0.5, 10.0.0.6")]);
        assert_eq!(
            client_ip(&h, peer(), &trusted),
            "10.0.0.5".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn untrusted_in_middle_returned() {
        // Spoof attempt: client claims a private IP in the header. Without a
        // trusted-proxies set we accept whatever's rightmost as the client —
        // operators must set the env var.
        let trusted = parse_trusted_proxies("10.0.0.0/8");
        let h = h(&[("x-forwarded-for", "8.8.8.8, 4.4.4.4, 10.0.0.5")]);
        assert_eq!(
            client_ip(&h, peer(), &trusted),
            "4.4.4.4".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn malformed_falls_back_to_peer() {
        let h = h(&[("x-forwarded-for", "not-an-ip, also-not")]);
        assert_eq!(client_ip(&h, peer(), &[]), peer());
    }

    #[test]
    fn ipv6_in_chain() {
        let trusted = parse_trusted_proxies("2001:db8::/32");
        let h = h(&[("x-forwarded-for", "2001:4860:4860::8888, 2001:db8::1")]);
        assert_eq!(
            client_ip(&h, peer(), &trusted),
            "2001:4860:4860::8888".parse::<IpAddr>().unwrap()
        );
    }
}
