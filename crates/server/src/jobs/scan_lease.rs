//! The per-library scan coalescing key as a lease (R-03).
//!
//! `scan:in_flight:<library_id>` says "a full-library scan of this library is
//! queued or running". Its value is the holder's token, the scan run id. Every
//! change to it is one atomic Redis command or Lua script:
//!
//! - **Claim** ([`CLAIM_OR_QUEUE_LUA`], from [`JobRuntime::coalesce_scan`]):
//!   `SET key token NX PX ttl`. When the key is already held the same script
//!   marks the queued follow-up instead, so a concurrent release can never
//!   slip between "the key is held" and "the follow-up is recorded".
//! - **Keep alive** ([`EXTEND_LUA`], from [`ScanLease`]): while the scan job
//!   runs, a task beside it re-sets the expiry every
//!   [`SCAN_LEASE_REFRESH`], but only while the key still holds this scan's
//!   token. A key that has expired and is free is taken back with the same
//!   token. A key that holds another token means another scan owns the
//!   library now: the lease reports itself lost and the scanner stops through
//!   its cooperative-cancel path.
//! - **Release** ([`RELEASE_LUA`], from [`JobRuntime::release_scan`]):
//!   compare-and-delete, or compare-and-hand-over to the queued follow-up.
//!   A scan that no longer holds the key leaves it alone.
//!
//! Lifetimes: a claimed-but-queued scan has no running code to refresh it, so
//! the key carries [`SCAN_QUEUED_TTL`] until the job starts (the apalis job
//! itself survives a restart, and the boot sweep clears the key). Once the job
//! starts, the key carries [`SCAN_LEASE_TTL`], so a process that dies mid-scan
//! frees the library within that time.
//!
//! [`JobRuntime::coalesce_scan`]: crate::jobs::JobRuntime::coalesce_scan
//! [`JobRuntime::release_scan`]: crate::jobs::JobRuntime::release_scan

use redis::aio::ConnectionManager;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// How often a running scan re-sets the expiry of its key. One apalis worker
/// heartbeat.
pub const SCAN_LEASE_REFRESH: Duration = Duration::from_secs(super::JOB_HEARTBEAT_SECS);

/// Refreshes a running scan may miss before its key expires.
pub const SCAN_LEASE_MISSED_REFRESHES: u32 = 4;

/// Lifetime of the key while its scan runs: [`SCAN_LEASE_MISSED_REFRESHES`]
/// refresh periods (2 minutes). A scan whose process died frees the library
/// within this time.
pub const SCAN_LEASE_TTL: Duration =
    Duration::from_secs(super::JOB_HEARTBEAT_SECS * SCAN_LEASE_MISSED_REFRESHES as u64);

/// Lifetime of the key, the queued-follow-up flags and the scan id between
/// the claim and the job's start (6 hours). Nothing refreshes a queued scan,
/// and a queue backed up behind other libraries' scans can take hours; the
/// boot sweep clears these keys after a restart.
pub const SCAN_QUEUED_TTL: Duration = Duration::from_secs(6 * 60 * 60);

const _: () = assert!(
    SCAN_LEASE_MISSED_REFRESHES >= 2,
    "the lease must survive at least one missed refresh"
);

/// Timing of a running scan's lease. Production uses [`Self::PRODUCTION`];
/// tests pass short values so expiry can be observed in milliseconds.
#[derive(Debug, Clone, Copy)]
pub struct ScanLeaseTiming {
    pub ttl: Duration,
    pub refresh: Duration,
}

impl ScanLeaseTiming {
    pub const PRODUCTION: Self = Self {
        ttl: SCAN_LEASE_TTL,
        refresh: SCAN_LEASE_REFRESH,
    };
}

/// Claim the key or record a queued follow-up, atomically.
///
/// KEYS: in_flight, scan_id, queued, queued:force, queued:scoped, queued:dirs.
/// ARGV: token, ttl_ms, force ("1"/"0"), scoped ("1"/"0"), mode, dirs...
/// `mode` is "claim" (claim when free, else queue) or "queue" (only record
/// the follow-up, even when the key is free: a hand-over whose enqueue
/// failed keeps its request for the next holder's release).
/// Returns `{"1", token}` when claimed, `{"0", current scan id}` when queued.
pub(crate) const CLAIM_OR_QUEUE_LUA: &str = r"
local ttl = tonumber(ARGV[2])
if ARGV[5] == 'claim' and redis.call('SET', KEYS[1], ARGV[1], 'NX', 'PX', ttl) then
    redis.call('SET', KEYS[2], ARGV[1], 'PX', ttl)
    return {'1', ARGV[1]}
end
if ARGV[4] == '1' then
    local already_queued = redis.call('EXISTS', KEYS[3]) == 1
    local scoped_marker = redis.call('EXISTS', KEYS[5]) == 1
    if (not already_queued) or scoped_marker then
        redis.call('SET', KEYS[5], '1', 'PX', ttl)
        for i = 6, #ARGV do
            redis.call('SADD', KEYS[6], ARGV[i])
        end
        redis.call('PEXPIRE', KEYS[6], ttl)
    end
else
    redis.call('DEL', KEYS[5], KEYS[6])
end
redis.call('SET', KEYS[3], '1', 'PX', ttl)
if ARGV[3] == '1' then
    redis.call('SET', KEYS[4], '1', 'PX', ttl)
end
local id = redis.call('GET', KEYS[2])
if not id then
    id = redis.call('GET', KEYS[1])
end
return {'0', id or ''}
";

/// Release by the holder only; hand the key to the queued follow-up if one
/// was recorded.
///
/// KEYS: as [`CLAIM_OR_QUEUE_LUA`]. ARGV: token, follow-up token, ttl_ms.
/// Returns `{"-1"}` (not the holder), `{"0"}` (released) or
/// `{"1", force, scoped, dirs...}` (handed over to the follow-up token).
pub(crate) const RELEASE_LUA: &str = r"
if redis.call('GET', KEYS[1]) ~= ARGV[1] then
    return {'-1'}
end
if redis.call('EXISTS', KEYS[3]) == 0 then
    redis.call('DEL', KEYS[1])
    return {'0'}
end
local out = {'1', tostring(redis.call('EXISTS', KEYS[4])), tostring(redis.call('EXISTS', KEYS[5]))}
if out[3] == '1' then
    for _, d in ipairs(redis.call('SMEMBERS', KEYS[6])) do
        table.insert(out, d)
    end
end
redis.call('DEL', KEYS[3], KEYS[4], KEYS[5], KEYS[6])
redis.call('SET', KEYS[1], ARGV[2], 'PX', tonumber(ARGV[3]))
redis.call('SET', KEYS[2], ARGV[2], 'PX', tonumber(ARGV[3]))
return out
";

/// Compare-and-delete with no hand-over: undo a claim whose job could not be
/// queued.
pub(crate) const COMPARE_AND_DELETE_LUA: &str = r"
if redis.call('GET', KEYS[1]) == ARGV[1] then
    return redis.call('DEL', KEYS[1])
end
return 0
";

/// Keep the key alive for its holder.
///
/// KEYS: in_flight, scan_id. ARGV: token, ttl_ms.
/// Returns 1 (held, expiry reset), 2 (was free, taken back) or 0 (another
/// token holds it).
pub(crate) const EXTEND_LUA: &str = r"
local ttl = tonumber(ARGV[2])
local cur = redis.call('GET', KEYS[1])
if cur == ARGV[1] then
    redis.call('PEXPIRE', KEYS[1], ttl)
    if redis.call('GET', KEYS[2]) == ARGV[1] then
        redis.call('PEXPIRE', KEYS[2], ttl)
    end
    return 1
end
if not cur then
    redis.call('SET', KEYS[1], ARGV[1], 'PX', ttl)
    redis.call('SET', KEYS[2], ARGV[1], 'PX', ttl)
    return 2
end
return 0
";

/// Result of one [`extend`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseCheck {
    /// The key holds this token; its expiry was reset.
    Held,
    /// The key had expired and nobody had claimed it; it holds this token again.
    Reclaimed,
    /// Another scan's token holds the key.
    Lost,
}

/// Compare-and-extend the key of `library_id` for `token`.
pub async fn extend(
    conn: &mut ConnectionManager,
    library_id: Uuid,
    token: Uuid,
    ttl: Duration,
) -> redis::RedisResult<LeaseCheck> {
    let n: i64 = redis::Script::new(EXTEND_LUA)
        .key(super::in_flight_key(library_id))
        .key(super::scan_id_key(library_id))
        .arg(token.to_string())
        .arg(ttl_ms(ttl))
        .invoke_async(conn)
        .await?;
    Ok(match n {
        1 => LeaseCheck::Held,
        2 => LeaseCheck::Reclaimed,
        _ => LeaseCheck::Lost,
    })
}

pub(crate) fn ttl_ms(ttl: Duration) -> u64 {
    u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX).max(1)
}

/// A running scan's hold on its library's coalescing key.
///
/// [`ScanLease::start`] checks the key once and then keeps it alive from a
/// task beside the scan. The task runs on the async runtime and does not
/// depend on the scanner's progress loop, so the blocking enumerate and plan
/// phases and the reconcile pass are covered too. [`ScanLease::stop`] ends
/// the task and waits for it, so no refresh can land after the release.
pub struct ScanLease {
    lost: Arc<AtomicBool>,
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl ScanLease {
    /// Check the key for `token` and start keeping it alive.
    ///
    /// Returns the first check's result with the lease. A Redis error on the
    /// first check is logged and the scan goes ahead: the refresh task keeps
    /// trying, and it stops the scan if it later finds another holder.
    pub async fn start(
        conn: ConnectionManager,
        library_id: Uuid,
        token: Uuid,
        timing: ScanLeaseTiming,
    ) -> (Self, Option<LeaseCheck>) {
        let lost = Arc::new(AtomicBool::new(false));
        let mut first_conn = conn.clone();
        let first = match extend(&mut first_conn, library_id, token, timing.ttl).await {
            Ok(check) => Some(check),
            Err(e) => {
                tracing::warn!(
                    %library_id,
                    scan_run_id = %token,
                    error = %e,
                    "scan lease: first check failed; scanning and retrying the refresh",
                );
                None
            }
        };
        if first == Some(LeaseCheck::Lost) {
            lost.store(true, Ordering::Relaxed);
            return (
                Self {
                    lost,
                    stop: CancellationToken::new(),
                    task: None,
                },
                first,
            );
        }
        let stop = CancellationToken::new();
        let task = tokio::spawn(refresh_loop(
            conn,
            library_id,
            token,
            timing,
            lost.clone(),
            stop.clone(),
        ));
        (
            Self {
                lost,
                stop,
                task: Some(task),
            },
            first,
        )
    }

    /// `true` once a refresh found another scan's token on the key.
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }

    /// The flag the scanner polls; set when the lease is lost.
    pub fn lost_flag(&self) -> Arc<AtomicBool> {
        self.lost.clone()
    }

    /// Stop refreshing and wait for the refresh task to finish. A refresh
    /// already sent completes first, so a release issued after this returns
    /// is the last command this lease sends.
    pub async fn stop(mut self) {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for ScanLease {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

async fn refresh_loop(
    mut conn: ConnectionManager,
    library_id: Uuid,
    token: Uuid,
    timing: ScanLeaseTiming,
    lost: Arc<AtomicBool>,
    stop: CancellationToken,
) {
    let mut tick = tokio::time::interval(timing.refresh);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick fires at once; the start check already ran.
    tick.tick().await;
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = tick.tick() => {}
        }
        match extend(&mut conn, library_id, token, timing.ttl).await {
            Ok(LeaseCheck::Held) => {}
            Ok(LeaseCheck::Reclaimed) => {
                tracing::warn!(
                    %library_id,
                    scan_run_id = %token,
                    "scan lease: key had expired and was free; taken back",
                );
            }
            Ok(LeaseCheck::Lost) => {
                tracing::warn!(
                    %library_id,
                    scan_run_id = %token,
                    "scan lease: another scan holds this library's key; stopping this scan",
                );
                lost.store(true, Ordering::Relaxed);
                return;
            }
            Err(e) => {
                // Transient: keep scanning and try again next tick. If Redis
                // stays away past the TTL the key lapses; the next successful
                // refresh takes it back or finds the new holder.
                tracing::warn!(
                    %library_id,
                    scan_run_id = %token,
                    error = %e,
                    "scan lease: refresh failed",
                );
            }
        }
    }
}
