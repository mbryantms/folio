# OCR pipeline

Server-side text reading for the reader: find the text regions of a
page, read the one the user picked. Three small models on ONNX Runtime
do the Western work, each in a session Folio builds itself; Japanese
goes through `manga-ocr`.

| Role | Model | Size | What it does |
|---|---|---|---|
| Page detector | `ogkalu/comic-text-and-bubble-detector` (`detector-v4-s_int8.onnx`, RT-DETR-v2) | 11 MB | Balloons and free text of a whole page, one inference |
| Line finder | `PaddlePaddle/PP-OCRv5_mobile_det_onnx` (DB) | 5 MB | Splits one balloon crop into text lines |
| Line recogniser | `PaddlePaddle/en_PP-OCRv5_mobile_rec_onnx` (CTC) | 8 MB | Reads the lines of a balloon in one batch |
| Japanese | `mayocream/manga-ocr-onnx` (`manga-ocr` crate) | 440 MB | Reads a balloon crop as one string |

The previous engines, `comic-text-detector` and Tesseract, stay in the
build for one release behind
[`COMIC_OCR_LEGACY_ENGINES`](#engine-switch-comic_ocr_legacy_engines).

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
  "text": "LOOK OUT!",
  "confidence": 0.98,
  "refined_bbox": { "x": 248, "y": 302, "w": 106, "h": 76 },
  "lang": "western",
  "raw_text": "LOOK\nOUT!",
  "words": [
    { "text": "LOOK", "confidence": 0.99, "x": 262, "y": 310, "w": 78, "h": 26 },
    { "text": "OUT!", "confidence": 0.98, "x": 258, "y": 340, "w": 84, "h": 27 }
  ],
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
  refused); a box that rounds to no whole pixel is 422 `validation`
  with a `region_pct` field detail.
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
- `detect` is the **snap opt-in**. When `true` the pipeline takes the
  page's detected regions (from the detect cache, else by running the
  page detector) and reads the region that overlaps the user's rect
  most, instead of the rect itself. When `false` or omitted (the
  default) the recognizer reads the user's rect verbatim: that is how a
  region from the text-regions route is read, since it already is a
  detected box. The reader sends `detect: true` for a rectangle the
  user dragged, and only when the page's text-regions fetch succeeded.
- `refined_bbox` is the detected box that was read when that is not
  the rectangle as sent: the region a `detect: true` request was
  snapped to. Absent when the rectangle was read as sent (`detect` off,
  or no region overlapped it).
- `raw_text` is the engine output before the postprocess cleanup;
  present only when cleanup changed the text. For Western text it holds
  the lines as read, one per line, where `text` has them joined.
- `words` holds the pieces the text was read in, in reading order, with
  page-pixel boxes. For Western text the piece is a **text line** (the
  recogniser reads whole lines), so an entry's `text` can hold several
  words; the field keeps its name because the response shape did not
  change. Absent for manga-ocr and when nothing was read. The web
  client does not use it.
- `confidence`: for Western text, the mean over the lines read, each
  line scored by the mean probability of its characters; a line that
  could not be read counts as 0. manga-ocr reports 1.0, or 0.0 for an
  empty result.

### `GET /me/issues/{id}/pages/{page}/text-regions`

The page's text regions in **percent** coordinates — drives the
reader's tappable outlines in text-capture mode and the page-text
panel. A region is a balloon (`class` 0) or a piece of text outside
balloons (`class` 2: a caption on the art, a sound effect, a sign);
each is a box to read as a whole. The detector's third class, the text
inside a balloon, is not returned: it lies inside a class 0 box.

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

Shares the per-page detect cache with the OCR POST. Cache hit = Redis
round-trip, no archive touch; miss = page decode + one detector
inference (tens of milliseconds for the model; decoding a large page
costs more). The endpoint keeps its own stricter rate bucket.

Error codes (envelope `{ "error": { "code", "message" } }`):

| Status | code | When |
|---|---|---|
| 422 | `invalid_region` | `region`: `w`/`h` is 0 OR rect extends outside the decoded page (POST only) |
| 422 | `validation` | `region_pct` not finite, `w`/`h` ≤ 0, wholly off the page, or rounding to no whole pixel; or no region at all; field-level `error.details` (POST only) |
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
permit. Because the result cache comes before the cheap checks, a cached
`region_pct` request, like a cached pixel request, is answered without
them (a missing file, an offline library, an encrypted row or a needed
conversion does not stop it). The page bytes are read only under the permit: the permit is
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
waiting), and the detection is written to the cache like any other
(fail-open: a write that errors or exceeds the 5 s bound is dropped and
logged, and the next request detects again). A recognition that fails,
with an error or a panic from the engine, still hands its finished
detection to the cache. A failure nobody receives (the request was
already gone) is logged once at warn. A caught engine panic is logged at
error with its message and counted in
`folio_ocr_recognizer_panics_total{model}`. A panic while an engine's
lock is held poisons the lock, and every later request for that engine
answers `ocr_failed` until the server restarts. Tesseract and manga-ocr
hold their lock for the whole call; the three sessions Folio builds
hold theirs for the inference only, so a panic in Folio's own image or
decoding code around them fails that one request and nothing else.

Every OCR cache read is bounded too (`CACHE_READ_TIMEOUT`, 2 s, on top
of the Redis connection's own 500 ms response timeout); a timed-out read
is a miss, counted in `folio_ocr_cache_read_timeouts_total` and logged
at warn, so a hung Redis cannot hold a permit.

At shutdown the runtime drops the detached tasks: a cache write still
pending then is lost (harmless; the next request recomputes it), while
blocking inference already running goes on to its end, as before.

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
   page detector on the full page; snap  │
   to the region with the largest        │
   overlap, or keep the user's rect if   │
   none overlaps                         │
   │                                     │
   ▼                                     ▼
crop → recognizer
   Western: line finder on the crop → lines cut → read in one batch
   Manga:   manga-ocr on the crop
   │
   ▼
postprocess (Western: join the lines; Manga: strip control characters)
   │
   ▼
permit released → detect-cache PUT (if the detector ran) + result-cache PUT
   │
   ▼
200 (if the request is still there)
```

## Engines

Everything below is the current engine set. The constants are the ones
the engines were measured with; each lives next to the code that uses
it, guarded by unit tests. **Changing any of them, or anything else in
preprocessing or postprocess, requires bumping `OCR_RESULT_VERSION`
(and `OCR_DETECT_VERSION` for the page detector) in `cache.rs`.**

### Page detector ([`ocr/bubbles.rs`](../../crates/server/src/ocr/bubbles.rs))

`ogkalu/comic-text-and-bubble-detector`, `detector-v4-s_int8.onnx`: an
RT-DETR-v2 export, 8-bit quantized.

- **Input** `images`: the page squashed to 640 × 640 (bilinear; not
  cropped, not padded), RGB, each value × 1/255, no mean/std
  normalisation. `orig_target_sizes`: `[[width, height]]` of the page,
  so the model returns its boxes in page pixels whatever the page's
  shape.
- **Output** `labels`, `boxes` (`x0, y0, x1, y1`), `scores`, one row per
  query. No non-maximum suppression is needed.
- **Kept**: rows scoring at least **0.5**, of all three classes (0
  `bubble`, 1 `text_bubble`, 2 `text_free`), clamped to the page. This
  is the page's detection, and what the detect cache stores.
- **Regions**: classes 0 and 2. They are what the text-regions route
  returns and what `detect: true` snaps to. Class 1, the text inside a
  balloon, never leaves the server; it is kept for the Japanese crop
  (below).
- Used for pages of every language.

### Line finder ([`ocr/lines.rs`](../../crates/server/src/ocr/lines.rs))

The Western recogniser reads one line at a time, so a balloon crop is
split into lines first, with `PaddlePaddle/PP-OCRv5_mobile_det_onnx` (a
DB text detector).

- **Input**: the crop enlarged **2×** (comic lettering is small for a
  detector trained on documents), less when the long side would pass
  **1600**, both sides rounded to a multiple of 32; planes in B, G, R
  order, `(x/255 − mean) / std` with the ImageNet mean `0.485, 0.456,
  0.406` and std `0.229, 0.224, 0.225` applied in that plane order
  (what PaddleOCR's own pipeline does).
- **Postprocess** (DB): pixels of the probability map above **0.3**
  form 8-connected components; a component's axis-aligned box is
  scored by the mean probability inside it and dropped below **0.6**
  (or when thinner than 3 map pixels); the box is grown on every side
  by `area × 1.5 / perimeter` (the unclip ratio; DB shrinks text regions
  in training) and mapped back to crop pixels. At most 1000 components
  are looked at. Rotated boxes are not produced.
- **Order**: rows top to bottom (a box joins a row when its vertical
  centre is within half the shorter height of the row's first box),
  left to right within a row.
- **No lines found**: the whole crop is read as one line. A line is
  read when both its sides have at least 4 pixels.

### Line recogniser ([`ocr/recognizer/ctc.rs`](../../crates/server/src/ocr/recognizer/ctc.rs))

`PaddlePaddle/en_PP-OCRv5_mobile_rec_onnx`, a CTC line model with a
436-character alphabet (it cannot produce characters of other scripts).

- **Input** `[n, 3, 48, w]`: each line resized to height **48** keeping
  its aspect ratio (bilinear), planes in B, G, R order, `(x/255 − 0.5)
  / 0.5`. The batch is as wide as its widest line and at least **320**;
  narrower lines are padded on the right with zeros. All lines of a
  balloon go in one run.
- **Decode**: greedy CTC. Per step the most probable class; class 0 is
  the blank, repeats collapse, the last class is the space, the classes
  between are the dictionary. A line's confidence is the mean
  probability of the characters kept.
- **Dictionary**: the `PostProcess.character_dict` list of the model's
  `inference.yml`, a pinned file like the model. It is read by a strict
  reader for that one list (single characters, unquoted or in single
  quotes, to the end of the block; anything else is an error), not by a
  YAML library. When the engine loads, the list must be exactly two
  shorter than the model's class count (blank + list + space); on a
  mismatch the engine does not start and Western requests answer
  `ocr_failed`, because every character would decode as a neighbour.
- **Safety bounds** outside what a balloon produces: a line more than
  66 times as wide as tall is squeezed to width 3200 (PaddleOCR's own
  ceiling), and a run holds at most 96,000 input columns (lines ×
  width), the rest going into a following run. They bound memory for a
  hand-drawn region that is one long strip or holds hundreds of lines.

**Another recogniser of the same family is a registry change.**
`ocr::models::WESTERN_REC` names a `LineRecModel` (its pinned files,
which one is the model, where the dictionary comes from).
`latin_PP-OCRv5_mobile_rec_onnx` and `PP-OCRv6_small_rec_onnx` are
registered with their pins and are never fetched unless selected. To
switch: point `WESTERN_REC` at the other entry, update the pin table
below and the expected text of the real-model tests. The cache keys
follow the pins by themselves.

### Sessions and threads ([`ocr/session.rs`](../../crates/server/src/ocr/session.rs))

Folio builds each of the three sessions with graph-optimization level 3
and an explicit intra-op thread count:

| Session | Threads |
|---|---|
| Page detector, line recogniser | `min(available_parallelism, 4)` |
| Line finder | `min(available_parallelism, 2)` (it measured faster with 2 than with 4) |

`available_parallelism` follows the cgroup CPU quota and the affinity
mask, so a container limited to 4 CPUs on a 40-core host gets 4
threads. ONNX Runtime's own default is one thread per physical core of
the machine whatever the process may use; that default is what made the
previous detector take tens of seconds a page in a small container.
More than 4 threads do not help: each call is a small image.

`OMP_NUM_THREADS` has no effect on any of this. The ONNX Runtime build
Folio links has no OpenMP (measured: the previous detector took the
same time with 1, 4 and 8), and Tesseract is built with OpenMP off. The
server used to set the variable at start; that code is gone.

Each session sits behind a mutex (a run needs exclusive access), taken
for the inference only. With `OCR_MAX_CONCURRENT` = 2 two requests can
overlap; they queue on a session for the milliseconds a run takes.

### What was measured

The engines were chosen with a benchmark run before they were adopted
(the tool is not in this repository). On one desktop CPU (i9-14900K)
pinned to four performance cores, over 12 scanned pages from 1941–1953
with 139 balloons and captions transcribed as the reference:

| | Previous engines | Current engines |
|---|---|---|
| A page's regions | 47 s | 68 ms |
| One balloon, page already detected | 64 ms | 141 ms |
| Tapping the middle of each of the 139: no region under the tap | 29 | 0 |
| … read with at most 10% character error | 43 | 114 |
| … read with at most 5% character error | 22 | 93 |

What ships reproduces that run: on the same pages the code here returns
the same regions and, for every reference region, the same crop and the
same text as the benchmark recorded.

Limits of the measurement: the pages are all upper-case hand lettering
(no modern digital lettering, no lower case, English only); the
reference text was transcribed by a model and spot-checked, not typed
by a person; a slower server CPU scales the times. Most of the
remaining errors are lines the line finder dropped or took from a
touching neighbour balloon, not misread letters.

To re-measure on a machine, on the fixture page:

```sh
cargo test --release -p server --test ocr_recognizer --no-run
taskset -c 0,2,4,6 <the printed test binary> --ignored --nocapture timing
```

It prints the detection time, the per-balloon time and the load
average. Run it on an idle machine; pin only the run, not the build.

### Japanese

`manga-ocr` reads a crop as one string (ViT encoder + BERT decoder,
greedy decode; confidence is 1.0, or 0.0 for an empty result). The
recogniser and its cleanup are unchanged and used by both engine sets.

What changes with the current engines is the crop. The previous
detector returned text blocks; the current one returns balloons, so a
tapped region is the balloon with the white around its text. Which
pixels a Japanese request reads is one function,
[`ocr::manga_crop::manga_crop`](../../crates/server/src/ocr/manga_crop.rs),
with two behaviours:

- `MangaCrop::Region`: the requested region as is (the balloon box);
- `MangaCrop::TextInRegion`: the detector's class 1 box inside the
  region, when exactly one has at least 80% of its area in the region
  and no other class 1 box has more than 20% of its area there; the
  region otherwise.

The constant `MANGA_CROP` picks one and is `Region` today. **Neither
has been measured on Japanese pages; the choice is open** until that
is done. The constant is part of the result-cache fingerprint, so
changing it starts from fresh results. In the `TextInRegion` mode a
Japanese request always needs the page's detection (from the detect
cache, else by running the detector), also for a tap, and
`refined_bbox` carries the text box that was read.

With the legacy engines a Japanese request reads what it always read:
the requested rectangle, or the `comic-text-detector` box it was
snapped to.

### Postprocess

[`crates/server/src/ocr/postprocess.rs`](../../crates/server/src/ocr/postprocess.rs)
is the single deterministic cleanup pass; `clean_for` picks the rules by
language and engine set.

**Western, current engines** (`clean_lines`): the recogniser returns one
string per line, and the cleanup only joins them into one utterance.

- **Line-end hyphen.** A line whose last token is a word followed by
  exactly one hyphen joins the next line's first token when that starts
  with a letter, and the hyphen stays: `SPIDER-` / `MAN` →
  `SPIDER-MAN` (a U+2010 hyphen is written as `-`). A double dash is an
  interruption and is not joined: `WAIT--` / `NO` → `WAIT-- NO`. A lone
  `-` and a digit on the next line are not joined either. **Accepted
  cost:** a word the letterer really split across lines keeps its
  hyphen, `WON-` / `DERFUL` → `WON-DERFUL`. Comic lettering rarely
  splits words, and deleting the hyphen breaks every compound name that
  wraps.
- **Whitespace.** Every other line break and every run of whitespace
  becomes one space.
- Nothing is dropped and no character is rewritten, and the confidence
  is the recogniser's. The rules below were written for what Tesseract
  makes of balloon borders; on this recogniser's output they would
  delete real text (a `|`, a low-scored word, a symbol outside the
  allowlist).

**Manga**: strips control, zero-width and replacement characters and
collapses whitespace; Japanese punctuation is all legitimate output.

**Western, legacy engines** (`clean`, unchanged): the Tesseract chain,
in order: per-word confidence drop (W1) → pronoun `I` read as `|` (W1b)
→ the same line-end hyphen join (W2) → symbol-only token drop sparing
expressive punctuation like `!?`/`...` and confident grawlix (W3) →
edge junk strip (W4) → per-char allowlist (W5) → whitespace collapse
(W6) → empty-out guard (W7) → confidence recompute over kept words
(W8). The rules and their thresholds are documented in the module.

Code map:

- [`crates/server/src/api/issue_ocr.rs`](../../crates/server/src/api/issue_ocr.rs)
  — request shapes, ACL, permit, decode (through the decoded-page
  cache), lang resolution, engine set from configuration, text-regions
  endpoint.
- [`crates/server/src/ocr/pipeline.rs`](../../crates/server/src/ocr/pipeline.rs)
  — `detect → snap → crop → recognize → clean`, all inside one
  `spawn_blocking` so the reactor isn't stalled, in a detached task
  that also writes the caches; also `detect_and_cache_regions` for the
  text-regions endpoint. `prepare_models` / `prepare_detector` take the
  `EngineSet` as a value. Debug builds carry `test_models`, per-content-
  hash stand-ins for the detector and recognizers used by the
  integration tests.
- [`crates/server/src/ocr/session.rs`](../../crates/server/src/ocr/session.rs)
  — ONNX Runtime session construction and thread counts.
- [`crates/server/src/ocr/bubbles.rs`](../../crates/server/src/ocr/bubbles.rs)
  — page detector.
- [`crates/server/src/ocr/lines.rs`](../../crates/server/src/ocr/lines.rs)
  — line finder.
- [`crates/server/src/ocr/recognizer/ctc.rs`](../../crates/server/src/ocr/recognizer/ctc.rs)
  — line recogniser, dictionary reader, and `LineOcr` (line finder +
  recogniser behind the `Recognizer` trait).
- [`crates/server/src/ocr/recognizer/manga.rs`](../../crates/server/src/ocr/recognizer/manga.rs)
  — `manga-ocr` (encoder + decoder + vocab ONNX, greedy decode).
- [`crates/server/src/ocr/manga_crop.rs`](../../crates/server/src/ocr/manga_crop.rs)
  — which rectangle a Japanese request reads.
- [`crates/server/src/ocr/models.rs`](../../crates/server/src/ocr/models.rs)
  — pinned model files, the fetch-and-verify pinner, the line
  recogniser registry.
- [`crates/server/src/ocr/postprocess.rs`](../../crates/server/src/ocr/postprocess.rs)
  — pure-function cleanup rules (see above).
- [`crates/server/src/ocr/cache.rs`](../../crates/server/src/ocr/cache.rs)
  — Redis-backed result + detect caches, version consts, engine
  fingerprint.
- [`crates/server/src/ocr/page_cache.rs`](../../crates/server/src/ocr/page_cache.rs)
  — in-process decoded-page cache.
- [`crates/server/src/api/admin_ocr.rs`](../../crates/server/src/api/admin_ocr.rs)
  — `GET /admin/ocr/models` reflection-on-disk endpoint.
- Legacy engine set:
  [`ocr/detector.rs`](../../crates/server/src/ocr/detector.rs)
  (`comic-text-detector` singleton) and
  [`ocr/recognizer/western.rs`](../../crates/server/src/ocr/recognizer/western.rs)
  (Tesseract via `tesseract-rs`).

## Engine switch (`COMIC_OCR_LEGACY_ENGINES`)

**Temporary, for one release.** `COMIC_OCR_LEGACY_ENGINES=true` in the
server's environment (read at start, default `false`, not an admin
setting) makes the server run the previous engines exactly as the
release before the engine change did: `comic-text-detector` for the
page (blocks and lines, classes 0 and 1), Tesseract for Western text
with the full W cleanup chain, `manga-ocr` for Japanese. It exists so a
deploy can go back without a code revert; the switch, both legacy
modules and the Tesseract build are removed in a later change.

- The two sets never share a cache entry: the engine fingerprint in the
  result and detect keys includes the set. Flipping the switch starts
  from an empty cache for that set, and flipping back finds the entries
  it left (until their 7-day expiry).
- Models are fetched on first use, per set. A server that never runs
  the legacy set never fetches the 95 MB `comic-text-detector` model;
  one that was upgraded keeps it on disk until someone deletes it.
- What the legacy set looks like from outside: a page's first detection
  takes tens of seconds (minutes in a small container), text-regions
  returns block and line boxes, and `words` holds words.
- In code the set is `ocr::EngineSet`, chosen once per request in
  `api::issue_ocr` and passed down, so a tool can run both sets in one
  process (`prepare_models(EngineSet::Legacy, …)`).

Legacy Western preprocessing, for reference (inside the recognizer):
grayscale → 3× Lanczos3 upscale → Otsu binarization (midpoint fallback
on near-uniform crops) → polarity detection (majority-black border ring
⇒ invert) → 24 px white border pad → PSM 6 + pinned 300 DPI.

## Cache

Two Redis-backed layers, both fail-open on every operation, plus an
in-process cache of decoded pages.

### Engine fingerprint

Both Redis keys carry a 16-hex fingerprint (BLAKE3, truncated) of what
decides their payload, computed once per process and engine set
(`ocr::cache::fingerprints_for`, logged at first use):

- result keys: the engine set, `OCR_RESULT_VERSION`,
  `OCR_DETECT_VERSION`, every pinned file (repository, commit, file,
  SHA-256 from `ocr::models`) of the page detector, the line finder,
  the line recogniser with its dictionary, and manga-ocr, the Japanese
  crop mode (`MANGA_CROP`), and the page-size fingerprint
  (`OCR_PAGE_SIZE_VERSION` and the decode limits);
- detect keys: the engine set, `OCR_DETECT_VERSION` and the page
  detector's pins only.

For the legacy set the page detector is `comic-text-detector`'s pin and
the Western part is the linked Tesseract version (`TessVersion()`) plus
a BLAKE3 of the `eng.traineddata` file the recognizer loads (same path
resolution: `TESSDATA_PREFIX`, else `~/.tesseract-rs/tessdata`;
`missing` when absent).

A moved model pin, a switch of line recogniser or of engine set, or
(legacy) an image upgrade that changes Tesseract or its language data
therefore rolls the keys by itself; old entries age out via the TTL.
The version constants are still bumped for changes in Folio's own code
(preprocessing, thresholds, cleanup, payload shape). Current values:
result **v4** (Western text read by the line finder and the line
recogniser, `words` one entry per line; Japanese crops from the
balloon detector), detect **v3** (the boxes of the balloon detector,
all three classes).

### Result cache

Stores the final recognized text per region.

**Key**: `ocr:cache:v{N}:{fingerprint}:{content_hash}:{page}:{lang}:{d|r}:{region_hash}` —
the `{d|r}` byte is `d` when run with the detector enabled, `r` for
recognizer-only. `v{N}` is `OCR_RESULT_VERSION` — bump it on any
change that alters what the pipeline would produce for the same
inputs (an engine's preprocessing, thresholds or decoding, postprocess
rules, response semantics); old keys age out via TTL, no flush needed.

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

### Detector-result cache

Stores the page's detected regions so the text-regions route answers a
repeat request without touching the archive, and `detect: true` reads
of _different_ regions of the same page share one detection.

**Key**: `ocr:detect:v{N}:{fingerprint}:{content_hash}:{page}` (`N` =
`OCR_DETECT_VERSION`, detector-only fingerprint)

**Value**: `{page_w, page_h, bboxes: [{xmin, ymin, xmax, ymax,
confidence, class}]}` in page-pixel coordinates: every box the
detector kept, of all three classes. The text-regions route and the
snap filter it to the regions (classes 0 and 2) on the way out. The
page dims ride along so the text-regions endpoint can convert to
percent on a cache hit without re-decoding the page (the v1 payload
was a bare bbox array; an entry of that shape deserialize-fails and
reads as a miss).

Flow per OCR call with `detect: true` (with `detect: false`, the
default, steps 3–5 are skipped and the user's rect is recognized
verbatim):

1. Result-cache lookup. Hit → return.
2. Take a permit; get the page from the decoded-page cache or decode it.
3. Detect-cache lookup. Hit → use the cached regions.
4. Miss → run the page detector on the **full page**.
5. Pick the region whose intersection with the user's rect is
   largest. Fall back to the user's rect verbatim if none overlap.
6. Recognize the chosen crop.
7. Release the permit; write the detect cache (step 4 ran) and the
   result cache. These writes happen in the detached task, also when
   the request is gone.

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
- **Order**: visibility, the result cache (for a `region_pct` request:
  through the stored page size, when there is one) and the archive
  checks run on every request before this cache is consulted, and it is
  consulted only with an OCR permit held. A `region_pct` request looks
  the result cache up after the decode, still under the permit, only when
  the early lookup did not already read that key: no page size was
  stored, or the stored one differed from the decode. Every result read
  by a `region_pct` request accepts only a payload whose `page_w` /
  `page_h` equal the size it converted with (anything else is a miss,
  the pipeline runs and the entry is overwritten), and result keys also
  carry the page-size fingerprint. Memory is the bound plus at most one
  in-flight page per permit; a waiter (holding a permit) waits only on
  a decode that runs under the other permit and ends. The map's lock
  is held for map updates only, never with an engine's mutex.
- **Visibility and timing**: a user who cannot see the issue gets the
  same 404 before the cache is looked at. A user who can see it may
  notice that a page answers faster because someone opened it in the
  last minute (also through another issue with identical bytes); the
  Redis result and detect caches, keyed the same way, already show
  that (a warm text-regions call answers in milliseconds), so this adds
  no new kind of signal.
- **Metrics**: `folio_ocr_page_cache_hits_total` /
  `folio_ocr_page_cache_misses_total`.

## Rate limits

Per-IP token buckets via `tower_governor`, in
[`crates/server/src/middleware/rate_limit.rs`](../../crates/server/src/middleware/rate_limit.rs):

- `OCR` (POST) = 60/min, burst 60. Reader users won't notice — the
  bucket caps a runaway script, not manual bubble exploration.
- `OCR_DETECT` (text-regions GET) = ~15/min, burst 10. Sized for the
  previous detector, where a detect-cache miss was the most expensive
  single operation the server exposed; a miss now costs a page decode
  and a short inference. The limits were left as they are (the client's
  `staleTime: Infinity` keeps real usage far under them, and the
  legacy engines can still be switched on).

Denied requests increment
`folio_rate_limit_denied_total{bucket="ocr"|"ocr.detect"}`.

## Telemetry

| Metric | Type | Labels | Purpose |
|---|---|---|---|
| `folio_ocr_cache_hits_total` | counter | — | Cache effectiveness |
| `folio_ocr_cache_misses_total` | counter | — | Pair with hits for hit-rate |
| `folio_ocr_pipeline_seconds` | histogram | `lang` | Wall time, detector + recognize (recorded only on success path) |
| `folio_ocr_recognize_seconds` | histogram | `lang` | Recognize-only — diff with `pipeline_seconds` exposes detector cost. For Western text it spans the line finder and the line recogniser |
| `folio_ocr_detect_seconds` | histogram | — | Detector-only inference via the text-regions endpoint |
| `folio_ocr_page_decode_seconds` | histogram | — | Archive read + decode of one page (decoded-page cache misses only) |
| `folio_ocr_permit_wait_seconds` | histogram | — | Time a request waited for an OCR permit (also recorded when it gave up) |
| `folio_ocr_busy_total` | counter | — | Requests refused with 503 `service_unavailable` after waiting `OCR_PERMIT_WAIT` (10 s) for a permit |
| `folio_ocr_detect_abandoned_total` | counter | — | Detections that finished after their request was gone (timeout, abort); each is written to the cache like any other (a write that fails or exceeds 5 s is dropped) |
| `folio_ocr_recognizer_panics_total` | counter | `model` | Recognizer panics caught during a request (`western`, `manga`); the engine answers `ocr_failed` until restart |
| `folio_ocr_cache_read_timeouts_total` | counter | — | OCR cache reads (result, detect, page size) that exceeded `CACHE_READ_TIMEOUT` (2 s) and were treated as misses |
| `folio_ocr_model_load_seconds` | histogram | `model` | Once per process per model (`detector`, `line_finder`, `western`, `manga`): verified download (if needed) + session build. `detector` is the page detector and `western` the Western recogniser of whichever engine set runs |
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
exporter's buckets (top bucket 10 s), so a detection by the legacy
detector only shows as `+Inf`.

## Models — auto-download + on-disk inspection

The model artifacts the pipeline depends on:

| Component | Where | Download source | Size |
| --- | --- | --- | --- |
| Page detector | `${HOME}/.cache/huggingface/hub/models--ogkalu--comic-text-and-bubble-detector/` | `ogkalu/comic-text-and-bubble-detector` | 11 MB |
| Line finder | `…/hub/models--PaddlePaddle--PP-OCRv5_mobile_det_onnx/` | `PaddlePaddle/PP-OCRv5_mobile_det_onnx` | 5 MB |
| Line recogniser + dictionary | `…/hub/models--PaddlePaddle--en_PP-OCRv5_mobile_rec_onnx/` | `PaddlePaddle/en_PP-OCRv5_mobile_rec_onnx` | 8 MB |
| `manga-ocr` (encoder + decoder + vocab) | `…/hub/models--mayocream--manga-ocr-onnx/` | `mayocream/manga-ocr-onnx` | ~440 MB |
| Legacy: `comic-text-detector` | `…/hub/models--mayocream--comic-text-detector-onnx/` | `mayocream/comic-text-detector-onnx` | ~95 MB |
| Legacy: Tesseract LSTM (`eng.traineddata`) | `${TESSDATA_PREFIX}` (default `${HOME}/.tesseract-rs/tessdata`) | tessdata_best (build script) | ~15 MB |

All model cards of the first four declare the Apache-2.0 licence. The
ONNX cache path follows `HOME`, not `HF_HOME` (see the gotcha below).
The first request that needs a model triggers its download and session
build: under a second each for the three small models once downloaded,
a few seconds for manga-ocr. The legacy rows are fetched or loaded only
with the engine switch on.

### Pinned model files

Every model file is pinned to a commit of its Hugging Face repository
and to a SHA-256 ([`ocr::models`](../../crates/server/src/ocr/models.rs)).
Before an engine builds its session, Folio itself fetches each missing
file from
`https://huggingface.co/<repo>/resolve/<commit>/<file>` over https
only (connect 20 s, per-read 60 s, 30 min overall, at most 5 redirects,
each https and never to an internal address literal, cut off past the
published size), hashes it while it streams into an exclusive,
randomly named temporary file (`.folio-part-*`, removed if the
download fails; a leftover from a killed process is swept after 35
minutes and is not counted by `GET /admin/ocr/models`), and only then moves it to
`hub/models--<org>--<repo>/snapshots/<commit>/<file>` and points
`refs/main` at `<commit>`. Folio's own sessions open the file at that
path; the `manga-ocr` and `comic-text-detector` constructors find it
there through the hub cache and do not download anything. A digest
mismatch deletes the
download, logs the file and both digests, and fails the request with
`500 ocr_model_unverified`; nothing unverified is loaded. A file
already in the cache (from before the pin, or changed on disk) is
hashed once per process; a mismatch deletes it and fetches it again
from the pinned commit.

The fetch runs in a background task, one per repository for the whole
process. OCR requests sit under the 60 s JSON route timeout; the three
small models (24 MB together) normally arrive well inside it, while the
first Japanese request after a cold start may still time out during
the ~440 MB manga download. The download carries on, and later
requests wait on it or find the verified files. After a failed attempt
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
| --- | --- | --- | --- | --- |
| `ogkalu/comic-text-and-bubble-detector` | `16e8a622f91fabc6b5b65c96d32d1183f8843546` | `detector-v4-s_int8.onnx` | `5fe9e4f5…c931ea79` | 11,120,765 |
| `PaddlePaddle/PP-OCRv5_mobile_det_onnx` | `e6f4fa85f00e168c862bc462aebca69eef9b3d3d` | `inference.onnx` | `a4319856…97b6e61d` | 4,826,518 |
| `PaddlePaddle/en_PP-OCRv5_mobile_rec_onnx` | `3fafbc3b5dcf93dd72add9f48368be8a3a2cd33b` | `inference.onnx` | `b5f833df…eabc557f` | 7,848,423 |
| | | `inference.yml` | `27e91d05…8b441067` | 3,964 |
| `mayocream/manga-ocr-onnx` | `24b12778d85800835e2ca409236de281b8ab7b9f` | `encoder_model.onnx` | `15fa8155…d995def85f` | 343,454,249 |
| | | `decoder_model.onnx` | `ef776526…fa61f5e8` | 117,480,262 |
| | | `vocab.txt` | `5cb5c558…4d6a36a3` | 30,216 |
| `mayocream/comic-text-detector-onnx` (legacy) | `a5d67ec772adef819ef5b0e7aa701fcf4c8bf74a` | `comic-text-detector.onnx` | `1a86ace7…071d718f` | 94,669,756 |

(Full digests in `models.rs`. Looked up 2026-11-12 for the first three
repositories, 2026-11-11 for the two `mayocream` ones.) Two more line
recognisers are registered there with their pins and never fetched
unless selected: `PaddlePaddle/latin_PP-OCRv5_mobile_rec_onnx` at
`89d3a50e…` and `PaddlePaddle/PP-OCRv6_small_rec_onnx` at `b8f84f0b…`.
There is no setting for another model source. For an air-gapped host,
copy the files to `snapshots/<commit>/` under the cache above; they are
verified like any cached file.

**Updating a pin:** `curl -s
"https://huggingface.co/api/models/<repo>/revision/main?blobs=true"`
gives the commit (`sha`) and, per LFS file, `lfs.sha256` and `size`.
For a non-LFS file (`vocab.txt`, `inference.yml`) download it from
`/resolve/<commit>/<file>` and run `sha256sum` (its `git hash-object`
should equal the listing's `blobId`). Change `revision`, `sha256` and
`size` in `models.rs` and this table together. A new line recogniser
revision must still pass the dictionary check at load (run the ignored
real-model tests).

**Tesseract data** (legacy engines) is not fetched at runtime. In the image build,
`scripts/docker/tesseract-inputs.sh` downloads `eng` and `tur` from a
pinned `tessdata_best` commit (and the Leptonica + Tesseract sources),
checks each against a pinned SHA-256 and places them where the
`tesseract-rs` build script would otherwise download them, so it skips
its own unchecked download; after the build the Dockerfile checks again
that the tessdata directory holds exactly those two files. A local
`cargo build` outside Docker runs the upstream build script unchanged
and downloads all four files unchecked into `~/.tesseract-rs/` (cached
there afterwards); see
[dependency-management.md](dependency-management.md#tesseract-rs-build-inputs).

In the Folio Docker image:

- `eng.traineddata` and `tur.traineddata` are baked into the image at
  `/app/tessdata/` (the build compiles Tesseract with the pinned sources
  in place; the pinned tessdata is copied into the runtime
  image). `TESSDATA_PREFIX`
  defaults to that path.
- `HOME=/data` so every ONNX model cache lands under
  `/data/.cache/huggingface/hub/` (in the persistent volume) and
  survives container restarts. `HF_HOME=/data/.cache/huggingface` is
  set in parallel — see the gotcha below.
- Both env vars are operator-overridable for air-gapped deploys; the
  loader exactly mirrors what `GET /admin/ocr/models` reports.

### Upstream cache-resolution gotcha

`manga-ocr` and `comic-text-detector` 0.5.1 call
`hf_hub::api::sync::Api::new()`, which internally uses
`Cache::default()`. **`Cache::default()` ignores `HF_HOME`** — only
`Api::from_env()` / `Cache::from_env()` honor it. The cache path those
crates read is `dirs::home_dir() + .cache/huggingface/hub/`, i.e. driven
by `HOME`, not `HF_HOME`. Folio's pinner and its own sessions use the
same `Cache::default()` root, so every model is in one place.

Folio's `Dockerfile` sets `HOME=/data` so this works out cleanly:
everything resolves to `/data/.cache/...` and the admin endpoint
reports the same. `HF_HOME` is set in parallel for forward
compatibility.

Local-dev builds without `HOME` set will land the cache at
`~/.cache/huggingface/hub/` (the user's real home): about 24 MB for
the Western engines the first time an OCR request or test runs, plus
manga-ocr's 440 MB on the first Japanese request.

## Admin surfaces

- `GET /admin/ocr/models` — JSON view of cache state. Read-only;
  `RequireAdmin` extractor. Rows, in order: the page detector
  (`comic-text-and-bubble-detector`), the line finder
  (`pp-ocrv5-mobile-det`), the line recogniser
  (`en-pp-ocrv5-mobile-rec`), `manga-ocr`, then the two legacy engines
  (`comic-text-detector`, `tesseract-eng`). The expected size of an
  ONNX row is the sum of its pinned files. On an install that never ran
  the legacy engines `comic-text-detector` stays "missing"; that is
  expected.
- `/admin/server` UI tile — pairs the JSON with the build /
  dependencies cards so an operator can verify both binary and
  models are healthy in one place. Lives in
  [`web/components/admin/observability/ServerInfoClient.tsx`](../../web/components/admin/observability/ServerInfoClient.tsx).

## Web client

Two reader features use these endpoints: **Show page text** (`r`,
`PageTextPanel.tsx`) and the markers tool's **Highlight + capture text**
(`x`, `MarkerOverlay.tsx` + `marker-selection.ts` + `MarkerEditor.tsx`).
Both are lazy chunks; the helpers below are imported only by them, so
none of it is in the reader's first load.

**Page size — one source.** The server crops the _original_ decoded
page, so every pixel rectangle the client sends must be in original
page pixels. `resolvePageSize` in
[`web/lib/reader/page-size.ts`](../../web/lib/reader/page-size.ts) is
the one definition, in order of trust: the text-regions response's
`page_w` / `page_h` (the decode the server crops), the page list's
`image_width` / `image_height`, and only then the `<img>`'s natural size
— and that only when the element has no `srcset`. With width-descriptor
variants (`pageBytesSrcSet`) the browser reports a density-corrected
natural size that follows the variant and the layout; computing a
rectangle from it sent a smaller rectangle shifted toward the top-left.
The page list's size is not guaranteed to be the decoded size either
(a tagger's stale ComicInfo page sizes, an EXIF-rotated JPEG), so the
client does not rely on any size for what it sends: every OCR request
carries `region_pct` (the percent region exactly as drawn, detected or
stored), which the server maps onto the page it decodes, plus the pixel
`region` computed with `resolvePageSize` only when a size is known, for a
server that predates `region_pct`. With no size known, `region_pct`
alone goes. A returned `refined_bbox` is converted back to percent with
the response's `page_w` / `page_h`, falling back to `resolvePageSize`
only for an older server that does not send them. The per-box query is
keyed by the percent box (`textBoxKey`), never by a pixel rectangle.
(`region_pct` and the response's `page_w` / `page_h` come with a
separate server change; until it is merged they are typed locally in
`marker-selection.ts`.) The image-hash highlight crops
the original page bytes it loads itself, against that image's own size,
so the same area always hashes the same pixels.

**The text boxes of a page — one definition.** The server returns
balloons and free-text regions. `pageTextBoxes` in
[`web/lib/reader/text-regions.ts`](../../web/lib/reader/text-regions.ts)
keeps the enclosing box and drops boxes 80% inside a larger one (written
for the previous detector's block and line boxes; with balloon boxes it
only matters when the detector returns one box inside another);
`readingOrderRegions` orders that set. Capture draws its outlines,
hit-tests a tap, and renders its keyboard proxies from this set, and the
panel reads it, so a tap, a key press and the panel read the same box
(the same pixel rectangle, so one server cache entry). A tap or key press
on a box goes through `boxTextQueryOptions` — the panel's query for that
box — so a box the panel already read is captured without a request.

**Capture mode.** Entering it fetches the page's text regions
(`useIssuePageTextRegions`, `staleTime: Infinity`) and draws the boxes.
Tapping a box reads it recognizer-only; dragging reads the drawn
rectangle and sends `detect: true` only when the regions fetch succeeded
(cache warm ⇒ snap is cheap). One capture runs at a time; leaving the
mode (Esc, the pill's Cancel) cancels it — the request finishes but its
answer is ignored and no editor opens. A tap that hits no box (or lands
before the boxes load) does nothing and stays in the mode. A live status
says "Finding text regions…", why finding them failed (with Try again
for transient failures), or that the page has none. In webtoon view the
overlay keeps `touch-action: pan-y pinch-zoom` so the page still scrolls;
on a touch screen a drag starts after a 350 ms press-and-hold, and a
quick tap still taps a box (cost: a touch drag on a device whose primary
pointer is fine, e.g. a touch laptop, scrolls instead of selecting).

**Failures are not "no text".** `ocrCroppedRegion` resolves to the
result, or `null` when the server read the region and found no text; any
failure rejects with an `OcrRequestError` carrying the status, the
envelope's `code` and `Retry-After`. The words for each code live in one
place, `ocrFailureMessage` in
[`web/lib/reader/ocr-errors.ts`](../../web/lib/reader/ocr-errors.ts)
(unit-tested over every code). 503 / 429 / 408 and a dropped connection
are transient: never cached as a result, retried by the panel on its own
(three times for a read, twice for a detection, waiting `Retry-After` or
2 / 4 / 8 s, capped at 30 s), and by the user in capture (the error toast
carries Try again). The panel says how many regions failed and why, and
offers Try again for transient ones.

**Request hygiene.** Detection GETs carry TanStack's query `signal`, so
one whose last observer goes away (page turned, panel closed) is aborted
client-side; the server still finishes its run. In webtoon view neither
the panel nor capture mode asks for a page's detection until that page
has been current for `WEBTOON_DETECT_DELAY_MS` (600 ms), so scrolling
past pages doesn't spend the 15-a-minute detection bucket. Detected boxes
and read text stay cached for `PAGE_TEXT_GC_MS` (30 minutes) after the
page is left.

The marker editor shows a copy-to-clipboard button, a low-confidence
hint (< 0.6 — effectively western-only since manga-ocr reports a
synthetic 1.0; the line recogniser scores clean lettering well above
0.9), and an Auto / Western (English) / Japanese select for
Detect text. Detect text on an existing marker saves at once, on the marker the read
was started for (by id; a read that lands after the editor moved to
another marker or closed is dropped), and the
editor then counts that text as saved (Cancel does not offer to discard
it, Save does not resend it). Detect text and Clear ask first when the
captured text was typed or corrected by hand (it differs from what
recognition produced); they do not ask when it is still exactly the
recognized text.

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
- Engine unit tests, no model file needed, next to the code: detector
  input, classes and clamping (`ocr/bubbles.rs`); which boxes are
  regions, what a run reads and what `refined_bbox` says
  (`ocr/pipeline.rs`); the Japanese crop (`ocr/manga_crop.rs`); line-finder
  input size, DB postprocess on synthetic probability maps and reading
  order (`ocr/lines.rs`); recogniser batch shape and padding, CTC
  decode, the dictionary reader and the class check
  (`ocr/recognizer/ctc.rs`); cleanup rules per engine set
  (`ocr/postprocess.rs`); fingerprints per engine set (`ocr/cache.rs`).
- Real models (ignored by default; they fetch the pinned files on first
  run):
  [`crates/server/tests/ocr_recognizer.rs`](../../crates/server/tests/ocr_recognizer.rs)
  runs the page detector and the line recogniser on a public-domain
  page and three balloon crops under
  [`crates/server/tests/fixtures/ocr/`](../../crates/server/tests/fixtures/ocr/README.md),
  loads every registered line recogniser against its dictionary, and
  prints timings. Run with
  `cargo test -p server --test ocr_recognizer -- --ignored`
  (the legacy Tesseract and the manga-ocr cases are in the same file;
  `--skip manga` avoids the 440 MB download).
- Web client fetch shape + failure paths:
  [`web/tests/reader/marker-selection.test.ts`](../../web/tests/reader/marker-selection.test.ts);
  page size, box set and failure words:
  [`web/tests/reader/text-capture-helpers.test.ts`](../../web/tests/reader/text-capture-helpers.test.ts);
  request bodies for tap / drag / snapped box, same box for tap / key /
  panel, one capture at a time, panel retries:
  [`web/tests/dom/text-capture.test.tsx`](../../web/tests/dom/text-capture.test.tsx).
- Speed and accuracy on real pages:
  [`crates/tools/ocr-bench`](../../crates/tools/ocr-bench/README.md), a
  development-only harness that times this pipeline and candidate
  engines and scores their text against reference transcriptions. Its
  runs are manual; CI only compiles it and runs its unit tests.

## Future work

- Remove the legacy engine set: `COMIC_OCR_LEGACY_ENGINES`,
  `ocr/detector.rs`, `ocr/recognizer/western.rs`, the Tesseract-only
  cleanup rules, and the Tesseract build plumbing (Dockerfile,
  `scripts/docker/tesseract-inputs.sh`, CI caches).
- Line finding: most of the remaining reading errors are lines the
  finder drops or takes from a touching neighbour balloon. Candidates:
  accept its result only when the lines cover the balloon's ink and
  retry at another scale otherwise; drop lines that belong to a
  neighbouring region's box.
- Japanese: measure the two crops (`MANGA_CROP`) on manga pages and
  settle the constant.
- Other Latin-script languages: `latin_PP-OCRv5_mobile_rec_onnx` is
  registered; choosing it per series (`series.text_language`) is not
  built.
- Sound effects, signs and titles are poorly covered by the regions
  (they are outside the "tap a balloon" use).
- manga-ocr real confidence: upstream PR to expose mean token
  probability from the decoder logits (today it's a synthetic
  1.0/0.0).
- Server-side "read all bubbles": the reader's page-text panel
  (`PageTextPanel.tsx`) already reads a whole page, from the client:
  the text regions, then one OCR request per region in reading order,
  a couple at a time. A batch route that does this in one request is
  not built.
- Background pre-detection at scan time (`issue_text_region`
  table) — not needed for reading a balloon: detection is fast enough
  on demand, and the content_hash-keyed Redis cache already gives
  invalidation-on-rescan. It would only serve a feature that needs the
  text of every page (search inside comics).
