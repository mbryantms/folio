# OCR pipeline

Server-side text recognition for reader markers (text-detection-1.0
plan, reworked by OCR rework 1.0). Replaces the pre-M6
tesseract.js-in-the-browser path with a detector + recognizer +
postprocess pipeline that ships with the Rust binary.

## Endpoints

### `POST /me/issues/{id}/ocr`

```json
// Request
{
  "page": 0,
  "region": { "x": 250, "y": 300, "w": 100, "h": 80 },
  "lang": "western",
  "detect": false
}
// Response (200)
{
  "text": "POW!",
  "confidence": 0.91,
  "refined_bbox": { "x": 248, "y": 302, "w": 106, "h": 76 },
  "lang": "western",
  "raw_text": "~POW!|",
  "words": [{ "text": "POW!", "confidence": 0.93, "x": 252, "y": 305, "w": 90, "h": 60 }]
}
```

- `region` is integer-pixel against the decoded page, not the
  archive's claimed dimensions — the handler decodes the page and
  uses the actual `(width, height)` for the bounds check.
- `lang` is `"western"` or `"manga"`. **Omitted → server-resolved**:
  `series.text_language` if set, else `manga` when
  `series.reading_direction == "rtl"`, else `western`. The response
  echoes the resolved value in `lang`, and the resolved value feeds
  the cache key.
- `detect` is the **snap-to-bubble opt-in** (v0.3.26). When `true`
  the pipeline runs `comic-text-detector` over the page to refine
  the user's rect to the tightest bubble polygon. When `false` or
  omitted (the default) the recognizer runs on the user's rect
  verbatim. The detector's first call on a fresh page costs ~50 s
  on a typical CPU-bound container; subsequent calls on the same
  page hit the polygon cache (~200 ms). The reader sends
  `detect: true` from the drag path only when the page's
  text-regions fetch already warmed the cache.
- `refined_bbox` is the detector's snap-to-bubble rect. `None`
  when `detect: false`, when no bbox overlapped the user's region,
  or when no detector was run.
- `raw_text` is the engine output before the postprocess cleanup;
  present only when cleanup changed the text. Use it to author
  golden fixtures from user reports.
- `words` is the per-word breakdown (page-pixel boxes, post-cleanup)
  — western only; manga-ocr exposes no word data.

### `GET /me/issues/{id}/pages/{page}/text-regions`

Full-page detector output in **percent** coordinates — drives the
reader's tappable bubble outlines in text-capture mode.

```json
// Response (200)
{
  "page_w": 1988,
  "page_h": 3056,
  "regions": [
    { "x": 12.4, "y": 8.1, "w": 14.2, "h": 9.7, "confidence": 0.93, "class": 0 }
  ]
}
```

Shares the per-page detect cache with the OCR POST: one regions
fetch makes every subsequent bubble OCR on that page
recognize-only (~200 ms–2 s). Cache hit = Redis round-trip, no
archive touch; miss = page decode + detector inference (seconds to
tens of seconds — see the `OMP_NUM_THREADS` section), which is why
the endpoint sits on its own stricter rate bucket.

Error codes (envelope `{ "error": { "code", "message" } }`):

| Status | code | When |
|---|---|---|
| 422 | `invalid_region` | `w`/`h` is 0 OR rect extends outside the decoded page (POST only) |
| 422 | `invalid_lang` | `lang` not in `western`/`manga` (POST only) |
| 401 / 403 | (CSRF / auth) | Standard CSRF + session guards |
| 404 | `not_found` | Issue missing or user lacks library access |
| 404 | `not_found` | `page` index out of range for the archive |
| 415 | `decode_failed` | Page entry isn't a decodable image |
| 429 | `rate_limited` | `ocr` bucket (60/min/IP, burst 60) on the POST; `ocr.detect` (15/min/IP, burst 10) on the GET |
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
postprocess (junk-token + confidence cleanup)
   │
   ▼
Redis cache PUT + 200
```

### Western preprocessing (inside the recognizer)

Each crop goes through: grayscale → 3× Lanczos3 upscale → **Otsu**
binarization (midpoint fallback on near-uniform crops) →
**polarity detection** (majority-black border ring ⇒ white-on-black
caption ⇒ invert) → **24 px white border pad** (Tesseract
mis-segments glyphs touching the canvas edge) → PSM 6 + pinned
300 DPI. `tessedit_char_blacklist` is deliberately not used: the
LSTM *substitutes* blacklisted chars instead of omitting them.

### Postprocess

[`crates/server/src/ocr/postprocess.rs`](../../crates/server/src/ocr/postprocess.rs)
is the single deterministic cleanup pass — this is what keeps
bubble borders/tails/halftone dots from coming back as stray
`| ~ ' _` symbols. Western rules in order: per-word confidence
drop (W1) → hyphenated line-break join (W2) → symbol-only token
drop sparing expressive punctuation like `!?`/`...` (W3) → edge
junk strip (W4) → per-char allowlist (W5) → whitespace collapse
(W6) → empty-out guard (W7) → confidence recompute over kept words
(W8). The manga path only strips control/zero-width/replacement
chars — Japanese punctuation is all legitimate output.

Thresholds/charsets are documented consts in that file (matcher-
style: consts + tests now, settings-registry promotion only on
operator demand). **Changing anything in preprocessing or
postprocess requires bumping `OCR_RESULT_VERSION`** in `cache.rs`.

Code map:

- [`crates/server/src/api/issue_ocr.rs`](../../crates/server/src/api/issue_ocr.rs)
  — request shapes, ACL, decode, cache plumbing, lang resolution,
  text-regions endpoint.
- [`crates/server/src/ocr/pipeline.rs`](../../crates/server/src/ocr/pipeline.rs)
  — `detect → snap → crop → recognize → clean`, all inside one
  `spawn_blocking` so the reactor isn't stalled; also
  `detect_and_cache_regions` for the text-regions endpoint.
- [`crates/server/src/ocr/detector.rs`](../../crates/server/src/ocr/detector.rs)
  — `comic-text-detector` singleton (`tokio::sync::OnceCell` +
  `std::sync::Mutex<ComicTextDetector>`).
- [`crates/server/src/ocr/recognizer/western.rs`](../../crates/server/src/ocr/recognizer/western.rs)
  — Tesseract via `tesseract-rs`, English-only (`tessdata_best/eng`),
  preprocessing + per-word `ResultIterator` walk.
- [`crates/server/src/ocr/recognizer/manga.rs`](../../crates/server/src/ocr/recognizer/manga.rs)
  — `manga-ocr` (encoder + decoder + vocab ONNX, greedy decode).
- [`crates/server/src/ocr/postprocess.rs`](../../crates/server/src/ocr/postprocess.rs)
  — pure-function cleanup rules (see above).
- [`crates/server/src/ocr/cache.rs`](../../crates/server/src/ocr/cache.rs)
  — Redis-backed result + detect caches, version consts.
- [`crates/server/src/api/admin_ocr.rs`](../../crates/server/src/api/admin_ocr.rs)
  — `GET /admin/ocr/models` reflection-on-disk endpoint.

## Cache

Two Redis-backed layers, both fail-open on every operation.

### Result cache

Stores the final recognized text per region.

**Key**: `ocr:cache:v{N}:{content_hash}:{page}:{lang}:{d|r}:{region_hash}` —
the `{d|r}` byte is `d` when run with the detector enabled, `r` for
recognizer-only. `v{N}` is `OCR_RESULT_VERSION` — bump it on any
change that alters what the pipeline would produce for the same
inputs (preprocessing, PSM, Tesseract variables, postprocess rules,
response semantics); old keys age out via TTL, no flush needed.

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

**Key**: `ocr:detect:v{N}:{content_hash}:{page}` (`N` =
`OCR_DETECT_VERSION`)

**Value**: `{page_w, page_h, bboxes: [{xmin, ymin, xmax, ymax,
confidence, class}]}` in page-pixel coordinates. The page dims ride
along so the text-regions endpoint can convert to percent on a
cache hit without re-decoding the page (the v1 payload was a bare
bbox array; v1 entries deserialize-fail and read as misses).

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

- `folio_ocr_cache_hits_total` / `folio_ocr_cache_misses_total`
  — counter (result cache).
- `folio_ocr_detect_cache_hits_total` /
  `folio_ocr_detect_cache_misses_total` — counter (detector cache).

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

## Rate limits

Per-IP token buckets via `tower_governor`, in
[`crates/server/src/middleware/rate_limit.rs`](../../crates/server/src/middleware/rate_limit.rs):

- `OCR` (POST) = 60/min, burst 60. Reader users won't notice — the
  bucket caps a runaway script, not manual bubble exploration
  (bubble taps are recognize-only and cheap).
- `OCR_DETECT` (text-regions GET) = ~15/min, burst 10. A
  detect-cache miss is the most expensive single operation the
  server exposes (full-page inference through a process-wide
  mutex); the client's `staleTime: Infinity` keeps real usage far
  under the limit.

Denied requests increment
`folio_rate_limit_denied_total{bucket="ocr"|"ocr.detect"}`.

## Telemetry

| Metric | Type | Labels | Purpose |
|---|---|---|---|
| `folio_ocr_cache_hits_total` | counter | — | Cache effectiveness |
| `folio_ocr_cache_misses_total` | counter | — | Pair with hits for hit-rate |
| `folio_ocr_pipeline_seconds` | histogram | `lang` | Wall time, detector + recognize (recorded only on success path) |
| `folio_ocr_recognize_seconds` | histogram | `lang` | Recognize-only — diff with `pipeline_seconds` exposes detector cost |
| `folio_ocr_detect_seconds` | histogram | — | Detector-only inference via the text-regions endpoint |
| `folio_rate_limit_denied_total{bucket="ocr"}` | counter | `bucket` | OCR bucket denials (also `bucket="ocr.detect"`) |

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

Text-capture mode (keybind `x`, or the chrome's "Highlight +
capture text") is **bubble-aware**: entering the mode fires the
text-regions query (`useIssuePageTextRegions`, `staleTime:
Infinity`, `retry: false`) and `MarkerOverlay` renders the detected
bubbles as dashed outlines. Tapping a bubble OCRs it directly
(recognize-only — the region already is the detector's bbox);
dragging a rectangle still works as the fallback for missed
regions, and sends `detect: true` only when the regions fetch
succeeded (cache warm ⇒ snap is cheap). While the detector runs, a
non-blocking "Finding text regions…" pill shows — the drag surface
is live the whole time. On 429/error/empty the outlines just don't
appear.

[`web/app/[locale]/read/[seriesSlug]/[issueSlug]/marker-selection.ts`](../../web/app/%5Blocale%5D/read/%5BseriesSlug%5D/%5BissueSlug%5D/marker-selection.ts)
owns `ocrCroppedRegion(input, opts)`. The function:

1. Maps `MarkerRegion` (0–100% floats) to integer pixel rect
   against `naturalSize`, clamping for rounding overshoot.
2. POSTs via `apiFetch` with the user's CSRF token. `opts.lang`
   and `opts.detect` are forwarded only when set — the server
   resolves the language default.
3. Returns `{ text, confidence, refinedBbox } | null`. `null`
   covers every not-the-happy-path: non-2xx, network error,
   malformed JSON, or empty text. The caller (`MarkerEditor` /
   `MarkerOverlay`) treats `null` as "couldn't read text" and falls
   back to a plain highlight. When `refinedBbox` comes back on a
   new highlight, the pending marker's region snaps to it.

The marker editor shows a copy-to-clipboard button, a
low-confidence hint (< 0.6 — effectively western-only since
manga-ocr reports a synthetic 1.0), and an Auto/Western/Japanese
select for the Re-detect button.

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

## Future work

- PaddleOCR swap behind the existing `Recognizer` trait — better
  Western quality than `tessdata_best`.
- Detector segmentation-mask blanking: `comic-text-detector`
  exposes a per-page text mask we currently discard; using it to
  blank non-text pixels needs mask caching alongside the bboxes.
- manga-ocr real confidence: upstream PR to expose mean token
  probability from the decoder logits (today it's a synthetic
  1.0/0.0).
- Full-page "read all bubbles" — deferred until reading-order
  inference (RTL panel flow) is solved; tap-to-OCR makes each
  bubble one click meanwhile.
- Background pre-detection at scan time (`issue_text_region`
  table) — deferred; the on-demand + content_hash-keyed Redis
  cache already gives invalidation-on-rescan and warm-cache
  interactivity after one fetch.
