//! Redis-backed OCR result cache (text-detection-1.0 plan, M4).
//!
//! Keyed by `(engine fingerprint, content_hash, page, lang, detect,
//! region_hash)`:
//!
//! - **engine fingerprint** is a 16-hex hash of the version constants,
//!   the pinned ONNX model revisions and digests, the Tesseract version
//!   and a digest of the language data it loads ([`EngineInputs`]),
//!   computed once per process. An image upgrade that changes any of
//!   them rolls every key; old entries age out via the TTL. The detect
//!   cache carries a detector-only fingerprint.
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
//! Hits/misses emit `folio_ocr_cache_hits_total` and
//! `folio_ocr_cache_misses_total` counters for operator dashboards.

use std::time::Duration;

use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};

use crate::api::issue_ocr::OcrResponse;

/// 7-day TTL on cache entries. OCR results for a given
/// `(content_hash, region, lang)` are deterministic in principle,
/// but a finite TTL bounds Redis storage in deployments that
/// occasionally re-render the same page from different region
/// guesses.
pub const CACHE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Longest a cache write may take. The writes run in the detached OCR
/// work (`ocr::pipeline`) after the request may be gone; a hung Redis
/// must not leave those tasks waiting forever.
pub const CACHE_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// `SET key payload EX ttl`, bounded by [`CACHE_WRITE_TIMEOUT`]. Failures
/// are logged and swallowed (fail-open).
async fn set_bounded(redis: &ConnectionManager, key: &str, payload: String, what: &str) {
    let mut conn = redis.clone();
    let set = tokio::time::timeout(
        CACHE_WRITE_TIMEOUT,
        conn.set_ex::<_, _, ()>(key, payload, CACHE_TTL.as_secs()),
    )
    .await;
    match set {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, key, "{what}: SET failed; ignoring"),
        Err(_) => tracing::warn!(key, "{what}: SET timed out; ignoring"),
    }
}

/// Version byte folded into every result-cache key. Bump on any
/// change that alters what the pipeline would produce for the same
/// inputs: preprocessing (binarization, padding, DPI), PSM,
/// Tesseract variables, postprocess rules or their consts, or
/// response semantics. Old-version keys age out via [`CACHE_TTL`] —
/// no flush job needed.
///
/// History: v3 = cleanup reads `|` as the pronoun `I`, keeps the hyphen
/// of a line-end join and the double dash of an interruption, keeps
/// confident grawlix; v2 = Otsu/polarity/padding preprocessing +
/// postprocess cleanup (OCR rework 1.0); v1 = unversioned launch shape.
pub const OCR_RESULT_VERSION: u32 = 3;

// ───────── engine fingerprint ─────────
//
// The version constants cover what this code does to an image; they do
// not cover what the engines are. An image upgrade that changes the
// Tesseract library, its language data, or a pinned ONNX model would
// otherwise keep serving results computed by the old engines until the
// TTL ran out. Both keys therefore carry a short hash of everything that
// decides their payload. Old keys simply age out.

/// Hex characters of the fingerprint kept in a key (64 bits).
const FINGERPRINT_HEX_LEN: usize = 16;

/// Everything that decides what a cached payload holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineInputs {
    pub result_version: u32,
    pub detect_version: u32,
    /// `repo@revision/file#sha256` of every pinned detector file.
    pub detector_pins: Vec<String>,
    /// The same for the manga-ocr files.
    pub manga_pins: Vec<String>,
    /// Tesseract version + language-data digest
    /// ([`super::recognizer::western::engine_identity`]).
    pub western: String,
}

impl EngineInputs {
    /// What this process runs. Blocking: hashes the Tesseract language
    /// data. Call through [`fingerprints`] (once per process).
    pub fn current() -> Self {
        let pins = |files: &[super::models::PinnedFile]| {
            files
                .iter()
                .map(|f| format!("{}@{}/{}#{}", f.repo, f.revision, f.file, f.sha256))
                .collect()
        };
        Self {
            result_version: OCR_RESULT_VERSION,
            detect_version: OCR_DETECT_VERSION,
            detector_pins: pins(super::models::DETECTOR_FILES),
            manga_pins: pins(super::models::MANGA_FILES),
            western: super::recognizer::western::engine_identity(),
        }
    }

    /// Fingerprint for result-cache keys: the cleanup + preprocessing
    /// version, both recognizers, and the detector (a `detect: true`
    /// result is read from the detector's crop).
    pub fn result_fingerprint(&self) -> String {
        let mut parts = vec![
            format!("result-v{}", self.result_version),
            format!("detect-v{}", self.detect_version),
            self.western.clone(),
        ];
        parts.extend(self.detector_pins.iter().cloned());
        parts.extend(self.manga_pins.iter().cloned());
        short_hash(&parts)
    }

    /// Fingerprint for detect-cache keys: the detector alone.
    pub fn detect_fingerprint(&self) -> String {
        let mut parts = vec![format!("detect-v{}", self.detect_version)];
        parts.extend(self.detector_pins.iter().cloned());
        short_hash(&parts)
    }
}

/// BLAKE3 over length-prefixed parts (so part boundaries cannot shift),
/// first [`FINGERPRINT_HEX_LEN`] hex characters.
fn short_hash(parts: &[String]) -> String {
    let mut hasher = blake3::Hasher::new();
    for p in parts {
        hasher.update(&(p.len() as u64).to_le_bytes());
        hasher.update(p.as_bytes());
    }
    let mut hex = hasher.finalize().to_hex().to_string();
    hex.truncate(FINGERPRINT_HEX_LEN);
    hex
}

/// The two key fingerprints of this process.
#[derive(Debug, Clone)]
pub struct Fingerprints {
    pub result: String,
    pub detect: String,
}

static FINGERPRINTS: std::sync::OnceLock<Fingerprints> = std::sync::OnceLock::new();

/// This process's fingerprints, computed on first use. The first call
/// hashes the Tesseract language data (blocking, tens of ms); the
/// handlers call [`warm_fingerprints`] first so that happens off the
/// async runtime.
pub fn fingerprints() -> &'static Fingerprints {
    FINGERPRINTS.get_or_init(|| {
        let inputs = EngineInputs::current();
        let fp = Fingerprints {
            result: inputs.result_fingerprint(),
            detect: inputs.detect_fingerprint(),
        };
        tracing::info!(
            result = %fp.result,
            detect = %fp.detect,
            western = %inputs.western,
            "ocr cache: engine fingerprints"
        );
        fp
    })
}

/// Compute [`fingerprints`] on the blocking pool if no one has yet.
pub async fn warm_fingerprints() {
    if FINGERPRINTS.get().is_none() {
        let _ = tokio::task::spawn_blocking(|| {
            fingerprints();
        })
        .await;
    }
}

/// Build the Redis key for one cache entry. Public so tests can
/// pre-seed entries without going through the full handler path.
///
/// `detect` distinguishes snap-to-bubble results (`true` → `d`) from
/// raw-rect results (`false` → `r`). The recognized text differs
/// between the two because the detector reframes the user's region;
/// mixing them in the cache would serve stale answers when an
/// operator (or future per-call override) flips detection state.
///
/// The segment after the version is the engine fingerprint
/// ([`Fingerprints::result`]).
pub fn cache_key(
    content_hash: &str,
    page: u32,
    lang: &str,
    detect: bool,
    region_hash: &str,
) -> String {
    result_key_with(
        &fingerprints().result,
        content_hash,
        page,
        lang,
        detect,
        region_hash,
    )
}

fn result_key_with(
    fingerprint: &str,
    content_hash: &str,
    page: u32,
    lang: &str,
    detect: bool,
    region_hash: &str,
) -> String {
    let d = if detect { "d" } else { "r" };
    format!(
        "ocr:cache:v{OCR_RESULT_VERSION}:{fingerprint}:{content_hash}:{page}:{lang}:{d}:{region_hash}"
    )
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
                metrics::counter!("folio_ocr_cache_hits_total").increment(1);
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
            metrics::counter!("folio_ocr_cache_misses_total").increment(1);
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
    let payload = match serde_json::to_string(response) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, key, "ocr cache: serialize failed; skipping put");
            return;
        }
    };
    set_bounded(redis, key, payload, "ocr cache").await;
}

// ───────── detect-result cache ─────────
//
// The detector is the expensive stage (typical ~3 s on CPU; ~30+ s on
// constrained hosts). Result-cache hits skip it but require the *same*
// region. Caching the detector output per `(content_hash, page)` lets
// re-OCRs on *different* regions of the same page skip the detector
// too. First OCR on a page still pays the detector cost; subsequent
// OCRs on any bubble in the same page are recognize-only.

/// Serializable mirror of `comic_text_detector::ClassifiedBbox`.
/// Stored in Redis under the per-page detect cache. We don't reuse the
/// upstream type because it isn't `Serialize` — and pinning our own
/// shape decouples the on-wire payload from upstream version churn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedBbox {
    pub xmin: f32,
    pub ymin: f32,
    pub xmax: f32,
    pub ymax: f32,
    pub confidence: f32,
    pub class: u32,
}

/// Full per-page detector output: the bbox list plus the decoded
/// page dimensions. The dims ride along so the text-regions endpoint
/// can convert pixel bboxes to percent coordinates on a cache hit
/// without re-decoding the page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedDetection {
    pub page_w: u32,
    pub page_h: u32,
    pub bboxes: Vec<CachedBbox>,
}

/// Version byte in the detect-cache key. v2 = payload grew page
/// dimensions ([`CachedDetection`]); v1 stored a bare bbox array.
/// Old-version keys age out via [`CACHE_TTL`].
pub const OCR_DETECT_VERSION: u32 = 2;

/// Redis key for the per-page detector output cache. The segment after
/// the version is the detector fingerprint ([`Fingerprints::detect`]).
pub fn detect_cache_key(content_hash: &str, page: u32) -> String {
    detect_key_with(&fingerprints().detect, content_hash, page)
}

fn detect_key_with(fingerprint: &str, content_hash: &str, page: u32) -> String {
    format!("ocr:detect:v{OCR_DETECT_VERSION}:{fingerprint}:{content_hash}:{page}")
}

/// Look up cached detector output for a page. `Some` on hit, `None`
/// on miss or Redis error (fail-open).
pub async fn get_detect(redis: &ConnectionManager, key: &str) -> Option<CachedDetection> {
    let mut conn = redis.clone();
    match conn.get::<_, Option<String>>(key).await {
        Ok(Some(raw)) => match serde_json::from_str::<CachedDetection>(&raw) {
            Ok(detection) => {
                metrics::counter!("folio_ocr_detect_cache_hits_total").increment(1);
                Some(detection)
            }
            Err(e) => {
                tracing::warn!(error = %e, key, "ocr detect cache: malformed payload; treating as miss");
                None
            }
        },
        Ok(None) => {
            metrics::counter!("folio_ocr_detect_cache_misses_total").increment(1);
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, key, "ocr detect cache: GET failed; treating as miss");
            None
        }
    }
}

/// Store the detector's output for a page. Failures are swallowed.
pub async fn put_detect(redis: &ConnectionManager, key: &str, detection: &CachedDetection) {
    let payload = match serde_json::to_string(detection) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, key, "ocr detect cache: serialize failed; skipping put");
            return;
        }
    };
    set_bounded(redis, key, payload, "ocr detect cache").await;
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
        let k = cache_key("hash1", 7, "western", true, "rh1");
        assert!(k.contains("hash1"));
        assert!(k.contains(":7:"));
        assert!(k.contains("western"));
        assert!(k.contains("rh1"));
        assert!(k.starts_with(&format!("ocr:cache:v{OCR_RESULT_VERSION}:")));
    }

    #[test]
    fn cache_key_distinguishes_detect_flag() {
        let with_det = cache_key("h", 0, "western", true, "rh");
        let without_det = cache_key("h", 0, "western", false, "rh");
        assert_ne!(with_det, without_det);
    }

    #[test]
    fn detect_cache_key_includes_hash_and_page() {
        let k = detect_cache_key("hashA", 12);
        assert!(k.contains("hashA"));
        assert!(k.contains(":12"));
        assert!(k.starts_with(&format!("ocr:detect:v{OCR_DETECT_VERSION}:")));
    }

    fn inputs() -> EngineInputs {
        EngineInputs {
            result_version: 3,
            detect_version: 2,
            detector_pins: vec!["acme/det@aaa/det.onnx#111".into()],
            manga_pins: vec![
                "acme/manga@bbb/enc.onnx#222".into(),
                "acme/manga@bbb/vocab.txt#333".into(),
            ],
            western: "tesseract=5.5.0;eng.traineddata=abc".into(),
        }
    }

    fn is_fingerprint(s: &str) -> bool {
        s.len() == FINGERPRINT_HEX_LEN && s.bytes().all(|b| b.is_ascii_hexdigit())
    }

    #[test]
    fn keys_carry_a_short_fingerprint() {
        let k = result_key_with("0123456789abcdef", "hashA", 4, "western", false, "rh");
        assert_eq!(
            k,
            format!("ocr:cache:v{OCR_RESULT_VERSION}:0123456789abcdef:hashA:4:western:r:rh")
        );
        let k = detect_key_with("fedcba9876543210", "hashA", 4);
        assert_eq!(
            k,
            format!("ocr:detect:v{OCR_DETECT_VERSION}:fedcba9876543210:hashA:4")
        );
        // The process's own keys use its own fingerprints.
        let fp = fingerprints();
        assert!(
            is_fingerprint(&fp.result) && is_fingerprint(&fp.detect),
            "{fp:?}"
        );
        let k = cache_key("hashA", 4, "western", true, "rh");
        let parts: Vec<&str> = k.split(':').collect();
        assert_eq!(
            parts[..3],
            ["ocr", "cache", &format!("v{OCR_RESULT_VERSION}")]
        );
        assert_eq!(parts[3], fp.result);
        assert_eq!(parts[4..], ["hashA", "4", "western", "d", "rh"]);
        let parts: Vec<String> = detect_cache_key("hashA", 4)
            .split(':')
            .map(str::to_owned)
            .collect();
        assert_eq!(parts[3], fp.detect);
        assert_eq!(parts[4..], ["hashA", "4"]);
    }

    #[test]
    fn every_engine_input_changes_the_result_fingerprint() {
        let base = inputs();
        let fp = base.result_fingerprint();
        assert!(is_fingerprint(&fp));
        assert_eq!(fp, inputs().result_fingerprint(), "deterministic");
        let variants: Vec<(&str, EngineInputs)> = vec![
            (
                "result version",
                EngineInputs {
                    result_version: 4,
                    ..inputs()
                },
            ),
            (
                "detect version",
                EngineInputs {
                    detect_version: 3,
                    ..inputs()
                },
            ),
            (
                "detector pin",
                EngineInputs {
                    detector_pins: vec!["acme/det@ccc/det.onnx#111".into()],
                    ..inputs()
                },
            ),
            (
                "detector digest",
                EngineInputs {
                    detector_pins: vec!["acme/det@aaa/det.onnx#999".into()],
                    ..inputs()
                },
            ),
            (
                "manga pin",
                EngineInputs {
                    manga_pins: vec![
                        "acme/manga@bbb/enc.onnx#222".into(),
                        "acme/manga@bbb/vocab.txt#444".into(),
                    ],
                    ..inputs()
                },
            ),
            (
                "tesseract version",
                EngineInputs {
                    western: "tesseract=5.5.1;eng.traineddata=abc".into(),
                    ..inputs()
                },
            ),
            (
                "language data",
                EngineInputs {
                    western: "tesseract=5.5.0;eng.traineddata=abd".into(),
                    ..inputs()
                },
            ),
        ];
        for (what, v) in variants {
            assert_ne!(
                v.result_fingerprint(),
                fp,
                "{what} must change the result fingerprint"
            );
        }
    }

    #[test]
    fn the_detect_fingerprint_follows_the_detector_only() {
        let fp = inputs().detect_fingerprint();
        assert!(is_fingerprint(&fp));
        for (what, v) in [
            (
                "detect version",
                EngineInputs {
                    detect_version: 3,
                    ..inputs()
                },
            ),
            (
                "detector digest",
                EngineInputs {
                    detector_pins: vec!["acme/det@aaa/det.onnx#999".into()],
                    ..inputs()
                },
            ),
        ] {
            assert_ne!(v.detect_fingerprint(), fp, "{what} must change it");
        }
        for (what, v) in [
            (
                "result version",
                EngineInputs {
                    result_version: 9,
                    ..inputs()
                },
            ),
            (
                "tesseract",
                EngineInputs {
                    western: "tesseract=6".into(),
                    ..inputs()
                },
            ),
            (
                "manga pins",
                EngineInputs {
                    manga_pins: vec![],
                    ..inputs()
                },
            ),
        ] {
            assert_eq!(
                v.detect_fingerprint(),
                fp,
                "{what} must not touch detections"
            );
        }
    }

    #[test]
    fn the_process_fingerprint_names_tesseract_and_every_pin() {
        let current = EngineInputs::current();
        assert!(
            current.western.starts_with("tesseract="),
            "{}",
            current.western
        );
        assert!(
            current.western.contains("eng.traineddata="),
            "{}",
            current.western
        );
        assert_eq!(
            current.detector_pins.len(),
            super::super::models::DETECTOR_FILES.len()
        );
        assert_eq!(
            current.manga_pins.len(),
            super::super::models::MANGA_FILES.len()
        );
    }

    #[test]
    fn cached_detection_roundtrips_through_json() {
        let detection = CachedDetection {
            page_w: 1988,
            page_h: 3056,
            bboxes: vec![CachedBbox {
                xmin: 10.0,
                ymin: 20.0,
                xmax: 100.0,
                ymax: 80.0,
                confidence: 0.91,
                class: 0,
            }],
        };
        let json = serde_json::to_string(&detection).unwrap();
        let back: CachedDetection = serde_json::from_str(&json).unwrap();
        assert_eq!(back.page_w, 1988);
        assert_eq!(back.bboxes.len(), 1);
        assert!((back.bboxes[0].xmin - 10.0).abs() < 1e-6);
        assert_eq!(back.bboxes[0].class, 0);
    }

    #[test]
    fn v1_bare_array_payload_is_rejected() {
        // Pre-v2 payloads were a bare bbox array; deserializing one
        // into `CachedDetection` must fail so `get_detect` treats it
        // as a miss rather than serving dimensionless data.
        let v1 = r#"[{"xmin":1.0,"ymin":2.0,"xmax":3.0,"ymax":4.0,"confidence":0.9,"class":0}]"#;
        assert!(serde_json::from_str::<CachedDetection>(v1).is_err());
    }
}
