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

Two Redis-backed layers, both fail-open on every operation.

### Result cache

Stores the final recognized text per region.

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

Hit → short-circuit before page load + decode. Miss → run the
full pipeline and write the result.

### Detector-result cache (M4 follow-up, v0.3.25)

Stores the detector's polygon list per page so re-OCRs on *different*
regions of the same page skip the expensive detector stage. The
detector is by far the heaviest part of the pipeline (~3 s on a fast
CPU, much more on constrained hosts) — caching its output is what
makes "OCR every bubble on this page" feasible.

**Key**: `ocr:detect:{content_hash}:{page}`

**Value**: JSON array of `{xmin, ymin, xmax, ymax, confidence, class}`
in page-pixel coordinates.

Flow per OCR call:

1. Result-cache lookup. Hit → return.
2. Decode the page.
3. Detect-cache lookup. Hit → use cached polygons.
4. Miss → run detector on the **full page**, cache the polygons.
5. Pick the polygon whose intersection with the user's rect is
   largest. Fall back to the user's rect verbatim if none overlap.
6. Recognize the chosen crop.
7. Write the result cache.

Pre-v0.3.25, step 3-4 ran the detector over a crop around the user's
rect, not the full page. The model resizes its input to 1024×1024
internally either way, so per-call inference cost is unchanged — but
moving to full-page detection lets two OCR calls on different bubbles
of the same page share the work.

**TTL**: 7 days for both caches. OCR + detection are deterministic
per `(content_hash, …)` but a finite TTL caps Redis size.

**Metrics**:

- `comic_ocr_cache_hits_total` / `comic_ocr_cache_misses_total`
  — counter (result cache).
- `comic_ocr_detect_cache_hits_total` /
  `comic_ocr_detect_cache_misses_total` — counter (detector cache).

## OpenMP threads (`OMP_NUM_THREADS`)

The ort/onnxruntime build the upstream `comic-text-detector` crate
uses is compiled with OpenMP. Per the ort docs,
`Session::with_intra_threads()` is **a no-op** with OpenMP builds —
the only knob that matters is the `OMP_NUM_THREADS` env var.

OpenMP doesn't read cgroup CPU quotas. Left to its own devices it
sees host cores, which in a 4-CPU LXC running on a 16-core host means
it spawns 16 threads that all fight for 4 cores. The result is severe
thrashing — observed ~48 s detector inference on a system that should
run in ~3 s. **This is by far the biggest perf gotcha in the
pipeline.**

Folio's `main.rs` auto-tunes this at process start: if `OMP_NUM_THREADS`
isn't already set, we call `std::thread::available_parallelism()`
(which *does* respect cgroups on Linux) and set the env var to that
value, clamped to 8 to avoid diminishing returns on fat boxes.

Operator override: set `OMP_NUM_THREADS=N` in compose / env-file to
pin the count. Lower it if the OCR endpoint is starving other
workers; raise it if you have idle cores.

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
- `HOME=/data` so the detector + manga-ocr ONNX caches land under
  `/data/.cache/huggingface/hub/` (in the persistent volume) and
  survive container restarts. `HF_HOME=/data/.cache/huggingface` is
  set in parallel — see the gotcha below.
- Both env vars are operator-overridable for air-gapped deploys; the
  loader exactly mirrors what `GET /admin/ocr/models` reports.

### Upstream cache-resolution gotcha

`comic-text-detector` 0.5.1 calls `hf_hub::api::sync::Api::new()`,
which internally uses `Cache::default()`. **`Cache::default()`
ignores `HF_HOME`** — only `Api::from_env()` / `Cache::from_env()`
honor it. The cache path the crate actually writes to is
`dirs::home_dir() + .cache/huggingface/hub/`, i.e. driven by `HOME`,
not `HF_HOME`.

Folio's `Dockerfile` sets `HOME=/data` so this works out cleanly:
the crate resolves to `/data/.cache/...` and the admin endpoint
reports the same. If upstream bumps to a version that fixes this
precedence we can drop the `HOME` override; `HF_HOME` is kept for
forward-compatibility.

Local-dev builds without `HOME` set will land the cache at
`~/.cache/huggingface/hub/` (the user's real home) — that's fine,
just don't be surprised when `du -sh ~/.cache/huggingface` grows by
a few hundred MB the first time you run an OCR test.

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
