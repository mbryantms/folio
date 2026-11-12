//! Redis-backed OCR result cache (text-detection-1.0 plan, M4).
//!
//! Keyed by `(engine fingerprint, content_hash, page, lang, detect,
//! region_hash)`:
//!
//! - **engine fingerprint** is a 16-hex hash of the engine set, the
//!   version constants and the engines' identity: the pinned model
//!   revisions and digests, and for the legacy set the Tesseract version
//!   and a digest of the language data it loads ([`EngineInputs`]),
//!   computed once per process and engine set. Anything that changes
//!   one of them rolls every key; old entries age out via the TTL. The
//!   detect cache carries a detector-only fingerprint. The two engine
//!   sets never share a key.
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

use super::EngineSet;
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
    set_bounded_ttl(redis, key, payload, CACHE_TTL, what).await;
}

async fn set_bounded_ttl(
    redis: &ConnectionManager,
    key: &str,
    payload: String,
    ttl: Duration,
    what: &str,
) {
    let mut conn = redis.clone();
    let set = tokio::time::timeout(
        CACHE_WRITE_TIMEOUT,
        conn.set_ex::<_, _, ()>(key, payload, ttl.as_secs()),
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
/// inputs: an engine's preprocessing or decoding (resize, normalisation,
/// thresholds, line splitting; for Tesseract binarization, padding, DPI,
/// PSM, variables), postprocess rules or their consts, or response
/// semantics. Old-version keys age out via [`CACHE_TTL`] — no flush job
/// needed.
///
/// History: v4 = Western text is read by the line finder and the CTC
/// line recogniser, `words` holds one entry per line, and only the line
/// join is applied to it; Japanese crops come from the balloon detector
/// (the legacy engines keep the v3 behaviour under their own
/// fingerprint); v3 = cleanup reads `|` as the pronoun `I`,
/// keeps the hyphen of a line-end join and the double dash of an
/// interruption, keeps confident grawlix; v2 = Otsu/polarity/padding
/// preprocessing + postprocess cleanup (OCR rework 1.0); v1 =
/// unversioned launch shape.
pub const OCR_RESULT_VERSION: u32 = 4;

// ───────── engine fingerprint ─────────
//
// The version constants cover what this code does to an image; they do
// not cover what the engines are. A change of engine set, a moved model
// pin, or (legacy set) an image upgrade that changes the Tesseract
// library or its language data would otherwise keep serving results
// computed by the old engines until the TTL ran out. Both keys therefore
// carry a short hash of everything that decides their payload. Old keys
// simply age out.

/// Hex characters of the fingerprint kept in a key (64 bits).
const FINGERPRINT_HEX_LEN: usize = 16;

/// Everything that decides what a cached payload holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineInputs {
    /// The engine set the payload was computed by.
    pub engines: EngineSet,
    pub result_version: u32,
    pub detect_version: u32,
    /// `repo@revision/file#sha256` of every pinned file of the set's
    /// page detector.
    pub detector_pins: Vec<String>,
    /// The same for the manga-ocr files.
    pub manga_pins: Vec<String>,
    /// The set's Western recogniser. Current set: the pins of the line
    /// finder and of the line recogniser ([`line_reader_identity`]).
    /// Legacy set: Tesseract version + language-data digest
    /// ([`super::recognizer::western::engine_identity`]).
    pub western: String,
    /// What a Japanese request hands to manga-ocr
    /// ([`super::manga_crop::MangaCrop::label`]); fixed for the legacy
    /// set, which reads the region as it always did.
    pub manga_crop: &'static str,
    /// The constants of the set's page detector that decide a detection
    /// ([`super::bubbles::fingerprint`]); empty for the legacy set, whose
    /// detector is an upstream crate.
    pub detector_tuning: String,
    /// The other constants that decide a result: the line finder's, the
    /// recogniser's, the Japanese crop's and the region de-duplication's
    /// ([`reader_tuning`]); empty for the legacy set.
    pub reader_tuning: String,
    /// What decides a decoded page's size ([`page_size_key`]'s
    /// fingerprint): a percent region becomes a pixel rect through it, so
    /// whatever rolls the page-size keys rolls the result keys too.
    pub page_size: String,
}

/// `repo@revision/file#sha256` of each file.
fn pins(files: &[super::models::PinnedFile]) -> Vec<String> {
    files
        .iter()
        .map(|f| format!("{}@{}/{}#{}", f.repo, f.revision, f.file, f.sha256))
        .collect()
}

/// The constants of the current set, beyond the detector's, that change
/// what a request returns: how a crop is split into lines, how lines are
/// fed to the recogniser, which box a Japanese request reads and which
/// boxes a rectangle can snap to. Each module lists its own, so a changed
/// threshold rolls the result keys without a manual version bump. (The
/// version constants remain for changes in the code itself.)
pub fn reader_tuning() -> String {
    [
        super::lines::fingerprint(),
        super::recognizer::ctc::fingerprint(),
        super::manga_crop::fingerprint(),
        format!("regions:iou={}", super::regions::NEAR_DUPLICATE_IOU),
    ]
    .join("|")
}

/// Identity of the current set's Western path: every pinned file of the
/// line finder and of the line recogniser the server runs
/// ([`super::models::WESTERN_REC`], model and dictionary).
pub fn line_reader_identity() -> String {
    format!(
        "line-finder={};line-recogniser={}:{}",
        pins(super::models::LINE_FINDER_FILES).join(","),
        super::models::WESTERN_REC.id,
        pins(super::models::WESTERN_REC.files).join(","),
    )
}

impl EngineInputs {
    /// What this process runs with `engines`. For the legacy set this is
    /// blocking: it hashes the Tesseract language data. Call through
    /// [`fingerprints_for`] (once per process and set).
    pub fn current(engines: EngineSet) -> Self {
        use super::manga_crop::{MANGA_CROP, MangaCrop};
        use super::models;
        let (detector, western, manga_crop, detector_tuning, reader_tuning) = match engines {
            EngineSet::Current => (
                models::BUBBLE_DETECTOR_FILES,
                line_reader_identity(),
                MANGA_CROP,
                super::bubbles::fingerprint(),
                reader_tuning(),
            ),
            EngineSet::Legacy => (
                models::DETECTOR_FILES,
                super::recognizer::western::engine_identity(),
                MangaCrop::Region,
                String::new(),
                String::new(),
            ),
        };
        Self {
            engines,
            manga_crop: manga_crop.label(),
            detector_tuning,
            reader_tuning,
            result_version: OCR_RESULT_VERSION,
            detect_version: OCR_DETECT_VERSION,
            detector_pins: pins(detector),
            manga_pins: pins(models::MANGA_FILES),
            western,
            page_size: page_size_fingerprint().to_owned(),
        }
    }

    /// Fingerprint for result-cache keys: the engine set, the cleanup +
    /// preprocessing version, both recognizers, and the detector (a
    /// `detect: true` result is read from the detector's crop).
    pub fn result_fingerprint(&self) -> String {
        let mut parts = vec![
            format!("engines={}", self.engines.label()),
            format!("result-v{}", self.result_version),
            format!("detect-v{}", self.detect_version),
            self.western.clone(),
            format!("manga-crop={}", self.manga_crop),
            format!("detector-tuning={}", self.detector_tuning),
            format!("reader-tuning={}", self.reader_tuning),
            format!("page-size={}", self.page_size),
        ];
        parts.extend(self.detector_pins.iter().cloned());
        parts.extend(self.manga_pins.iter().cloned());
        short_hash(&parts)
    }

    /// Fingerprint for detect-cache keys: the engine set and its
    /// detector alone (its pins and the constants that decide which
    /// boxes it keeps).
    pub fn detect_fingerprint(&self) -> String {
        let mut parts = vec![
            format!("engines={}", self.engines.label()),
            format!("detect-v{}", self.detect_version),
            format!("detector-tuning={}", self.detector_tuning),
        ];
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

/// The two key fingerprints of one engine set in this process.
#[derive(Debug, Clone)]
pub struct Fingerprints {
    pub result: String,
    pub detect: String,
}

fn fingerprint_slot(engines: EngineSet) -> &'static std::sync::OnceLock<Fingerprints> {
    static CURRENT: std::sync::OnceLock<Fingerprints> = std::sync::OnceLock::new();
    static LEGACY: std::sync::OnceLock<Fingerprints> = std::sync::OnceLock::new();
    match engines {
        EngineSet::Current => &CURRENT,
        EngineSet::Legacy => &LEGACY,
    }
}

/// [`fingerprints_for`] the default engine set ([`EngineSet::Current`]).
pub fn fingerprints() -> Fingerprints {
    fingerprints_for(EngineSet::Current)
}

/// This process's fingerprints for `engines`. The first complete
/// computation is kept for the life of the process. For the current set
/// that is the first one (model pins are constants). For the legacy set
/// it hashes the Tesseract language data (blocking, tens of ms), so the
/// handlers call [`warm_fingerprints_for`] first to do that off the
/// async runtime; while the language data is missing or unreadable the
/// result is *not* kept: it is computed again on every call (a failed
/// `open`, cheap), so data staged later is picked up without a restart.
pub fn fingerprints_for(engines: EngineSet) -> Fingerprints {
    let slot = fingerprint_slot(engines);
    if let Some(fp) = slot.get() {
        return fp.clone();
    }
    let inputs = EngineInputs::current(engines);
    let fp = Fingerprints {
        result: inputs.result_fingerprint(),
        detect: inputs.detect_fingerprint(),
    };
    if engines == EngineSet::Current || western_identity_complete(&inputs.western) {
        let kept = slot.get_or_init(|| {
            tracing::info!(
                engines = engines.label(),
                result = %fp.result,
                detect = %fp.detect,
                western = %inputs.western,
                "ocr cache: engine fingerprints"
            );
            fp
        });
        return kept.clone();
    }
    tracing::debug!(western = %inputs.western, "ocr cache: Tesseract language data not readable yet");
    fp
}

/// `false` when the language data could not be hashed (see
/// [`super::recognizer::western::engine_identity`]).
fn western_identity_complete(western: &str) -> bool {
    !["=missing", "=unreadable", "=no-tessdata-dir"]
        .iter()
        .any(|m| western.contains(m))
}

/// [`warm_fingerprints_for`] the default engine set.
pub async fn warm_fingerprints() {
    warm_fingerprints_for(EngineSet::Current).await;
}

/// Compute [`fingerprints_for`] `engines` on the blocking pool if none is
/// kept yet.
pub async fn warm_fingerprints_for(engines: EngineSet) {
    if fingerprint_slot(engines).get().is_none() {
        let _ = tokio::task::spawn_blocking(move || fingerprints_for(engines)).await;
    }
}

/// Longest a cache read may take. The connection's own response timeout
/// (500 ms, the redis crate's default, which `apalis_redis::connect`
/// keeps) already bounds a command; this is the OCR code's own bound,
/// so a hung Redis can never hold a permit or a request for longer,
/// whatever the connection is configured with.
pub const CACHE_READ_TIMEOUT: Duration = Duration::from_secs(2);

/// `GET key`, bounded by [`CACHE_READ_TIMEOUT`]. `None` on a miss, an
/// error or a timeout (fail-open); a timeout counts
/// `folio_ocr_cache_read_timeouts_total` and logs at warn.
async fn get_bounded(redis: &ConnectionManager, key: &str, what: &str) -> Option<String> {
    let mut conn = redis.clone();
    match tokio::time::timeout(CACHE_READ_TIMEOUT, conn.get::<_, Option<String>>(key)).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, key, "{what}: GET failed; treating as miss");
            None
        }
        Err(_) => {
            metrics::counter!("folio_ocr_cache_read_timeouts_total").increment(1);
            tracing::warn!(key, "{what}: GET timed out; treating as miss");
            None
        }
    }
}

/// [`cache_key_for`] the default engine set ([`EngineSet::Current`]):
/// the key a server without the legacy switch uses. Public so tests can
/// pre-seed entries without going through the full handler path.
pub fn cache_key(
    content_hash: &str,
    page: u32,
    lang: &str,
    detect: bool,
    region_hash: &str,
) -> String {
    cache_key_for(
        EngineSet::Current,
        content_hash,
        page,
        lang,
        detect,
        region_hash,
    )
}

/// Build the Redis key for one result-cache entry of an engine set.
///
/// `detect` distinguishes snap-to-bubble results (`true` → `d`) from
/// raw-rect results (`false` → `r`). The recognized text differs
/// between the two because the detector reframes the user's region;
/// mixing them in the cache would serve stale answers when an
/// operator (or future per-call override) flips detection state.
///
/// The segment after the version is the engine fingerprint
/// ([`Fingerprints::result`]), which differs between the engine sets.
pub fn cache_key_for(
    engines: EngineSet,
    content_hash: &str,
    page: u32,
    lang: &str,
    detect: bool,
    region_hash: &str,
) -> String {
    result_key_with(
        &fingerprints_for(engines).result,
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
    let Some(raw) = get_bounded(redis, key, "ocr cache").await else {
        metrics::counter!("folio_ocr_cache_misses_total").increment(1);
        return None;
    };
    match serde_json::from_str::<OcrResponse>(&raw) {
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
// Result-cache hits skip the detector but require the *same* region.
// Caching the detector output per `(content_hash, page)` lets reads of
// *different* regions of the same page skip the detector too, and lets
// the text-regions route answer without touching the archive. (With the
// legacy detector, tens of seconds a page, this cache is what made the
// feature usable at all.)

/// One detected box in page pixels, as stored in Redis under the
/// per-page detect cache. `class` is the detector's own class index:
/// 0 (balloon), 1 (the text inside a balloon) or 2 (free text) from the
/// current detector (`ocr::bubbles`), 0 (block) or 1 (line) from the
/// legacy one. Which classes are regions for clients is
/// [`EngineSet::is_region_class`].
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

/// Version byte in the detect-cache key. v3 = the boxes are those of the
/// balloon detector, all three classes (the legacy detector's boxes sit
/// under their own fingerprint); v2 = payload grew page dimensions
/// ([`CachedDetection`]); v1 stored a bare bbox array. Old-version keys
/// age out via [`CACHE_TTL`].
pub const OCR_DETECT_VERSION: u32 = 3;

/// [`detect_cache_key_for`] the default engine set
/// ([`EngineSet::Current`]).
pub fn detect_cache_key(content_hash: &str, page: u32) -> String {
    detect_cache_key_for(EngineSet::Current, content_hash, page)
}

/// Redis key for the per-page detector output cache of an engine set.
/// The segment after the version is the detector fingerprint
/// ([`Fingerprints::detect`]).
pub fn detect_cache_key_for(engines: EngineSet, content_hash: &str, page: u32) -> String {
    detect_key_with(&fingerprints_for(engines).detect, content_hash, page)
}

fn detect_key_with(fingerprint: &str, content_hash: &str, page: u32) -> String {
    format!("ocr:detect:v{OCR_DETECT_VERSION}:{fingerprint}:{content_hash}:{page}")
}

/// [`get`] for a result that must have been computed on a page of
/// `size` (`page_w`, `page_h`). A payload of another size is a miss (and
/// counted as one): a percent region must never be answered with a
/// result from a page decoded to a different size.
pub async fn get_sized(
    redis: &ConnectionManager,
    key: &str,
    size: (u32, u32),
) -> Option<OcrResponse> {
    let Some(raw) = get_bounded(redis, key, "ocr cache").await else {
        metrics::counter!("folio_ocr_cache_misses_total").increment(1);
        return None;
    };
    match serde_json::from_str::<OcrResponse>(&raw) {
        Ok(resp) if (resp.page_w, resp.page_h) == size => {
            metrics::counter!("folio_ocr_cache_hits_total").increment(1);
            Some(resp)
        }
        Ok(resp) => {
            metrics::counter!("folio_ocr_cache_misses_total").increment(1);
            tracing::debug!(
                key,
                cached_w = resp.page_w,
                cached_h = resp.page_h,
                page_w = size.0,
                page_h = size.1,
                "ocr cache: payload for another page size; treating as miss"
            );
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, key, "ocr cache: malformed payload; treating as miss");
            None
        }
    }
}

/// Look up cached detector output for a page. `Some` on hit, `None`
/// on miss or Redis error (fail-open).
pub async fn get_detect(redis: &ConnectionManager, key: &str) -> Option<CachedDetection> {
    let Some(raw) = get_bounded(redis, key, "ocr detect cache").await else {
        metrics::counter!("folio_ocr_detect_cache_misses_total").increment(1);
        return None;
    };
    match serde_json::from_str::<CachedDetection>(&raw) {
        Ok(detection) => {
            metrics::counter!("folio_ocr_detect_cache_hits_total").increment(1);
            Some(detection)
        }
        Err(e) => {
            tracing::warn!(error = %e, key, "ocr detect cache: malformed payload; treating as miss");
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
            engines: EngineSet::Legacy,
            manga_crop: "region",
            detector_tuning: "detector:input=640;score=0.5".into(),
            reader_tuning: "lines:scale=2".into(),
            result_version: 3,
            detect_version: 2,
            detector_pins: vec!["acme/det@aaa/det.onnx#111".into()],
            manga_pins: vec![
                "acme/manga@bbb/enc.onnx#222".into(),
                "acme/manga@bbb/vocab.txt#333".into(),
            ],
            western: "tesseract=5.5.0;eng.traineddata=abc".into(),
            page_size: "0011223344556677".into(),
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
                "engine set",
                EngineInputs {
                    engines: EngineSet::Current,
                    ..inputs()
                },
            ),
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
                "page size",
                EngineInputs {
                    page_size: "7766554433221100".into(),
                    ..inputs()
                },
            ),
            (
                "japanese crop",
                EngineInputs {
                    manga_crop: "text-in-region",
                    ..inputs()
                },
            ),
            (
                "a detector threshold",
                EngineInputs {
                    detector_tuning: "detector:input=640;score=0.6".into(),
                    ..inputs()
                },
            ),
            (
                "a line finder or recogniser constant",
                EngineInputs {
                    reader_tuning: "lines:scale=3".into(),
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
                "engine set",
                EngineInputs {
                    engines: EngineSet::Current,
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
                "detector digest",
                EngineInputs {
                    detector_pins: vec!["acme/det@aaa/det.onnx#999".into()],
                    ..inputs()
                },
            ),
            (
                "a detector threshold",
                EngineInputs {
                    detector_tuning: "detector:input=640;score=0.6".into(),
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
            (
                "japanese crop",
                EngineInputs {
                    manga_crop: "text-in-region",
                    ..inputs()
                },
            ),
            (
                "a line finder or recogniser constant",
                EngineInputs {
                    reader_tuning: "lines:scale=3".into(),
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
    fn the_legacy_fingerprint_names_tesseract_and_every_pin() {
        let legacy = EngineInputs::current(EngineSet::Legacy);
        assert!(
            legacy.western.starts_with("tesseract="),
            "{}",
            legacy.western
        );
        assert!(
            legacy.western.contains("eng.traineddata="),
            "{}",
            legacy.western
        );
        assert_eq!(
            legacy.detector_pins,
            pins(super::super::models::DETECTOR_FILES)
        );
        assert_eq!(
            legacy.manga_pins.len(),
            super::super::models::MANGA_FILES.len()
        );
    }

    /// The current set's fingerprint inputs name every file its engines
    /// load: the balloon detector, the line finder, the line recogniser
    /// with its dictionary, and manga-ocr.
    #[test]
    fn the_current_fingerprint_covers_every_model_file() {
        use super::super::models;
        let current = EngineInputs::current(EngineSet::Current);
        assert_eq!(current.engines, EngineSet::Current);
        assert_eq!(current.result_version, OCR_RESULT_VERSION);
        assert_eq!(current.detect_version, OCR_DETECT_VERSION);
        assert_eq!(current.detector_pins, pins(models::BUBBLE_DETECTOR_FILES));
        assert_eq!(current.manga_pins, pins(models::MANGA_FILES));
        assert_eq!(
            current.manga_crop,
            crate::ocr::manga_crop::MANGA_CROP.label()
        );
        // The constants that change output are named, value and all.
        use crate::ocr::{bubbles, lines, recognizer::ctc, regions};
        for part in [
            format!("input={}", bubbles::INPUT_SIZE),
            format!("score={}", bubbles::SCORE_THRESHOLD),
        ] {
            assert!(current.detector_tuning.contains(&part), "{part}");
        }
        for part in [
            format!("scale={}", lines::LINE_SCALE),
            format!("long={}", lines::LINE_MAX_LONG_SIDE),
            format!("thresh={}", lines::DB_THRESH),
            format!("box={}", lines::DB_BOX_THRESH),
            format!("unclip={}", lines::DB_UNCLIP_RATIO),
            format!("max-lines={}", lines::MAX_LINES_PER_REGION),
            format!("height={}", ctc::REC_HEIGHT),
            format!("min={}", ctc::REC_MIN_WIDTH),
            format!("max={}", ctc::REC_MAX_WIDTH),
            format!("columns={}", ctc::REC_MAX_BATCH_COLUMNS),
            format!("inside={}", crate::ocr::manga_crop::TEXT_INSIDE_MIN),
            format!("stray={}", crate::ocr::manga_crop::TEXT_STRAY_MAX),
            format!("iou={}", regions::NEAR_DUPLICATE_IOU),
        ] {
            assert!(current.reader_tuning.contains(&part), "{part}");
        }
        // The legacy set carries none of them.
        let legacy = EngineInputs::current(EngineSet::Legacy);
        assert!(legacy.detector_tuning.is_empty() && legacy.reader_tuning.is_empty());
        assert!(
            !current.western.contains("tesseract"),
            "{}",
            current.western
        );
        for f in models::LINE_FINDER_FILES
            .iter()
            .chain(models::WESTERN_REC.files)
        {
            for part in [f.repo, f.revision, f.file, f.sha256] {
                assert!(
                    current.western.contains(part),
                    "{part}: {}",
                    current.western
                );
            }
        }
        assert!(current.western.contains(models::WESTERN_REC.id));
        // A pin that moves changes the keys.
        let moved = EngineInputs {
            western: current
                .western
                .replace(models::WESTERN_REC.files[0].sha256, "0"),
            ..current.clone()
        };
        assert_ne!(moved.result_fingerprint(), current.result_fingerprint());
        assert_eq!(moved.detect_fingerprint(), current.detect_fingerprint());
    }

    /// The two engine sets never share a result or a detection: their
    /// fingerprints, and so their keys, differ.
    #[test]
    fn the_engine_sets_have_different_keys() {
        let current = fingerprints_for(EngineSet::Current);
        let legacy = fingerprints_for(EngineSet::Legacy);
        assert!(is_fingerprint(&current.result) && is_fingerprint(&current.detect));
        assert_ne!(current.result, legacy.result);
        assert_ne!(current.detect, legacy.detect);
        assert_ne!(
            cache_key_for(EngineSet::Current, "h", 0, "western", false, "rh"),
            cache_key_for(EngineSet::Legacy, "h", 0, "western", false, "rh")
        );
        assert_ne!(
            detect_cache_key_for(EngineSet::Current, "h", 0),
            detect_cache_key_for(EngineSet::Legacy, "h", 0)
        );
        // The shorthand functions are the default (current) set's keys.
        assert_eq!(
            cache_key("h", 0, "western", false, "rh"),
            cache_key_for(EngineSet::Current, "h", 0, "western", false, "rh")
        );
        assert_eq!(
            detect_cache_key("h", 0),
            detect_cache_key_for(EngineSet::Current, "h", 0)
        );
        assert_eq!(fingerprints().result, current.result);
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

// ───────── decoded page size ─────────
//
// A `region_pct` request needs the decoded page size to find its pixel
// rectangle, and with it its result-cache key. Storing the size of every
// page the OCR routes decode lets a repeat request answer from the result
// cache before the models, the permit and the decode, like a pixel
// request. The decode is the authority: the handler compares the stored
// size with every decode it does and overwrites it (with a warning) when
// they differ.

/// Version of the page-size entry. Bump when the decode path changes
/// what size it produces for the same bytes (orientation handling, the
/// decoder); the decode limits are folded in by [`page_size_key`].
pub const OCR_PAGE_SIZE_VERSION: u32 = 1;

/// Expiry of a page-size entry: two integers, kept for 30 days.
pub const PAGE_SIZE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Hash of what decides a decoded page's size besides its bytes: the
/// entry version and the decode limits (a page past a limit does not
/// decode at all).
fn page_size_fingerprint() -> &'static str {
    static FP: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        short_hash(&[
            format!("page-size-v{OCR_PAGE_SIZE_VERSION}"),
            format!(
                "max-dim={}",
                crate::util::image_decode::MAX_DECODE_DIMENSION
            ),
            format!(
                "max-alloc={}",
                crate::util::image_decode::MAX_DECODE_ALLOC_BYTES
            ),
        ])
    });
    &FP
}

/// Redis key of the decoded size of one page of one file version.
pub fn page_size_key(content_hash: &str, page: u32) -> String {
    format!(
        "ocr:pagesize:v{OCR_PAGE_SIZE_VERSION}:{}:{content_hash}:{page}",
        page_size_fingerprint()
    )
}

/// The stored decoded size `(width, height)`, or `None` on a miss, a
/// malformed value or a Redis error (fail-open).
pub async fn get_page_size(redis: &ConnectionManager, key: &str) -> Option<(u32, u32)> {
    let raw = get_bounded(redis, key, "ocr page size").await?;
    let (w, h) = raw.split_once('x')?;
    match (w.parse::<u32>(), h.parse::<u32>()) {
        (Ok(w), Ok(h)) if w > 0 && h > 0 => Some((w, h)),
        _ => None,
    }
}

/// Store a decoded size (`"{w}x{h}"`). Bounded and fail-open like the
/// other cache writes.
pub async fn put_page_size(redis: &ConnectionManager, key: &str, (w, h): (u32, u32)) {
    set_bounded_ttl(
        redis,
        key,
        format!("{w}x{h}"),
        PAGE_SIZE_TTL,
        "ocr page size",
    )
    .await;
}

#[cfg(test)]
mod page_size_tests {
    use super::*;

    #[test]
    fn page_size_key_shape() {
        let k = page_size_key("hashA", 7);
        let parts: Vec<&str> = k.split(':').collect();
        assert_eq!(
            parts[..3],
            ["ocr", "pagesize", &format!("v{OCR_PAGE_SIZE_VERSION}")]
        );
        assert_eq!(parts[3].len(), FINGERPRINT_HEX_LEN);
        assert_eq!(parts[4..], ["hashA", "7"]);
        assert_ne!(page_size_key("hashA", 7), page_size_key("hashB", 7));
        assert_ne!(page_size_key("hashA", 7), page_size_key("hashA", 8));
    }
}

#[cfg(test)]
mod fingerprint_memo_tests {
    use super::*;

    /// Only a fingerprint with hashed language data is kept for the life
    /// of the process; one computed while the data is missing is not.
    #[test]
    fn only_a_complete_western_identity_is_kept() {
        assert!(western_identity_complete(
            "tesseract=5.5.0;eng.traineddata=0123abcd"
        ));
        for incomplete in [
            "tesseract=5.5.0;eng.traineddata=missing",
            "tesseract=5.5.0;eng.traineddata=unreadable:PermissionDenied",
            "tesseract=5.5.0;eng.traineddata=no-tessdata-dir",
        ] {
            assert!(!western_identity_complete(incomplete), "{incomplete}");
        }
    }
}
