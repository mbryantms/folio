//! Defence-in-depth against server-side request forgery on user- or
//! provider-supplied URLs: CBL reading lists imported from a URL (and the
//! refreshes that re-fetch them), and provider cover images. Without it an
//! authenticated caller could have the server reach internal network
//! ranges, cloud metadata endpoints and the loopback interface.
//!
//! Layers:
//!
//! 1. **Pre-flight** — parse the URL, require `https://` where the caller
//!    asks, resolve the hostname (via `tokio::net::lookup_host`), and refuse
//!    if *any* resolved address is in a range [`is_internal_ip`] lists. A
//!    single A/AAAA record pointing into private space taints the request.
//!
//! 2. **Redirects** — [`fetch_public`] follows redirects itself, one hop at
//!    a time, re-running the pre-flight on every target, up to a bounded
//!    hop count. The pooled client ([`shared_public_client`]) uses
//!    [`outbound_redirect_policy`] plus a vetting resolver instead.
//!
//! 3. **Address pinning (SEC-3)** — the DNS-rebind window (host resolves
//!    public on the pre-flight lookup, private on the connect lookup) is
//!    closed by pinning: [`fetch_public`] builds the per-hop client with
//!    `reqwest::ClientBuilder::resolve(host, vetted_ip)`, so reqwest connects
//!    to the exact address we vetted instead of re-resolving the hostname.
//!
//! 4. **Bounds** — a body cap enforced while streaming (and up front from
//!    `Content-Length`), a per-step timeout, and an optional overall
//!    deadline for the whole fetch.
//!
//! Refusal messages are generic: they never name the resolved address or
//! the range it matched. The detail is logged at DEBUG.
//!
//! Admin-configured endpoints that are documented to allow private hosts
//! (the OIDC issuer and its discovery probe, metadata-provider base URLs)
//! do not go through this module.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use futures::StreamExt;
use reqwest::header::{
    HeaderMap, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH, LOCATION, USER_AGENT,
};
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
    /// The URL's host is, or resolves to, an address outside the public
    /// internet. The message is deliberately generic: it names neither the
    /// address nor the range it fell in (both are logged at DEBUG).
    #[error("URL points to an address that is not allowed")]
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
    /// The host answered with a bot-protection interstitial (Cloudflare's
    /// `cf-mitigated: challenge`) instead of the resource. Not retryable
    /// by a server-side client — only a browser that runs the challenge
    /// script gets through — so callers fail fast instead of retrying.
    #[error(
        "{host} answered with a bot challenge (status {status}); it refuses non-browser downloads"
    )]
    Challenged {
        host: String,
        status: reqwest::StatusCode,
    },
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
    validate_public_url_with(url, true, is_internal_ip)
}

pub fn validate_public_http_url(url: &str) -> Result<url::Url, SsrfError> {
    validate_public_url_with(url, false, is_internal_ip)
}

fn validate_public_url_with(
    url: &str,
    require_https: bool,
    refuse: RefusePolicy,
) -> Result<url::Url, SsrfError> {
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
        && refuse(&ip)
    {
        return Err(refused(ip));
    }
    Ok(parsed)
}

/// True when a non-success response is a bot-protection interstitial
/// rather than a real error: Cloudflare marks its managed/JS challenges
/// with `cf-mitigated: challenge` (status 403, sometimes 503/429).
pub fn is_bot_challenge(status: reqwest::StatusCode, headers: &HeaderMap) -> bool {
    !status.is_success()
        && headers
            .get("cf-mitigated")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("challenge"))
}

/// Classify a non-success status: a bot challenge gets its own variant so
/// callers can fail fast (and remember the host) instead of retrying.
fn status_error(
    url: &url::Url,
    status: reqwest::StatusCode,
    headers: &HeaderMap,
) -> FetchBytesError {
    if is_bot_challenge(status, headers) {
        FetchBytesError::Challenged {
            host: url.host_str().unwrap_or_default().to_owned(),
            status,
        }
    } else {
        FetchBytesError::HttpStatus(status)
    }
}

/// Fetch a public HTTP(S) URL with SSRF validation on the initial URL and every
/// redirect target, then stream the body into memory with `max_bytes` enforced.
/// `require_https` preserves stricter HTTPS-only call sites such as CBL URL
/// imports while allowing provider-owned cover URLs that still use plain HTTP.
/// `timeout` bounds each step (sending a request, waiting for the next body
/// chunk); callers that need one overall deadline use [`fetch_public`].
pub async fn fetch_public_bytes(
    url: &str,
    max_bytes: usize,
    timeout: std::time::Duration,
    user_agent: &'static str,
    max_redirects: usize,
    require_https: bool,
) -> Result<FetchedBytes, FetchBytesError> {
    let opts = FetchOptions {
        max_bytes,
        idle_timeout: timeout,
        deadline: None,
        user_agent,
        max_redirects,
        require_https,
    };
    match fetch_public(url, &opts, Conditional::default()).await? {
        FetchOutcome::Fetched(fetched) => Ok(*fetched),
        // No validators were sent, so a 304 is not an answer to anything
        // this caller asked; treat it like any other non-success status.
        FetchOutcome::NotModified => Err(FetchBytesError::HttpStatus(
            reqwest::StatusCode::NOT_MODIFIED,
        )),
    }
}

/// Limits for one [`fetch_public`] call.
#[derive(Debug, Clone, Copy)]
pub struct FetchOptions {
    /// Body cap; a larger `Content-Length` is refused before reading.
    pub max_bytes: usize,
    /// Bound on each step: one request/response exchange, one body chunk.
    pub idle_timeout: std::time::Duration,
    /// One deadline for the whole fetch (every hop's connect, headers and
    /// body). `None` leaves only the per-step bound.
    pub deadline: Option<std::time::Duration>,
    pub user_agent: &'static str,
    pub max_redirects: usize,
    pub require_https: bool,
}

/// Cache validators for a conditional GET. Sent on every hop.
#[derive(Debug, Clone, Copy, Default)]
pub struct Conditional<'a> {
    pub etag: Option<&'a str>,
    pub last_modified: Option<&'a str>,
}

#[derive(Debug)]
pub enum FetchOutcome {
    Fetched(Box<FetchedBytes>),
    /// The server answered `304 Not Modified` to the [`Conditional`]
    /// validators.
    NotModified,
}

/// The one fetch path for user- or provider-supplied URLs: syntactic
/// validation, DNS vetting of every hop with the connection pinned to the
/// vetted address, redirects followed by hand (each target re-vetted, at
/// most `max_redirects` hops), optional conditional validators, a streamed
/// body cap and an optional overall deadline.
pub async fn fetch_public(
    url: &str,
    opts: &FetchOptions,
    conditional: Conditional<'_>,
) -> Result<FetchOutcome, FetchBytesError> {
    fetch_public_with_policy(url, opts, conditional, is_internal_ip).await
}

/// Which addresses a fetch refuses. Production always passes
/// [`is_internal_ip`]; the parameter exists so this module's own tests can
/// reach a loopback test server while still refusing every other internal
/// address. It is private: no caller outside this module can relax it.
type RefusePolicy = fn(&IpAddr) -> bool;

async fn fetch_public_with_policy(
    url: &str,
    opts: &FetchOptions,
    conditional: Conditional<'_>,
    refuse: RefusePolicy,
) -> Result<FetchOutcome, FetchBytesError> {
    let fut = fetch_hops(url, opts, conditional, refuse);
    match opts.deadline {
        Some(deadline) => tokio::time::timeout(deadline, fut)
            .await
            .map_err(|_| FetchBytesError::Timeout)?,
        None => fut.await,
    }
}

async fn fetch_hops(
    url: &str,
    opts: &FetchOptions,
    conditional: Conditional<'_>,
    refuse: RefusePolicy,
) -> Result<FetchOutcome, FetchBytesError> {
    let mut current = validate_public_url_with(url, opts.require_https, refuse)?;

    for hop in 0..=opts.max_redirects {
        // Vet the host and capture the exact address we approved.
        let pinned = validate_and_resolve(&current, opts.require_https, refuse).await?;
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
        let mut req = client
            .get(current.clone())
            .header(USER_AGENT, HeaderValue::from_static(opts.user_agent));
        if let Some(etag) = conditional.etag {
            req = req.header(IF_NONE_MATCH, etag);
        }
        if let Some(lm) = conditional.last_modified {
            req = req.header(IF_MODIFIED_SINCE, lm);
        }
        let resp = tokio::time::timeout(opts.idle_timeout, req.send())
            .await
            .map_err(|_| FetchBytesError::Timeout)?
            .map_err(|e| FetchBytesError::Transport(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_MODIFIED
            && (conditional.etag.is_some() || conditional.last_modified.is_some())
        {
            return Ok(FetchOutcome::NotModified);
        }

        if resp.status().is_redirection() {
            if hop == opts.max_redirects {
                return Err(FetchBytesError::TooManyRedirects(opts.max_redirects));
            }
            let location = resp
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(FetchBytesError::InvalidRedirect)?;
            let next = current
                .join(location)
                .map_err(|_| FetchBytesError::InvalidRedirect)?;
            current = validate_public_url_with(next.as_str(), opts.require_https, refuse)?;
            continue;
        }

        if !resp.status().is_success() {
            return Err(status_error(&current, resp.status(), resp.headers()));
        }
        let headers = resp.headers().clone();
        let bytes = read_body_capped(resp, opts.max_bytes, Some(opts.idle_timeout)).await?;

        return Ok(FetchOutcome::Fetched(Box::new(FetchedBytes {
            final_url: current,
            headers,
            bytes,
        })));
    }

    Err(FetchBytesError::TooManyRedirects(opts.max_redirects))
}

/// Read a response body into memory, refusing it as soon as it is known to
/// exceed `max_bytes`: up front when `Content-Length` says so, otherwise
/// while streaming (never buffering more than `max_bytes` plus one chunk).
/// `idle_timeout` bounds the wait for each chunk.
pub async fn read_body_capped(
    resp: reqwest::Response,
    max_bytes: usize,
    idle_timeout: Option<std::time::Duration>,
) -> Result<Vec<u8>, FetchBytesError> {
    if resp
        .content_length()
        .is_some_and(|len| len > max_bytes as u64)
    {
        return Err(FetchBytesError::TooLarge { max_bytes });
    }
    let mut stream = resp.bytes_stream();
    let mut bytes = Vec::new();
    loop {
        let next = match idle_timeout {
            Some(t) => tokio::time::timeout(t, stream.next())
                .await
                .map_err(|_| FetchBytesError::Timeout)?,
            None => stream.next().await,
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|e| FetchBytesError::Transport(e.to_string()))?;
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(FetchBytesError::TooLarge { max_bytes });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Syntactically validate `url`, resolve its host, and return the exact
/// address the caller should pin the connection to. Fails if *any* resolved
/// address is refused (a single tainted A/AAAA record rejects the request).
async fn validate_and_resolve(
    url: &url::Url,
    require_https: bool,
    refuse: RefusePolicy,
) -> Result<SocketAddr, SsrfError> {
    validate_public_url_with(url.as_str(), require_https, refuse)?;
    let host = url.host_str().ok_or(SsrfError::MissingHost)?;
    let port = url.port_or_known_default().unwrap_or(match url.scheme() {
        "http" => 80,
        _ => 443,
    });
    let ip = resolve_pinned_addr(host, port, refuse).await?;
    Ok(SocketAddr::new(ip, port))
}

/// Resolve `host`:`port` and return the first address, failing if *any*
/// resolved address is refused. The returned IP is the one the fetch pins the
/// connection to, so the request connects to exactly the vetted address rather
/// than re-resolving (SEC-3). IP-literal hosts short-circuit (no DNS); a
/// bracketed IPv6 literal (`[::1]`, as `host_str` returns it) counts as one.
async fn resolve_pinned_addr(
    host: &str,
    port: u16,
    refuse: RefusePolicy,
) -> Result<IpAddr, SsrfError> {
    let literal = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = literal.parse::<IpAddr>() {
        return if refuse(&ip) {
            Err(refused(ip))
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
        if refuse(&addr.ip()) {
            return Err(refused(addr.ip()));
        }
    }
    Ok(addrs[0].ip())
}

/// Build the refusal for a disallowed address. The user-facing text is
/// generic (see [`SsrfError::PrivateAddress`]); the address itself is only
/// logged, at DEBUG, for an operator diagnosing a refused URL.
fn refused(ip: IpAddr) -> SsrfError {
    tracing::debug!(addr = %ip, "outbound URL refused: address is not publicly routable");
    SsrfError::PrivateAddress(ip)
}

// ───────── pooled client (WP-2.9) ─────────

/// DNS resolver that vets every answer before hyper connects to it —
/// the pooled-client counterpart of [`resolve_pinned_addr`]. Because
/// hyper connects to exactly the addresses this returns, a rebinding
/// answer can't slip a private IP past the check (SEC-3 holds without
/// a per-request `.resolve()` pin, so connections pool across calls).
/// IP-literal hosts never reach a resolver; the URL validators and the
/// redirect policy gate those.
struct PublicOnlyResolver;

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| Box::new(SsrfError::Unresolvable(e.to_string())) as BoxError)?
                .collect();
            if addrs.is_empty() {
                return Err(
                    Box::new(SsrfError::Unresolvable(format!("no addresses for {host}")))
                        as BoxError,
                );
            }
            for addr in &addrs {
                if is_internal_ip(&addr.ip()) {
                    return Err(Box::new(refused(addr.ip())) as BoxError);
                }
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Build a long-lived, connection-pooling client that is safe for
/// user- or provider-supplied URLs: every DNS answer is vetted by
/// [`PublicOnlyResolver`], every redirect hop by
/// [`outbound_redirect_policy`]. Pair with [`fetch_public_bytes_pooled`],
/// which validates the initial URL. Used for the search-time cover
/// hashing fan-out, where a fresh client per fetch threw away the
/// pooled connection on every one of ~100 URLs per search.
pub fn shared_public_client(
    user_agent: &'static str,
    timeout: std::time::Duration,
    max_redirects: usize,
) -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(user_agent)
        .timeout(timeout)
        .connect_timeout(std::time::Duration::from_secs(10))
        .redirect(outbound_redirect_policy(max_redirects))
        .dns_resolver(std::sync::Arc::new(PublicOnlyResolver))
        .build()
        .expect("reqwest client init")
}

/// [`fetch_public_bytes`] over a [`shared_public_client`]: same URL
/// validation and body cap, but the DNS vetting happens inside the
/// client's resolver and redirects are followed by the client's policy,
/// so the connection pool survives across calls. `timeout` bounds the
/// whole fetch including the body.
pub async fn fetch_public_bytes_pooled(
    client: &reqwest::Client,
    url: &str,
    max_bytes: usize,
    timeout: std::time::Duration,
    require_https: bool,
) -> Result<FetchedBytes, FetchBytesError> {
    let parsed = if require_https {
        validate_outbound_url(url)?
    } else {
        validate_public_http_url(url)?
    };
    let fut = async {
        let resp = client
            .get(parsed)
            .send()
            .await
            .map_err(|e| FetchBytesError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(status_error(resp.url(), resp.status(), resp.headers()));
        }
        let final_url = resp.url().clone();
        let headers = resp.headers().clone();
        let bytes = read_body_capped(resp, max_bytes, None).await?;
        Ok(FetchedBytes {
            final_url,
            headers,
            bytes,
        })
    };
    tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| FetchBytesError::Timeout)?
}

/// Build a redirect policy that limits hops to `max_hops` and rejects
/// any hop whose URL host is a private-range IP literal. Hostname
/// targets are *not* re-resolved here (sync callback, no async DNS);
/// the per-request pre-flight — or, on a [`shared_public_client`], the
/// vetting resolver — covers that.
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
            return attempt.error(refused(ip));
        }
        attempt.follow()
    })
}

/// `true` if `ip` is in any range the server should refuse to talk to
/// from a user- or provider-supplied URL: every IANA special-purpose block
/// that is not globally reachable, plus multicast and the reserved space.
/// IPv6 forms that carry an IPv4 address (mapped, compatible, NAT64, 6to4)
/// are unwrapped and the embedded address is checked by the IPv4 rules.
///
/// IPv4: 0.0.0.0/8, 10.0.0.0/8, 100.64.0.0/10, 127.0.0.0/8, 169.254.0.0/16,
/// 172.16.0.0/12, 192.0.0.0/24, 192.0.2.0/24, 192.88.99.0/24,
/// 192.168.0.0/16, 198.18.0.0/15, 198.51.100.0/24, 203.0.113.0/24,
/// 224.0.0.0/4, 240.0.0.0/4 (includes 255.255.255.255).
///
/// IPv6: ::, ::1, ::ffff:0:0/96 and ::/96 (embedded IPv4), 64:ff9b::/96 and
/// the /96 form of 64:ff9b:1::/48 (embedded IPv4; the rest of that /48 is
/// refused), 100::/64, 2001::/32 (Teredo), 2001:2::/48, 2001:db8::/32,
/// 2002::/16 (embedded IPv4), 3fff::/20, 5f00::/16, fc00::/7, fe80::/10,
/// fec0::/10, ff00::/8.
pub fn is_internal_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_internal_v4(v4),
        IpAddr::V6(v6) => is_internal_v6(v6),
    }
}

fn is_internal_v4(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    let in_net = |net: [u8; 4], prefix: u32| -> bool {
        let mask = if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - prefix)
        };
        (u32::from(*ip) & mask) == (u32::from_be_bytes(net) & mask)
    };
    o[0] == 0 // 0.0.0.0/8 "this network"
        || ip.is_private() // 10/8, 172.16/12, 192.168/16
        || ip.is_loopback() // 127/8
        || ip.is_link_local() // 169.254/16
        || in_net([100, 64, 0, 0], 10) // carrier-grade NAT
        || in_net([192, 0, 0, 0], 24) // IETF protocol assignments
        || in_net([192, 0, 2, 0], 24) // TEST-NET-1
        || in_net([192, 88, 99, 0], 24) // deprecated 6to4 relay anycast
        || in_net([198, 18, 0, 0], 15) // benchmarking
        || in_net([198, 51, 100, 0], 24) // TEST-NET-2
        || in_net([203, 0, 113, 0], 24) // TEST-NET-3
        || ip.is_multicast() // 224/4
        || o[0] >= 240 // 240/4 reserved, incl. 255.255.255.255
}

fn is_internal_v6(ip: &Ipv6Addr) -> bool {
    let seg = ip.segments();
    if ip.is_unspecified() || ip.is_loopback() {
        return true;
    }
    // ::ffff:0:0/96 (mapped) and ::/96 (deprecated compatible) carry an
    // IPv4 address in the low 32 bits.
    if seg[..5] == [0; 5] && (seg[5] == 0xffff || seg[5] == 0) {
        return is_internal_v4(&low_v4(ip));
    }
    // 64:ff9b::/96 well-known NAT64 prefix.
    if seg[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return is_internal_v4(&low_v4(ip));
    }
    // 64:ff9b:1::/48 local-use NAT64: only the /96 form has a known
    // embedded address; anything else in the block is refused.
    if seg[..3] == [0x64, 0xff9b, 1] {
        return seg[3..6] != [0, 0, 0] || is_internal_v4(&low_v4(ip));
    }
    // 2002::/16 6to4: the IPv4 address is the next 32 bits.
    if seg[0] == 0x2002 {
        let v4 = Ipv4Addr::from((u32::from(seg[1]) << 16) | u32::from(seg[2]));
        return is_internal_v4(&v4);
    }
    (seg[..4] == [0x100, 0, 0, 0]) // 100::/64 discard-only
        || (seg[0] == 0x2001 && seg[1] == 0) // 2001::/32 Teredo
        || (seg[0] == 0x2001 && seg[1] == 2 && seg[2] == 0) // 2001:2::/48 benchmarking
        || (seg[0] == 0x2001 && seg[1] == 0x0db8) // documentation
        || (seg[0] == 0x3fff && (seg[1] & 0xf000) == 0) // 3fff::/20 documentation
        || seg[0] == 0x5f00 // 5f00::/16 segment routing SIDs
        || (seg[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local
        || (seg[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        || (seg[0] & 0xffc0) == 0xfec0 // fec0::/10 deprecated site-local
        || ip.is_multicast() // ff00::/8
}

/// The low 32 bits of an IPv6 address as IPv4.
fn low_v4(ip: &Ipv6Addr) -> Ipv4Addr {
    let o = ip.octets();
    Ipv4Addr::new(o[12], o[13], o[14], o[15])
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
    async fn pooled_fetch_rejects_internal_ip_literal_before_fetch() {
        let client = shared_public_client("folio-test", std::time::Duration::from_secs(1), 2);
        let err = fetch_public_bytes_pooled(
            &client,
            "http://127.0.0.1/cover.jpg",
            MAX_IMAGE_BYTES,
            std::time::Duration::from_secs(1),
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
    async fn pooled_client_resolver_rejects_hosts_that_resolve_internally() {
        // `localhost` passes the syntactic check (it's a domain) but the
        // vetting resolver refuses the loopback answer, so the request
        // never reaches the (loopback-bound) mock server: `.expect(0)`
        // is verified when the server drops.
        let mock = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(vec![0u8; 8]))
            .expect(0)
            .mount(&mock)
            .await;
        let client = shared_public_client("folio-test", std::time::Duration::from_secs(2), 2);
        let url = format!("http://localhost:{}/cover.jpg", mock.address().port());
        let err = fetch_public_bytes_pooled(
            &client,
            &url,
            MAX_IMAGE_BYTES,
            std::time::Duration::from_secs(2),
            false,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, FetchBytesError::Transport(_)),
            "expected the resolver to fail the connect, got {err:?}"
        );
    }

    #[tokio::test]
    async fn resolve_pinned_returns_public_literal() {
        // IP-literal hosts short-circuit DNS and return the literal for pinning.
        let ip = resolve_pinned_addr("8.8.8.8", 443, is_internal_ip)
            .await
            .unwrap();
        assert_eq!(ip, "8.8.8.8".parse::<IpAddr>().unwrap());
    }

    #[tokio::test]
    async fn resolve_pinned_rejects_internal_resolution() {
        // `localhost` resolves to loopback via the local resolver — the
        // resolved address (not just an IP literal) must be gated before it can
        // be pinned, which is the SEC-3 rebinding defense.
        let err = resolve_pinned_addr("localhost", 443, is_internal_ip)
            .await
            .unwrap_err();
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
    fn added_v4_ranges_are_internal() {
        for raw in [
            "0.1.2.3",         // 0.0.0.0/8
            "0.255.255.255",   // 0.0.0.0/8 upper
            "192.0.0.1",       // 192.0.0.0/24
            "192.0.0.255",     // 192.0.0.0/24 upper
            "192.88.99.1",     // deprecated 6to4 relay anycast
            "198.18.0.1",      // benchmarking 198.18.0.0/15
            "198.19.255.255",  // benchmarking upper
            "239.255.255.255", // multicast upper
            "254.1.2.3",       // reserved 240.0.0.0/4
        ] {
            let ip: IpAddr = raw.parse().unwrap();
            assert!(is_internal_ip(&ip), "expected {raw} to be internal");
        }
        // Neighbours of the added blocks stay public.
        for raw in [
            "1.0.0.1",
            "192.0.1.1",
            "192.88.98.1",
            "198.17.255.255",
            "198.20.0.1",
            "223.255.255.255",
        ] {
            let ip: IpAddr = raw.parse().unwrap();
            assert!(!is_internal_ip(&ip), "expected {raw} to be public");
        }
    }

    #[test]
    fn added_v6_ranges_are_internal() {
        for raw in [
            "100::1",         // discard-only 100::/64
            "2001::1",        // Teredo 2001::/32
            "2001:0:ffff::1", // Teredo
            "2001:2::1",      // benchmarking 2001:2::/48
            "3fff::1",        // documentation 3fff::/20
            "3fff:fff::1",    // documentation upper
            "5f00::1",        // segment routing SIDs
            "fec0::1",        // deprecated site-local
            "64:ff9b:1:1::1", // local-use NAT64 outside its /96 form
        ] {
            let ip: IpAddr = raw.parse().unwrap();
            assert!(is_internal_ip(&ip), "expected {raw} to be internal");
        }
        for raw in [
            "100:0:0:1::1",
            "2001:4860::8888",
            "3fff:1000::1",
            "2620:fe::fe",
        ] {
            let ip: IpAddr = raw.parse().unwrap();
            assert!(!is_internal_ip(&ip), "expected {raw} to be public");
        }
    }

    #[test]
    fn ipv4_embedded_v6_forms_are_checked_by_the_embedded_address() {
        // (address, internal?) — each form wraps one internal and one
        // public IPv4 address.
        for (raw, internal) in [
            ("::ffff:10.0.0.1", true),     // mapped
            ("::ffff:8.8.8.8", false),     // mapped, public
            ("::127.0.0.1", true),         // compatible
            ("::169.254.169.254", true),   // compatible
            ("::8.8.8.8", false),          // compatible, public
            ("64:ff9b::7f00:1", true),     // NAT64 of 127.0.0.1
            ("64:ff9b::a9fe:a9fe", true),  // NAT64 of 169.254.169.254
            ("64:ff9b::808:808", false),   // NAT64 of 8.8.8.8
            ("64:ff9b:1::c0a8:101", true), // local-use NAT64 of 192.168.1.1
            ("64:ff9b:1::808:808", false), // local-use NAT64 of 8.8.8.8
            ("2002:7f00:1::1", true),      // 6to4 of 127.0.0.1
            ("2002:a00:1::1", true),       // 6to4 of 10.0.0.1
            ("2002:c612:1::1", true),      // 6to4 of 198.18.0.1
            ("2002:808:808::1", false),    // 6to4 of 8.8.8.8
        ] {
            let ip: IpAddr = raw.parse().unwrap();
            assert_eq!(is_internal_ip(&ip), internal, "{raw}");
        }
    }

    #[test]
    fn refusal_message_names_no_address() {
        let err = validate_outbound_url("https://169.254.169.254/latest").unwrap_err();
        let msg = err.to_string();
        assert!(matches!(err, SsrfError::PrivateAddress(_)));
        assert!(!msg.contains("169.254"), "{msg}");
        let fetch = FetchBytesError::from(err).to_string();
        assert!(!fetch.contains("169.254"), "{fetch}");
        let v6 = validate_outbound_url("https://[fd12::1]/list.cbl")
            .unwrap_err()
            .to_string();
        assert!(!v6.contains("fd12"), "{v6}");
    }

    // ───── real-HTTP tests of the shared fetch path ─────
    //
    // The test server listens on 127.0.0.1, which production refuses. These
    // tests use the module-private policy seam with a policy that admits
    // exactly 127.0.0.1 and refuses everything production refuses besides.

    fn admit_test_loopback(ip: &IpAddr) -> bool {
        *ip != IpAddr::V4(Ipv4Addr::LOCALHOST) && is_internal_ip(ip)
    }

    fn opts(deadline: Option<std::time::Duration>) -> FetchOptions {
        FetchOptions {
            max_bytes: 64 * 1024,
            idle_timeout: std::time::Duration::from_secs(5),
            deadline,
            user_agent: "folio-test",
            max_redirects: 2,
            require_https: false,
        }
    }

    const CONDITIONAL: Conditional<'static> = Conditional {
        etag: Some("\"v1\""),
        last_modified: Some("Mon, 02 Nov 2026 00:00:00 GMT"),
    };

    async fn fetch_test(
        url: &str,
        o: &FetchOptions,
        conditional: Conditional<'_>,
    ) -> Result<FetchOutcome, FetchBytesError> {
        fetch_public_with_policy(url, o, conditional, admit_test_loopback).await
    }

    #[tokio::test]
    async fn conditional_fetch_returns_not_modified_and_sends_validators() {
        use wiremock::matchers::{header, header_exists, method};
        let mock = wiremock::MockServer::start().await;
        // (wiremock splits header values on commas, so the date is only
        // checked for presence.)
        wiremock::Mock::given(method("GET"))
            .and(header("if-none-match", "\"v1\""))
            .and(header_exists("if-modified-since"))
            .respond_with(wiremock::ResponseTemplate::new(304))
            .expect(1)
            .mount(&mock)
            .await;
        let url = format!("http://127.0.0.1:{}/list.cbl", mock.address().port());
        let out = fetch_test(&url, &opts(None), CONDITIONAL).await.unwrap();
        assert!(matches!(out, FetchOutcome::NotModified));
    }

    #[tokio::test]
    async fn redirect_to_an_internal_address_is_refused_on_both_paths() {
        use wiremock::matchers::method;
        for target in [
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.1/list.cbl",
            "http://[::1]/list.cbl",
            "http://[::ffff:127.0.0.2]/list.cbl",
            "http://[64:ff9b::a00:1]/list.cbl",
        ] {
            for conditional in [Conditional::default(), CONDITIONAL] {
                let mock = wiremock::MockServer::start().await;
                wiremock::Mock::given(method("GET"))
                    .respond_with(
                        wiremock::ResponseTemplate::new(302).insert_header("location", target),
                    )
                    .expect(1)
                    .mount(&mock)
                    .await;
                let url = format!("http://127.0.0.1:{}/list.cbl", mock.address().port());
                let err = fetch_test(&url, &opts(None), conditional)
                    .await
                    .unwrap_err();
                assert!(
                    matches!(err, FetchBytesError::Ssrf(SsrfError::PrivateAddress(_))),
                    "{target} conditional={conditional:?}: {err:?}"
                );
                assert!(!err.to_string().contains("169.254"), "{err}");
            }
        }
    }

    #[tokio::test]
    async fn redirect_hops_are_bounded_on_both_paths() {
        use wiremock::matchers::method;
        let mock = wiremock::MockServer::start().await;
        let self_url = format!("http://127.0.0.1:{}/loop", mock.address().port());
        wiremock::Mock::given(method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(302).insert_header("location", self_url.as_str()),
            )
            .mount(&mock)
            .await;
        for conditional in [Conditional::default(), CONDITIONAL] {
            let err = fetch_test(&self_url, &opts(None), conditional)
                .await
                .unwrap_err();
            assert!(
                matches!(err, FetchBytesError::TooManyRedirects(2)),
                "{err:?}"
            );
        }
    }

    /// A raw server that sends `head`, then `body_chunk` every `every`
    /// forever (or until the client goes away).
    async fn trickle_server(
        head: &'static str,
        body_chunk: Vec<u8>,
        every: std::time::Duration,
    ) -> u16 {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let body_chunk = body_chunk.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut buf).await;
                    if sock.write_all(head.as_bytes()).await.is_err() {
                        return;
                    }
                    loop {
                        if sock.write_all(&body_chunk).await.is_err() {
                            return;
                        }
                        tokio::time::sleep(every).await;
                    }
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn a_trickling_body_hits_the_overall_deadline() {
        // One byte every 100 ms never trips the 5 s per-step timeout; only
        // the overall deadline ends the fetch.
        let port = trickle_server(
            "HTTP/1.1 200 OK\r\nContent-Length: 60000\r\n\r\n",
            b"x".to_vec(),
            std::time::Duration::from_millis(100),
        )
        .await;
        let url = format!("http://127.0.0.1:{port}/list.cbl");
        for conditional in [Conditional::default(), CONDITIONAL] {
            let started = std::time::Instant::now();
            let err = fetch_test(
                &url,
                &opts(Some(std::time::Duration::from_millis(800))),
                conditional,
            )
            .await
            .unwrap_err();
            assert!(matches!(err, FetchBytesError::Timeout), "{err:?}");
            assert!(started.elapsed() < std::time::Duration::from_secs(4));
        }
    }

    #[tokio::test]
    async fn oversize_bodies_are_refused_while_streaming() {
        // Declared too large: refused from the header, before any body.
        let port = trickle_server(
            "HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n",
            Vec::new(),
            std::time::Duration::from_secs(60),
        )
        .await;
        let url = format!("http://127.0.0.1:{port}/a");
        let err = fetch_test(
            &url,
            &opts(Some(std::time::Duration::from_secs(5))),
            Conditional::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, FetchBytesError::TooLarge { .. }), "{err:?}");

        // No length, endless chunked body: refused once the running total
        // passes the cap (it could never finish if it were buffered first).
        let port = trickle_server(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
            format!("400\r\n{}\r\n", "x".repeat(1024)).into_bytes(),
            std::time::Duration::from_millis(1),
        )
        .await;
        let url = format!("http://127.0.0.1:{port}/b");
        let err = fetch_test(
            &url,
            &opts(Some(std::time::Duration::from_secs(10))),
            Conditional::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, FetchBytesError::TooLarge { .. }), "{err:?}");
    }

    #[test]
    fn validate_url_accepts_well_formed_https() {
        let url = validate_outbound_url("https://example.com/list.cbl").unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("example.com"));
    }
}
