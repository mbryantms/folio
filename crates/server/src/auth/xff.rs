//! `X-Forwarded-For` walker (§17.7).
//!
//! Returns the first untrusted hop from the right of the `X-Forwarded-For`
//! chain. Hops listed in `trusted_proxies` (parsed from
//! `COMIC_TRUSTED_PROXIES`, a comma-separated list of CIDRs or bare IPs) are
//! skipped — those are our own reverse-proxies. The first hop that is *not*
//! trusted is the real client.
//!
//! Only `X-Forwarded-For` is read. RFC 7239 `Forwarded` is ignored: trusted
//! proxies append to `X-Forwarded-For` but usually pass a client's own
//! `Forwarded` header through, so it is client-controlled.
//!
//! Forwarding headers are honored **only when the immediate TCP peer is itself
//! a trusted proxy** (listed in `trusted_proxies`). A client connecting to us
//! directly can set any `X-Forwarded-For` value it likes, so if we trusted the
//! header unconditionally an attacker could spoof their IP and defeat every
//! control that keys off the result — the per-route rate limiter, the
//! failed-auth IP lockout, and the audit-log client IP. We therefore ignore the
//! header and use the connection peer whenever the peer is not trusted.
//!
//! Without `COMIC_TRUSTED_PROXIES` set, the trusted set is empty, so the header
//! is never honored and every request is attributed to its peer address. In a
//! reverse-proxied deploy that means all traffic buckets into the proxy's
//! address until the operator sets the env var to cover the front proxy — safe
//! by default, useful once configured.

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

/// True when the immediate TCP peer is one of our own reverse proxies
/// (covered by `trusted`). Only then may forwarding headers be believed.
pub fn peer_is_trusted(peer: IpAddr, trusted: &[IpNet]) -> bool {
    trusted.iter().any(|net| net.contains(&peer))
}

/// Resolve the real client IP for this request.
///
/// Forwarding headers are honored only when `peer` (the immediate TCP peer) is
/// covered by `trusted` — otherwise the caller is talking to us directly and any
/// `X-Forwarded-For` it sent is attacker-controlled, so we return `peer`.
///
/// When the peer is trusted, walks `X-Forwarded-For` right-to-left and returns
/// the first IP *not* covered by `trusted`. If every hop is trusted (and there's
/// at least one), returns the leftmost hop — the original client per the header
/// contract. If the header is missing or unparseable, returns `peer`.
pub fn client_ip(headers: &HeaderMap, peer: IpAddr, trusted: &[IpNet]) -> IpAddr {
    // Authenticate the hop we can: only walk the forwarding chain when the
    // immediate sender is one of our own proxies. A direct (untrusted) peer
    // cannot be allowed to dictate its own client IP.
    if !peer_is_trusted(peer, trusted) {
        return peer;
    }

    // Only `X-Forwarded-For` is read; an RFC 7239 `Forwarded` header is
    // ignored entirely. Trusted proxies append to `X-Forwarded-For` but
    // commonly pass a client's own `Forwarded` header through untouched, so
    // that header is client-controlled even behind a trusted proxy. Letting
    // it win would let a client choose (or hide) the address the rate
    // limiter, the sign-in lockout and the audit log record.
    let Some(raw) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) else {
        return peer;
    };

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

    /// A direct, untrusted client peer.
    fn peer() -> IpAddr {
        "203.0.113.99".parse().unwrap()
    }

    /// A peer inside the `10.0.0.0/8` trusted set used by the walk tests.
    fn trusted_peer() -> IpAddr {
        "10.0.0.6".parse().unwrap()
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
    fn spoofed_xff_ignored_when_peer_untrusted() {
        // The core SEC-1 guard: a client hitting us directly (peer not in the
        // trusted set — here, empty) cannot dictate its IP via the header.
        let h = h(&[("x-forwarded-for", "203.0.113.5")]);
        assert_eq!(client_ip(&h, peer(), &[]), peer());
    }

    #[test]
    fn spoofed_xff_ignored_even_with_trusted_set_when_peer_untrusted() {
        // A trusted set is configured, but THIS peer isn't in it (direct hit).
        // The header must still be ignored.
        let trusted = parse_trusted_proxies("10.0.0.0/8");
        let h = h(&[("x-forwarded-for", "8.8.8.8, 4.4.4.4, 10.0.0.5")]);
        assert_eq!(client_ip(&h, peer(), &trusted), peer());
    }

    #[test]
    fn walks_right_to_left_through_trusted_proxies() {
        let trusted = parse_trusted_proxies("10.0.0.0/8");
        // peer is our trusted ingress; chain is client → trusted lb → ingress.
        let h = h(&[("x-forwarded-for", "203.0.113.5, 10.0.0.5")]);
        assert_eq!(
            client_ip(&h, trusted_peer(), &trusted),
            "203.0.113.5".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn all_trusted_returns_leftmost() {
        let trusted = parse_trusted_proxies("10.0.0.0/8");
        let h = h(&[("x-forwarded-for", "10.0.0.5, 10.0.0.6")]);
        assert_eq!(
            client_ip(&h, trusted_peer(), &trusted),
            "10.0.0.5".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn untrusted_in_middle_returned() {
        // Peer is trusted, so we walk: rightmost trusted hop is skipped, the
        // first untrusted hop from the right is the real client.
        let trusted = parse_trusted_proxies("10.0.0.0/8");
        let h = h(&[("x-forwarded-for", "8.8.8.8, 4.4.4.4, 10.0.0.5")]);
        assert_eq!(
            client_ip(&h, trusted_peer(), &trusted),
            "4.4.4.4".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn malformed_falls_back_to_peer() {
        // Peer trusted (past the gate); the header itself is unparseable.
        let trusted = parse_trusted_proxies("10.0.0.0/8");
        let h = h(&[("x-forwarded-for", "not-an-ip, also-not")]);
        assert_eq!(client_ip(&h, trusted_peer(), &trusted), trusted_peer());
    }

    #[test]
    fn forwarded_header_ignored_from_trusted_peer() {
        // RFC 7239 `Forwarded:` is never read, even from a trusted peer: a
        // trusted proxy passes the client's own `Forwarded` through.
        let trusted = parse_trusted_proxies("10.0.0.0/8");
        let with_for = h(&[("forwarded", "for=1.2.3.4")]);
        assert_eq!(
            client_ip(&with_for, trusted_peer(), &trusted),
            trusted_peer()
        );
        // A bare-address `Forwarded` value is not read as a hop list either.
        let bare = h(&[("forwarded", "1.2.3.4")]);
        assert_eq!(client_ip(&bare, trusted_peer(), &trusted), trusted_peer());
    }

    #[test]
    fn forwarded_header_does_not_override_x_forwarded_for() {
        let trusted = parse_trusted_proxies("10.0.0.0/8");
        for fwd in ["for=1.2.3.4", "1.2.3.4", "for=10.0.0.9"] {
            let headers = h(&[("forwarded", fwd), ("x-forwarded-for", "198.51.100.7")]);
            assert_eq!(
                client_ip(&headers, trusted_peer(), &trusted),
                "198.51.100.7".parse::<IpAddr>().unwrap(),
                "Forwarded: {fwd}"
            );
        }
    }

    #[test]
    fn ipv6_in_chain() {
        let trusted = parse_trusted_proxies("2001:db8::/32");
        let peer = "2001:db8::1".parse::<IpAddr>().unwrap();
        let h = h(&[("x-forwarded-for", "2001:4860:4860::8888, 2001:db8::2")]);
        assert_eq!(
            client_ip(&h, peer, &trusted),
            "2001:4860:4860::8888".parse::<IpAddr>().unwrap()
        );
    }
}
