//! Per-route token-bucket rate limiting (§17.7).
//!
//! Buckets are keyed by the real client IP (via [`crate::middleware::RequestContext`]
//! → XFF walker → trusted-proxy CIDRs), or, for the signed-in web-app
//! surfaces built with [`Bucket::build_per_user`], by the user id of a
//! verified access token ([`UserKey`]). Token state lives in-memory inside
//! the `governor` crate's keyed-state store — fine for a single-replica
//! deployment, which is the Folio target. Multi-replica deploys would
//! require a Redis backend; that's tracked as a post-1.0 enhancement.
//! governor keeps one entry per key, so every built store is registered
//! (weakly) in a process-wide [`LimiterRegistry`] that [`spawn_pruner`]
//! sweeps once a minute, dropping keys whose bucket is full again.
//!
//! The failed-auth lockout (10/min/IP → 15-min lockout after threshold) is
//! Redis-backed and lives in [`crate::auth::failed_auth`] — it spans process
//! restarts so a brute-forcer can't reset by triggering a restart.
//!
//! On denial we return a JSON envelope matching the project's error contract
//! and set `Retry-After` (seconds). Denials emit
//! `folio_rate_limit_denied_total{bucket="…"}`.

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Request, Response, StatusCode, header};
use governor::RateLimiter;
use governor::clock::{Clock, DefaultClock};
use governor::middleware::RateLimitingMiddleware;
use governor::state::keyed::DefaultKeyedStateStore;
use std::hash::Hash;
use std::net::IpAddr;
use std::sync::{Arc, LazyLock, Mutex, PoisonError, Weak};
use std::time::Duration;
use tower_governor::governor::{GovernorConfig, GovernorConfigBuilder};
use tower_governor::key_extractor::KeyExtractor;
use tower_governor::{GovernorError, GovernorLayer};

use uuid::Uuid;

use super::RequestContext;
use crate::auth::extractor::access_token_subject;
use crate::auth::jwt::JwtKeys;

// ───────── key extractor ─────────

/// Pulls the real client IP from the [`RequestContext`] populated by
/// [`super::request_context::set_context`]. That middleware runs the XFF
/// walker against `COMIC_TRUSTED_PROXIES`, so this is the right answer
/// for per-IP buckets behind a reverse proxy.
#[derive(Debug, Clone, Copy)]
pub struct ClientIpKey;

impl KeyExtractor for ClientIpKey {
    type Key = IpAddr;

    fn extract<T>(&self, req: &Request<T>) -> Result<Self::Key, GovernorError> {
        req.extensions()
            .get::<RequestContext>()
            .and_then(|ctx| ctx.client_ip)
            .ok_or(GovernorError::UnableToExtractKey)
    }
}

/// The verifier [`UserKey`] checks access tokens with. The router inserts
/// it as a request extension (`app::router_with_timeouts`), since a key
/// extractor has no access to `AppState`.
#[derive(Clone)]
pub struct RateLimitIdentity(pub Arc<JwtKeys>);

/// Key of a per-user bucket: the signed-in user, or the client IP when the
/// request carries no valid JWT access token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UserOrIp {
    User(Uuid),
    Ip(IpAddr),
}

/// Per-user key (E3, audit N-A5). The user id comes from the signature-checked
/// JWT access token (`auth::extractor::access_token_subject`): a forged token
/// never names a user, so nobody can drain another user's bucket, and the
/// key set stays bounded by real users plus client IPs. An expired but
/// genuine token still names its user, so a tab waking up with a stale token
/// spends its own budget, not the shared IP one, and gets the 401 that makes
/// the client refresh. Requests without a signed JWT (anonymous, forged,
/// app-password, Basic) fall back to the [`ClientIpKey`] key; the handler
/// rejects the anonymous ones anyway.
/// Household members behind one NAT therefore get a budget each, which a
/// per-IP key would not give them.
#[derive(Debug, Clone, Copy)]
pub struct UserKey;

impl KeyExtractor for UserKey {
    type Key = UserOrIp;

    fn extract<T>(&self, req: &Request<T>) -> Result<Self::Key, GovernorError> {
        let user = req
            .extensions()
            .get::<RateLimitIdentity>()
            .and_then(|id| access_token_subject(req.headers(), &id.0));
        match user {
            Some(id) => Ok(UserOrIp::User(id)),
            None => ClientIpKey.extract(req).map(UserOrIp::Ip),
        }
    }
}

// ───────── bucket factory ─────────

/// A single rate-limit bucket. Built once per process via [`Bucket::build`]
/// (per-IP key) or [`Bucket::build_per_user`] (per-user key). Each call
/// makes an independent token store: routes share a budget only when they
/// sit under the same built layer.
pub struct Bucket {
    /// Telemetry name for the denial counter (`folio_rate_limit_denied_total{bucket=…}`).
    pub name: &'static str,
    /// Replenishment interval — one token per `period`.
    pub period: Duration,
    /// Burst size — max tokens in the bucket. Must be `>= 1`.
    pub burst: u32,
}

impl Bucket {
    /// Build a per-IP tower layer ready to plug onto a route.
    pub fn build(&self) -> GovernorLayer<ClientIpKey, governor::middleware::NoOpMiddleware, Body> {
        self.build_keyed(ClientIpKey)
    }

    /// Build a per-user tower layer ([`UserKey`]).
    pub fn build_per_user(
        &self,
    ) -> GovernorLayer<UserKey, governor::middleware::NoOpMiddleware, Body> {
        self.build_keyed(UserKey)
    }

    fn build_keyed<K>(&self, key: K) -> GovernorLayer<K, governor::middleware::NoOpMiddleware, Body>
    where
        K: KeyExtractor,
        K::Key: Send + Sync + 'static,
    {
        let config = self.config(key);
        LIMITERS.register(self.name, config.limiter().clone());
        let name = self.name;
        GovernorLayer::new(Arc::new(config))
            .error_handler(move |err| handle_governor_error(name, err))
    }
}

impl Bucket {
    /// The governor config [`Bucket::build_keyed`] wraps: this bucket's
    /// quota over `key`.
    fn config<K: KeyExtractor>(
        &self,
        key: K,
    ) -> GovernorConfig<K, governor::middleware::NoOpMiddleware> {
        let mut default = GovernorConfigBuilder::default();
        let mut builder = default.key_extractor(key);
        builder.period(self.period).burst_size(self.burst);
        builder.finish().expect("non-zero period + burst")
    }
}

// ───────── key-store pruning ─────────

/// How often [`spawn_pruner`] sweeps every registered limiter.
pub const PRUNE_INTERVAL: Duration = Duration::from_secs(60);

/// A keyed limiter whose store can drop the keys it no longer needs.
trait Prunable: Send + Sync {
    /// Drop every key whose bucket is back to full (governor's
    /// `retain_recent`), then release the freed capacity.
    fn prune(&self);
    /// Keys currently held.
    fn key_count(&self) -> usize;
}

impl<Key, M> Prunable for RateLimiter<Key, DefaultKeyedStateStore<Key>, DefaultClock, M>
where
    Key: Hash + Eq + Clone + Send + Sync,
    M: RateLimitingMiddleware<<DefaultClock as Clock>::Instant> + Send + Sync,
{
    fn prune(&self) {
        self.retain_recent();
        self.shrink_to_fit();
    }

    fn key_count(&self) -> usize {
        self.len()
    }
}

/// Every keyed store built by [`Bucket::build`] / [`Bucket::build_per_user`].
///
/// governor keeps one entry per key forever, so without a sweep a client
/// that rotates addresses grows a per-IP store without bound. Entries are
/// weak: the router owns its limiter, and a dropped router (the integration
/// tests build many apps per process) frees its store; the registry only
/// forgets the dead handle on its next touch.
#[derive(Default)]
pub struct LimiterRegistry {
    /// Each store with the name of the bucket it was built from.
    limiters: Mutex<Vec<(&'static str, Weak<dyn Prunable>)>>,
}

/// The process-wide registry [`Bucket::build`] registers into.
static LIMITERS: LazyLock<LimiterRegistry> = LazyLock::new(LimiterRegistry::default);

/// What one [`LimiterRegistry::prune`] pass saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PruneReport {
    /// Limiters still alive after the pass.
    pub limiters: usize,
    /// Keys left across them.
    pub keys: usize,
}

impl LimiterRegistry {
    fn register<L: Prunable + 'static>(&self, name: &'static str, limiter: Arc<L>) {
        let limiter: Arc<dyn Prunable> = limiter;
        let mut list = self.limiters.lock().unwrap_or_else(PoisonError::into_inner);
        // Forget dropped routers here as well, so a process that builds
        // routers but never runs the sweep (the test harness) keeps a
        // registry the size of its live routers.
        list.retain(|(_, w)| w.strong_count() > 0);
        list.push((name, Arc::downgrade(&limiter)));
    }

    /// Prune every live limiter and forget the dropped ones. The lock is
    /// held only to copy the live handles, never while a store is swept.
    pub fn prune(&self) -> PruneReport {
        let live: Vec<Arc<dyn Prunable>> = {
            let mut list = self.limiters.lock().unwrap_or_else(PoisonError::into_inner);
            list.retain(|(_, w)| w.strong_count() > 0);
            list.iter().filter_map(|(_, w)| w.upgrade()).collect()
        };
        let mut keys = 0;
        for limiter in &live {
            limiter.prune();
            keys += limiter.key_count();
        }
        PruneReport {
            limiters: live.len(),
            keys,
        }
    }

    /// Key counts of the live stores built from the bucket named `name`.
    #[cfg(test)]
    fn key_counts(&self, name: &str) -> Vec<usize> {
        let list = self.limiters.lock().unwrap_or_else(PoisonError::into_inner);
        list.iter()
            .filter(|(n, _)| *n == name)
            .filter_map(|(_, w)| w.upgrade())
            .map(|l| l.key_count())
            .collect()
    }
}

/// Sweep the process-wide registry every [`PRUNE_INTERVAL`] until
/// `shutdown` fires. Started once, from `app::serve`, next to the other
/// background upkeep tasks; the test harness never calls it.
pub fn spawn_pruner(shutdown: tokio_util::sync::CancellationToken) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(PRUNE_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = tick.tick() => {}
            }
            let report = LIMITERS.prune();
            tracing::debug!(
                limiters = report.limiters,
                keys = report.keys,
                "rate-limit stores pruned"
            );
        }
    })
}

// ───────── bucket catalog ─────────

// Buckets per `docs/architecture/rate-limits.md`. `period` is the token
// replenishment interval — `5/min` = one token every 12 seconds with a
// burst of 10.

/// `POST /auth/local/login` — 5/min/IP + burst 10.
pub const LOGIN: Bucket = Bucket {
    name: "auth.login",
    period: Duration::from_secs(12),
    burst: 10,
};

/// `GET /auth/oidc/callback` — 5/min/IP + burst 10.
pub const OIDC_CALLBACK: Bucket = Bucket {
    name: "auth.oidc_callback",
    period: Duration::from_secs(12),
    burst: 10,
};

/// `POST /auth/local/register` — 5/min/IP + burst 10. Same shape as login;
/// the bucket is named separately so the metric tag distinguishes.
pub const REGISTER: Bucket = Bucket {
    name: "auth.register",
    period: Duration::from_secs(12),
    burst: 10,
};

/// `POST /auth/local/request-password-reset` — 5/hour/IP + burst 5.
pub const PASSWORD_RESET_REQUEST: Bucket = Bucket {
    name: "auth.password_reset_request",
    period: Duration::from_secs(720),
    burst: 5,
};

/// `POST /auth/local/reset-password` — 10/hour/IP + burst 10.
pub const PASSWORD_RESET_REDEEM: Bucket = Bucket {
    name: "auth.password_reset_redeem",
    period: Duration::from_secs(360),
    burst: 10,
};

/// `POST /auth/local/resend-verification` — 5/hour/IP + burst 5.
pub const RESEND_VERIFICATION: Bucket = Bucket {
    name: "auth.resend_verification",
    period: Duration::from_secs(720),
    burst: 5,
};

/// `POST /auth/ws-ticket` — 30/s/IP + burst 60.
pub const WS_TICKET: Bucket = Bucket {
    name: "ws_ticket",
    period: Duration::from_millis(33),
    burst: 60,
};

/// `POST /csp-report` — 100/min/IP + burst 200.
pub const CSP_REPORT: Bucket = Bucket {
    name: "csp_report",
    period: Duration::from_millis(600),
    burst: 200,
};

/// OPDS catalog / navigation / search feeds (`/opds/v1/*`, `/opds/v2/*`
/// minus the byte + progress routes) — 30/min/IP + burst 60. Bounds indexers
/// and unattended catalog crawlers without throttling normal browsing (a
/// client paging through a feed rarely sustains more than one request every
/// two seconds). Split from [`OPDS_STREAM`] by security-audit H-3.2 /
/// WP-6.3: one shared budget let a crawler hammering `search` / `series`
/// starve the same IP's page reads.
pub const OPDS_CATALOG: Bucket = Bucket {
    name: "opds_catalog",
    period: Duration::from_secs(2),
    burst: 60,
};

/// OPDS reading path — downloads (`…/issues/{id}/file`, the Komga-shape
/// alias), PSE page streaming (`/opds/pse/*`), and progress writes
/// (including the KOReader sync shim) — 120/min/IP + burst 120. A PSE
/// reader fetches one request per page turn plus prefetch, so this needs
/// more headroom than the catalog bucket; it is independent of it (H-3.2).
pub const OPDS_STREAM: Bucket = Bucket {
    name: "opds_stream",
    period: Duration::from_millis(500),
    burst: 120,
};

/// `POST /me/issues/{id}/ocr` — 60/min/IP + burst 60. OCR is CPU-heavy
/// (text-detection-1.0 plan, M4): each request can run the page
/// detector and a recognition (Western line reader or manga-ocr). The Redis-backed
/// cache covers retries on the same region for free, so the bucket
/// only needs to bound *novel* requests from a single IP. 60/min
/// matches the old shared OPDS bucket — generous enough for a reader exploring
/// every bubble on a page, tight enough to make a runaway script
/// noticeable.
pub const OCR: Bucket = Bucket {
    name: "ocr",
    period: Duration::from_secs(1),
    burst: 60,
};

/// `GET /me/issues/{id}/pages/{page}/text-regions` — 15/min/IP +
/// burst 10. Sized for the previous page detector, when a detect-cache
/// miss was the most expensive single operation the server exposed (tens
/// of seconds a page); a miss now costs a page decode and a short
/// inference. The limit was left as it is. Cache hits are a Redis round-trip,
/// but the bucket has to protect the miss path — page-flipping in
/// text mode could otherwise queue dozens of detector runs. The web
/// client fetches each page's regions at most once per session
/// (`staleTime: Infinity`), so real usage sits far under the limit.
pub const OCR_DETECT: Bucket = Bucket {
    name: "ocr.detect",
    period: Duration::from_secs(4),
    burst: 10,
};

/// `GET /me/export` — 6/min/IP + burst 6. The export walks every
/// user-owned table and hydrates identity in IN-batches; a single call
/// is cheap, but it is also the one endpoint whose response size scales
/// with a user's whole history, so a runaway retry loop should trip
/// early. Six per minute is far above any human "download my data"
/// cadence.
pub const USER_EXPORT: Bucket = Bucket {
    name: "user_export",
    period: Duration::from_secs(10),
    burst: 6,
};

/// `GET /me/markers/export` — same budget as [`USER_EXPORT`] (6/min/IP +
/// burst 6), in its own bucket so a notes download and a full account
/// export don't starve each other.
pub const NOTES_EXPORT: Bucket = Bucket {
    name: "notes_export",
    period: Duration::from_secs(10),
    burst: 6,
};

/// `GET /me/holdings.csv` — same budget as [`USER_EXPORT`] (6/min/IP +
/// burst 6), in its own bucket. The CSV streams the caller's whole
/// visible catalogue in keyset batches, so a retry loop should trip
/// early; a person downloads it a handful of times at most.
pub const HOLDINGS_EXPORT: Bucket = Bucket {
    name: "holdings_export",
    period: Duration::from_secs(10),
    burst: 6,
};

/// `DELETE /me` — 1/min/IP + burst 5. A person deletes their account once;
/// the burst covers a few mistyped passwords. Wrong passwords also count
/// against the failed-auth lockout ([`crate::auth::failed_auth`]), so a
/// stolen session can't use this endpoint as a password oracle.
pub const ACCOUNT_DELETE: Bucket = Bucket {
    name: "account_delete",
    period: Duration::from_secs(60),
    burst: 5,
};

/// Marker writes — `POST /me/markers`, `PATCH` / `DELETE
/// /me/markers/{id}`, `POST /me/markers/bulk-delete`, `POST
/// /me/markers/restore` (WP-5.3, audit SE-5; burst cut in WP-8.4).
/// 10/s/IP sustained with a burst of 60. Every UI action is now one
/// request: Undo of any delete, including a 500-marker bulk delete, is a
/// single `restore` call (it used to be one `POST` per marker, which is
/// what the old 600 burst covered). The busiest human pattern left is
/// key-repeat on the reader's `b` / `s` toggles — a create or delete per
/// page turn, a handful per second — and a burst of 60 absorbs six
/// seconds of that on top of the 10/s refill, plus the delete + Undo
/// round trips around it. A runaway script still trips it in seconds.
pub const MARKER_WRITE: Bucket = Bucket {
    name: "marker_write",
    period: Duration::from_millis(100),
    burst: 60,
};

/// Free-text search — per user (E3, audit N-A5), 10/s sustained with a
/// burst of 60, one independent instance per route family: `GET
/// /issues/search` + `GET /issues`, `GET /series`, `GET /people`, `GET
/// /me/markers/search`, and each entity index (`/characters`, `/teams`,
/// `/locations`, `/arcs`). The global search fans one debounced query out
/// to every family (200 ms debounce in the modal and top bar, 250 ms on
/// `/search`), so non-stop typing costs each family at most 5 requests a
/// second, half the refill. The burst covers a `/search` session that
/// pages a result grid and flips filters on top of that. `/series` and
/// `/issues` also back the library grids; infinite scroll asks for one
/// page per sentinel hit, far below 10/s.
pub const SEARCH: Bucket = Bucket {
    name: "search",
    period: Duration::from_millis(100),
    burst: 60,
};

/// Metadata edits — per user (E3), 1/s sustained with a burst of 30. One
/// instance per module: `PATCH /series/{slug}`; `PATCH
/// /series/{series_slug}/issues/{issue_slug}`, `PATCH
/// /me/issues/bulk-metadata` and the issue field-provenance clear / lock
/// (`DELETE`/`PUT …/field-provenance/{field}`) sharing one; the cover
/// picks (`PATCH …/issues/{issue_slug}/cover`, `POST`/`DELETE
/// …/cover/variant[/{variant_id}]`, `PATCH /series/{slug}/cover`); and
/// the external-id add / unlink on series and issues (E12). Every UI save
/// is one request: the issue drawer and the series editor send one PATCH
/// per save, and the bulk "Edit metadata" dialog sends the whole selection
/// (up to its 500-id cap) in a single PATCH. Locks, pin releases, cover
/// picks and id edits are one click each, with the control disabled while
/// the request is in flight. A person saving issue after issue manages a
/// few a minute; a writeback library turns each save into an archive
/// rewrite job, so a script stops at 60 a minute on each instance.
pub const METADATA_WRITE: Bucket = Bucket {
    name: "metadata_write",
    period: Duration::from_secs(1),
    burst: 30,
};

/// Collection writes — per user (E3), 5/s sustained with a burst of 60:
/// create, rename/update, delete, add entry, remove entry, bulk add and
/// bulk remove under `/me/collections`. The busiest human pattern is
/// toggling Want to Read (an add or remove entry) on card after card in a
/// grid, a couple per second; bulk adds from a selection are one request.
pub const COLLECTION_WRITE: Bucket = Bucket {
    name: "collection_write",
    period: Duration::from_millis(200),
    burst: 60,
};

/// Saved-view writes — per user (E12), the same 5/s sustained and burst of
/// 60 as [`COLLECTION_WRITE`]: `PATCH` and `DELETE /me/saved-views/{id}`,
/// which accept collection rows as well as filter views and CBL lists.
/// A sibling bucket rather than the collections instance itself, because
/// every `build_per_user()` is its own token store and the two modules
/// build their routers separately; its own telemetry name also tells the
/// denial counter which surface tripped. The web edits a view with one
/// PATCH per save (rename, filter change, description) and deletes with
/// one DELETE behind a confirm.
pub const SAVED_VIEW_WRITE: Bucket = Bucket {
    name: "saved_view_write",
    ..COLLECTION_WRITE
};

/// Saved-view previews — per user, 2/s sustained with a burst of 20:
/// `POST /me/saved-views/preview` and `POST /me/saved-views/preview-issues`
/// (one shared instance). Each preview runs an arbitrary filter for up to
/// the filter statement timeout (5 s), so this is the budget that keeps a
/// script from parking the pool on them. The web builder previews on an
/// explicit button click, not on a keystroke debounce, and disables the
/// button while a preview is in flight, so a person sends at most one or
/// two a second even clicking fast; the burst absorbs a quick
/// edit-preview-edit-preview session.
pub const SAVED_VIEW_PREVIEW: Bucket = Bucket {
    name: "saved_view_preview",
    period: Duration::from_millis(500),
    burst: 20,
};

/// Collection reorder — per user (E3), 4/s sustained with a burst of 30:
/// `POST /me/collections/{id}/entries/reorder`, its own bucket so a drag
/// session never starves the writes above. A drop sends one request with
/// the full order, and a person drops at most about once a second; each
/// request rewrites the position of every entry, so it is the heavier
/// write and gets the smaller budget.
pub const COLLECTION_REORDER: Bucket = Bucket {
    name: "collection_reorder",
    period: Duration::from_millis(250),
    burst: 30,
};

/// Page bytes — `GET /issues/{id}/pages/{n}` — per user (IP fallback for
/// app-password and anonymous requests), 250/s sustained with a burst of
/// 2,000. Sized from the web clients (A9, 2026-11):
///
/// - The reader warms 3 pages ahead and 2 behind, at most 4 in flight
///   (`web/lib/reader/use-prefetch.ts`); holding the next-page key turns
///   about 30 pages a second with about 2 new URLs per turn, so roughly
///   60/s, about 1,000 requests across a 500-page issue.
/// - The offline downloader keeps 3 fetches in flight with no pacing
///   (`web/lib/pwa/downloads.ts`), so its rate is 3 over the round trip
///   plus its cache and IndexedDB writes. Estimated, not measured: about
///   75–150 pages a second on a LAN, more for small pages. It waits out a
///   429 (`Retry-After`) and retries the same page, so outrunning the
///   bucket slows a download and never fails it.
///
/// 250/s is 4x the reader's hold-to-turn pace; the burst covers that whole
/// 500-page sweep twice. It caps a single token's request flood (error
/// paths, conditional requests), not bandwidth.
pub const PAGES: Bucket = Bucket {
    name: "pages",
    period: Duration::from_millis(4),
    burst: 2_000,
};

/// Thumbnails and stored covers — `GET /issues/{id}/pages/{n}/thumb` and
/// `GET /issues/{id}/covers/{cover_id}`, one shared store — per user (IP
/// fallback), 500/s sustained with a burst of 4,000. Sized from:
///
/// - The page editor loads every page's thumbnail at once with a fresh
///   URL per open: 500 requests for a 500-page issue, each time it opens.
/// - The reader page strip warms 25 thumbnails when it opens and more per
///   turn or strip scroll.
/// - Cover grids are virtualized and lazy, and grow 60 cards per list
///   page through the `search` bucket (10/s), so at most about 600 new
///   covers a second by construction, a few hundred in a fast fling.
///   Inside the installed app the service worker revalidates every paint,
///   so revisited covers count too (a 304 still spends a token).
/// - The offline downloader's strip-thumbnail phase (estimated about
///   300/s on a LAN), which waits out a 429 like its page phase.
///
/// The burst absorbs eight page-editor opens of a 500-page issue
/// back to back; the sustained rate sits above the grid's construction
/// limit. Covers and strip thumbnails retry a 429 after a short back-off.
pub const THUMBNAILS: Bucket = Bucket {
    name: "thumbnails",
    period: Duration::from_millis(2),
    burst: 4_000,
};

// ───────── error handler ─────────

fn handle_governor_error(bucket: &'static str, err: GovernorError) -> Response<Body> {
    match err {
        GovernorError::TooManyRequests { wait_time, headers } => {
            metrics::counter!("folio_rate_limit_denied_total", "bucket" => bucket).increment(1);
            tracing::info!(bucket, wait_time, "rate limit denied");
            // tower_governor floors `wait_time` to integer seconds, which
            // can be 0 when the bucket trips mid-second. Floor to 1 so the
            // envelope + Retry-After header never tell the client to retry
            // instantly.
            let wait_time = wait_time.max(1);
            envelope_response(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                &format!("rate limit exceeded for {bucket}; retry in {wait_time}s",),
                wait_time,
                headers,
            )
        }
        GovernorError::UnableToExtractKey => {
            // `RequestContext` should always be present (set by the outer
            // `set_context` middleware). If it's missing, that's a wiring
            // bug — let the request through rather than failing closed,
            // since a malformed deployment shouldn't lock everyone out.
            tracing::warn!(bucket, "rate-limit key extractor failed; fail-open");
            envelope_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "rate limiter key extraction failed",
                0,
                None,
            )
        }
        GovernorError::Other { msg, code, headers } => {
            metrics::counter!("folio_rate_limit_denied_total", "bucket" => bucket).increment(1);
            envelope_response(
                code,
                "rate_limited",
                msg.as_deref().unwrap_or("rate limited"),
                0,
                headers,
            )
        }
    }
}

/// The 429 `rate_limited` envelope this middleware answers with, for a
/// limit checked inside a handler (the email-change request limit), so
/// both carry `retry_after_seconds` and `Retry-After` alike.
pub fn rate_limited_response(message: &str, retry_after_seconds: u64) -> Response<Body> {
    envelope_response(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        message,
        retry_after_seconds.max(1),
        None,
    )
}

fn envelope_response(
    status: StatusCode,
    code: &str,
    message: &str,
    retry_after_seconds: u64,
    extra_headers: Option<HeaderMap>,
) -> Response<Body> {
    let body = serde_json::json!({
        "error": {
            "code": code,
            "message": message,
            "retry_after_seconds": retry_after_seconds,
        }
    })
    .to_string();
    let mut resp = Response::new(Body::from(body));
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if retry_after_seconds > 0
        && let Ok(v) = HeaderValue::from_str(&retry_after_seconds.to_string())
    {
        resp.headers_mut().insert(header::RETRY_AFTER, v);
    }
    if let Some(extra) = extra_headers {
        for (k, v) in extra.into_iter().flat_map(|(k, v)| k.map(|k| (k, v))) {
            // Avoid clobbering Content-Type we just set; governor's headers
            // are X-RateLimit-After + Retry-After only. Its Retry-After is
            // the unfloored wait, `0` whenever the next token is under a
            // second away (every bucket refilling faster than 1/s), so it
            // must not replace the floored value set above.
            if k == header::CONTENT_TYPE || (k == header::RETRY_AFTER && retry_after_seconds > 0) {
                continue;
            }
            resp.headers_mut().insert(k, v);
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use governor::Quota;
    use std::num::NonZeroU32;

    type TestLimiter = RateLimiter<u32, DefaultKeyedStateStore<u32>, DefaultClock>;

    fn limiter(period: Duration) -> Arc<TestLimiter> {
        let quota = Quota::with_period(period)
            .expect("non-zero period")
            .allow_burst(NonZeroU32::MIN);
        Arc::new(RateLimiter::keyed(quota))
    }

    #[test]
    fn prune_drops_stale_keys_and_keeps_live_ones() {
        let registry = LimiterRegistry::default();
        let lim = limiter(Duration::from_millis(100));
        registry.register("test", lim.clone());

        assert!(lim.check_key(&1).is_ok());
        // Key 1's bucket refills after one period; wait well past it.
        std::thread::sleep(Duration::from_millis(400));
        assert!(lim.check_key(&2).is_ok());
        assert_eq!(lim.len(), 2);

        let report = registry.prune();
        assert_eq!(
            report,
            PruneReport {
                limiters: 1,
                keys: 1
            }
        );
        assert_eq!(lim.len(), 1);
        // Key 2 kept its spent token: pruning never refills a live bucket.
        assert!(lim.check_key(&2).is_err());
    }

    /// The page and thumbnail buckets at their real constants: one user
    /// spending the whole burst is refused, and another user keeps a full
    /// budget (`UserKey` names the user of a verified token; the HTTP side
    /// of that is `tests/rate_limits_per_user.rs`, and the routes'
    /// anonymous floods are `tests/page_rate_limits.rs`).
    #[test]
    fn page_and_thumbnail_budgets_are_per_user() {
        for bucket in [&PAGES, &THUMBNAILS] {
            let config = bucket.config(UserKey);
            let limiter = config.limiter();
            let alice = UserOrIp::User(Uuid::now_v7());
            let bob = UserOrIp::User(Uuid::now_v7());
            let address = UserOrIp::Ip(IpAddr::from([192, 0, 2, 7]));
            // Spend until refused: at least the burst gets through, and no
            // more than the burst plus what refilled while the loop ran.
            let started = std::time::Instant::now();
            let mut allowed: u32 = 0;
            while limiter.check_key(&alice).is_ok() {
                allowed += 1;
                assert!(
                    allowed <= 10 * bucket.burst,
                    "{} never refuses",
                    bucket.name
                );
            }
            let refilled = started.elapsed().as_nanos() / bucket.period.as_nanos() + 1;
            assert!(allowed >= bucket.burst, "{}", bucket.name);
            assert!(
                u128::from(allowed) <= u128::from(bucket.burst) + refilled,
                "{}: {allowed} allowed",
                bucket.name
            );
            assert!(limiter.check_key(&alice).is_err() || started.elapsed() > bucket.period);
            for _ in 0..bucket.burst {
                assert!(limiter.check_key(&bob).is_ok(), "{}", bucket.name);
            }
            assert!(limiter.check_key(&address).is_ok(), "{}", bucket.name);
        }
    }

    /// A bucket built through the public `Bucket::build` lands in the
    /// process-wide registry the sweep walks: the keys its route sees are
    /// counted and pruned there, and dropping the router drops the store.
    #[tokio::test]
    async fn built_buckets_are_swept_by_the_pruner() {
        use axum::routing::get;
        use tower::ServiceExt;

        const PROBE: Bucket = Bucket {
            name: "registry-probe",
            period: Duration::from_millis(100),
            burst: 1,
        };
        let router = axum::Router::new()
            .route("/", get(|| async { "ok" }))
            .route_layer(PROBE.build());
        assert_eq!(LIMITERS.key_counts(PROBE.name), vec![0]);

        let request = Request::builder()
            .uri("/")
            .extension(RequestContext {
                client_ip: Some(IpAddr::from([192, 0, 2, 9])),
                peer_trusted: false,
                user_agent: None,
            })
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(request).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(LIMITERS.key_counts(PROBE.name), vec![1]);

        // The key's bucket refills after one period; the sweep drops it.
        tokio::time::sleep(Duration::from_millis(400)).await;
        LIMITERS.prune();
        assert_eq!(LIMITERS.key_counts(PROBE.name), vec![0]);

        drop(router);
        assert!(LIMITERS.key_counts(PROBE.name).is_empty());
    }

    #[test]
    fn a_dropped_limiter_is_forgotten_not_kept_alive() {
        let registry = LimiterRegistry::default();
        let kept = limiter(Duration::from_secs(1));
        let dropped = limiter(Duration::from_secs(1));
        let weak = Arc::downgrade(&dropped);
        registry.register("kept", kept.clone());
        registry.register("dropped", dropped);

        // The registry holds no strong reference: the store is freed as soon
        // as its owner (the router, here the test) lets go.
        assert_eq!(weak.strong_count(), 0);
        assert_eq!(registry.prune().limiters, 1);
        assert_eq!(
            registry
                .limiters
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len(),
            1
        );
    }
}
