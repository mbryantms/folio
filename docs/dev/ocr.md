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
  "region_pct": { "x": 12.5, "y": 9.8, "w": 5.0, "h": 2.6 },
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
  "words": [{ "text": "POW!", "confidence": 0.93, "x": 252, "y": 305, "w": 90, "h": 60 }],
  "page_w": 1988,
  "page_h": 3056
}
```

- The region comes as **`region_pct`** (percent 0–100 of the decoded
  page, floats: the reader's marker representation) or as **`region`**
  (integer pixels of the decoded page). Exactly one is needed; with
  both, `region_pct` is used and `region` is ignored (a client may send
  both while older servers, which only read `region`, are around: the
  request struct has no `deny_unknown_fields`, so an older server
  ignores `region_pct`). Neither → 422 `validation` with a `region`
  field detail.
- `region_pct` is checked on extraction (`Validated<OcrRequest>`):
  finite numbers, `w` and `h` above 0, and the box must overlap the page
  (`x < 100`, `y < 100`, `x + w > 0`, `y + h > 0`); a failure is 422
  `validation` with `error.details` naming `region_pct.x` … or
  `region_pct` (the overlap rule). After the decode it is converted with
  the decoded width and height, each edge rounded to a whole pixel and
  **clamped** to the page (an overshoot at any edge is cut, not
  refused); a box that rounds to no whole pixel is 422 `invalid_region`.
  The client never needs the page size, which the page list cannot
  always give right (ComicInfo-declared sizes that are stale, EXIF
  rotations).
- The result cache keys on the pixel rectangle after conversion, so a
  `region_pct` request and the equivalent `region` request share an
  entry. Both are looked up before any page work: a `region` request
  directly, a `region_pct` request through the **stored page size** —
  every OCR decode (either route) writes the decoded `{w}x{h}` to
  `ocr:pagesize:v{N}:{fp}:{content_hash}:{page}` (30-day expiry; `N` =
  `OCR_PAGE_SIZE_VERSION`, bumped when the decode path would size the
  same bytes differently; `fp` hashes it with the decode limits). With
  the size stored, the percent box is converted, the result key built
  and looked up before the models, the permit and the decode, so a
  repeat tap is two Redis GETs. A cached payload is used only when its
  `page_w`/`page_h` equal the stored size. A miss on either key goes on
  to the normal path; after its decode, a stored size that differs from
  the decoded one is replaced (logged at warn), and the decoded size is
  what the conversion uses. The detect cache also carries page sizes,
  but only for detected pages, so it is not used for this.
- `page_w` / `page_h` (always present) are the decoded page size: the
  space `refined_bbox` and `words` are in. Cached payloads from before
  these fields existed are retired with result version 3.
- `region` is integer-pixel against the decoded page, not the
  archive's claimed dimensions — the handler decodes the page and
  uses the actual `(width, height)` for the bounds check (unchanged:
  `w`/`h` of 0 or a rect past the page is 422 `invalid_region`).
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
| 422 | `invalid_region` | `region`: `w`/`h` is 0 OR rect extends outside the decoded page; `region_pct`: rounds to no whole pixel (POST only) |
| 422 | `validation` | `region_pct` not finite, `w`/`h` ≤ 0, or wholly off the page; or no region at all; field-level `error.details` (POST only) |
| 422 | `invalid_lang` | `lang` not in `western`/`manga` (POST only) |
| 401 / 403 | (CSRF / auth) | Standard CSRF + session guards |
| 404 | `not_found` | Issue missing or user lacks library access |
| 404 | `not_found` | `page` index out of range for the archive |
| 415 | `decode_failed` | Page entry isn't a decodable image |
| 429 | `rate_limited` | `ocr` bucket (60/min/IP, burst 60) on the POST; `ocr.detect` (15/min/IP, burst 10) on the GET |
| 410 | `archive_missing` | The issue's file is gone (shared mapping in `api/page_source.rs`) |
| 409 | `archive_needs_conversion` | The file is a CBR / CB7 the page reader cannot read until it is converted to CBZ |
| 410 | `archive_unreadable` | The scanner marked the issue encrypted, or the file no longer opens as an archive |
| 500 | `ocr_failed` | Detector or recognizer threw |
| 503 | `service_unavailable` | Every OCR permit busy for 10 s (`OCR_MAX_CONCURRENT` = 2, process-wide), or an I/O error reading the file; carries `Retry-After` |
| 503 | `library_offline` | The file is missing and the library root itself cannot be opened or is empty (an unmounted share); `Retry-After: 30` |

Order of work in both routes: visibility (a hidden issue is the
unknown-id 404), the result cache (a hit takes no permit; a
`region_pct` request finds its key through the stored page size, see
above), the cheap
checks (encrypted row, missing file, offline library, unreadable archive,
page past the end, the page entry's listed size over the per-entry cap;
none waits for a permit, and none reads the page), the models (a
first-use download is waited on without holding a permit), then an OCR
permit. The page bytes are read only under the permit: the permit is
moved into the blocking read + decode and inference tasks
(`ocr::pipeline::blocking_with_permit`) and released when they finish, so
a client that disconnects mid-request cannot free it early and start more
concurrent work than there are permits.

With the permit held, the page comes from the [decoded-page
cache](#decoded-page-cache) (one decode serves every balloon request on
that page) or is read and decoded. Detection and recognition run in a
detached task that owns the permit and writes the detect and result
caches itself, after releasing the permit: a result that finishes after
the request is gone (the 60 s route timeout, a client abort) is still
cached, so the retry is a cache hit instead of a second full detection.
At most `OCR_MAX_CONCURRENT` detections or recognitions, abandoned or
not, run at once: each runs only under a permit, and a new request must
take one first (or get the 503 above). What outlives the permit is one
Redis write per finished result, each bounded by
`ocr::cache::CACHE_WRITE_TIMEOUT` (5 s). A detection whose request was
gone before it finished is counted in `folio_ocr_detect_abandoned_total`;
on the POST the recognition that would follow it is skipped (nobody is
waiting), the detection is still cached.

## Pipeline stages

```text
client region (pct → px, in the browser)
   │
   ▼
visibility (ACL)  ── hidden or unknown issue: 404
   │
   ▼
Redis result cache ── HIT? 200, no permit taken
   (region_pct: via the stored page size)
   │
   ▼ miss
cheap checks (encrypted, missing file, offline library, page range, entry size)
   │
   ▼
models ready (first-use download waits here, without a permit)
   │
   ▼
OCR permit (≤ 10 s wait, else 503)
   │
   ▼
decoded-page cache ── miss: read + decode the page (415 on a bad image)
   │
   ▼
region bounds check (422) / region_pct → pixels with the decoded size
   (clamped; a stored page size that disagrees is replaced)
   │
   ▼
region_pct only: Redis result cache ── HIT? 200
   (normally already answered before the models: see the
   region_pct notes under Endpoints)
   │
   ▼  ── from here on a detached task owning the permit ──
detect: false (default) ─────────────────┐
detect: true → detect cache, else the    │
   detector (comic-text-detector) on the │
   full page; snap to the box with the   │
   largest overlap, or keep the user's   │
   rect if none overlaps                 │
   │                                     │
   ▼                                     ▼
crop → recognizer (Western: Tesseract LSTM ; Manga: manga-ocr)
   │
   ▼
postprocess (junk-token + confidence cleanup)
   │
   ▼
permit released → detect-cache PUT (if the detector ran) + result-cache PUT
   │
   ▼
200 (if the request is still there)
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
drop (W1) → pronoun `I` (W1b) → line-end hyphen join (W2) →
symbol-only token drop sparing expressive punctuation like
`!?`/`...` and confident grawlix (W3) → edge junk strip (W4) →
per-char allowlist (W5) → whitespace collapse (W6) → empty-out guard
(W7) → confidence recompute over kept words (W8). The manga path only
strips control/zero-width/replacement chars — Japanese punctuation is
all legitimate output.

Rules that keep comic lettering intact (result version 3):

- **W1b, pronoun `I`.** Tesseract usually reads a sans-serif capital I
  as `|`. A token that is exactly `|` with a word (a token holding a
  letter) on both sides *on the same line* becomes `I`
  (`WHAT DO | DO` → `WHAT DO I DO`). A token starting `|'M`, `|'LL`,
  `|'VE` or `|'D` (straight or curly apostrophe, any case, optionally
  followed by `!?,.` and the like) becomes `I'M` … anywhere on a line
  (`|'M FINE` → `I'M FINE`). Everything else keeps the old treatment:
  a `|` at the start or end of a line (where balloon borders sit) is
  dropped, `|~`, `||`, `|_` are dropped, `|'S` loses its `|` (`'S`).
  A `|` scored below 0.60 is still dropped by W1 before this rule
  sees it. `I` and both apostrophes pass the W5 allowlist.
- **W2, line-end hyphen.** A line whose last token is a word followed
  by exactly one hyphen joins the next line's first token when that
  starts with a letter, and the hyphen stays: `SPIDER-` / `MAN` →
  `SPIDER-MAN` (a U+2010 hyphen is written as `-`). A double dash is
  an interruption and is not joined: `WAIT--` / `NO` → `WAIT-- NO`.
  A lone `-` and a digit on the next line are not joined either
  (unchanged). **Accepted cost:** a word the letterer really split
  across lines keeps its hyphen, `WON-` / `DERFUL` → `WON-DERFUL`.
  Comic lettering rarely splits words; the old rule deleted the hyphen
  of every compound name that wrapped (`SPIDERMAN`) and merged
  interruptions (`WAIT-NO`).
- **W3, grawlix.** A symbol-only token of three or more of
  `# @ $ % & * ! ?` with at least two different of `# @ $ % & *`
  (`#@$%!`, `@#$%&!`, `*#@!`) is kept as written (W4 does not strip
  it) when its confidence is at least 0.60 (the word's own score, or
  the whole recognition's when Tesseract gave no per-word data). Runs
  of one repeated symbol (`***`, `@@@`), anything shorter, anything
  with a border character (`|#@$`), and border junk (`~|'_`) are still
  dropped.

Thresholds/charsets are documented consts in that file (matcher-
style: consts + tests now, settings-registry promotion only on
operator demand). **Changing anything in preprocessing or
postprocess requires bumping `OCR_RESULT_VERSION`** in `cache.rs`.

Code map:

- [`crates/server/src/api/issue_ocr.rs`](../../crates/server/src/api/issue_ocr.rs)
  — request shapes, ACL, permit, decode (through the decoded-page
  cache), lang resolution, text-regions endpoint.
- [`crates/server/src/ocr/pipeline.rs`](../../crates/server/src/ocr/pipeline.rs)
  — `detect → snap → crop → recognize → clean`, all inside one
  `spawn_blocking` so the reactor isn't stalled, in a detached task
  that also writes the caches; also `detect_and_cache_regions` for the
  text-regions endpoint. Debug builds carry `test_models`, per-content-
  hash stand-ins for the detector and recognizers used by the
  integration tests.
- [`crates/server/src/ocr/detector.rs`](../../crates/server/src/ocr/detector.rs)
  — `comic-text-detector` singleton (`tokio::sync::OnceCell` +
  `std::sync::Mutex<ComicTextDetector>`).
- [`crates/server/src/ocr/recognizer/western.rs`](../../crates/server/src/ocr/recognizer/western.rs)
  — Tesseract via `tesseract-rs`, loading `eng` only
  (`tessdata_best/eng`), preprocessing + per-word `ResultIterator`
  walk. The `tesseract-rs` build script also downloads
  `tur.traineddata` (it expects both files), and the image ships it;
  nothing loads it.
- [`crates/server/src/ocr/recognizer/manga.rs`](../../crates/server/src/ocr/recognizer/manga.rs)
  — `manga-ocr` (encoder + decoder + vocab ONNX, greedy decode).
- [`crates/server/src/ocr/postprocess.rs`](../../crates/server/src/ocr/postprocess.rs)
  — pure-function cleanup rules (see above).
- [`crates/server/src/ocr/cache.rs`](../../crates/server/src/ocr/cache.rs)
  — Redis-backed result + detect caches, version consts, engine
  fingerprint.
- [`crates/server/src/ocr/page_cache.rs`](../../crates/server/src/ocr/page_cache.rs)
  — in-process decoded-page cache.
- [`crates/server/src/api/admin_ocr.rs`](../../crates/server/src/api/admin_ocr.rs)
  — `GET /admin/ocr/models` reflection-on-disk endpoint.

## Cache

Two Redis-backed layers, both fail-open on every operation, plus an
in-process cache of decoded pages.

### Engine fingerprint

Both Redis keys carry a 16-hex fingerprint (BLAKE3, truncated) of what
decides their payload, computed once per process
(`ocr::cache::fingerprints`, logged at first use):

- result keys: `OCR_RESULT_VERSION`, `OCR_DETECT_VERSION`, the linked
  Tesseract version (`TessVersion()`), a BLAKE3 of the
  `eng.traineddata` file the recognizer loads (same path resolution:
  `TESSDATA_PREFIX`, else `~/.tesseract-rs/tessdata`; `missing` when
  absent), and every pinned ONNX file of the detector and manga-ocr
  (repository, commit, file, SHA-256 from `ocr::models`);
- detect keys: `OCR_DETECT_VERSION` and the detector pins only.

An image upgrade that changes Tesseract, its language data or a model
pin therefore rolls the keys by itself; old entries age out via the
TTL. The version constants are still bumped for changes in Folio's own
code (preprocessing, cleanup, payload shape).

### Result cache

Stores the final recognized text per region.

**Key**: `ocr:cache:v{N}:{fingerprint}:{content_hash}:{page}:{lang}:{d|r}:{region_hash}` —
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

**Key**: `ocr:detect:v{N}:{fingerprint}:{content_hash}:{page}` (`N` =
`OCR_DETECT_VERSION`, detector-only fingerprint)

**Value**: `{page_w, page_h, bboxes: [{xmin, ymin, xmax, ymax,
confidence, class}]}` in page-pixel coordinates. The page dims ride
along so the text-regions endpoint can convert to percent on a
cache hit without re-decoding the page (the v1 payload was a bare
bbox array; v1 entries deserialize-fail and read as misses).

Flow per OCR call with `detect: true` (with `detect: false`, the
default, steps 3–5 are skipped and the user's rect is recognized
verbatim):

1. Result-cache lookup. Hit → return.
2. Take a permit; get the page from the decoded-page cache or decode it.
3. Detect-cache lookup. Hit → use cached polygons.
4. Miss → run detector on the **full page**.
5. Pick the polygon whose intersection with the user's rect is
   largest. Fall back to the user's rect verbatim if none overlap.
6. Recognize the chosen crop.
7. Release the permit; write the detect cache (step 4 ran) and the
   result cache. These writes happen in the detached task, also when
   the request is gone.

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

### Decoded-page cache

Reading a page's balloons one at a time (a tap per balloon, or the
page-text panel) sends one request per balloon, and each needs the whole
decoded page. `ocr::page_cache` keeps decoded pages in process so a page
is read from the archive and decoded once, not once per balloon.

- **Key**: `(content_hash, page)`, both read from the issue row after
  the visibility check; nothing a user controls. A rescan that changes
  the file changes the hash (a miss).
- **Bound**: `DECODED_PAGE_CACHE_BYTES` = 128 MiB of decoded pixels,
  counted as width × height × bytes per pixel of the decoded image (a
  2000 × 3000 page is 18 MB as RGB, 24 MB as RGBA: five to seven
  pages). Least-recently-used pages are evicted to fit; a page larger
  than the bound is not cached.
- **Expiry**: `DECODED_PAGE_IDLE` = 60 s without use. Expired pages are
  dropped on the next lookup and by a sweep task that runs only while
  the cache holds pages, so an idle server gives the memory back within
  90 s.
- **Single flight**: the first request for a page claims it and decodes
  it; requests for the same page meanwhile wait for that decode and
  then find the page. The claim is filled from inside the blocking
  decode, so it completes even when the claiming request is gone; a
  failed decode wakes the waiters and the next one decodes.
- **Order**: visibility, the result cache and the archive checks run on
  every request before this cache is consulted, and it is consulted
  only with an OCR permit held. Memory is the bound plus at most one
  in-flight page per permit; a waiter (holding a permit) waits only on
  a decode that runs under the other permit and ends. The map's lock
  is held for map updates only, never with the Tesseract or detector
  mutexes.
- **Visibility and timing**: a user who cannot see the issue gets the
  same 404 before the cache is looked at. A user who can see it may
  notice that a page answers faster because someone opened it in the
  last minute (also through another issue with identical bytes); the
  Redis result and detect caches, keyed the same way, already show
  that (a warm text-regions call answers in milliseconds), so this adds
  no new kind of signal.
- **Metrics**: `folio_ocr_page_cache_hits_total` /
  `folio_ocr_page_cache_misses_total`.

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
| `folio_ocr_page_decode_seconds` | histogram | — | Archive read + decode of one page (decoded-page cache misses only) |
| `folio_ocr_permit_wait_seconds` | histogram | — | Time a request waited for an OCR permit (also recorded when it gave up) |
| `folio_ocr_busy_total` | counter | — | Requests refused with 503 `service_unavailable` after waiting `OCR_PERMIT_WAIT` (10 s) for a permit |
| `folio_ocr_detect_abandoned_total` | counter | — | Detections that finished after their request was gone (timeout, abort); each is still cached |
| `folio_ocr_model_load_seconds` | histogram | `model` | Once per process per model (`detector`, `western`, `manga`): verified download (if needed) + session build |
| `folio_ocr_model_first_use_seconds` | histogram | `model` | Once per process per model: the first inference after load (session warm-up included) |
| `folio_ocr_page_cache_hits_total` / `folio_ocr_page_cache_misses_total` | counter | — | Decoded-page cache effectiveness |
| `folio_rate_limit_denied_total{bucket="ocr"}` | counter | `bucket` | OCR bucket denials (also `bucket="ocr.detect"`) |

All visible at `/metrics`. Operator dashboards reading these should
key on `lang` to spot manga-OCR-bound vs western-OCR-bound boxes. No
metric carries a per-user or per-issue label.

Reading slowness: a high `permit_wait_seconds` with a rising
`busy_total` means the two permits are saturated (look at
`detect_seconds` / `recognize_seconds` for what holds them); a high
`page_decode_seconds` with a low page-cache hit rate means pages are
large or the cache is too small for the readers at once; a large
`model_first_use_seconds` beside a small steady-state inference time is
session warm-up after a restart. Every `*_seconds` histogram shares the
exporter's buckets (top bucket 10 s), so a cold 50 s detection only
shows as `+Inf`.

## Models — auto-download + on-disk inspection

The three model artifacts the pipeline depends on:

| Component | Where | Download source | Approx size |
|---|---|---|---|
| `comic-text-detector` ONNX | `${HOME}/.cache/huggingface/hub/models--mayocream--comic-text-detector-onnx/` | `mayocream/comic-text-detector-onnx` | ~95 MB |
| `manga-ocr` ONNX (encoder + decoder + vocab) | `${HOME}/.cache/huggingface/hub/models--mayocream--manga-ocr-onnx/` | `mayocream/manga-ocr-onnx` | ~440 MB |
| Tesseract LSTM (`eng.traineddata`) | `${TESSDATA_PREFIX}` (default `${HOME}/.tesseract-rs/tessdata`) | tessdata_best (build script) | ~15 MB |

The ONNX cache path follows `HOME`, not `HF_HOME` (see the gotcha
below). The first request that touches a model triggers download + ~1–2 s session
build for ONNX, or a one-time cmake build for Tesseract (cached at
`~/.tesseract-rs/`).

### Pinned model files

Every ONNX file is pinned to a commit of its Hugging Face repository
and to a SHA-256 ([`ocr::models`](../../crates/server/src/ocr/models.rs)).
Before `ComicTextDetector::new` / `MangaOCR::new` run, Folio itself
fetches each missing file from
`https://huggingface.co/<repo>/resolve/<commit>/<file>` over https
only (connect 20 s, per-read 60 s, 30 min overall, at most 5 redirects,
each https and never to an internal address literal, cut off past the
published size), hashes it while it streams into an exclusive,
randomly named temporary file (`.folio-part-*`, removed if the
download fails; a leftover from a killed process is swept after 35
minutes and is not counted by `GET /admin/ocr/models`), and only then moves it to
`hub/models--<org>--<repo>/snapshots/<commit>/<file>` and points
`refs/main` at `<commit>`. The upstream constructor finds the file
there and does not download anything. A digest mismatch deletes the
download, logs the file and both digests, and fails the request with
`500 ocr_model_unverified`; nothing unverified is loaded. A file
already in the cache (from before the pin, or changed on disk) is
hashed once per process; a mismatch deletes it and fetches it again
from the pinned commit.

The fetch runs in a background task, one per repository for the whole
process. OCR requests sit under the 60 s JSON route timeout, so the
first request after a cold start may still time out while the ~440 MB
manga models download; the download carries on, and later requests
wait on it or find the verified files. After a failed attempt
(network down, a digest mismatch) the repository is not tried again
for 5 minutes; requests in that window get `ocr_model_unverified`
without a new download.

The check covers what arrives over the network and what sits in the
cache at startup. Anyone who can write the cache directory can swap a
file between the check and the load: that directory needs the same
trust as the database and `/data/secrets/`. After an upgrade that moves
the pin, the previous `snapshots/<old commit>/` folder (and, for files
the hub downloaded before pinning, its `blobs/`) stays on disk unused
and can be deleted.

| Repository | Commit | File | SHA-256 | Bytes |
|---|---|---|---|---|
| `mayocream/comic-text-detector-onnx` | `a5d67ec772adef819ef5b0e7aa701fcf4c8bf74a` | `comic-text-detector.onnx` | `1a86ace7…071d718f` | 94,669,756 |
| `mayocream/manga-ocr-onnx` | `24b12778d85800835e2ca409236de281b8ab7b9f` | `encoder_model.onnx` | `15fa8155…d995def85f` | 343,454,249 |
| | | `decoder_model.onnx` | `ef776526…fa61f5e8` | 117,480,262 |
| | | `vocab.txt` | `5cb5c558…4d6a36a3` | 30,216 |

(Full digests in `models.rs`; looked up 2026-11-11, both `main`
branches last changed 2025-05-08.) There is no setting for another
model source: the upstream crates hardcode the repositories and
`Api::new()` ignores `HF_ENDPOINT`. For an air-gapped host, copy the
files to `snapshots/<commit>/` under the cache above; they are
verified like any cached file.

**Updating the pin:** `curl -s
"https://huggingface.co/api/models/<repo>/revision/main?blobs=true"`
gives the commit (`sha`) and, per LFS file, `lfs.sha256` and `size`.
For a non-LFS file (`vocab.txt`) download it from
`/resolve/<commit>/vocab.txt` and run `sha256sum` (its `git
hash-object` should equal the listing's `blobId`). Change `revision`,
`sha256` and `size` in `models.rs` and this table together.

**Tesseract data** is not fetched at runtime: the `tesseract-rs`
build script downloads `eng` and `tur` from `tessdata_best`'s `main`
branch while the image builds, and the Dockerfile checks both against
pinned SHA-256s right after `cargo build` (a changed upstream file
fails the image build). Local `cargo build` runs of the build script
are not checked.

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
- Permit order (what waits for a permit, what never does):
  [`crates/server/tests/ocr_permits.rs`](../../crates/server/tests/ocr_permits.rs).
- Work that outlives its request is cached; the decoded-page cache
  (one decode per page, a hidden issue still 404s, a new content hash
  is a miss), with debug-build model stand-ins:
  [`crates/server/tests/ocr_work.rs`](../../crates/server/tests/ocr_work.rs).
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
- Server-side "read all bubbles": the reader's page-text panel
  (`PageTextPanel.tsx`) already reads a whole page, from the client:
  the text regions, then one OCR request per region in reading order,
  a couple at a time. A batch route that does this in one request is
  not built.
- Background pre-detection at scan time (`issue_text_region`
  table) — deferred; the on-demand + content_hash-keyed Redis
  cache already gives invalidation-on-rescan and warm-cache
  interactivity after one fetch.
