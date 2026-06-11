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
//! 3. **Address pinning (SEC-3)** — the DNS-rebind window (host resolves
//!    public on the pre-flight lookup, private on the connect lookup) is
//!    closed by pinning: [`fetch_public_bytes`] builds the per-hop client with
//!    `reqwest::ClientBuilder::resolve(host, vetted_ip)`, so reqwest connects
//!    to the exact address we vetted instead of re-resolving the hostname.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, LOCATION, USER_AGENT};
use reqwest::redirect::Policy;

pub const MAX_IMAGE_BYTES: usize = 24 * 1024 * 1024;

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

#[derive(Debug, thiserror::Error)]
pub enum FetchBytesError {
    #[error("{0}")]
    Ssrf(#[from] SsrfError),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("upstream returned status {0}")]
    HttpStatus(reqwest::StatusCode),
    #[error("response exceeds {max_bytes} bytes")]
    TooLarge { max_bytes: usize },
    #[error("missing or invalid redirect location")]
    InvalidRedirect,
    #[error("exceeded {0} redirect hops")]
    TooManyRedirects(usize),
    #[error("request timed out")]
    Timeout,
}

#[derive(Debug)]
pub struct FetchedBytes {
    pub final_url: url::Url,
    pub headers: HeaderMap,
    pub bytes: Vec<u8>,
}

/// Parse + syntactic validation. Does not touch the network. Returns the
/// parsed URL on success so callers can pass the canonical form to reqwest.
pub fn validate_outbound_url(url: &str) -> Result<url::Url, SsrfError> {
    validate_public_url(url, true)
}

pub fn validate_public_http_url(url: &str) -> Result<url::Url, SsrfError> {
    validate_public_url(url, false)
}

fn validate_public_url(url: &str, require_https: bool) -> Result<url::Url, SsrfError> {
    let parsed = url::Url::parse(url).map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;
    if require_https && parsed.scheme() != "https" {
        return Err(SsrfError::SchemeNotHttps);
    }
    if !matches!(parsed.scheme(), "https" | "http") {
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

/// Fetch a public HTTP(S) URL with SSRF validation on the initial URL and every
/// redirect target, then stream the body into memory with `max_bytes` enforced.
/// `require_https` preserves stricter HTTPS-only call sites such as CBL URL
/// imports while allowing provider-owned cover URLs that still use plain HTTP.
pub async fn fetch_public_bytes(
    url: &str,
    max_bytes: usize,
    timeout: std::time::Duration,
    user_agent: &'static str,
    max_redirects: usize,
    require_https: bool,
) -> Result<FetchedBytes, FetchBytesError> {
    let mut current = if require_https {
        validate_outbound_url(url)?
    } else {
        validate_public_http_url(url)?
    };

    for hop in 0..=max_redirects {
        // Vet the host and capture the exact address we approved.
        let pinned = validate_and_resolve(&current, require_https).await?;
        let host = current
            .host_str()
            .ok_or(FetchBytesError::Ssrf(SsrfError::MissingHost))?;
        // Pin the connection to that vetted address: reqwest connects to
        // `pinned` instead of re-resolving `host`, so a rebinding DNS answer
        // can't redirect the second lookup to a private IP (SEC-3). A fresh
        // per-hop client keeps the override scoped to this one request; the
        // path is low-volume (CBL import / cover fetch) so the lost connection
        // pooling is irrelevant.
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .resolve(host, pinned)
            .build()
            .map_err(|e| FetchBytesError::Transport(e.to_string()))?;
        let resp = tokio::time::timeout(
            timeout,
            client
                .get(current.clone())
                .header(USER_AGENT, HeaderValue::from_static(user_agent))
                .send(),
        )
        .await
        .map_err(|_| FetchBytesError::Timeout)?
        .map_err(|e| FetchBytesError::Transport(e.to_string()))?;

        if resp.status().is_redirection() {
            if hop == max_redirects {
                return Err(FetchBytesError::TooManyRedirects(max_redirects));
            }
            let location = resp
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(FetchBytesError::InvalidRedirect)?;
            let next = current
                .join(location)
                .map_err(|_| FetchBytesError::InvalidRedirect)?;
            current = if require_https {
                validate_outbound_url(next.as_str())?
            } else {
                validate_public_http_url(next.as_str())?
            };
            continue;
        }

        if !resp.status().is_success() {
            return Err(FetchBytesError::HttpStatus(resp.status()));
        }
        if resp
            .content_length()
            .is_some_and(|len| len > max_bytes as u64)
        {
            return Err(FetchBytesError::TooLarge { max_bytes });
        }

        let headers = resp.headers().clone();
        let mut stream = resp.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = tokio::time::timeout(timeout, stream.next())
            .await
            .map_err(|_| FetchBytesError::Timeout)?
        {
            let chunk = chunk.map_err(|e| FetchBytesError::Transport(e.to_string()))?;
            if bytes.len().saturating_add(chunk.len()) > max_bytes {
                return Err(FetchBytesError::TooLarge { max_bytes });
            }
            bytes.extend_from_slice(&chunk);
        }

        return Ok(FetchedBytes {
            final_url: current,
            headers,
            bytes,
        });
    }

    Err(FetchBytesError::TooManyRedirects(max_redirects))
}

/// Syntactically validate `url`, resolve its host, and return the exact
/// address the caller should pin the connection to. Fails if *any* resolved
/// address is internal (a single tainted A/AAAA record rejects the request).
async fn validate_and_resolve(
    url: &url::Url,
    require_https: bool,
) -> Result<SocketAddr, SsrfError> {
    if require_https {
        validate_outbound_url(url.as_str())?;
    } else {
        validate_public_http_url(url.as_str())?;
    }
    let host = url.host_str().ok_or(SsrfError::MissingHost)?;
    let port = url.port_or_known_default().unwrap_or(match url.scheme() {
        "http" => 80,
        _ => 443,
    });
    let ip = resolve_pinned_addr(host, port).await?;
    Ok(SocketAddr::new(ip, port))
}

/// Resolve `host`:`port` and return the first address, failing if *any*
/// resolved address is internal. The returned IP is the one the fetch pins the
/// connection to, so the request connects to exactly the vetted address rather
/// than re-resolving (SEC-3). IP-literal hosts short-circuit (no DNS).
async fn resolve_pinned_addr(host: &str, port: u16) -> Result<IpAddr, SsrfError> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return if is_internal_ip(&ip) {
            Err(SsrfError::PrivateAddress(ip))
        } else {
            Ok(ip)
        };
    }
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| SsrfError::Unresolvable(e.to_string()))?
        .collect();
    if addrs.is_empty() {
        return Err(SsrfError::Unresolvable(format!("no addresses for {host}")));
    }
    for addr in &addrs {
        if is_internal_ip(&addr.ip()) {
            return Err(SsrfError::PrivateAddress(addr.ip()));
        }
    }
    Ok(addrs[0].ip())
}

/// Resolve `host` (with the URL's port, defaulting to 443 for https) and
/// fail if *any* resolved address is in an internal range. A standalone guard
/// for callers that fetch with their own client (e.g. the admin OIDC-discovery
/// probe); the bytes-fetch path uses [`resolve_pinned_addr`] directly so it can
/// pin the vetted address.
pub async fn check_host_resolves_public(host: &str, port: u16) -> Result<(), SsrfError> {
    resolve_pinned_addr(host, port).await.map(|_| ())
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
    fn validate_public_http_url_accepts_http_for_cover_fetches() {
        let parsed = validate_public_http_url("http://example.com/cover.jpg").unwrap();
        assert_eq!(parsed.scheme(), "http");
    }

    #[tokio::test]
    async fn fetch_public_bytes_rejects_internal_ip_before_fetch() {
        let err = fetch_public_bytes(
            "http://127.0.0.1/cover.jpg",
            MAX_IMAGE_BYTES,
            std::time::Duration::from_secs(1),
            "folio-test",
            2,
            false,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            FetchBytesError::Ssrf(SsrfError::PrivateAddress(_))
        ));
    }

    #[tokio::test]
    async fn resolve_pinned_returns_public_literal() {
        // IP-literal hosts short-circuit DNS and return the literal for pinning.
        let ip = resolve_pinned_addr("8.8.8.8", 443).await.unwrap();
        assert_eq!(ip, "8.8.8.8".parse::<IpAddr>().unwrap());
    }

    #[tokio::test]
    async fn resolve_pinned_rejects_internal_resolution() {
        // `localhost` resolves to loopback via the local resolver — the
        // resolved address (not just an IP literal) must be gated before it can
        // be pinned, which is the SEC-3 rebinding defense.
        let err = resolve_pinned_addr("localhost", 443).await.unwrap_err();
        assert!(
            matches!(err, SsrfError::PrivateAddress(_)),
            "expected PrivateAddress, got {err:?}"
        );
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
