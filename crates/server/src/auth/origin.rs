//! Origin checks shared by the cookie-authenticated WebSocket upgrade
//! (`api::ws_scan_events`) and the cross-site guard on the CSRF-exempt
//! sign-in POSTs (`auth::csrf`).

use axum::http::{
    HeaderMap, HeaderValue,
    header::{HOST, ORIGIN},
};

const SEC_FETCH_SITE: &str = "sec-fetch-site";

/// True when `origin` is present and names the same origin (scheme, host,
/// port) as the configured `public_url`. Both sides are parsed and
/// compared by their serialized origin so default ports and letter case
/// normalize (`https://Example.com:443` equals `https://example.com`).
/// `null` and unparsable values never match.
pub fn origin_is_public(origin: Option<&HeaderValue>, public_url: &str) -> bool {
    let Some(origin) = origin.and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let (Ok(got), Ok(want)) = (url::Url::parse(origin), url::Url::parse(public_url)) else {
        return false;
    };
    let got = got.origin();
    got.is_tuple() && got == want.origin()
}

/// True when `origin` names the host and port the request itself was sent
/// to: `host` is the request's `Host` header (or its HTTP/2 authority). The
/// port missing on either side is the default port of the `Origin`'s
/// scheme; host names compare case-insensitively. This is what makes a
/// same-origin form on a LAN address (`http://192.168.1.20:8080`, which is
/// not the configured public URL) count as same-origin: a page on another
/// site can set neither header to the other's value, so its `Origin` never
/// equals the `Host` it posts to. `null` and unparsable values never match.
pub fn origin_matches_host(origin: Option<&HeaderValue>, host: Option<&str>) -> bool {
    let (Some(origin), Some(host)) = (origin.and_then(|v| v.to_str().ok()), host) else {
        return false;
    };
    let Ok(got) = url::Url::parse(origin) else {
        return false;
    };
    if !got.origin().is_tuple() || host.is_empty() || host.contains(['/', '@', '?', '#']) {
        return false;
    }
    let Ok(sent_to) = url::Url::parse(&format!("{}://{host}", got.scheme())) else {
        return false;
    };
    got.host() == sent_to.host() && got.port_or_known_default() == sent_to.port_or_known_default()
}

/// Why a request counts as cross-site for the CSRF-exempt sign-in POSTs, or
/// `None` when it may proceed. The exact rule:
///
/// 1. When `Sec-Fetch-Site` is present it decides alone: `same-origin` and
///    `none` (typed address, bookmark) proceed WITHOUT consulting `Origin`;
///    `cross-site`, `same-site` and any other value are refused. It is a
///    forbidden header name, set by the browser and never by a page, which
///    is why it is trusted over `Origin`; a same-origin form post can carry
///    `Origin: null` (an opaque origin under some referrer policies) while
///    `Sec-Fetch-Site` still says `same-origin`. `same-site` is refused on
///    purpose: a sibling subdomain must not sign visitors in, and nothing in
///    Folio posts these forms from another subdomain.
/// 2. Only without `Sec-Fetch-Site` (older browsers) does `Origin` decide:
///    it proceeds when it equals the configured public origin
///    ([`origin_is_public`]), or its host and port equal the request's own
///    `Host` (the HTTP/2 authority when there is no `Host`), or, when the
///    peer is a trusted proxy, the well-formed `X-Forwarded-Host` it set
///    ([`crate::upstream::forwarded_host_proto`]; an untrusted peer's
///    `X-Forwarded-Host` is never read). `Origin: null` is refused.
/// 3. Neither header (native clients): proceed.
pub fn cross_site_reason(
    headers: &HeaderMap,
    authority: Option<&str>,
    public_url: &str,
    peer_trusted: bool,
) -> Option<&'static str> {
    if let Some(site) = headers.get(SEC_FETCH_SITE) {
        let site = site.to_str().unwrap_or_default();
        return if site.eq_ignore_ascii_case("same-origin") || site.eq_ignore_ascii_case("none") {
            None
        } else {
            Some("sec-fetch-site")
        };
    }
    let origin = headers.get(ORIGIN)?;
    let host = headers
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .or(authority);
    let forwarded_host = if peer_trusted {
        crate::upstream::forwarded_host_proto(headers, true).1
    } else {
        None
    };
    if origin_is_public(Some(origin), public_url)
        || origin_matches_host(Some(origin), host)
        || origin_matches_host(Some(origin), forwarded_host)
    {
        None
    } else {
        Some("foreign-origin")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(origin: &str, public_url: &str) -> bool {
        origin_is_public(Some(&HeaderValue::from_str(origin).unwrap()), public_url)
    }

    fn matches_host(origin: &str, host: &str) -> bool {
        origin_matches_host(Some(&HeaderValue::from_str(origin).unwrap()), Some(host))
    }

    #[test]
    fn same_origin_matches_after_normalization() {
        assert!(check("http://localhost:8080", "http://localhost:8080"));
        assert!(check("http://localhost:8080", "http://localhost:8080/"));
        assert!(check(
            "https://Comics.Example.com",
            "https://comics.example.com:443/"
        ));
    }

    #[test]
    fn foreign_or_missing_origin_does_not_match() {
        assert!(!origin_is_public(None, "https://comics.example.com"));
        assert!(!check("null", "https://comics.example.com"));
        assert!(!check(
            "https://evil.example.com",
            "https://comics.example.com"
        ));
        assert!(!check(
            "https://other.example.com",
            "https://comics.example.com"
        ));
        assert!(!check(
            "http://comics.example.com",
            "https://comics.example.com"
        ));
        assert!(!check(
            "https://comics.example.com:8443",
            "https://comics.example.com"
        ));
        assert!(!check("http://localhost:3000", "http://localhost:8080"));
    }

    #[test]
    fn origin_equal_to_the_request_host_matches() {
        assert!(matches_host(
            "http://192.168.1.20:8080",
            "192.168.1.20:8080"
        ));
        assert!(matches_host(
            "https://Comics.Example.com",
            "comics.example.com"
        ));
        assert!(matches_host(
            "https://comics.example.com",
            "comics.example.com:443"
        ));
        assert!(matches_host("http://[::1]:8080", "[::1]:8080"));
        assert!(!matches_host(
            "http://192.168.1.20:8080",
            "192.168.1.20:3000"
        ));
        assert!(!matches_host("https://evil.example", "comics.example.com"));
        assert!(!matches_host("null", "comics.example.com"));
        assert!(!matches_host(
            "https://comics.example.com",
            "evil.example@comics.example.com"
        ));
        assert!(!origin_matches_host(None, Some("comics.example.com")));
        assert!(!origin_matches_host(
            Some(&HeaderValue::from_static("https://comics.example.com")),
            None
        ));
    }

    fn reason_with(
        pairs: &[(&str, &str)],
        host: Option<&str>,
        peer_trusted: bool,
    ) -> Option<&'static str> {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        cross_site_reason(&h, host, "https://comics.example.com", peer_trusted)
    }

    fn reason(pairs: &[(&str, &str)], host: Option<&str>) -> Option<&'static str> {
        reason_with(pairs, host, false)
    }

    #[test]
    fn sec_fetch_site_decides_alone_when_present() {
        let refused = Some("sec-fetch-site");
        assert_eq!(reason(&[("sec-fetch-site", "same-origin")], None), None);
        assert_eq!(
            reason(
                &[("sec-fetch-site", "same-origin"), ("origin", "null")],
                None
            ),
            None,
            "a same-origin form post under a strict referrer policy sends Origin: null"
        );
        assert_eq!(
            reason(&[("sec-fetch-site", "none"), ("origin", "null")], None),
            None
        );
        assert_eq!(reason(&[("sec-fetch-site", "cross-site")], None), refused);
        assert_eq!(
            reason(
                &[
                    ("sec-fetch-site", "cross-site"),
                    ("origin", "https://comics.example.com")
                ],
                None
            ),
            refused,
            "a matching Origin does not override Sec-Fetch-Site"
        );
        assert_eq!(reason(&[("sec-fetch-site", "same-site")], None), refused);
        assert_eq!(reason(&[("sec-fetch-site", "bogus")], None), refused);
    }

    #[test]
    fn origin_decides_without_sec_fetch_site() {
        let refused = Some("foreign-origin");
        assert_eq!(reason(&[], None), None, "no header: a native client");
        assert_eq!(
            reason(&[("origin", "https://comics.example.com")], None),
            None
        );
        assert_eq!(
            reason(
                &[
                    ("origin", "http://192.168.1.20:8080"),
                    ("host", "192.168.1.20:8080")
                ],
                None
            ),
            None
        );
        assert_eq!(
            reason(
                &[
                    ("origin", "https://lan.example"),
                    ("host", "lan.example:443")
                ],
                None
            ),
            None
        );
        assert_eq!(
            reason(
                &[("origin", "http://lan.example"), ("host", "lan.example:80")],
                None
            ),
            None
        );
        assert_eq!(
            reason(
                &[("origin", "http://192.168.1.20:8080")],
                Some("192.168.1.20:8080")
            ),
            None,
            "HTTP/2 authority stands in for a missing Host"
        );
        assert_eq!(
            reason(
                &[
                    ("origin", "https://evil.example"),
                    ("host", "comics.example.com")
                ],
                None
            ),
            refused
        );
        assert_eq!(
            reason(&[("origin", "null"), ("host", "comics.example.com")], None),
            refused
        );
    }

    #[test]
    fn forwarded_host_counts_only_from_a_trusted_proxy() {
        let headers = [
            ("origin", "https://comics.lan"),
            ("host", "folio:8080"),
            ("x-forwarded-host", "comics.lan"),
        ];
        assert_eq!(reason_with(&headers, None, true), None);
        assert_eq!(
            reason_with(&headers, None, false),
            Some("foreign-origin"),
            "an untrusted peer's X-Forwarded-Host is never read"
        );
        assert_eq!(
            reason_with(
                &[
                    ("origin", "https://comics.lan"),
                    ("host", "folio:8080"),
                    ("x-forwarded-host", "comics.lan/evil"),
                ],
                None,
                true
            ),
            Some("foreign-origin"),
            "a malformed X-Forwarded-Host is ignored"
        );
    }
}
