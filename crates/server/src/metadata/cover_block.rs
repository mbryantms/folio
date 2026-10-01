//! Process-wide memo of provider cover hosts that answer with a bot
//! challenge instead of image bytes.
//!
//! GCD's image CDN (`files1.comics.org`) sits behind a Cloudflare managed
//! challenge (`403` + `cf-mitigated: challenge`) for every non-browser
//! request, and the challenge can only be passed by a browser running
//! Cloudflare's script — Folio does **not** attempt that. Without a memo,
//! every candidate cover hash, every "Apply cover" and every variant
//! download would re-hit the CDN, wait for the 403 and log it.
//!
//! The first challenged response for a host marks it blocked for
//! [`BLOCK_TTL`]; until then [`blocked_host`] short-circuits every cover
//! fetch to that host (no network, no retry) and the caller degrades:
//! the phash matcher treats the candidate as cover-less, the apply
//! records `cover_skipped_reason = "cover_unavailable: …"`, and variant
//! rows keep their `source_url` without local bytes. The memo expires so
//! a host that lifts its challenge is picked up again without a restart.
//! Exactly one `info` line is logged per host per window.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a challenged host stays blocked before the next real probe.
pub const BLOCK_TTL: Duration = Duration::from_secs(3600);

fn memo() -> &'static Mutex<HashMap<String, Instant>> {
    static MEMO: std::sync::OnceLock<Mutex<HashMap<String, Instant>>> = std::sync::OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(HashMap::new()))
}

fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .host_str()
        .map(|h| h.to_ascii_lowercase())
}

/// `Some(host)` when `url`'s host is currently known to challenge
/// non-browser requests.
pub fn blocked_host(url: &str) -> Option<String> {
    let host = host_of(url)?;
    let mut map = memo().lock().unwrap_or_else(|e| e.into_inner());
    match map.get(&host) {
        Some(since) if since.elapsed() < BLOCK_TTL => Some(host),
        Some(_) => {
            map.remove(&host);
            None
        }
        None => None,
    }
}

/// Record that `host` answered with a challenge. Logs once per window
/// (the first caller to mark it); concurrent fetches that were already
/// in flight just refresh nothing.
pub fn note_blocked(host: &str) {
    let host = host.to_ascii_lowercase();
    if host.is_empty() {
        return;
    }
    let mut map = memo().lock().unwrap_or_else(|e| e.into_inner());
    let fresh = map
        .get(&host)
        .is_none_or(|since| since.elapsed() >= BLOCK_TTL);
    if fresh {
        map.insert(host.clone(), Instant::now());
        drop(map);
        tracing::info!(
            host = %host,
            ttl_secs = BLOCK_TTL.as_secs(),
            "cover host answered with a bot challenge; skipping cover downloads from it until the memo expires"
        );
    }
}

/// Test hook: forget every blocked host.
pub fn clear() {
    memo().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memo_blocks_by_host_case_insensitively() {
        note_blocked("Blocked-Test.Example");
        assert_eq!(
            blocked_host("https://blocked-test.example/img/1.jpg").as_deref(),
            Some("blocked-test.example")
        );
        assert!(blocked_host("https://other-host.example/img/1.jpg").is_none());
        assert!(blocked_host("not a url").is_none());
    }
}
