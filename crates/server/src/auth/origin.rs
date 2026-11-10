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

/// Why a browser request counts as cross-site for the CSRF-exempt sign-in
/// POSTs, or `None` when it may proceed. The exact rule:
///
/// 1. `Sec-Fetch-Site: cross-site` or `same-site` refuses. `same-site` is
///    refused on purpose: a sibling subdomain (another app on the same
///    registrable domain) must not be able to sign a visitor in either, and
///    no part of Folio posts these forms from another subdomain.
///    `same-origin` and `none` (typed address, bookmark) pass this step.
/// 2. An `Origin` header refuses unless it equals the configured public
///    origin ([`origin_is_public`]) OR its host and port equal the
///    request's own `Host` ([`origin_matches_host`]). `Origin: null` never
///    passes.
/// 3. A request with neither header (native clients, older browsers)
///    proceeds.
pub fn cross_site_reason(
    headers: &HeaderMap,
    authority: Option<&str>,
    public_url: &str,
) -> Option<&'static str> {
    if let Some(site) = headers.get(SEC_FETCH_SITE).and_then(|v| v.to_str().ok())
        && (site.eq_ignore_ascii_case("cross-site") || site.eq_ignore_ascii_case("same-site"))
    {
        return Some("sec-fetch-site");
    }
    if let Some(origin) = headers.get(ORIGIN) {
        let host = headers
            .get(HOST)
            .and_then(|v| v.to_str().ok())
            .or(authority);
        if !origin_is_public(Some(origin), public_url) && !origin_matches_host(Some(origin), host) {
            return Some("foreign-origin");
        }
    }
    None
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

    fn reason(pairs: &[(&str, &str)], host: Option<&str>) -> Option<&'static str> {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        cross_site_reason(&h, host, "https://comics.example.com")
    }

    #[test]
    fn cross_site_rule() {
        assert_eq!(reason(&[], None), None);
        assert_eq!(reason(&[("sec-fetch-site", "same-origin")], None), None);
        assert_eq!(reason(&[("sec-fetch-site", "none")], None), None);
        assert_eq!(
            reason(&[("sec-fetch-site", "cross-site")], None),
            Some("sec-fetch-site")
        );
        assert_eq!(
            reason(&[("sec-fetch-site", "same-site")], None),
            Some("sec-fetch-site")
        );
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
            Some("foreign-origin")
        );
        assert_eq!(
            reason(&[("origin", "null"), ("host", "comics.example.com")], None),
            Some("foreign-origin")
        );
    }
}
