#!/bin/sh
# Pinned, verified inputs for the `tesseract-rs` build script (Dockerfile
# `deps` and `rust-builder` stages only; never shipped in the runtime image).
#
# With its `build-tesseract` feature, tesseract-rs's build.rs downloads the
# Leptonica and Tesseract source archives (tag archives on GitHub) and
# `eng`/`tur` traineddata (the `main` branch of tessdata_best), none of them
# checked, and compiles the sources into the server binary. It skips each
# download when the target is already on disk (tesseract-rs 0.4.0 build.rs):
#   - `$HOME/.tesseract-rs/third_party/leptonica` exists     (line 79)
#   - `$HOME/.tesseract-rs/third_party/tesseract` exists     (line 87)
#   - `$HOME/.tesseract-rs/tessdata/<lang>.traineddata` exists (line 492)
# and its own extraction drops the archive's top-level folder
# (`leptonica-1.87.0/` -> `third_party/leptonica/`). So the image build
# fetches the same four files itself, checks each against a SHA-256 below,
# and lays them out there; the build script then downloads nothing.
#
# Subcommands:
#   fetch              download, verify and lay out the four inputs
#   check-crate [FILE] fail unless the tesseract-rs build.rs in the cargo
#                      registry (or FILE) fetches exactly the pinned sources
#                      and still skips on the paths above
#   verify             after a cargo build: the source trees and tessdata on
#                      disk are still the verified bytes (a fallback download
#                      of anything else fails here)
#
# Updating (docs/dev/dependency-management.md, "tesseract-rs build inputs"):
# when Renovate bumps tesseract-rs and `check-crate` fails, copy the new URLs
# from the crate's build.rs, download each archive and `sha256sum` it, and
# record the commit the tag points at (`git ls-remote <repo> 'refs/tags/<tag>*'`;
# the `^{}` line is the commit of an annotated tag). For tessdata, take a
# commit of tessdata_best (`git ls-remote https://github.com/tesseract-ocr/tessdata_best
# refs/heads/main`) and `sha256sum` the files downloaded from
# https://github.com/tesseract-ocr/tessdata_best/raw/<commit>/<lang>.traineddata.
set -eu

OUT=/root/.tesseract-rs
STATE=/usr/local/share/folio-tesseract-inputs
REGISTRY="${CARGO_HOME:-/usr/local/cargo}/registry/src"

# Leptonica 1.87.0: lightweight tag on commit 13275a278eb55b5746e33f95fbf5a2c8f604b3ab
# (the archive's zip comment names the same commit).
LEPTONICA_URL=https://github.com/DanBloomberg/leptonica/archive/refs/tags/1.87.0.zip
LEPTONICA_SHA256=2cfb7ebe6036f3d017280fa9a000c9699ab28be8dff8be8e2e948e2c79917eda
# Tesseract 5.5.2: annotated tag 9c516f4c6dc5b3a962dc7a2a39b5c3e21ce27466 on
# commit 6e1d56a847e697de07b38619356550e5cf4e8633 (also the zip comment).
TESSERACT_URL=https://github.com/tesseract-ocr/tesseract/archive/refs/tags/5.5.2.zip
TESSERACT_SHA256=449621680d529fde7a2d079805dce4687fef0a692c272be3ce431c30c69162d2
# tessdata_best at commit e12c65a9 (2024-03-09; `main` still pointed here
# on 2026-11-12, so these are the bytes the build script's own download got).
TESSDATA_COMMIT=e12c65a915945e4c28e237a9b52bc4a8f39a0cec
ENG_SHA256=8280aed0782fe27257a68ea10fe7ef324ca0f8d85bd2fd145d1c2b560bcb66ba
TUR_SHA256=e0c3338dc17503dc7d335a507c9ae01b2b46cfd07561171e1e1ac55d85e8e438

# What the crate's build.rs must contain for the layout above to stand in
# for its downloads. Any other https URL in it is a fetch nothing here pins.
CRATE_TESSDATA_BASE=https://github.com/tesseract-ocr/tessdata_best/raw/main/
CRATE_LANGUAGES='let languages = ["eng", "tur"];'

# Files the build script edits in place inside the Leptonica tree before
# compiling (build.rs lines 105-136); left out of the post-build digest check.
LEPTONICA_PATCHED='./src/environ.h ./prog/makefile.static'

die() {
    echo "tesseract-inputs: $*" >&2
    exit 1
}

download() { # url dest
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL \
        --retry 5 --retry-all-errors --connect-timeout 20 -o "$2" "$1" \
        || die "download failed: $1"
}

check_sha256() { # expected file url
    actual=$(sha256sum "$2" | cut -d' ' -f1)
    [ "$actual" = "$1" ] \
        || die "SHA-256 mismatch for $3: expected $1, got $actual. Nothing was compiled; see the update steps at the top of scripts/docker/tesseract-inputs.sh."
}

# Unpack like build.rs's download_and_extract: everything under the single
# top-level folder ends up directly in third_party/<name>.
extract() { # name zip
    work="$TMP/unpack-$1"
    mkdir "$work"
    unzip -q "$2" -d "$work"
    entries=$(find "$work" -mindepth 1 -maxdepth 1 | wc -l)
    top=$(find "$work" -mindepth 1 -maxdepth 1 -type d)
    [ "$entries" -eq 1 ] && [ -n "$top" ] \
        || die "$1 archive does not hold exactly one top-level folder"
    mv "$top" "$OUT/third_party/$1"
}

# sha256sum manifest of every regular file in third_party/<name>, minus the
# paths given as extra arguments.
manifest() { # name [excluded ./path ...]
    name=$1
    shift
    set -f
    excludes=''
    for path in "$@"; do
        excludes="$excludes ! -path $path"
    done
    # shellcheck disable=SC2086 # word splitting of $excludes is intended
    (cd "$OUT/third_party/$name" && find . -type f $excludes -print0 | LC_ALL=C sort -z | xargs -0 sha256sum)
    set +f
}

cmd_fetch() {
    [ "$(id -u)" -eq 0 ] && [ "${HOME:-}" = /root ] \
        || die "must run as root with HOME=/root (the build script reads \$HOME/.tesseract-rs)"
    [ ! -e "$OUT/third_party" ] && [ ! -e "$OUT/tessdata" ] \
        || die "$OUT is already populated; refusing to mix with content this script did not verify"

    TMP=$(mktemp -d)
    download "$LEPTONICA_URL" "$TMP/leptonica.zip"
    download "$TESSERACT_URL" "$TMP/tesseract.zip"
    tessdata_base="https://github.com/tesseract-ocr/tessdata_best/raw/$TESSDATA_COMMIT"
    download "$tessdata_base/eng.traineddata" "$TMP/eng.traineddata"
    download "$tessdata_base/tur.traineddata" "$TMP/tur.traineddata"

    # Every digest is checked before anything is unpacked or placed.
    check_sha256 "$LEPTONICA_SHA256" "$TMP/leptonica.zip" "$LEPTONICA_URL"
    check_sha256 "$TESSERACT_SHA256" "$TMP/tesseract.zip" "$TESSERACT_URL"
    check_sha256 "$ENG_SHA256" "$TMP/eng.traineddata" "$tessdata_base/eng.traineddata"
    check_sha256 "$TUR_SHA256" "$TMP/tur.traineddata" "$tessdata_base/tur.traineddata"

    mkdir -p "$OUT/third_party" "$OUT/tessdata" "$STATE"
    extract leptonica "$TMP/leptonica.zip"
    extract tesseract "$TMP/tesseract.zip"
    mv "$TMP/eng.traineddata" "$TMP/tur.traineddata" "$OUT/tessdata/"

    # shellcheck disable=SC2086 # one path per word
    manifest leptonica $LEPTONICA_PATCHED >"$STATE/leptonica.sha256"
    manifest tesseract >"$STATE/tesseract.sha256"
    printf '%s  %s\n' "$ENG_SHA256" eng.traineddata "$TUR_SHA256" tur.traineddata \
        >"$STATE/tessdata.sha256"
    rm -rf "$TMP"
    echo "tesseract-inputs: placed verified Leptonica, Tesseract and tessdata under $OUT"
}

cmd_check_crate() {
    if [ "$#" -gt 0 ]; then
        build_rs=$1
    else
        set -- "$REGISTRY"/*/tesseract-rs-*/build.rs
        [ "$#" -eq 1 ] && [ -f "$1" ] \
            || die "expected exactly one tesseract-rs build.rs under $REGISTRY, found: $*"
        build_rs=$1
    fi

    expected=$(printf '%s\n' "$LEPTONICA_URL" "$TESSERACT_URL" "$CRATE_TESSDATA_BASE" | LC_ALL=C sort)
    found=$(grep -o 'https\?://[^"[:space:]]*' "$build_rs" | LC_ALL=C sort -u)
    if [ "$found" != "$expected" ]; then
        echo "tesseract-inputs: $build_rs fetches from:" >&2
        printf '  %s\n' $found >&2
        echo "but the pinned inputs are:" >&2
        printf '  %s\n' $expected >&2
        die "the tesseract-rs version changed what it downloads. The pre-placed sources would be compiled in place of the ones it expects; update the pins in scripts/docker/tesseract-inputs.sh (steps at the top of the file)."
    fi
    grep -qF "$CRATE_LANGUAGES" "$build_rs" \
        || die "$build_rs no longer downloads exactly eng + tur traineddata; a new language would be fetched unverified. Update the pins in scripts/docker/tesseract-inputs.sh."
    for marker in 'join(".tesseract-rs")' 'third_party_dir.join("leptonica").exists()' \
        'third_party_dir.join("tesseract").exists()' 'if !file_path.exists()'; do
        grep -qF "$marker" "$build_rs" \
            || die "$build_rs no longer contains \`$marker\`: the skip-if-present logic this layout relies on changed. Re-read the build script before updating the pins."
    done
    echo "tesseract-inputs: $build_rs fetches exactly the pinned inputs"
}

# sha256sum -c in DIR against MANIFEST; on failure print the first few
# mismatches (not one line per file of a missing tree) and return 1.
check_tree() { # dir manifest
    report=$(mktemp)
    if (cd "$1" && sha256sum --quiet --strict -c "$2") >"$report" 2>&1; then
        rm -f "$report"
        return 0
    fi
    head -n 10 "$report" >&2
    rm -f "$report"
    return 1
}

cmd_verify() {
    for name in leptonica tesseract; do
        [ -d "$OUT/third_party/$name" ] || die "third_party/$name is missing"
        check_tree "$OUT/third_party/$name" "$STATE/$name.sha256" \
            || die "third_party/$name no longer matches the verified archive: the build used other sources"
    done
    for path in $LEPTONICA_PATCHED; do
        [ -f "$OUT/third_party/leptonica/$path" ] || die "third_party/leptonica/$path is missing"
    done
    check_tree "$OUT/tessdata" "$STATE/tessdata.sha256" \
        || die "tessdata no longer matches the pinned files"
    # The runtime image copies this whole directory: nothing but the two
    # pinned files may be in it.
    present=$(cd "$OUT/tessdata" && find . -mindepth 1 | LC_ALL=C sort | tr '\n' ' ')
    [ "$present" = "./eng.traineddata ./tur.traineddata " ] \
        || die "unexpected content in $OUT/tessdata: $present"
    echo "tesseract-inputs: sources and tessdata under $OUT are the verified bytes"
}

case "${1:-}" in
fetch) cmd_fetch ;;
check-crate)
    shift
    cmd_check_crate "$@"
    ;;
verify) cmd_verify ;;
*) die "usage: tesseract-inputs.sh fetch | check-crate [build.rs] | verify" ;;
esac
