//! Application passwords — long-lived Bearer tokens for OPDS readers,
//! scripts, and other API consumers that can't go through the cookie
//! flow (M7, audit M-14).
//!
//! Plaintext format: `app_<base32-no-padding(32 bytes)>` — 64 ASCII chars
//! total, 32 bytes of entropy. The `app_` prefix lets the extractor
//! distinguish app passwords from JWT access tokens without a DB hit
//! (JWTs start with `eyJ`).
//!
//! Storage: argon2id under the same pepper as user passwords. Bearer
//! auth scans the user's active rows and tries each — cardinality is
//! tiny (≤ handful per user) so the per-request cost is bounded.
//!
//! Soft-delete only — `revoked_at` flips the row to inactive without
//! losing audit history.

use chrono::Utc;
use lru::LruCache;
use rand::Rng;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use sha2::{Digest, Sha256};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use entity::app_password::{self, ActiveModel as AppPasswordAM, Entity as AppPasswordEntity};

use super::password;

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

/// Create a new app-password for `user_id` with the given `scope`.
/// Returns the new row's id and the plaintext — the plaintext must be
/// displayed to the user immediately and is never retrievable again.
pub async fn issue(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
    label: &str,
    scope: &str,
    pepper: &[u8],
) -> anyhow::Result<(Uuid, String)> {
    if !is_valid_scope(scope) {
        anyhow::bail!("invalid scope {scope:?}");
    }
    let plaintext = random_plaintext();
    let hash = password::hash(&plaintext, pepper)?;
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
    };
    am.insert(db).await?;
    Ok((id, plaintext))
}

/// Resolved app-password — the parts the extractor needs after a
/// successful Bearer/Basic match. The row id is informational (logs);
/// `scope` drives `RequireScope` enforcement on progress-write paths.
pub struct ResolvedAppPassword {
    pub user_id: Uuid,
    pub app_password_id: Uuid,
    pub scope: String,
}

/// Don't re-bump `last_used_at` more than once per this interval — every
/// authenticated OPDS request would otherwise be a row UPDATE (PERF-1).
const LAST_USED_DEBOUNCE_SECS: i64 = 60;

/// Process-wide cache mapping `SHA-256(plaintext)` → the app-password row id,
/// so a repeat Bearer/Basic auth skips the argon2 scan over every active token
/// (PERF-1). argon2id verification is ~10-30 ms on x86 and 100-300 ms on a Pi,
/// and the old path ran it once per active token per request on the async
/// executor — the dominant OPDS hot-path cost.
///
/// Keyed by a hash of the token, never the plaintext, so secrets don't sit in a
/// long-lived map. Entries are only ever written after a successful argon2
/// match, so the cache can't be poisoned. Revocation stays immediate: a cache
/// hit still re-reads the row by id and honors `revoked_at` + current `scope`.
#[derive(Clone)]
pub struct AppPasswordCache(Arc<Mutex<LruCache<[u8; 32], Uuid>>>);

impl AppPasswordCache {
    /// Bounded so a token-spray (many distinct bad tokens never reach the
    /// cache anyway, but valid-then-revoked churn) can't grow it without limit.
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
}

impl Default for AppPasswordCache {
    fn default() -> Self {
        Self::new()
    }
}

fn token_hash(plaintext: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(plaintext.as_bytes());
    h.finalize().into()
}

/// Resolve a plaintext app-password to its owning user + scope.
/// Returns `None` for malformed prefix, missing user, or no matching
/// active row. Updates `last_used_at` on success (debounced).
///
/// Fast path: a previously-verified token hits the `cache` and skips argon2 —
/// but still re-reads its row so revocation and scope stay authoritative. Slow
/// path: scan active rows and argon2 each, on a blocking thread so the per-row
/// hashing never stalls the async runtime.
pub async fn verify(
    db: &DatabaseConnection,
    cache: &AppPasswordCache,
    plaintext: &str,
    pepper: &[u8],
) -> Option<ResolvedAppPassword> {
    if !looks_like_app_password(plaintext) {
        return None;
    }
    let key = token_hash(plaintext);

    // Fast path: we've argon2-verified this exact token before. Re-read the row
    // by id so `revoked_at` / `scope` stay authoritative — only the argon2 work
    // is skipped, not the revocation check.
    if let Some(id) = cache.get(&key) {
        match AppPasswordEntity::find_by_id(id).one(db).await {
            Ok(Some(row)) if row.revoked_at.is_none() => {
                let resolved = ResolvedAppPassword {
                    user_id: row.user_id,
                    app_password_id: row.id,
                    scope: row.scope.clone(),
                };
                touch_last_used(db, row).await;
                return Some(resolved);
            }
            // Revoked or deleted — drop the stale entry. (A revoked token would
            // also fail the scan below, so return None rather than re-scan.)
            Ok(_) => {
                cache.invalidate(&key);
                return None;
            }
            // Transient DB error — leave the entry and fail this request.
            Err(_) => return None,
        }
    }

    // Slow path: scan active rows. The partial index keeps this O(active-tokens)
    // rather than O(all-tokens); argon2 runs on a blocking thread.
    let rows = AppPasswordEntity::find()
        .filter(app_password::Column::RevokedAt.is_null())
        .all(db)
        .await
        .ok()?;
    let candidates: Vec<(Uuid, String)> = rows.iter().map(|r| (r.id, r.hash.clone())).collect();
    let plaintext_owned = plaintext.to_owned();
    let pepper_owned = pepper.to_vec();
    let matched_id = tokio::task::spawn_blocking(move || {
        candidates.into_iter().find_map(|(id, hash)| {
            match password::verify(&hash, &plaintext_owned, &pepper_owned) {
                Ok(true) => Some(id),
                _ => None,
            }
        })
    })
    .await
    .ok()??;

    let row = rows.into_iter().find(|r| r.id == matched_id)?;
    let resolved = ResolvedAppPassword {
        user_id: row.user_id,
        app_password_id: row.id,
        scope: row.scope.clone(),
    };
    cache.put(key, row.id);
    touch_last_used(db, row).await;
    Some(resolved)
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
}
