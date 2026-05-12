//! Pruner that deletes long-expired `auth_sessions` rows (M3, audit S-11).
//!
//! Refresh tokens have a 30-day default TTL; once `expires_at` is in the past
//! the row is dead — the `auth/local.rs::refresh` handler rejects it, and
//! the unique index on `refresh_token_hash` prevents reuse. Keeping the
//! rows forever bloats the table without buying anything. We keep a 7-day
//! grace after expiry for the off-chance someone wants to grep recent
//! auth activity by IP, then drop them.
//!
//! This is purely table-hygiene — it doesn't affect security guarantees
//! either way.

use entity::auth_session;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use std::time::Duration as StdDuration;

/// How long after `expires_at` to keep a row around before deletion. The
/// session is already invalid by then (refresh rejects past-expiry rows),
/// but a short grace window keeps recent-history queries useful.
pub const GRACE_AFTER_EXPIRY: StdDuration = StdDuration::from_secs(7 * 24 * 60 * 60);

/// Delete any `auth_sessions` whose `expires_at` is older than
/// `now - GRACE_AFTER_EXPIRY`. Returns the number of rows deleted.
pub async fn run(db: &DatabaseConnection) -> Result<u64, DbErr> {
    let cutoff = chrono::Utc::now().fixed_offset()
        - chrono::Duration::from_std(GRACE_AFTER_EXPIRY).expect("grace fits in chrono");
    let res = auth_session::Entity::delete_many()
        .filter(auth_session::Column::ExpiresAt.lt(cutoff))
        .exec(db)
        .await?;
    Ok(res.rows_affected)
}
