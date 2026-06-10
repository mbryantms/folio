//! Per-issue Redis mutex for archive rewrites.
//!
//! Pattern cloned from
//! [`metadata_apply.rs:60-90`](crate::jobs::metadata_apply) — SET NX EX with a
//! unique per-claim token, released via a compare-and-delete on that token so
//! an overrun hold can't delete a lock another worker has since re-claimed
//! (SEC-7). The TTL guards against worker crashes leaving the key stuck;
//! mid-rewrite a stale lock just means the next attempt waits a couple of
//! minutes before claiming.
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

use redis::aio::ConnectionManager;
use uuid::Uuid;

const KEY_PREFIX: &str = "archive:rewrite:";

fn mutex_key(issue_id: &str) -> String {
    format!("{KEY_PREFIX}{issue_id}")
}

/// Try to claim the rewrite mutex for `issue_id`. Returns `Ok(Some(token))`
/// when the lock was acquired — the caller must pass that token back to
/// [`release`] — or `Ok(None)` when another worker holds it (caller's choice:
/// re-queue, return 409, etc.). Errors propagate Redis failures so the caller
/// can soft-fail / log.
pub async fn try_claim(
    redis: &mut ConnectionManager,
    issue_id: &str,
    ttl_secs: u64,
) -> Result<Option<String>, redis::RedisError> {
    let token = Uuid::now_v7().to_string();
    let set: Option<String> = redis::cmd("SET")
        .arg(mutex_key(issue_id))
        .arg(&token)
        .arg("NX")
        .arg("EX")
        .arg(ttl_secs)
        .query_async(redis)
        .await?;
    Ok(set.map(|_| token))
}

/// Compare-and-delete Lua: only remove the key when it still holds our token.
const RELEASE_CAS: &str = "if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('del', KEYS[1]) else return 0 end";

/// Release the rewrite mutex for `issue_id`, but only if it still holds the
/// `token` returned by [`try_claim`]. If our hold overran its TTL and another
/// worker re-claimed the lock, the stored token differs and we delete nothing —
/// so we can't tear down a lock we no longer own (SEC-7). Best-effort: Redis
/// errors are swallowed because TTL expiration is the safety net. Always call
/// in a `release(.., &token).await` pattern at the tail of the job.
pub async fn release(redis: &mut ConnectionManager, issue_id: &str, token: &str) {
    let _: Result<i64, _> = redis::cmd("EVAL")
        .arg(RELEASE_CAS)
        .arg(1)
        .arg(mutex_key(issue_id))
        .arg(token)
        .query_async(redis)
        .await;
}

/// Default TTLs by consumer. Picked to match each consumer's typical
/// worst-case duration with a comfortable safety margin.
pub const SIDECAR_TTL_SECS: u64 = 120;
pub const EDIT_TTL_SECS: u64 = 180;
