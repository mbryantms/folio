# ocr-bench

A development-only harness that times the OCR pipeline and scores its text
against reference transcriptions, for the previous engines and for candidate
replacements, on the same pages and through the same scoring.

It is not part of the application. Nothing depends on this crate, the image
build (`cargo build --bin server --bin migration`) does not compile it, and
adding it changes no dependency's feature set. It never starts the HTTP
server, Postgres or Redis, and it never downloads: models and pages are read
from directories you name.

Pipeline background: [`docs/dev/ocr.md`](../../../docs/dev/ocr.md).

## What it measures

### The previous pipeline (the `full` step)

The pipeline the server ran before the engine swap: comic-text-detector's
model and Tesseract. The server no longer runs either; the step stays as the
baseline the candidates were chosen against. It calls the server's own code
where that code still exists (`util::image_decode::decode_limited`,
`WesternOcr::recognize`, which the server keeps for this tool, and
`postprocess::clean`) and runs comic-text-detector's model in a session the
tool builds (`ctd`, with `--threads`). The report has:

- **Timings**, cold (first run in the process) and warm (median, 90th
  percentile): file read, page decode, model load, detection, crop,
  preprocessing, the Tesseract call, postprocess; and latencies composed
  from them (outlines appear, tap one balloon, read the whole page).
- **Detection per page**, with contact sheets (detected boxes over the page,
  reference boxes in green).
- **End to end**: detect, recognise every box, pair the results with the
  reference regions.
- **Recognition alone** on the reference boxes with a 0, 4 and 8 % margin.
- **Tesseract variants**: one change each against the server's settings
  (binarisation, page-segmentation mode, scale, language data).
- The 25 worst regions, as crops with reference and output.

### Candidate engines (sections A to E)

| | Section | What is measured |
|---|---|---|
| A | Recognition alone | Each recogniser on each reference box (+4 % margin), once per line split. Rows ending `@tight` use the `ogk-m` text box that covers the region best instead of the rough reference box (only when `ogk-m` is among the detectors). |
| B | Detection alone | Each detector's boxes against the reference boxes: found, split, merged, partial, missed, and boxes matching no reference. |
| C | Tap one balloon | A tap at the centre of each reference box: the smallest detected box under it is cropped and read (for comic-text-detector, after the rule the reader applied to it, which drops a box nested in a larger one). `missed` is a tap with no box under it. Reported for all regions and for tap targets (balloons and captions) only. |
| D | Timings | Page decode, detection (cold and warm), line split, one balloon, one line, and a tap on a page never seen (decode + detect + one balloon) or already detected (one balloon). |
| E | Whole-page throughput | Decode, detect and read every balloon of a page; with `--throughput`, one worker process doing only that, so several can run side by side. |

Engines, by the names the arguments take:

| Kind | Name | What it is |
|---|---|---|
| detector | `ctd` | comic-text-detector's model (the previous detector) in a session the tool builds, with `--threads` |
| detector | `ogk-s`, `ogk-m` | ogkalu comic text and bubble detector, small and medium 8-bit models; units `bubble` and `text_bubble` |
| detector | `ppdetN` | PP-OCRv5 text-line detector on the page at long side N; units `lines` and `blocks` |
| recogniser | `tess-today` | the previous Western recogniser (Tesseract, kept in the server for this tool), scored after the server's postprocess |
| recogniser | `tess-best` | the Tesseract variant named by `--best-tess` |
| recogniser | `pp-v5-en`, `pp-v5-latin`, `pp-v6-tiny`, `pp-v6-small`, `pp-v6-medium` | PP-OCR line recognisers, scored on their raw text |
| line split | `ppdet`, `ppdet2x` | PP-OCRv5 line detector on the crop, or on the crop enlarged 2x |
| line split | `page-lines`, `ctd-lines` | lines from a page-level pass that fall inside the crop |
| line split | `profile`, `none` | row projection profile; the whole crop as one line (control) |

The PP-OCR recognisers read one line at a time, so a balloon is first split
into lines; the split is part of what sections A and C compare.

### Scoring

Character error rate (CER) is the edit distance to the reference divided by
the reference length; word error rate (WER) the same over words. Both sides
are upper-cased, whitespace is collapsed, curly quotes and long dashes are
unified, and spacing around punctuation is normalised (`src/score.rs`). The
tables count regions at CER ≤ 5 %, ≤ 10 % and ≤ 25 %; `mean CER, missed=1`
caps each region at 100 % and counts a missed one as 100 %.

## Data

| What | Where it comes from |
|---|---|
| Reference transcriptions | [`reference/truth/`](reference/truth/) (12 pages, 166 regions; conventions and known weaknesses in its README) |
| The 12 page images | not in the repository; `python3 -I reference/fetch.py pages DIR` |
| Candidate models | not in the repository; `python3 -I reference/fetch.py models DIR` (14 files, 178 MB; `--only TEXT` for a subset) |
| Previous detector model | comic-text-detector's model in the Hugging Face cache; the tool refuses to run it unless it is already there with the digest pinned in `src/engines.rs` (the tool never downloads it) |
| Tesseract language data | the `tesseract-rs` build, or `TESSDATA_PREFIX` |

`fetch.py` takes every address, size and SHA-256 from
[`reference/pages.json`](reference/pages.json) and
[`reference/models.json`](reference/models.json), writes only into a new or
empty directory, keeps a file only when its size and digest match, and can
check a directory you already have (`fetch.py verify pages|models DIR`).
Sources, rights statements and licences:
[`reference/PROVENANCE.md`](reference/PROVENANCE.md).

Any other pair of directories works the same way: `--pages` holds
`<book>/p<NNN>.jpg` (png and webp too), `--truth` holds
`<book>-p<NNN>.json`. Pages without a reference file are timed but not
scored. `scripts/make_synthetic.py OUT` renders a few synthetic pages with a
known transcription for a smoke test (it needs Pillow and the fonts listed at
its top).

## Running

```sh
cargo build --release -p ocr-bench
python3 -I crates/tools/ocr-bench/reference/fetch.py pages  "$DATA/pages"
python3 -I crates/tools/ocr-bench/reference/fetch.py models "$DATA/models"

MODELS="$DATA/models" crates/tools/ocr-bench/scripts/run.sh \
  target/release/ocr-bench "$DATA/pages" \
  crates/tools/ocr-bench/reference/truth "$DATA/results" STEP
```

`run.sh BIN PAGES_DIR TRUTH_DIR OUT_DIR STEP [extra ocr-bench arguments]`
runs one step:

| Step | What it runs | Needs a quiet machine |
|---|---|---|
| `full` | the previous pipeline: timings, accuracy, Tesseract variants, contact sheets | yes |
| `cand` | sections A, B and C for every detector and recogniser | for its time columns |
| `tap` | section C for `BEST_DET` with each recogniser in `TAP_RECS` on `BEST_SPLIT` lines; the previous detector is not run | no (ignore its time columns) |
| `time4` | 4 pinned cores: the previous pipeline, the chosen pair, `ALT_DET`, and a recogniser thread sweep (1, 2, 4) | yes |
| `time8` | 8 pinned cores: the previous pipeline and the chosen pair | yes |
| `timefree` | no pinning, ONNX Runtime default threads: the previous pipeline and the chosen pair | yes |
| `workers` | throughput of the chosen pair as W processes x T threads on 8 and on 4 pinned cores | yes |
| `omp` | the previous detector on 4 pinned cores with `OMP_NUM_THREADS` = 1, 4, 8 | yes |

Today's detector can take tens of seconds a page, so `full`, `cand` and the
timing steps are slow; `tap` is the quick accuracy check.

| Variable | Default | Meaning |
|---|---|---|
| `MODELS` | (required, except `full` and `omp`) | candidate model directory |
| `BEST_DET` | `ogk-s` | detector of the chosen pair |
| `BEST_PP` | `pp-v6-small` | recogniser of the chosen pair |
| `BEST_SPLIT` | `ppdet2x` | line split used with it |
| `ALT_DET` | `ogk-m` | second detector timed with `BEST_PP` |
| `TAP_RECS` | `pp-v5-en pp-v5-latin pp-v6-small` | recognisers the `tap` step compares |
| `CPU4`, `CPU8` | `0,2,4,6` and `0,2,…,14` | CPU lists for `taskset` in the pinned steps |
| `LOAD_MAX` | `2` | 1-minute load average a timed run may start at |
| `LOAD_WAIT` | `300` | seconds to wait for that before giving up |
| `SKIP_TODAY` | unset | set to skip the previous pipeline in the timing steps |
| `OCR_BENCH_ORT_PROFILE` | unset | directory for ONNX Runtime per-operator profiles of the candidate sessions |

The binary can also be run directly; `ocr-bench --help` lists every
argument, and `--only TEXT` / `--max-pages N` limit the pages.

Each run writes `<out>/<label>.md` and `<out>/<label>.json` (every
per-region record), plus `<label>-sheets/` and `<label>-worst/` where they
apply. The reports name the CPU and local paths of the machine they ran on:
keep result directories out of the repository.

## Timings need a quiet machine and pinned cores

- Use the release build, and do not run it under `nice`.
- Pin it. `time4`, `time8`, `workers` and `omp` run under `taskset` with
  `CPU4` / `CPU8`; set those to one hardware thread per physical core of
  the machine at hand (`lscpu -e`), and to cores of one kind on a machine
  that mixes fast and slow cores.
- Nothing else should be running. `run.sh` waits for the 1-minute load
  average to fall to `LOAD_MAX` and gives up after `LOAD_WAIT` seconds; each
  report records the load average at its start and end, the CPUs the
  process was allowed, the thread settings and the build profile. Discard a
  run whose recorded load is high.
- The thread count is part of the result. Every session the tool builds
  uses `--threads`.
- Cold figures include one-off work (model load, first inference); warm
  figures are repeats in the same process. Report them apart.
- Figures do not carry over to another CPU. The 8-bit detector models in
  particular depend on which vector instructions the CPU has.

Accuracy does not depend on load: `cand` and `tap` give the same counts on a
busy machine.

## What the numbers do not cover

- **The HTTP route.** No authentication or access check, rate limit, OCR
  permit (the server runs at most two OCR jobs at once), Redis result and
  detection caches, decoded-page cache, or reading the page out of an
  archive: pages here are loose image files. A tap in the reader costs these
  on top of one recognition.
- **Concurrency.** One request at a time, in one process (apart from the
  `workers` step, which measures candidates only).
- **The Japanese path** (the manga-ocr recogniser) is not measured.
- **The reference set is narrow**: 12 pages of 1940 to 1953 scans, upper-case
  hand lettering, English. No modern digital lettering, no lower-case font,
  one light-on-dark region, no page with more than 13 balloons. The
  transcription was made by an AI model and has not been checked by a
  person.
- **Reference boxes are approximate**, so section B is indicative and the
  tap in section C is the centre of a rough box, not a real finger.
- **Display lettering** (sound effects, signs, titles) is scored but is not
  what the tap use is about; read the tap-target tables for that.
- **Candidate results describe this tool's own pre- and postprocessing** of
  those models (`src/cand.rs`: axis-aligned line boxes, greedy decoding), not
  the upstream reference implementations.
- **Tesseract** is whatever version the local `tesseract-rs` build linked;
  the report records it. It can differ from the version in the image.
- **Two reference pages are re-encoded from JPEG 2000** by `fetch.py`. If
  the local Pillow build writes different bytes, the page is refused unless
  `--accept-reencode` is given, and figures for it may then differ slightly.

## Adding an engine

A detector implements `PageDetector` and a recogniser `CropRecognizer`
(`src/engines.rs`); timing, scoring and reports are shared. Candidate
engines on ONNX Runtime live in `src/cand.rs` and are registered in
`load_engines` (`src/stage2.rs`).
