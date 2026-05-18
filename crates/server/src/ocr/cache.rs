//! Redis-backed OCR result cache (text-detection-1.0 plan, M4).
//!
//! Keyed by `(content_hash, page, lang, region_hash)`:
//!
//! - **`content_hash`** is the issue's BLAKE3 of the on-disk bytes,
//!   not its (stable) row id. This makes invalidation automatic: a
//!   rescan that retags the same issue with new content rolls the
//!   key over, the old entries die via TTL, and we re-OCR. As a
//!   side benefit, two deduplicated issues with identical bytes
//!   share cache entries.
//! - **`region_hash`** is a stable 16-byte BLAKE3 of the user's
//!   integer-pixel rect (`x|y|w|h`). The lang hint is part of the
//!   key separately so two callers asking for different recognizers
//!   over the same region cache independently.
//!
//! The cache stores the public JSON response (`OcrResponse` from the
//! handler) verbatim. On hit we deserialize and return; on miss the
//! pipeline runs and we PUT the response with [`CACHE_TTL`] expiry.
//!
//! Redis failures are logged but never bubble up — the worst-case
//! degraded mode is just "every request re-OCRs," not "OCR is
//! broken." That matches the failed-auth lockout module's
//! fail-open posture.
//!
//! Hits/misses emit `comic_ocr_cache_hits_total` and
//! `comic_ocr_cache_misses_total` counters for operator dashboards.

use std::time::Duration;

use redis::AsyncCommands;
use redis::aio::ConnectionManager;

use crate::api::issue_ocr::OcrResponse;

/// 7-day TTL on cache entries. OCR results for a given
/// `(content_hash, region, lang)` are deterministic in principle,
/// but a finite TTL bounds Redis storage in deployments that
/// occasionally re-render the same page from different region
/// guesses.
pub const CACHE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Build the Redis key for one cache entry. Public so tests can
/// pre-seed entries without going through the full handler path.
pub fn cache_key(content_hash: &str, page: u32, lang: &str, region_hash: &str) -> String {
    format!("ocr:cache:{content_hash}:{page}:{lang}:{region_hash}")
}

/// Stable 32-hex BLAKE3 of the integer-pixel rect. Keeping it
/// human-inspectable (lowercase hex, no truncation surprises) helps
/// operators eyeball cache keys during incident triage.
pub fn region_hash(x: u32, y: u32, w: u32, h: u32) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&x.to_le_bytes());
    hasher.update(&y.to_le_bytes());
    hasher.update(&w.to_le_bytes());
    hasher.update(&h.to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

/// Look up a cached response. Returns `Some` on hit, `None` on miss
/// or Redis error (fail-open).
pub async fn get(redis: &ConnectionManager, key: &str) -> Option<OcrResponse> {
    let mut conn = redis.clone();
    match conn.get::<_, Option<String>>(key).await {
        Ok(Some(raw)) => match serde_json::from_str::<OcrResponse>(&raw) {
            Ok(resp) => {
                metrics::counter!("comic_ocr_cache_hits_total").increment(1);
                Some(resp)
            }
            Err(e) => {
                // Stale payload format (e.g. shape change across versions).
                // Treat as a miss so the pipeline reruns and overwrites it.
                tracing::warn!(error = %e, key, "ocr cache: malformed payload; treating as miss");
                None
            }
        },
        Ok(None) => {
            metrics::counter!("comic_ocr_cache_misses_total").increment(1);
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, key, "ocr cache: GET failed; treating as miss");
            None
        }
    }
}

/// Store a successful OCR response. Failures are swallowed.
pub async fn put(redis: &ConnectionManager, key: &str, response: &OcrResponse) {
    let mut conn = redis.clone();
    let payload = match serde_json::to_string(response) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, key, "ocr cache: serialize failed; skipping put");
            return;
        }
    };
    let set: Result<(), _> = conn.set_ex(key, payload, CACHE_TTL.as_secs()).await;
    if let Err(e) = set {
        tracing::warn!(error = %e, key, "ocr cache: SET failed; ignoring");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_hash_is_deterministic() {
        assert_eq!(region_hash(10, 20, 30, 40), region_hash(10, 20, 30, 40));
    }

    #[test]
    fn region_hash_differs_per_dim() {
        assert_ne!(region_hash(10, 20, 30, 40), region_hash(10, 20, 30, 41));
        assert_ne!(region_hash(10, 20, 30, 40), region_hash(11, 20, 30, 40));
    }

    #[test]
    fn cache_key_includes_every_component() {
        let k = cache_key("hash1", 7, "western", "rh1");
        assert!(k.contains("hash1"));
        assert!(k.contains(":7:"));
        assert!(k.contains("western"));
        assert!(k.contains("rh1"));
        assert!(k.starts_with("ocr:cache:"));
    }
}
