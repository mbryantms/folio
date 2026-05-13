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
use rand::Rng;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
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
    rand::thread_rng().fill(&mut bytes);
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

/// Resolve a plaintext app-password to its owning user + scope.
/// Returns `None` for malformed prefix, missing user, or no matching
/// active row. Updates `last_used_at` on success.
pub async fn verify(
    db: &sea_orm::DatabaseConnection,
    plaintext: &str,
    pepper: &[u8],
) -> Option<ResolvedAppPassword> {
    if !looks_like_app_password(plaintext) {
        return None;
    }
    // Scan only active rows. The partial index from the migration keeps
    // this O(active-tokens) rather than O(all-tokens). In practice
    // active-tokens ≤ a dozen per active user.
    let rows = AppPasswordEntity::find()
        .filter(app_password::Column::RevokedAt.is_null())
        .all(db)
        .await
        .ok()?;
    for row in rows {
        if let Ok(true) = password::verify(&row.hash, plaintext, pepper) {
            // Best-effort timestamp bump — failure here is non-fatal
            // (lock contention, etc).
            let now = Utc::now().fixed_offset();
            let user_id = row.user_id;
            let app_password_id = row.id;
            let scope = row.scope.clone();
            let mut am: AppPasswordAM = row.into();
            am.last_used_at = Set(Some(now));
            let _ = am.update(db).await;
            return Some(ResolvedAppPassword {
                user_id,
                app_password_id,
                scope,
            });
        }
    }
    None
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
