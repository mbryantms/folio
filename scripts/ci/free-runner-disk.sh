#!/usr/bin/env bash
# Free root-disk space on a GitHub-hosted ubuntu-24.04 runner before a big
# Rust build (`rust-test` shards, `sidecar-interop`).
#
# Hosted runners come in two sizes at random: a 145 GB `/dev/root` (~86 GB
# free on arrival) and a 72 GB `/dev/mapper/cow-root` (~13 GB free). A
# cold workspace test build needs ~40 GB (2026-11-01, strip=symbols; see the
# rust-test job comments in .github/workflows/ci.yml), so:
#
#   * >= SKIP_ABOVE_GB free on arrival  → nothing to do (the big runner);
#     skipping saves the ~2.5-3.5 min the deletes cost.
#   * otherwise                         → delete preinstalled SDKs this job
#     never touches, reporting how much each one freed.
#
# Nothing listed here is used by the build: the Rust toolchain comes from
# rustup, nextest is a prebuilt binary, C/C++ deps (Tesseract/Leptonica,
# aws-lc, zstd, libwebp) build with the image's gcc/g++ + cmake, and ort
# downloads its own onnxruntime. `docker image prune` keeps the running
# service containers' images (postgres/redis) and drops the rest.
#
# Usage: free-runner-disk.sh [--keep-toolcache] [--force]
#   --keep-toolcache  keep /opt/hostedtoolcache (jobs that use setup-python)
#   --force           delete even on a big runner
# Env: SKIP_ABOVE_GB (default 80).
set -euo pipefail

keep_toolcache=0
force=0
for arg in "$@"; do
  case "$arg" in
    --keep-toolcache) keep_toolcache=1 ;;
    --force) force=1 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done
skip_above_gb="${SKIP_ABOVE_GB:-80}"

avail_mb() { df --output=avail -BM / | tail -1 | tr -dc '0-9'; }

df -h
lsblk -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINTS 2>/dev/null || true

before=$(avail_mb)
if [ "$force" -eq 0 ] && [ "$before" -ge $((skip_above_gb * 1024)) ]; then
  echo "$((before / 1024)) GB free on / (>= ${skip_above_gb} GB): big runner, nothing to delete."
  exit 0
fi

paths=(
  /usr/local/lib/android /usr/share/dotnet /opt/ghc /usr/local/.ghcup
  /usr/local/share/powershell /usr/share/swift /usr/local/share/chromium
  /usr/local/lib/node_modules /usr/lib/google-cloud-sdk /opt/microsoft
  /opt/az /usr/share/miniconda /opt/pipx
  # Added 2026-11-01 (E14): JVMs, Julia, vcpkg, browsers, cloud CLIs, LLVM
  # (rustc ships its own; the cc/cmake builds use gcc).
  /usr/lib/jvm /usr/local/julia* /usr/share/kotlinc /usr/local/share/vcpkg
  /opt/google /usr/lib/firefox /usr/lib/mono /usr/local/aws-cli
  /usr/local/aws-sam-cli /usr/share/gradle* /usr/share/apache-maven*
  /usr/share/sbt /opt/mssql-tools /usr/lib/llvm-*
)
if [ "$keep_toolcache" -eq 0 ]; then
  paths+=(/opt/hostedtoolcache)
fi

start=$(date +%s)
echo "| path | freed (MB) |"
echo "| --- | ---: |"
for p in "${paths[@]}"; do
  [ -e "$p" ] || continue
  a=$(avail_mb)
  sudo rm -rf "$p"
  echo "| $p | $(($(avail_mb) - a)) |"
done
a=$(avail_mb)
sudo docker image prune -af >/dev/null
echo "| docker image prune | $(($(avail_mb) - a)) |"
echo "freed $((($(avail_mb) - before) / 1024)) GB in $(($(date +%s) - start))s"
df -h /
