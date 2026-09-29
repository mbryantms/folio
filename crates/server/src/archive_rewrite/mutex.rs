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
//! race — the loser re-queues itself with backoff (both jobs) or surfaces
//! a per-issue skip reason (series fan-out).
//!
//! ## TTL + heartbeat (WP-2.6 (h), audit DI-13)
//!
//! Sidecar writes claim a 120s TTL (zip rewrite is fast). Page edits claim
//! 180s (re-encoding pages can blow past 120s on large archives). Each
//! consumer picks the TTL when claiming. The TTL is only the *crash*
//! safety net, not the expected hold time: a slow rewrite — a 2 GB
//! omnibus on a NAS, a CBR→CBZ conversion that has to decompress every
//! page — can legitimately outlive it, and pre-fix the key simply expired
//! mid-rewrite, letting a page edit claim the lock and race the
//! in-flight swap. Every holder now runs a [`Heartbeat`] that re-arms the
//! TTL every [`HEARTBEAT_INTERVAL_SECS`] (compare-and-`EXPIRE` on the
//! claim token, so a lost lock is never resurrected) for as long as the
//! blocking rewrite runs; dropping the guard stops it. If the process
//! dies, the heartbeat dies with it and the TTL reaps the key as before.

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

/// Compare-and-expire Lua: only re-arm the TTL when the key still holds our
/// token. Returns 1 when extended, 0 when the lock is no longer ours (it
/// expired and was re-claimed, or was released).
const EXTEND_CAS: &str = "if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('expire', KEYS[1], ARGV[2]) else return 0 end";

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

/// Re-arm the TTL on a lock we hold. `Ok(true)` when the key still carried
/// `token` and now has `ttl_secs` to live again; `Ok(false)` when the lock
/// is no longer ours (expired + re-claimed, or released) — the caller
/// should log loudly, since its in-flight rewrite is now unprotected.
pub async fn extend(
    redis: &mut ConnectionManager,
    issue_id: &str,
    token: &str,
    ttl_secs: u64,
) -> Result<bool, redis::RedisError> {
    let n: i64 = redis::cmd("EVAL")
        .arg(EXTEND_CAS)
        .arg(1)
        .arg(mutex_key(issue_id))
        .arg(token)
        .arg(ttl_secs)
        .query_async(redis)
        .await?;
    Ok(n == 1)
}

/// Default TTLs by consumer. Picked to match each consumer's typical
/// worst-case duration with a comfortable safety margin — and, since the
/// [`Heartbeat`] re-arms them, they now only bound how long a *crashed*
/// holder keeps the key.
pub const SIDECAR_TTL_SECS: u64 = 120;
pub const EDIT_TTL_SECS: u64 = 180;

/// How often a live holder re-arms its TTL. A quarter of the shortest TTL,
/// so three consecutive missed beats (Redis hiccup, a worker starved of
/// the runtime) still leave the lock held.
pub const HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// RAII guard that keeps a claimed lock alive while a long rewrite runs.
/// Spawns a tokio task that calls [`extend`] every
/// [`HEARTBEAT_INTERVAL_SECS`]; dropping the guard aborts the task. Hold it
/// across the `spawn_blocking` rewrite and drop it before [`release`].
pub struct Heartbeat {
    task: tokio::task::JoinHandle<()>,
}

impl Heartbeat {
    pub fn start(
        mut redis: ConnectionManager,
        issue_id: String,
        token: String,
        ttl_secs: u64,
    ) -> Self {
        let task = tokio::spawn(async move {
            let every = std::time::Duration::from_secs(HEARTBEAT_INTERVAL_SECS);
            loop {
                tokio::time::sleep(every).await;
                match extend(&mut redis, &issue_id, &token, ttl_secs).await {
                    Ok(true) => {
                        tracing::debug!(issue_id = %issue_id, ttl_secs, "archive rewrite lock: heartbeat extended");
                    }
                    Ok(false) => {
                        // Nothing to re-arm: another holder owns the key (or
                        // it was released under us). Keep looping — the
                        // rewrite is already in flight and stopping the
                        // beat wouldn't make it safer — but say so.
                        tracing::warn!(
                            issue_id = %issue_id,
                            "archive rewrite lock: heartbeat found the lock no longer ours; in-flight rewrite is unprotected",
                        );
                    }
                    Err(e) => {
                        tracing::warn!(issue_id = %issue_id, error = %e, "archive rewrite lock: heartbeat extend failed");
                    }
                }
            }
        });
        Self { task }
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.task.abort();
    }
}
