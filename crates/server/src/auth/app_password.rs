//! Application passwords — long-lived Bearer tokens for OPDS readers,
//! scripts, and other API consumers that can't go through the cookie
//! flow (M7, audit M-14).
//!
//! Plaintext format: `app_<base32-no-padding(32 bytes)>` — 56 ASCII chars
//! total, 32 bytes of entropy. The `app_` prefix lets the extractor
//! distinguish app passwords from JWT access tokens without a DB hit
//! (JWTs start with `eyJ`).
//!
//! Storage: argon2id under the same pepper as user passwords, plus a
//! `lookup_digest` (SHA-256 of the plaintext, [`lookup_digest`]) that
//! indexes a token to its one row.
//!
//! Verification ([`verify`]), in order:
//! 1. **Cache**: a token this process already verified skips argon2 but
//!    re-reads its row, so revocation and scope stay authoritative.
//! 2. **Lockout**: the client IP's failed-auth lockout
//!    ([`super::failed_auth`], shared with sign-in) is checked before any
//!    lookup or hashing; a locked-out IP is refused.
//! 3. **Digest**: one indexed lookup by digest, then one argon2 check of
//!    that row (defence in depth; it also drives the pepper rehash).
//! 4. **Legacy**: only when no row has the digest, the newest
//!    [`MAX_LEGACY_CANDIDATES`] active rows written before the digest
//!    column existed (`lookup_digest IS NULL`) are tried; a match stores
//!    its digest, so that token never takes this path again.
//!
//! A miss counts as a failed-auth attempt for the client IP. Every argon2
//! verification runs on the blocking pool behind a small global gate
//! ([`AppPasswordGate`], [`VERIFY_PERMITS`]); a request that can't get a
//! permit within [`VERIFY_PERMIT_WAIT`] is answered 503.
//!
//! Soft-delete only — `revoked_at` flips the row to inactive without
//! losing audit history.

use chrono::Utc;
use lru::LruCache;
use rand::Rng;
use sea_orm::sea_query::{Expr, NullOrdering};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, Order, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Set,
};
use sha2::{Digest, Sha256};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use entity::app_password::{self, ActiveModel as AppPasswordAM, Entity as AppPasswordEntity};

use super::password;
use crate::middleware::RequestContext;
use crate::state::AppState;

/// Token prefix used to distinguish app-password Bearer tokens from JWTs
/// at extractor time. A JWT can never start with this string because
/// `eyJ...` (base64 of `{"`) is the JWT marker.
pub const PREFIX: &str = "app_";

/// Read-only scope — the default. Browse, download, page-stream.
pub const SCOPE_READ: &str = "read";

/// Read + progress-write scope. Adds the ability to PUT to the OPDS
/// progress endpoint + KOReader sync shim.
pub const SCOPE_READ_PROGRESS: &str = "read+progress";

/// True iff `s` is one of the values we accept at issue/verify time.
pub fn is_valid_scope(s: &str) -> bool {
    matches!(s, SCOPE_READ | SCOPE_READ_PROGRESS)
}

/// Length of the random component (in bytes, pre-base32).
const SECRET_BYTES: usize = 32;

/// How many rows without a lookup digest one request may try. Bounds the
/// argon2 work of a request that matches no digest while such rows remain.
/// Candidates are the most recently used first, so tokens in active use
/// are verified (and given their digest) first; a legacy token that has
/// gone unused while more than this many newer legacy tokens exist is
/// never reached and has to be re-created.
pub const MAX_LEGACY_CANDIDATES: u64 = 20;

/// Global cap on concurrent app-password argon2 verifications.
pub const VERIFY_PERMITS: usize = 2;

/// How long a request waits for a verification permit before it is
/// answered 503 (overridable per gate for tests, [`AppPasswordGate::set_permit_wait`]).
pub const VERIFY_PERMIT_WAIT: Duration = Duration::from_secs(2);

/// `Retry-After` (seconds) on the 503 a request gets when no permit freed up.
pub const BUSY_RETRY_AFTER_SECS: u64 = 1;

/// Gauge: active app passwords that still have no lookup digest.
pub const LEGACY_GAUGE: &str = "folio_auth_app_password_legacy_remaining";

/// True if the token has the app-password shape.
pub fn looks_like_app_password(token: &str) -> bool {
    token.starts_with(PREFIX)
}

fn random_plaintext() -> String {
    let mut bytes = [0u8; SECRET_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    // Base32-without-padding keeps the token URL/header safe and avoids
    // the `/` and `+` characters of base64 that some OPDS clients
    // mangle when copy-pasting.
    let body = data_encoding::BASE32_NOPAD
        .encode(&bytes)
        .to_ascii_lowercase();
    format!("{PREFIX}{body}")
}

/// The token's lookup key: SHA-256 of the plaintext. Unkeyed on purpose.
/// The token is 256 random bits, so the digest can't be inverted or
/// guessed, and a digest that doesn't involve the pepper keeps working
/// across a pepper rotation (a pepper-keyed digest would orphan every row
/// written under the old pepper). The argon2 hash stays peppered.
pub fn lookup_digest(plaintext: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(plaintext.as_bytes());
    h.finalize().into()
}

/// Create a new app-password for `user_id` with the given `scope`.
/// Returns the new row's id and the plaintext — the plaintext must be
/// displayed to the user immediately and is never retrievable again.
///
/// The row stores the token's [`lookup_digest`]. The digest column is
/// unique, so a colliding digest (two tokens with the same SHA-256) fails
/// the insert instead of producing a second row that the lookup could
/// confuse with the first.
pub async fn issue(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
    label: &str,
    scope: &str,
    pepper: &[u8],
    cost: password::HashCost,
) -> anyhow::Result<(Uuid, String)> {
    if !is_valid_scope(scope) {
        anyhow::bail!("invalid scope {scope:?}");
    }
    let plaintext = random_plaintext();
    let hash = password::hash(&plaintext, pepper, cost)?;
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    let am = AppPasswordAM {
        id: Set(id),
        user_id: Set(user_id),
        label: Set(label.to_owned()),
        hash: Set(hash),
        last_used_at: Set(None),
        created_at: Set(now),
        revoked_at: Set(None),
        scope: Set(scope.to_owned()),
        lookup_digest: Set(Some(lookup_digest(&plaintext).to_vec())),
    };
    am.insert(db).await?;
    refresh_legacy_gauge(db).await;
    Ok((id, plaintext))
}

/// Count the active rows without a lookup digest and publish it as
/// [`LEGACY_GAUGE`]. Called at startup and when a row is created,
/// revoked or given its digest, never per request. Rows removed by a user
/// deletion are picked up at the next of those.
pub async fn refresh_legacy_gauge(db: &DatabaseConnection) {
    match AppPasswordEntity::find()
        .filter(app_password::Column::RevokedAt.is_null())
        .filter(app_password::Column::LookupDigest.is_null())
        .count(db)
        .await
    {
        Ok(n) => metrics::gauge!(LEGACY_GAUGE).set(n as f64),
        Err(e) => tracing::warn!(error = %e, "app password legacy count failed"),
    }
}

/// Resolved app-password — the parts the extractor needs after a
/// successful Bearer/Basic match. The row id is informational (logs);
/// `scope` drives `RequireScope` enforcement on progress-write paths.
pub struct ResolvedAppPassword {
    pub user_id: Uuid,
    pub app_password_id: Uuid,
    pub scope: String,
}

/// Why [`verify`] did not resolve a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyError {
    /// No active row matches (or the lookup failed).
    Invalid,
    /// The client IP is in failed-auth lockout; nothing was looked up.
    /// Carries the remaining lockout in seconds.
    LockedOut(u64),
    /// No verification permit freed up within the gate's wait.
    Busy,
}

/// Don't re-bump `last_used_at` more than once per this interval — every
/// authenticated OPDS request would otherwise be a row UPDATE (PERF-1).
const LAST_USED_DEBOUNCE_SECS: i64 = 60;

/// Process-wide cache mapping the token's [`lookup_digest`] to its row id,
/// so a repeat Bearer/Basic auth skips argon2 (PERF-1): argon2id is
/// ~10-60 ms per verification and a reader app sends a request per page.
///
/// Keyed by a hash of the token, never the plaintext, so secrets don't sit in a
/// long-lived map. Entries are only ever written after a successful argon2
/// match, so the cache can't be poisoned and holds no negative results.
/// Revocation stays immediate: a cache hit still re-reads the row by id and
/// honors `revoked_at` + current `scope`.
#[derive(Clone)]
pub struct AppPasswordCache(Arc<Mutex<LruCache<[u8; 32], Uuid>>>);

impl AppPasswordCache {
    /// Bounded so valid-then-revoked churn can't grow it without limit.
    /// 1024 live tokens is far beyond any self-host's real fleet.
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(LruCache::new(
            NonZeroUsize::new(1024).expect("nonzero"),
        ))))
    }

    fn get(&self, key: &[u8; 32]) -> Option<Uuid> {
        self.0
            .lock()
            .expect("app-password cache poisoned")
            .get(key)
            .copied()
    }

    fn put(&self, key: [u8; 32], id: Uuid) {
        self.0
            .lock()
            .expect("app-password cache poisoned")
            .put(key, id);
    }

    fn invalidate(&self, key: &[u8; 32]) {
        self.0.lock().expect("app-password cache poisoned").pop(key);
    }

    /// Forget every entry, so the next request of each token verifies again
    /// (test hook: exercises the lookup paths behind the cache).
    pub fn clear(&self) {
        self.0.lock().expect("app-password cache poisoned").clear();
    }
}

impl Default for AppPasswordCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Global gate around app-password argon2 verification: [`VERIFY_PERMITS`]
/// concurrent verifications, each on the blocking pool, and a bounded wait
/// for a permit. Also counts the verifications it ran.
#[derive(Clone)]
pub struct AppPasswordGate(Arc<GateInner>);

struct GateInner {
    permits: Arc<Semaphore>,
    wait_ms: AtomicU64,
    verifications: AtomicU64,
}

impl AppPasswordGate {
    pub fn new() -> Self {
        Self(Arc::new(GateInner {
            permits: Arc::new(Semaphore::new(VERIFY_PERMITS)),
            wait_ms: AtomicU64::new(VERIFY_PERMIT_WAIT.as_millis() as u64),
            verifications: AtomicU64::new(0),
        }))
    }

    /// How many argon2 verifications this gate has run (one per candidate
    /// row tried, whether or not it matched).
    pub fn verifications(&self) -> u64 {
        self.0.verifications.load(Ordering::Relaxed)
    }

    /// Change the permit wait. Tests shorten it; production keeps
    /// [`VERIFY_PERMIT_WAIT`].
    pub fn set_permit_wait(&self, wait: Duration) {
        self.0
            .wait_ms
            .store(wait.as_millis() as u64, Ordering::Relaxed);
    }

    /// Take every permit (test hook: makes the next verification wait out
    /// the permit wait and answer busy). Dropping the guard frees them.
    pub async fn hold_all_permits(&self) -> OwnedSemaphorePermit {
        self.0
            .permits
            .clone()
            .acquire_many_owned(VERIFY_PERMITS as u32)
            .await
            .expect("gate semaphore never closes")
    }

    /// Verify `plaintext` against one stored hash on the blocking pool,
    /// holding a permit. On a previous-pepper match also returns the
    /// plaintext rehashed under the current pepper. `Err(())` = no permit
    /// within the wait.
    async fn verify_one(
        &self,
        stored: String,
        plaintext: &str,
        peppers: password::Peppers<'_>,
        cost: password::HashCost,
    ) -> Result<(password::Verified, Option<String>), ()> {
        let wait = Duration::from_millis(self.0.wait_ms.load(Ordering::Relaxed));
        let permit = match tokio::time::timeout(wait, self.0.permits.clone().acquire_owned()).await
        {
            Ok(Ok(p)) => p,
            _ => return Err(()),
        };
        self.0.verifications.fetch_add(1, Ordering::Relaxed);
        let plaintext = zeroize::Zeroizing::new(plaintext.to_owned());
        let current = zeroize::Zeroizing::new(peppers.current.to_vec());
        let previous = peppers
            .previous
            .map(|p| zeroize::Zeroizing::new(p.to_vec()));
        let joined = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let peppers = password::Peppers {
                current: &current,
                previous: previous.as_ref().map(|p| p.as_slice()),
            };
            let verified = password::verify_rotating(&stored, &plaintext, peppers)
                .unwrap_or(password::Verified::No);
            // L-1 (WP-6.3): matched under the previous pepper → rehash under
            // the current one while we still hold the plaintext.
            let rehash = if verified == password::Verified::Previous {
                password::hash(&plaintext, peppers.current, cost).ok()
            } else {
                None
            };
            (verified, rehash)
        })
        .await;
        Ok(joined.unwrap_or((password::Verified::No, None)))
    }
}

impl Default for AppPasswordGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve a plaintext app-password to its owning user + scope (module docs
/// for the order of checks). Updates `last_used_at` on success (debounced).
/// `ctx` supplies the client IP for the failed-auth lockout.
pub async fn verify(
    app: &AppState,
    ctx: Option<&RequestContext>,
    plaintext: &str,
) -> Result<ResolvedAppPassword, VerifyError> {
    if !looks_like_app_password(plaintext) {
        return Err(VerifyError::Invalid);
    }
    let db = &app.db;
    let cache = &app.app_password_cache;
    let digest = lookup_digest(plaintext);

    // Fast path: we've argon2-verified this exact token before. Re-read the row
    // by id so `revoked_at` / `scope` stay authoritative — only the argon2 work
    // is skipped, not the revocation check.
    if let Some(id) = cache.get(&digest) {
        return match AppPasswordEntity::find_by_id(id).one(db).await {
            Ok(Some(row)) if row.revoked_at.is_none() => Ok(resolve(db, row).await),
            // Revoked or deleted — drop the stale entry.
            Ok(_) => {
                cache.invalidate(&digest);
                Err(VerifyError::Invalid)
            }
            // Transient DB error — leave the entry and fail this request.
            Err(_) => Err(VerifyError::Invalid),
        };
    }

    // Lockout before any lookup or hashing, the same IP axis sign-in uses.
    if let Some(ip) = ctx.and_then(|c| c.client_ip)
        && let Ok(Some(retry)) = super::failed_auth::check_lockout_for(app, ip).await
    {
        return Err(VerifyError::LockedOut(retry));
    }

    let peppers = app.secrets.peppers();
    let cost = app.cfg().password_hash_cost;
    let gate = &app.app_password_gate;

    // Digest path: one indexed lookup, one argon2 check.
    let by_digest = match AppPasswordEntity::find()
        .filter(app_password::Column::LookupDigest.eq(digest.to_vec()))
        .one(db)
        .await
    {
        Ok(row) => row,
        Err(e) => {
            tracing::warn!(error = %e, "app password digest lookup failed");
            return Err(VerifyError::Invalid);
        }
    };
    if let Some(row) = by_digest {
        // A revoked token is a real token, not a guess: refuse it without
        // hashing and without counting it against the IP.
        if row.revoked_at.is_some() {
            return Err(VerifyError::Invalid);
        }
        let (verified, rehash) = gate
            .verify_one(row.hash.clone(), plaintext, peppers, cost)
            .await
            .map_err(|()| VerifyError::Busy)?;
        if !verified.ok() {
            record_miss(app, ctx).await;
            return Err(VerifyError::Invalid);
        }
        store_rehash(db, row.id, rehash).await;
        cache.put(digest, row.id);
        return Ok(resolve(db, row).await);
    }

    // Legacy path: rows written before the digest column, newest use first,
    // at most MAX_LEGACY_CANDIDATES of them.
    let legacy = match AppPasswordEntity::find()
        .filter(app_password::Column::RevokedAt.is_null())
        .filter(app_password::Column::LookupDigest.is_null())
        .order_by_with_nulls(
            app_password::Column::LastUsedAt,
            Order::Desc,
            NullOrdering::Last,
        )
        .order_by_desc(app_password::Column::CreatedAt)
        .limit(MAX_LEGACY_CANDIDATES)
        .all(db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "app password legacy lookup failed");
            return Err(VerifyError::Invalid);
        }
    };
    for row in legacy {
        let (verified, rehash) = gate
            .verify_one(row.hash.clone(), plaintext, peppers, cost)
            .await
            .map_err(|()| VerifyError::Busy)?;
        if !verified.ok() {
            continue;
        }
        store_rehash(db, row.id, rehash).await;
        backfill_digest(db, row.id, &digest).await;
        cache.put(digest, row.id);
        return Ok(resolve(db, row).await);
    }

    record_miss(app, ctx).await;
    Err(VerifyError::Invalid)
}

/// A presented token matched nothing: count it for the client IP (no-op
/// when the failed-auth counters are switched off).
async fn record_miss(app: &AppState, ctx: Option<&RequestContext>) {
    if let Some(ctx) = ctx {
        super::failed_auth::record_failure_for(app, ctx).await;
    }
}

async fn resolve(db: &DatabaseConnection, row: app_password::Model) -> ResolvedAppPassword {
    let resolved = ResolvedAppPassword {
        user_id: row.user_id,
        app_password_id: row.id,
        scope: row.scope.clone(),
    };
    touch_last_used(db, row).await;
    resolved
}

/// Persist a previous-pepper rehash (pepper rotation, L-1). Best-effort.
async fn store_rehash(db: &DatabaseConnection, id: Uuid, rehash: Option<String>) {
    let Some(new_hash) = rehash else { return };
    let am = AppPasswordAM {
        id: Set(id),
        hash: Set(new_hash),
        ..Default::default()
    };
    match am.update(db).await {
        Ok(_) => {
            metrics::counter!("folio_auth_pepper_rehash_total", "kind" => "app_password")
                .increment(1);
            tracing::info!(app_password_id = %id, "app password rehashed onto the current pepper");
        }
        Err(e) => {
            tracing::warn!(error = %e, app_password_id = %id, "app password pepper rehash failed");
        }
    }
}

/// Give a legacy row its lookup digest after its token verified, so the
/// next request takes the digest path. Only fills a NULL, so two
/// concurrent first uses write the same value once. Best-effort: a
/// failure only means the next request scans again.
async fn backfill_digest(db: &DatabaseConnection, id: Uuid, digest: &[u8; 32]) {
    let res = AppPasswordEntity::update_many()
        .col_expr(
            app_password::Column::LookupDigest,
            Expr::value(digest.to_vec()),
        )
        .filter(app_password::Column::Id.eq(id))
        .filter(app_password::Column::LookupDigest.is_null())
        .exec(db)
        .await;
    match res {
        Ok(_) => {
            tracing::info!(app_password_id = %id, "app password given its lookup digest");
            refresh_legacy_gauge(db).await;
        }
        Err(e) => {
            tracing::error!(error = %e, app_password_id = %id, "app password digest backfill failed");
        }
    }
}

/// Best-effort `last_used_at` bump, debounced to at most once per
/// [`LAST_USED_DEBOUNCE_SECS`] so a busy reader doesn't UPDATE the row on every
/// request. Failure here is non-fatal (lock contention, etc).
async fn touch_last_used(db: &DatabaseConnection, row: app_password::Model) {
    let now = Utc::now();
    let fresh = row
        .last_used_at
        .map(|t| (now - t.with_timezone(&Utc)).num_seconds() < LAST_USED_DEBOUNCE_SECS)
        .unwrap_or(false);
    if fresh {
        return;
    }
    let mut am: AppPasswordAM = row.into();
    am.last_used_at = Set(Some(now.fixed_offset()));
    let _ = am.update(db).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_has_prefix_and_length() {
        let t = random_plaintext();
        assert!(t.starts_with(PREFIX));
        assert!(looks_like_app_password(&t));
        // Lowercase base32-nopad of 32 bytes is 52 chars; plus 4-char prefix.
        assert_eq!(t.len(), PREFIX.len() + 52);
        assert!(t.is_ascii());
    }

    #[test]
    fn random_plaintexts_are_distinct() {
        let a = random_plaintext();
        let b = random_plaintext();
        assert_ne!(a, b, "32 bytes of entropy should never collide");
    }

    #[test]
    fn lookup_digest_is_sha256_of_the_token() {
        let t = random_plaintext();
        let d = lookup_digest(&t);
        assert_eq!(d, lookup_digest(&t));
        assert_ne!(d, lookup_digest(&random_plaintext()));
        let expected: [u8; 32] = Sha256::digest(t.as_bytes()).into();
        assert_eq!(d, expected);
    }
}
