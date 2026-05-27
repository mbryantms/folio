//! Per-issue Redis mutex for archive rewrites.
//!
//! Pattern cloned from
//! [`metadata_apply.rs:60-90`](crate::jobs::metadata_apply) — SET NX EX with
//! a value that no caller cares about. The TTL guards against worker
//! crashes leaving the key stuck; mid-rewrite a stale lock just means the
//! next attempt waits a couple of minutes before claiming.
//!
//! Two consumers serialize against each other via this lock:
//!   - Sidecar writeback (`metadata-sidecar-writeback-1.0` M3+).
//!   - Page-byte edits (`archive-rewrite-1.0` M2+).
//!
//! So a page edit and a sidecar refresh on the same issue can never
//! race — second loser silently re-queues or surfaces a 409.
//!
//! Sidecar writes target a 120s TTL (zip rewrite is fast). Page edits
//! target 180s (re-encoding pages can blow past 120s on large archives).
//! Each consumer picks the TTL when claiming.

use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use uuid::Uuid;

const KEY_PREFIX: &str = "archive:rewrite:";

fn mutex_key(issue_id: &str) -> String {
    format!("{KEY_PREFIX}{issue_id}")
}

/// Try to claim the rewrite mutex for `issue_id`. Returns `Ok(true)` when
/// the lock was acquired; `Ok(false)` when another worker holds it
/// (caller's choice: re-queue, return 409, etc.). Errors propagate Redis
/// failures so the caller can soft-fail / log.
pub async fn try_claim(
    redis: &mut ConnectionManager,
    issue_id: &str,
    ttl_secs: u64,
) -> Result<bool, redis::RedisError> {
    let set: Option<String> = redis::cmd("SET")
        .arg(mutex_key(issue_id))
        .arg(Uuid::now_v7().to_string())
        .arg("NX")
        .arg("EX")
        .arg(ttl_secs)
        .query_async(redis)
        .await?;
    Ok(set.is_some())
}

/// Release the rewrite mutex for `issue_id`. Best-effort — Redis errors
/// are swallowed because TTL expiration is the safety net. Always call
/// in a `_ = release(...).await` pattern at the tail of the job.
pub async fn release(redis: &mut ConnectionManager, issue_id: &str) {
    let _: Result<(), _> = redis.del::<_, ()>(mutex_key(issue_id)).await;
}

/// Default TTLs by consumer. Picked to match each consumer's typical
/// worst-case duration with a comfortable safety margin.
pub const SIDECAR_TTL_SECS: u64 = 120;
pub const EDIT_TTL_SECS: u64 = 180;
