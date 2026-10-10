//! Redis-backed token bucket for provider HTTP quota gating.
//!
//! ComicVine: "200 requests per resource, per hour" (api page) — one
//! bucket per resource (`/volumes`, `/search`, `/issues`, `/volume`,
//! `/issue`), plus a 1 req/sec velocity bucket shared by every client,
//! worker and replica (CV documents "velocity detection" with temporary
//! blocks, no number; 1/s is the figure the community settled on).
//! Metron: 20 req/min (burst) + 5,000 req/day (sustained) — the
//! March 2026 limits; supporters get a higher sustained cap, which the
//! upstream reports via `X-RateLimit-*` headers (see `metadata::budget`).
//! GCD: ~100 req/hour pacing + 2,000 req/day (the upstream user tier),
//! plus a 1 req/sec velocity floor in the client.
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
//! The ComicVine velocity cap is a 1-second bucket of capacity 1
//! ([`COMICVINE_SEC`]): a denied reservation means "someone else fired
//! this second", and the client sleeps the window and tries again rather
//! than parking the run. A per-process `Mutex<Instant>` can't do this —
//! every job builds its own client, and the search, apply and coverage
//! workers plus the API handlers run side by side.
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

/// A ComicVine API resource — the unit the hourly limit is counted
/// against. One bucket each, so a per-issue batch (`/issues` searches
/// plus `/issue` details) spends up to 400 requests an hour instead of
/// the 200 a pooled bucket allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComicVineResource {
    /// `/volumes` — series search.
    Volumes,
    /// `/search` — keyword search.
    Search,
    /// `/issues` — issue search / a volume's issue list.
    Issues,
    /// `/volume/4050-{id}` — series detail.
    Volume,
    /// `/issue/4000-{id}` — issue detail.
    Issue,
}

pub const COMICVINE_RESOURCES: [ComicVineResource; 5] = [
    ComicVineResource::Volumes,
    ComicVineResource::Search,
    ComicVineResource::Issues,
    ComicVineResource::Volume,
    ComicVineResource::Issue,
];

impl ComicVineResource {
    /// The resource a request path counts against (`/issues/?filter=…`
    /// → `Issues`, `/issue/4000-12/` → `Issue`). Unknown paths count
    /// against `Search`, the catch-all.
    pub fn from_path(path: &str) -> Self {
        let first = path
            .trim_start_matches('/')
            .split(['/', '?'])
            .next()
            .unwrap_or("");
        match first {
            "volumes" => Self::Volumes,
            "issues" => Self::Issues,
            "volume" => Self::Volume,
            "issue" => Self::Issue,
            _ => Self::Search,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Volumes => "volumes",
            Self::Search => "search",
            Self::Issues => "issues",
            Self::Volume => "volume",
            Self::Issue => "issue",
        }
    }
}

/// ComicVine's documented limit, per resource: "200 requests per
/// resource, per hour". Keys are `comicvine:hour:<resource>`; the old
/// pooled `comicvine:hour` key simply expires.
pub const fn comicvine_hour(resource: ComicVineResource) -> BucketDef {
    let key = match resource {
        ComicVineResource::Volumes => "comicvine:hour:volumes",
        ComicVineResource::Search => "comicvine:hour:search",
        ComicVineResource::Issues => "comicvine:hour:issues",
        ComicVineResource::Volume => "comicvine:hour:volume",
        ComicVineResource::Issue => "comicvine:hour:issue",
    };
    BucketDef {
        key,
        capacity: COMICVINE_HOUR_CAPACITY,
        window: Duration::from_secs(3600),
    }
}

/// The per-resource hourly capacity ComicVine documents.
pub const COMICVINE_HOUR_CAPACITY: u32 = 200;

/// ComicVine velocity cap: one request per second across every client,
/// worker and replica. A denial here is a one-second wait, not quota
/// exhaustion — see `comicvine::reserve_slot`.
pub const COMICVINE_SEC: BucketDef = BucketDef {
    key: "comicvine:sec",
    capacity: 1,
    window: Duration::from_secs(1),
};

/// The ComicVine hourly gauge: the resource with the least budget left
/// is the one that binds, so that is what the dashboard shows. Returns
/// `(remaining, seconds_until_reset)` of that resource; a fresh hour is
/// `(200, 0)` like any untouched bucket.
pub async fn comicvine_hour_snapshot(
    redis: &mut ConnectionManager,
) -> Result<(u32, u64), BucketError> {
    let mut tightest: Option<(u32, u64)> = None;
    for r in COMICVINE_RESOURCES {
        let snap = snapshot(redis, &comicvine_hour(r)).await?;
        tightest = Some(match tightest {
            Some(cur) if cur.0 <= snap.0 => cur,
            _ => snap,
        });
    }
    Ok(tightest.unwrap_or((COMICVINE_HOUR_CAPACITY, 0)))
}

// ───────── Metron ─────────

/// Metron's burst window. 20/min since March 2026 (was 30). The
/// upstream enforces this per account and returns the live figure in
/// `X-RateLimit-Burst-*`; this bucket is the local pre-flight so a
/// worker never fires a request it knows will 429.
pub const METRON_MIN: BucketDef = BucketDef {
    key: "metron:min",
    capacity: 20,
    window: Duration::from_secs(60),
};

/// Metron's sustained window. 5,000/day is the base tier; donors get
/// more, but the local bucket stays at the base so a shared account
/// never over-commits.
pub const METRON_DAY: BucketDef = BucketDef {
    key: "metron:day",
    capacity: 5000,
    window: Duration::from_secs(86_400),
};

// ───────── GCD (WP-6.1) ─────────

/// Grand Comics Database hourly pacing bucket. GCD throttles an
/// authenticated account at 2,000 requests/day (anonymous: 30/hour);
/// ~100/hour keeps a burst of searches from spending the whole day in
/// one sitting while still allowing the daily total over a working day.
pub const GCD_HOUR: BucketDef = BucketDef {
    key: "gcd:hour",
    capacity: 100,
    window: Duration::from_secs(3600),
};

/// GCD's authenticated daily throttle (`user: 2000/day` upstream).
pub const GCD_DAY: BucketDef = BucketDef {
    key: "gcd:day",
    capacity: 2000,
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

// The provider itself said no (HTTP 429 / ComicVine status 107): make
// the local bucket agree for the provider's own `Retry-After`, so the
// other workers stop spending real requests on answers we already have.
const EXHAUST_SCRIPT: &str = r#"
local key = KEYS[1]
local want = tonumber(ARGV[1])
if want < 1 then want = 1 end
redis.call('SET', key, 0, 'EX', want)
return want
"#;

/// Drain `bucket` for `retry_after_secs` because the provider reported
/// its quota exhausted — the API's own word beats the local count, in
/// both directions: a short `Retry-After` reopens the bucket sooner than
/// the local window would have (a wrong guess costs one more 429, which
/// drains it again). Best-effort; a Redis error is logged, since the
/// bucket still gates the next call and the provider still answers 429.
pub async fn exhaust(redis: &mut ConnectionManager, bucket: &BucketDef, retry_after_secs: u64) {
    let key = redis_key(bucket.key);
    let res: Result<i64, redis::RedisError> = redis::Script::new(EXHAUST_SCRIPT)
        .key(key)
        .arg(retry_after_secs as i64)
        .invoke_async(redis)
        .await;
    match res {
        Ok(ttl) => tracing::info!(
            bucket = bucket.key,
            ttl_secs = ttl,
            "rate limit: provider reported quota exhausted; local bucket drained"
        ),
        Err(e) => tracing::warn!(bucket = bucket.key, error = %e, "rate limit: exhaust failed"),
    }
}

/// Forget `bucket`'s current window, as if it had expired: the next
/// reservation starts a fresh one at full capacity. For tests that
/// simulate a provider's window passing, and for an operator reset.
pub async fn refill(redis: &mut ConnectionManager, bucket: &BucketDef) -> Result<(), BucketError> {
    use redis::AsyncCommands;
    let _: () = redis.del(redis_key(bucket.key)).await?;
    Ok(())
}

/// [`refill`] every ComicVine bucket: the five hourly resources and the
/// velocity bucket.
pub async fn comicvine_refill(redis: &mut ConnectionManager) -> Result<(), BucketError> {
    for r in COMICVINE_RESOURCES {
        refill(redis, &comicvine_hour(r)).await?;
    }
    refill(redis, &COMICVINE_SEC).await
}

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

#[cfg(test)]
mod comicvine_resource_tests {
    use super::*;

    #[test]
    fn paths_count_against_their_resource() {
        assert_eq!(
            ComicVineResource::from_path("/volumes"),
            ComicVineResource::Volumes
        );
        assert_eq!(
            ComicVineResource::from_path("/volumes?filter=name:saga"),
            ComicVineResource::Volumes
        );
        assert_eq!(
            ComicVineResource::from_path("/issues/"),
            ComicVineResource::Issues
        );
        assert_eq!(
            ComicVineResource::from_path("/issues?filter=volume:1"),
            ComicVineResource::Issues
        );
        assert_eq!(
            ComicVineResource::from_path("/volume/4050-3790"),
            ComicVineResource::Volume
        );
        assert_eq!(
            ComicVineResource::from_path("/issue/4000-28171"),
            ComicVineResource::Issue
        );
        assert_eq!(
            ComicVineResource::from_path("/search"),
            ComicVineResource::Search
        );
        assert_eq!(
            ComicVineResource::from_path("/publishers"),
            ComicVineResource::Search
        );
        // Five distinct hourly buckets, 200 each; one shared 1 req/s bucket.
        let keys: std::collections::HashSet<&str> = COMICVINE_RESOURCES
            .iter()
            .map(|r| comicvine_hour(*r).key)
            .collect();
        assert_eq!(keys.len(), 5);
        assert!(
            COMICVINE_RESOURCES
                .iter()
                .all(|r| comicvine_hour(*r).capacity == 200)
        );
        assert_eq!(COMICVINE_SEC.capacity, 1);
        assert_eq!(COMICVINE_SEC.window, Duration::from_secs(1));
    }
}
