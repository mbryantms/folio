//! Redis-backed token bucket for provider HTTP quota gating.
//!
//! ComicVine: 200 req / resource / hour + ~1 req/sec velocity cap.
//! Metron: 30 req/min + 5,000 req/day.
//!
//! Both providers need quota state that:
//!   - **survives restarts** — restarting the server shouldn't reset
//!     the hourly bucket and lure us into a 429 cluster.
//!   - **is shared across replicas** — a future scale-out shouldn't
//!     multiply our effective quota by replica count.
//!
//! The bucket is implemented with a single atomic Redis EVAL: decrement
//! the counter if there's budget, return the new value + the seconds-
//! until-reset; otherwise return `0, retry_after` without decrementing.
//!
//! The "velocity cap" (ComicVine's ~1 req/sec) is enforced at the
//! client layer via a `tokio::time::sleep` between successful
//! reservations — the token bucket handles per-hour budget, the sleep
//! handles per-second pacing.
//!
//! Two parallel buckets per provider — `hour` and `day` — let us
//! surface both numbers in the admin gauges and refuse work when
//! *either* is exhausted.

use redis::aio::ConnectionManager;
use std::time::Duration;
use thiserror::Error;

/// Result of attempting to reserve one token from a bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reservation {
    /// Budget was deducted. `remaining` is the post-decrement count,
    /// `seconds_until_reset` is when the bucket will refill.
    Granted {
        remaining: u32,
        seconds_until_reset: u64,
    },
    /// Bucket was empty; no tokens deducted. Caller should wait
    /// `retry_after_secs` and try again.
    Denied { retry_after_secs: u64 },
}

#[derive(Debug, Error)]
pub enum BucketError {
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("unexpected Lua return: {0}")]
    InvalidReply(String),
}

/// Static bucket definition — capacity + refill window. One bucket
/// instance per (provider, scope) pair.
#[derive(Clone, Copy, Debug)]
pub struct BucketDef {
    /// Short stable identifier used as the Redis key suffix
    /// (`metadata:bucket:{key}`). Don't change post-deploy — the
    /// existing bucket state would orphan.
    pub key: &'static str,
    /// Max tokens in the bucket — refilled to this value every
    /// `window`.
    pub capacity: u32,
    /// Refill window length. The bucket key is set with `EXPIRE`
    /// equal to this on the *first* decrement of a fresh window, so
    /// Redis naturally garbages it after the window passes with no
    /// further activity.
    pub window: Duration,
}

// ───────── ComicVine ─────────

/// ComicVine's per-resource per-hour limit (`/issues`, `/volumes`,
/// `/characters`, etc each get their own 200/hour budget). We treat
/// the budget as a single shared pool across endpoints — being
/// conservative protects us from the worst-case where one endpoint
/// burns the budget and starves the others.
pub const COMICVINE_HOUR: BucketDef = BucketDef {
    key: "comicvine:hour",
    capacity: 200,
    window: Duration::from_secs(3600),
};

// ───────── Metron ─────────

pub const METRON_MIN: BucketDef = BucketDef {
    key: "metron:min",
    capacity: 30,
    window: Duration::from_secs(60),
};

pub const METRON_DAY: BucketDef = BucketDef {
    key: "metron:day",
    capacity: 5000,
    window: Duration::from_secs(86_400),
};

// ───────── core decrement script ─────────
//
// Lua params: KEYS[1] = bucket key, ARGV[1] = capacity, ARGV[2] = window seconds.
// Returns: { granted (0|1), remaining, ttl_secs }.
//
// First decrement of a fresh window: SET key=capacity-1 EX window.
// Subsequent: DECR + read TTL. Denial: return { 0, 0, ttl }.
//
// The TTL read is what makes the bucket "self-resetting" without a
// background sweeper — Redis evicts the key when `EXPIRE` runs out.
const DECREMENT_SCRIPT: &str = r#"
local key = KEYS[1]
local capacity = tonumber(ARGV[1])
local window = tonumber(ARGV[2])
local current = redis.call('GET', key)
if current == false then
  redis.call('SET', key, capacity - 1, 'EX', window)
  return {1, capacity - 1, window}
end
current = tonumber(current)
if current <= 0 then
  local ttl = redis.call('TTL', key)
  if ttl < 0 then ttl = 0 end
  return {0, 0, ttl}
end
local remaining = redis.call('DECR', key)
local ttl = redis.call('TTL', key)
if ttl < 0 then ttl = 0 end
return {1, remaining, ttl}
"#;

/// Atomically reserve one token from `bucket`.
pub async fn reserve(
    redis: &mut ConnectionManager,
    bucket: &BucketDef,
) -> Result<Reservation, BucketError> {
    let key = redis_key(bucket.key);
    let script = redis::Script::new(DECREMENT_SCRIPT);
    let raw: Vec<i64> = script
        .key(key)
        .arg(bucket.capacity as i64)
        .arg(bucket.window.as_secs() as i64)
        .invoke_async(redis)
        .await?;
    parse_reply(&raw)
}

/// Snapshot without decrementing — used by the admin dashboard
/// gauges. Returns (remaining, ttl_secs); when the bucket key doesn't
/// exist (window not started), reports `capacity` remaining and `0`
/// ttl so the UI shows "full".
pub async fn snapshot(
    redis: &mut ConnectionManager,
    bucket: &BucketDef,
) -> Result<(u32, u64), BucketError> {
    use redis::AsyncCommands;
    let key = redis_key(bucket.key);
    let current: Option<i64> = redis.get(&key).await?;
    match current {
        None => Ok((bucket.capacity, 0)),
        Some(n) => {
            let ttl: i64 = redis.ttl(&key).await?;
            let ttl = if ttl < 0 { 0 } else { ttl as u64 };
            let remaining = n.max(0) as u32;
            Ok((remaining, ttl))
        }
    }
}

fn redis_key(suffix: &str) -> String {
    format!("metadata:bucket:{suffix}")
}

fn parse_reply(raw: &[i64]) -> Result<Reservation, BucketError> {
    if raw.len() != 3 {
        return Err(BucketError::InvalidReply(format!(
            "expected 3 elements, got {}",
            raw.len()
        )));
    }
    let granted = raw[0];
    let remaining = raw[1].max(0) as u32;
    let ttl = raw[2].max(0) as u64;
    if granted == 1 {
        Ok(Reservation::Granted {
            remaining,
            seconds_until_reset: ttl,
        })
    } else {
        Ok(Reservation::Denied {
            retry_after_secs: ttl.max(1),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reply_grants() {
        let r = parse_reply(&[1, 42, 3600]).unwrap();
        assert_eq!(
            r,
            Reservation::Granted {
                remaining: 42,
                seconds_until_reset: 3600
            }
        );
    }

    #[test]
    fn parse_reply_denies_with_floor() {
        let r = parse_reply(&[0, 0, 0]).unwrap();
        // Denied always floors retry_after to ≥1 so callers never busy-loop.
        assert_eq!(
            r,
            Reservation::Denied {
                retry_after_secs: 1
            }
        );
    }

    #[test]
    fn parse_reply_rejects_unexpected_shape() {
        let err = parse_reply(&[1]).unwrap_err();
        assert!(matches!(err, BucketError::InvalidReply(_)));
    }
}
