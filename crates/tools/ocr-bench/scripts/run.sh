#!/usr/bin/env bash
# Run the OCR bench in the configurations the report compares.
#
#   run.sh BIN PAGES_DIR TRUTH_DIR OUT_DIR STEP [extra ocr-bench args]
#
# STEP is one of:
#   full     unconstrained: timings, accuracy, variants, contact sheets
#   cpu4     timings only, pinned to 4 performance cores (taskset -c 0,2,4,6)
#            with OMP_NUM_THREADS=4
#   omp      decode + detection only, pinned to the same 4 cores, with
#            OMP_NUM_THREADS = 1, 4 and 8 (does ONNX Runtime read it?)
#   cand     candidate accuracy (A, B, C) with MODELS=<models dir>
#   candtime candidate timings (D, E), one process per detector +
#            recogniser combination and per recogniser thread count, both
#            unconstrained and pinned; BEST_PP picks the PP recogniser
#            (default pp-v6-small)
#
# Each step refuses to start while the 1-minute load average is above
# LOAD_MAX (default 2): timings on a busy machine mean little. Run the
# release binary, not under `nice`.
set -euo pipefail
BIN=$1 PAGES=$2 TRUTH=$3 OUT=$4 STEP=$5
shift 5
LOAD_MAX=${LOAD_MAX:-2}
CPUS=0,2,4,6

check_load() {
  read -r l1 _ </proc/loadavg
  if ! awk -v l="$l1" -v m="$LOAD_MAX" 'BEGIN { exit !(l <= m) }'; then
    echo "load average $l1 is above $LOAD_MAX; not timing now" >&2
    exit 3
  fi
  echo "load average before run: $(cat /proc/loadavg)" >&2
}

case $STEP in
full)
  check_load
  env -u OMP_NUM_THREADS "$BIN" --pages "$PAGES" --truth "$TRUTH" --out "$OUT" \
    --label full-unconstrained "$@"
  ;;
cpu4)
  check_load
  OMP_NUM_THREADS=4 taskset -c "$CPUS" "$BIN" --pages "$PAGES" --out "$OUT" \
    --timing --label timing-4cpu "$@"
  ;;
omp)
  for n in 1 4 8; do
    check_load
    OMP_NUM_THREADS=$n taskset -c "$CPUS" "$BIN" --pages "$PAGES" --out "$OUT" \
      --detector-only --label "detect-4cpu-omp$n" "$@"
  done
  ;;
cand)
  check_load
  "$BIN" --pages "$PAGES" --truth "$TRUTH" --out "$OUT" --models "$MODELS" \
    --cand --label cand-accuracy "$@"
  ;;
candtime)
  BEST_PP=${BEST_PP:-pp-v6-small}
  for cfg in free cpu4; do
    if [ "$cfg" = cpu4 ]; then pin=(taskset -c "$CPUS"); else pin=(); fi
    for combo in "ctd-crate:tess-today" "ctd:$BEST_PP" "ogk-s:$BEST_PP" "ogk-m:$BEST_PP" "ppdet960:$BEST_PP"; do
      det=${combo%%:*} rec=${combo#*:}
      check_load
      "${pin[@]}" "$BIN" --pages "$PAGES" --truth "$TRUTH" --out "$OUT" --models "$MODELS" \
        --cand-timing --dets "$det" --recs "$rec" --best-pp "$BEST_PP" --threads 4 \
        --label "time-$cfg-$det-$rec" "$@"
    done
    for th in 1 2 4; do
      check_load
      "${pin[@]}" "$BIN" --pages "$PAGES" --truth "$TRUTH" --out "$OUT" --models "$MODELS" \
        --cand-timing --dets none --threads "$th" --label "time-$cfg-recs-t$th" "$@"
    done
    for rec in pp-v6-small pp-v6-medium; do
      check_load
      "${pin[@]}" "$BIN" --pages "$PAGES" --truth "$TRUTH" --out "$OUT" --models "$MODELS" \
        --cand-timing --dets none --recs "$rec" --threads 4 --label "time-$cfg-rss-$rec" "$@"
    done
  done
  ;;
*)
  echo "unknown step $STEP" >&2
  exit 2
  ;;
esac
