#!/usr/bin/env bash
# Run the OCR bench in the configurations the README describes.
#
#   run.sh BIN PAGES_DIR TRUTH_DIR OUT_DIR STEP [extra ocr-bench args]
#
# Environment:
#   MODELS      candidate model directory (see reference/models.json);
#               required by every step but `full` and `omp`
#   BEST_DET    detector of the chosen pair            (default ogk-s)
#   BEST_PP     PP-OCR recogniser of the chosen pair   (default pp-v6-small)
#   BEST_SPLIT  line split used with it                (default ppdet2x)
#   ALT_DET     second detector timed with BEST_PP     (default ogk-m)
#   TAP_RECS    recognisers the `tap` step compares
#               (default "pp-v5-en pp-v5-latin pp-v6-small")
#   CPU4, CPU8  CPU lists for `taskset` in the pinned steps (defaults
#               0,2,4,6 and 0,2,...,14: one hardware thread per core on a
#               machine whose sibling threads are numbered in pairs; set
#               them for the machine at hand, see `lscpu -e`)
#   LOAD_MAX    1-minute load average a timed run may start at (default 2)
#   LOAD_WAIT   seconds to wait for it before giving up    (default 300)
#   SKIP_TODAY  set to skip the previous pipeline in time4/time8/timefree
#
# STEP is one of:
#   full     unconstrained baseline: timings, accuracy, Tesseract variants,
#            contact sheets
#   cand     candidate accuracy (sections A, B, C), every detector and
#            recogniser the binary knows
#   tap      accuracy only: section C for BEST_DET with each recogniser
#            in TAP_RECS on BEST_SPLIT lines (plus the A and B rows for
#            that pair). Does not run the previous detector and does not wait
#            for a quiet machine; ignore its time columns.
#   time4    4 pinned cores (CPU4, OMP_NUM_THREADS=4, 4 intra-op
#            threads): the previous pipeline, the chosen pair, the
#            alternative detector, then the recogniser thread sweep
#            (1/2/4) for BEST_PP and pp-v6-medium
#   time8    8 pinned cores (CPU8, 8 threads): the previous pipeline and the
#            chosen pair
#   timefree unconstrained, ORT default threads: the previous pipeline and
#            the chosen pair
#   workers  throughput of the chosen pair as W parallel processes x T
#            intra-op threads: 1x8 2x4 4x2 8x1 on 8 cores, 1x4 2x2 4x1 on
#            4 cores. Each worker builds its own sessions.
#   omp      the previous detector only on 4 pinned cores with OMP_NUM_THREADS
#            = 1, 4, 8 (does ONNX Runtime read it?)
#
# Each timed run waits for the 1-minute load average to fall to LOAD_MAX.
# Run the release binary, never under `nice`.
set -euo pipefail
BIN=$1 PAGES=$2 TRUTH=$3 OUT=$4 STEP=$5
shift 5
LOAD_MAX=${LOAD_MAX:-2}
CPU4=${CPU4:-0,2,4,6}
CPU8=${CPU8:-0,2,4,6,8,10,12,14}
BEST_DET=${BEST_DET:-ogk-s}
ALT_DET=${ALT_DET:-ogk-m}
BEST_PP=${BEST_PP:-pp-v6-small}
BEST_SPLIT=${BEST_SPLIT:-ppdet2x}
TAP_RECS=${TAP_RECS:-pp-v5-en pp-v5-latin pp-v6-small}

case $STEP in
full | omp) ;;
*) : "${MODELS:?set MODELS to the candidate model directory}" ;;
esac

# Waits up to LOAD_WAIT seconds (default 300) for the 1-minute load
# average to fall to LOAD_MAX (the previous run's own threads keep it up
# for a minute or two), then refuses.
check_load() {
  local waited=0 l1
  while :; do
    read -r l1 _ </proc/loadavg
    if awk -v l="$l1" -v m="$LOAD_MAX" 'BEGIN { exit !(l <= m) }'; then break; fi
    if [ "$waited" -ge "${LOAD_WAIT:-300}" ]; then
      echo "load average $l1 is above $LOAD_MAX; not timing now" >&2
      exit 3
    fi
    sleep 10
    waited=$((waited + 10))
  done
  echo "load average before run: $(cat /proc/loadavg)" >&2
}

# timed CFG CPUS THREADS LABEL [ocr-bench args]
timed() {
  local cpus=$2 th=$3 label=$4
  shift 4
  check_load
  local pin=()
  if [ -n "$cpus" ]; then pin=(taskset -c "$cpus"); fi
  OMP_NUM_THREADS=$th "${pin[@]}" "$BIN" --pages "$PAGES" --truth "$TRUTH" \
    --out "$OUT" --models "$MODELS" --cand-timing --threads "$th" \
    --best-pp "$BEST_PP" --best-split "$BEST_SPLIT" --label "$label" "$@"
}

pair_runs() { # CFG CPUS THREADS
  local cfg=$1 cpus=$2 th=$3
  # The previous pipeline: comic-text-detector's model (own session, $th
  # threads) and Tesseract. The detector takes seconds a page: no warm
  # repeat. Tesseract reads the whole crop: no PP splitter to load.
  # SKIP_TODAY=1 resumes a step after that (slow) run.
  [ -n "${SKIP_TODAY:-}" ] || BEST_SPLIT=profile timed "$cfg" "$cpus" "$th" "time-$cfg-today" --dets ctd --recs tess-today \
    --warm 0 --rec-warm 2 "${EXTRA[@]}"
  timed "$cfg" "$cpus" "$th" "time-$cfg-$BEST_DET-$BEST_PP" --dets "$BEST_DET" \
    --recs "$BEST_PP" --warm 2 --rec-warm 2 "${EXTRA[@]}"
}

EXTRA=("$@")
case $STEP in
full)
  check_load
  env -u OMP_NUM_THREADS "$BIN" --pages "$PAGES" --truth "$TRUTH" --out "$OUT" \
    --label full-unconstrained "$@"
  ;;
cand)
  check_load
  "$BIN" --pages "$PAGES" --truth "$TRUTH" --out "$OUT" --models "$MODELS" \
    --cand --threads 4 --best-pp "$BEST_PP" --best-split "$BEST_SPLIT" \
    --label cand-accuracy "$@"
  ;;
tap)
  for rec in $TAP_RECS; do
    "$BIN" --pages "$PAGES" --truth "$TRUTH" --out "$OUT" --models "$MODELS" \
      --cand --threads 4 --dets "$BEST_DET" --recs "$rec" --best-pp "$rec" \
      --splits "$BEST_SPLIT" --tap-splits "$BEST_SPLIT" \
      --best-split "$BEST_SPLIT" --label "tap-$BEST_DET-$rec" "$@"
  done
  ;;
time4)
  pair_runs cpu4 "$CPU4" 4
  timed cpu4 "$CPU4" 4 "time-cpu4-$ALT_DET-$BEST_PP" --dets "$ALT_DET" \
    --recs "$BEST_PP" --warm 2 --rec-warm 2 "$@"
  for rec in "$BEST_PP" pp-v6-medium; do
    for th in 1 2 4; do
      timed cpu4 "$CPU4" "$th" "time-cpu4-$rec-t$th" --dets none --recs "$rec" \
        --warm 0 --rec-warm 2 "$@"
    done
  done
  ;;
time8)
  pair_runs cpu8 "$CPU8" 8
  ;;
timefree)
  pair_runs free "" 0
  ;;
workers)
  for spec in "$CPU8:8:1" "$CPU8:4:2" "$CPU8:2:4" "$CPU8:1:8" \
    "$CPU4:4:1" "$CPU4:2:2" "$CPU4:1:4"; do
    IFS=: read -r cpus th w <<<"$spec"
    check_load
    n=$(tr ',' '\n' <<<"$cpus" | wc -l)
    for i in $(seq 1 "$w"); do
      OMP_NUM_THREADS=$th taskset -c "$cpus" "$BIN" --pages "$PAGES" --out "$OUT" \
        --models "$MODELS" --throughput --threads "$th" --dets "$BEST_DET" --recs "$BEST_PP" \
        --best-pp "$BEST_PP" --best-split "$BEST_SPLIT" \
        --label "workers-${n}c-${w}x${th}-w$i" "$@" &
    done
    wait
  done
  ;;
omp)
  for n in 1 4 8; do
    check_load
    OMP_NUM_THREADS=$n taskset -c "$CPU4" "$BIN" --pages "$PAGES" --out "$OUT" \
      --detector-only --warm 0 --label "detect-cpu4-omp$n" "$@"
  done
  ;;
*)
  echo "unknown step $STEP" >&2
  exit 2
  ;;
esac
