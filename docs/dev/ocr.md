# OCR pipeline

Server-side text recognition for reader markers (text-detection-1.0
plan). Replaces the pre-M6 tesseract.js-in-the-browser path with a
detector + recognizer pipeline that ships with the Rust binary.

## Endpoint

`POST /api/me/issues/{id}/ocr`

```json
// Request
{
  "page": 0,
  "region": { "x": 250, "y": 300, "w": 100, "h": 80 },
  "lang": "western"
}
// Response (200)
{
  "text": "POW!",
  "confidence": 0.91,
  "refined_bbox": { "x": 248, "y": 302, "w": 106, "h": 76 }
}
```

- `region` is integer-pixel against the decoded page, not the
  archive's claimed dimensions — the handler decodes the page and
  uses the actual `(width, height)` for the bounds check.
- `lang` is `"western"` (default) or `"manga"`. Phase 2 will read
  `series.text_language`.
- `refined_bbox` is the detector's snap-to-bubble rect. Absent when
  no detector hit overlapped the user's region — the recognizer ran
  on the user's rect verbatim.

Error codes (envelope `{ "error": { "code", "message" } }`):

| Status | code | When |
|---|---|---|
| 400 | `invalid_region` | `w`/`h` is 0 OR rect extends outside the decoded page |
| 400 | `invalid_lang` | `lang` not in `western`/`manga` |
| 401 / 403 | (CSRF / auth) | Standard CSRF + session guards |
| 404 | `not_found` | Issue missing or user lacks library access |
| 404 | `not_found` | `page` index out of range for the archive |
| 415 | `decode_failed` | Page entry isn't a decodable image |
| 429 | `rate_limited` | OCR bucket (60/min/IP, burst 60) tripped |
| 500 | `archive_unreadable` | `zip_lru.get_or_open` failed |
| 500 | `ocr_failed` | Detector or recognizer threw |

## Pipeline stages

```text
client region (pct → px)
   │
   ▼
ACL + page decode  ── 404 / 415 fall through here
   │
   ▼
Redis cache lookup ── HIT? short-circuits to 200
   │
   ▼ miss
detector (comic-text-detector)
   │
   ▼
snap to bubble polygon (largest overlap with user rect)
   │
   ▼
crop (or fall back to the user's rect if no hit)
   │
   ▼
recognizer (Western: Tesseract LSTM ; Manga: manga-ocr)
   │
   ▼
Redis cache PUT + 200
```

Code map:

- [`crates/server/src/api/issue_ocr.rs`](../../crates/server/src/api/issue_ocr.rs)
  — request shape, ACL, decode, cache plumbing.
- [`crates/server/src/ocr/pipeline.rs`](../../crates/server/src/ocr/pipeline.rs)
  — `detect → snap → crop → recognize`, all inside one
  `spawn_blocking` so the reactor isn't stalled.
- [`crates/server/src/ocr/detector.rs`](../../crates/server/src/ocr/detector.rs)
  — `comic-text-detector` singleton (`tokio::sync::OnceCell` +
  `std::sync::Mutex<ComicTextDetector>`).
- [`crates/server/src/ocr/recognizer/western.rs`](../../crates/server/src/ocr/recognizer/western.rs)
  — Tesseract via `tesseract-rs`, English-only (`tessdata_best/eng`).
- [`crates/server/src/ocr/recognizer/manga.rs`](../../crates/server/src/ocr/recognizer/manga.rs)
  — `manga-ocr` (encoder + decoder + vocab ONNX, greedy decode).
- [`crates/server/src/ocr/cache.rs`](../../crates/server/src/ocr/cache.rs)
  — Redis-backed result cache.
- [`crates/server/src/api/admin_ocr.rs`](../../crates/server/src/api/admin_ocr.rs)
  — `GET /admin/ocr/models` reflection-on-disk endpoint.

## Cache

Redis-backed; fail-open on every operation.

**Key**: `ocr:cache:{content_hash}:{page}:{lang}:{region_hash}`

- `content_hash` (the issue's mutable BLAKE3 of on-disk bytes, not
  the stable `issue.id`) makes invalidation automatic — a rescan
  that retags the row rolls the key, the old entries age out via
  TTL, and the next request re-OCRs. As a bonus, two deduplicated
  issues with identical bytes share cache entries.
- `region_hash` = BLAKE3 of the integer-pixel rect (`x|y|w|h` LE
  bytes), hex.
- `lang` is part of the key so the manga and western recognizers
  cache independently.

**TTL**: 7 days. OCR is deterministic per
`(content_hash, region, lang)` but a finite TTL caps Redis size in
deploys that occasionally re-render with different region guesses.

**Metrics**:

- `comic_ocr_cache_hits_total` / `comic_ocr_cache_misses_total` —
  counter.

## Rate limit

Per-IP token bucket via `tower_governor`. Bucket: `OCR` =
60 tokens / minute, burst 60. Lives in
[`crates/server/src/middleware/rate_limit.rs`](../../crates/server/src/middleware/rate_limit.rs).
Reader users won't notice — the bucket caps a runaway script, not
manual bubble exploration. Denied requests increment
`comic_rate_limit_denied_total{bucket="ocr"}`.

## Telemetry

| Metric | Type | Labels | Purpose |
|---|---|---|---|
| `comic_ocr_cache_hits_total` | counter | — | Cache effectiveness |
| `comic_ocr_cache_misses_total` | counter | — | Pair with hits for hit-rate |
| `comic_ocr_pipeline_seconds` | histogram | `lang` | Wall time, detector + recognize (recorded only on success path) |
| `comic_ocr_recognize_seconds` | histogram | `lang` | Recognize-only — diff with `pipeline_seconds` exposes detector cost |
| `comic_rate_limit_denied_total{bucket="ocr"}` | counter | `bucket` | OCR bucket denials |

All visible at `/metrics`. Operator dashboards reading these should
key on `lang` to spot manga-OCR-bound vs western-OCR-bound boxes.

## Models — auto-download + on-disk inspection

The three model artifacts the pipeline depends on:

| Component | Where | Auto-download source | Approx size |
|---|---|---|---|
| `comic-text-detector` ONNX | `${HF_HOME}/hub/models--mayocream--comic-text-detector-onnx/` | `mayocream/comic-text-detector-onnx` | ~95 MB |
| `manga-ocr` ONNX (encoder + decoder + vocab) | `${HF_HOME}/hub/models--mayocream--manga-ocr-onnx/` | `mayocream/manga-ocr-onnx` | ~250 MB |
| Tesseract LSTM (`eng.traineddata`) | `${TESSDATA_PREFIX}` (default `${HOME}/.tesseract-rs/tessdata`) | tessdata_best (build script) | ~15 MB |

`HF_HOME` defaults to `${HOME}/.cache/huggingface`. The first
request that touches a model triggers download + ~1–2 s session
build for ONNX, or a one-time cmake build for Tesseract (cached at
`~/.tesseract-rs/`).

In the Folio Docker image:

- `eng.traineddata` is baked into the image at `/app/tessdata/` (the
  rust-builder stage compiles Tesseract from source and downloads
  tessdata; both are copied into the runtime image). `TESSDATA_PREFIX`
  defaults to that path.
- `HF_HOME` defaults to `/data/.cache/huggingface` so the detector +
  manga-ocr downloads land in the operator's persistent volume and
  survive container restarts.
- Both env vars are operator-overridable for air-gapped deploys; the
  loader exactly mirrors what `GET /admin/ocr/models` reports.

## Admin surfaces

- `GET /admin/ocr/models` — JSON view of cache state. Read-only;
  `RequireAdmin` extractor.
- `/admin/server` UI tile — pairs the JSON with the build /
  dependencies cards so an operator can verify both binary and
  models are healthy in one place. Lives in
  [`web/components/admin/observability/ServerInfoClient.tsx`](../../web/components/admin/observability/ServerInfoClient.tsx).

## Web client

[`web/app/[locale]/read/[seriesSlug]/[issueSlug]/marker-selection.ts`](../../web/app/%5Blocale%5D/read/%5BseriesSlug%5D/%5BissueSlug%5D/marker-selection.ts)
owns `ocrCroppedRegion(...)`. The function:

1. Maps `MarkerRegion` (0–100% floats) to integer pixel rect
   against `naturalSize`, clamping for rounding overshoot.
2. POSTs via `apiFetch` with the user's CSRF token.
3. Returns `{ text, confidence } | null`. `null` covers every
   not-the-happy-path: non-2xx, network error, malformed JSON, or
   empty text. The caller (`MarkerEditor` /
   `MarkerOverlay`) treats `null` as "couldn't read text" and falls
   back to a plain highlight.

The image-hash path (`sha256CroppedRegion`) is unchanged — it still
crops + hashes in the browser because the server has no equivalent
endpoint.

## Test layout

- Server validation / ACL / archive / decode / region bounds /
  pipeline-reach: [`crates/server/tests/issue_ocr.rs`](../../crates/server/tests/issue_ocr.rs).
- Cache hit / miss / lang-scoped key:
  [`crates/server/tests/issue_ocr.rs`](../../crates/server/tests/issue_ocr.rs)
  (M4 section).
- Admin endpoint + RequireAdmin:
  [`crates/server/tests/admin_ocr.rs`](../../crates/server/tests/admin_ocr.rs).
- Recognizer trait + heavy E2E (ignored by default):
  [`crates/server/tests/ocr_recognizer.rs`](../../crates/server/tests/ocr_recognizer.rs).
  Run with `cargo test -p server --test ocr_recognizer -- --ignored`.
- Web client fetch shape + fallback paths:
  [`web/tests/reader/marker-selection.test.ts`](../../web/tests/reader/marker-selection.test.ts).

## Future work (Phase 2+)

- `series.text_language` column + per-library default. Replaces the
  hard-coded `lang: "western"` in the client.
- PaddleOCR swap behind the existing `Recognizer` trait — better
  Western quality than `tessdata_best`.
- Page-level text-region indexing (apalis job): pre-detect on scan
  so the reader can hover-to-highlight bubbles without re-running
  the detector. See plan §"Phase 3" in
  [`~/.claude/plans/text-detection-1.0.md`](../../../.claude/plans/text-detection-1.0.md).
