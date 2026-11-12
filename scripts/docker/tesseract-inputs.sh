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
#   fetch                  download, verify and lay out the four inputs
#   check-crate [FILE]     fail unless the tesseract-rs build.rs in the cargo
#                          registry (or FILE) is the one these pins were made
#                          for: same download URLs, languages, output
#                          directory, skip-if-present checks and in-place edits
#   verify                 the placed trees and tessdata still match the pins
#   verify-build TARGET    `verify`, plus evidence that the cargo build in
#                          TARGET (a cargo target dir) built from those trees
#   tree-digests NAME DIR  print the listing and manifest digests of an
#                          unpacked source tree (used when moving a pin)
#
# Updating: docs/dev/dependency-management.md, "tesseract-rs build inputs".
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

# Digests of the unpacked trees (`tree-digests`): the exact listing (type +
# path of every entry, so an added file or a symlink shows) and the
# sha256sum manifest of every regular file except the edited ones below.
# Pinned so a rewritten manifest under $STATE cannot vouch for other bytes.
LEPTONICA_LISTING_SHA256=e35218a8048eca0fcf1e53c9fd0e153abfec113f42e33be562986546d6854428
LEPTONICA_MANIFEST_SHA256=781d86729f105f8b0a5fd1557e50bdc101399bfdbb6d9d6f631502e5944aa09f
TESSERACT_LISTING_SHA256=5cb0777a6f7ec521ade260c334ca10fc54a47ce9aad254111a882e5e520e1fc2
TESSERACT_MANIFEST_SHA256=53255e0ca36b7f9443920d1b734d5355fc0ee7b830afa33449e9f8f9e5f495e7

# The three files build.rs rewrites in place before compiling (0.4.0:
# environ.h + makefile.static at lines 105-136, CMakeLists.txt at 201-206;
# all three patterns occur in these sources). Each must be byte-identical
# to its pristine form or to the pristine form with exactly those
# replacements applied (scripts/docker/tesseract-edited-digests.py).
# Format: <tree> <path> <pristine sha256> <edited sha256>
EDITED_FILES='leptonica ./src/environ.h c68c894a79701a54b84d294dff944014680faa828ddfaa2a18352d1e8b220b62 dc44982bc48275e9b5b673fbc118f0384eaa2279cbfca4e12f671853191305fe
leptonica ./prog/makefile.static 78d8acbe41a3450798883b6dce56561bf05e9ca1e241eaf7fbcde2f1e922244b cd2ea85ee5befbc9a0797988b6eca184edca354d44e2716c851a66070016173a
tesseract ./CMakeLists.txt b93bc557de10898643dd4753ab0d68f9ecaf7617c77ff6b693b813860f4fc9c7 c3b754ab4291d01a30727c00ccc7f67452af82cc0f7844fcb9880db63eb356ef'

# What the crate's build.rs must contain for the layout above to stand in
# for its downloads. Any other https URL in it is a fetch nothing here pins.
CRATE_TESSDATA_BASE=https://github.com/tesseract-ocr/tessdata_best/raw/main/
CRATE_LANGUAGES='let languages = ["eng", "tur"];'
# SHA-256 of the whole `get_custom_out_dir` function (all platform
# branches; on Linux it returns $HOME/.tesseract-rs).
CRATE_OUT_DIR_FN_SHA256=db3099bbdce27697230f0afd66efabad8f435e430c246e2b71881c7a0c9b1658
# `std::fs::write(` calls in build.rs = the in-place edits above.
CRATE_IN_PLACE_WRITES=3

die() {
    echo "tesseract-inputs: $*" >&2
    exit 1
}

download() { # url dest
    # --speed-limit/--speed-time: a connected but stalled transfer fails
    # after a minute below 1 KiB/s instead of waiting for the job timeout.
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL \
        --retry 5 --retry-all-errors --connect-timeout 20 \
        --speed-limit 1024 --speed-time 60 --max-time 900 \
        -o "$2" "$1" \
        || die "download failed: $1"
}

sha256_of() {
    sha256sum "$1" | cut -d' ' -f1
}

check_sha256() { # expected file label [hint]
    actual=$(sha256_of "$2")
    [ "$actual" = "$1" ] \
        || die "SHA-256 mismatch for $3: expected $1, got $actual. ${4:-See docs/dev/dependency-management.md (\"tesseract-rs build inputs\").}"
}

# Edited paths of one tree, one per line.
edited_paths() { # tree
    printf '%s\n' "$EDITED_FILES" | awk -v t="$1" '$1 == t { print $2 }'
}

# Exact listing of DIR: entry type (find %y) and path, sorted. Fails on an
# empty tree or a failed find (dash has no pipefail, so no pipes here).
listing() { # dir dest
    raw=$(mktemp)
    (cd "$1" && find . -mindepth 1 -printf '%y %p\n') >"$raw" || die "cannot list $1"
    LC_ALL=C sort "$raw" >"$2" || die "cannot sort the listing of $1"
    rm -f "$raw"
    [ -s "$2" ] || die "$1 is empty"
}

# sha256sum manifest of every regular file in DIR except the tree's edited
# files, sorted by path.
manifest() { # tree dir dest
    files=$(mktemp)
    sorted=$(mktemp)
    excludes=$(mktemp)
    edited_paths "$1" >"$excludes"
    (cd "$2" && find . -type f -print) >"$files" || die "cannot list $2"
    grep -vxF -f "$excludes" "$files" | LC_ALL=C sort >"$sorted" || die "no files to hash in $2"
    [ -s "$sorted" ] || die "no files to hash in $2"
    # Paths come from find in a tree whose archive was already verified;
    # one per line, so a newline in a name would break this (none exist).
    (cd "$2" && tr '\n' '\0' <"$sorted" | xargs -0 sha256sum --) >"$3" || die "cannot hash $2"
    [ "$(wc -l <"$3")" -eq "$(wc -l <"$sorted")" ] || die "manifest of $2 is incomplete"
    rm -f "$files" "$sorted" "$excludes"
}

cmd_tree_digests() { # tree dir
    [ "$#" -eq 2 ] && [ -d "$2" ] || die "usage: tree-digests leptonica|tesseract DIR"
    tmp_l=$(mktemp)
    tmp_m=$(mktemp)
    listing "$2" "$tmp_l"
    manifest "$1" "$2" "$tmp_m"
    echo "listing  $(sha256_of "$tmp_l")  ($(wc -l <"$tmp_l") entries)"
    echo "manifest $(sha256_of "$tmp_m")  ($(wc -l <"$tmp_m") files)"
    printf '%s\n' "$EDITED_FILES" | awk -v t="$1" '$1 == t { print $2 }' | while read -r path; do
        echo "edited   $path pristine $(sha256_of "$2/$path")"
    done
    rm -f "$tmp_l" "$tmp_m"
}

# Defence in depth before unzip: no absolute paths, no `..` components,
# no symlink entries, exactly one top-level folder.
check_zip_entries() { # name zip
    names=$(mktemp)
    unzip -Z1 "$2" >"$names" || die "$1 archive cannot be listed"
    [ -s "$names" ] || die "$1 archive is empty"
    if grep -qE '^/|(^|/)\.\.(/|$)' "$names"; then
        die "$1 archive has an absolute or '..' entry"
    fi
    tops=$(cut -d/ -f1 "$names" | sort -u | wc -l)
    [ "$tops" -eq 1 ] || die "$1 archive does not hold exactly one top-level folder"
    # `unzip -Z` long listing: the first column is the mode, `l` = symlink.
    if unzip -Z "$2" | grep -q '^l'; then
        die "$1 archive contains a symlink entry"
    fi
    rm -f "$names"
}

# Unpack like build.rs's download_and_extract: everything under the single
# top-level folder ends up directly in third_party/<name>.
extract() { # name zip
    check_zip_entries "$1" "$2"
    work="$TMP/unpack-$1"
    mkdir "$work"
    unzip -q "$2" -d "$work" || die "$1 archive failed to unpack"
    entries=$(find "$work" -mindepth 1 -maxdepth 1 | wc -l)
    top=$(find "$work" -mindepth 1 -maxdepth 1 -type d)
    [ "$entries" -eq 1 ] && [ -n "$top" ] \
        || die "$1 archive does not hold exactly one top-level folder"
    mv "$top" "$OUT/third_party/$1"
}

pinned_tree_sha() { # tree listing|manifest
    case "$1/$2" in
    leptonica/listing) echo "$LEPTONICA_LISTING_SHA256" ;;
    leptonica/manifest) echo "$LEPTONICA_MANIFEST_SHA256" ;;
    tesseract/listing) echo "$TESSERACT_LISTING_SHA256" ;;
    tesseract/manifest) echo "$TESSERACT_MANIFEST_SHA256" ;;
    esac
}

cmd_fetch() {
    [ "$(id -u)" -eq 0 ] && [ "${HOME:-}" = /root ] \
        || die "must run as root with HOME=/root (the build script reads \$HOME/.tesseract-rs)"
    [ ! -e "$OUT" ] || die "$OUT already exists; refusing to mix with content this script did not verify"

    TMP=$(mktemp -d)
    download "$LEPTONICA_URL" "$TMP/leptonica.zip"
    download "$TESSERACT_URL" "$TMP/tesseract.zip"
    tessdata_base="https://github.com/tesseract-ocr/tessdata_best/raw/$TESSDATA_COMMIT"
    download "$tessdata_base/eng.traineddata" "$TMP/eng.traineddata"
    download "$tessdata_base/tur.traineddata" "$TMP/tur.traineddata"

    # Every digest is checked before anything is unpacked or placed.
    hint="Nothing was unpacked or compiled. A changed digest for an unchanged URL means stop and investigate before re-pinning (docs/dev/dependency-management.md, \"tesseract-rs build inputs\")."
    check_sha256 "$LEPTONICA_SHA256" "$TMP/leptonica.zip" "$LEPTONICA_URL" "$hint"
    check_sha256 "$TESSERACT_SHA256" "$TMP/tesseract.zip" "$TESSERACT_URL" "$hint"
    check_sha256 "$ENG_SHA256" "$TMP/eng.traineddata" "$tessdata_base/eng.traineddata" "$hint"
    check_sha256 "$TUR_SHA256" "$TMP/tur.traineddata" "$tessdata_base/tur.traineddata" "$hint"

    mkdir -p "$OUT/third_party" "$OUT/tessdata" "$STATE"
    extract leptonica "$TMP/leptonica.zip"
    extract tesseract "$TMP/tesseract.zip"
    mv "$TMP/eng.traineddata" "$TMP/tur.traineddata" "$OUT/tessdata/"
    printf '%s  %s\n' "$ENG_SHA256" eng.traineddata "$TUR_SHA256" tur.traineddata \
        >"$STATE/tessdata.sha256"

    for tree in leptonica tesseract; do
        listing "$OUT/third_party/$tree" "$STATE/$tree.listing"
        manifest "$tree" "$OUT/third_party/$tree" "$STATE/$tree.sha256"
    done
    check_state
    printf '%s\n' "$EDITED_FILES" | while read -r tree path pristine _edited; do
        check_sha256 "$pristine" "$OUT/third_party/$tree/$path" "pristine third_party/$tree/$path"
    done || exit 1
    # Reference time for verify-build: the libraries must be newer.
    touch "$STATE/placed"
    rm -rf "$TMP"
    echo "tesseract-inputs: placed verified Leptonica, Tesseract and tessdata under $OUT"
}

# The recorded listings and manifests are the pinned ones.
check_state() {
    for tree in leptonica tesseract; do
        check_sha256 "$(pinned_tree_sha "$tree" listing)" "$STATE/$tree.listing" "listing of third_party/$tree"
        check_sha256 "$(pinned_tree_sha "$tree" manifest)" "$STATE/$tree.sha256" "manifest of third_party/$tree"
    done
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
    check_state
    for tree in leptonica tesseract; do
        dir="$OUT/third_party/$tree"
        [ -d "$dir" ] && [ ! -L "$dir" ] || die "third_party/$tree is missing or not a directory"
        now=$(mktemp)
        listing "$dir" "$now"
        if ! cmp -s "$now" "$STATE/$tree.listing"; then
            diff "$STATE/$tree.listing" "$now" | head -n 10 >&2 || true
            die "third_party/$tree has entries that are not in the verified archive (added, removed or changed type; '>' lines above were added)"
        fi
        rm -f "$now"
        check_tree "$dir" "$STATE/$tree.sha256" \
            || die "third_party/$tree no longer matches the verified archive"
    done
    printf '%s\n' "$EDITED_FILES" | while read -r tree path pristine edited; do
        actual=$(sha256_of "$OUT/third_party/$tree/$path")
        [ "$actual" = "$pristine" ] || [ "$actual" = "$edited" ] \
            || die "third_party/$tree/${path#./} is neither the verified file nor the verified file with the build script's edits (got $actual)"
    done || exit 1
    check_tree "$OUT/tessdata" "$STATE/tessdata.sha256" \
        || die "tessdata no longer matches the pinned files"
    # The runtime image copies this whole directory: nothing but the two
    # pinned files may be in it.
    now=$(mktemp)
    listing "$OUT/tessdata" "$now"
    [ "$(cat "$now")" = "$(printf 'f ./eng.traineddata\nf ./tur.traineddata')" ] \
        || die "unexpected content in $OUT/tessdata: $(tr '\n' ' ' <"$now")"
    rm -f "$now"
    echo "tesseract-inputs: placed trees and tessdata under $OUT match the pins"
}

# Evidence that the build used the placed trees (tesseract-rs 0.4.0
# build_or_use_cached): the static libraries it built are in cache/ and in
# the install dirs, newer than the placement; ~/.tesseract-rs holds nothing
# else; no second third_party or .tesseract-rs appeared anywhere the build
# script could have used.
cmd_verify_build() { # target-dir
    [ "$#" -eq 1 ] && [ -d "$1" ] || die "usage: verify-build CARGO_TARGET_DIR"
    cmd_verify
    for lib in cache/leptonica/libleptonica.a cache/tesseract/libtesseract.a \
        leptonica/lib/libleptonica.a tesseract/lib/libtesseract.a; do
        [ -f "$OUT/$lib" ] && [ -s "$OUT/$lib" ] || die "$OUT/$lib is missing: the build did not build Tesseract from the placed trees"
        [ -n "$(find "$OUT/$lib" -newer "$STATE/placed")" ] \
            || die "$OUT/$lib is older than the placed sources"
    done
    top=$(cd "$OUT" && find . -mindepth 1 -maxdepth 1 | LC_ALL=C sort | tr '\n' ' ')
    [ "$top" = "./cache ./leptonica ./tessdata ./tesseract ./third_party " ] \
        || die "unexpected entries in $OUT: $top"
    third=$(cd "$OUT/third_party" && find . -mindepth 1 -maxdepth 1 | LC_ALL=C sort | tr '\n' ' ')
    [ "$third" = "./leptonica ./tesseract " ] || die "unexpected entries in $OUT/third_party: $third"
    out_dirs=$(find "$1" -maxdepth 4 -type d -path '*/build/tesseract-rs-*/out')
    [ -n "$out_dirs" ] || die "no tesseract-rs OUT_DIR under $1"
    # The cmake build of Tesseract (the last cmake project the build script
    # configures in OUT_DIR/build) records its source directory: it must be
    # the placed tree, in every OUT_DIR that has one, and in at least one.
    homes=$(mktemp)
    # shellcheck disable=SC2086 # one path per word
    for dir in $out_dirs; do
        if [ -f "$dir/build/CMakeCache.txt" ]; then
            grep '^CMAKE_HOME_DIRECTORY:INTERNAL=' "$dir/build/CMakeCache.txt" >>"$homes" || true
        fi
    done
    [ -s "$homes" ] || die "no CMakeCache.txt with a source directory in the tesseract-rs OUT_DIR under $1"
    other=$(grep -vxF "CMAKE_HOME_DIRECTORY:INTERNAL=$OUT/third_party/tesseract" "$homes" || true)
    [ -z "$other" ] || die "a tesseract-rs cmake build used another source directory: $other"
    rm -f "$homes"
    # shellcheck disable=SC2086 # one path per word
    stray=$(find /root /home $out_dirs \
        \( -path "$OUT/third_party" -prune \) -o \
        \( \( -name third_party -o -name .tesseract-rs \) -print \) 2>/dev/null \
        | grep -vxF "$OUT" || true)
    [ -z "$stray" ] || die "build left other source locations: $stray"
    echo "tesseract-inputs: libraries built after the placement, cmake source = the placed Tesseract tree, no other source location"
}

cmd_check_crate() {
    if [ "$#" -gt 0 ]; then
        build_rs=$1
    else
        set -- "$REGISTRY"/*/tesseract-rs-[0-9]*/build.rs
        [ "$#" -eq 1 ] && [ -f "$1" ] \
            || die "expected exactly one tesseract-rs build.rs under $REGISTRY, found: $*"
        build_rs=$1
    fi
    retry="update the pins (docs/dev/dependency-management.md, \"tesseract-rs build inputs\")"

    expected=$(printf '%s\n' "$LEPTONICA_URL" "$TESSERACT_URL" "$CRATE_TESSDATA_BASE" | LC_ALL=C sort)
    found=$(grep -o 'https\?://[^"[:space:]]*' "$build_rs" | LC_ALL=C sort -u || true)
    if [ "$found" != "$expected" ]; then
        echo "tesseract-inputs: $build_rs fetches from:" >&2
        printf '  %s\n' $found >&2
        echo "but the pinned inputs are:" >&2
        printf '  %s\n' $expected >&2
        die "the tesseract-rs version changed what it downloads. The pre-placed sources would be compiled in place of the ones it expects; $retry."
    fi
    grep -qF "$CRATE_LANGUAGES" "$build_rs" \
        || die "$build_rs no longer downloads exactly eng + tur traineddata; a new language would be fetched unverified; $retry."
    out_fn=$(mktemp)
    awk '/^    pub fn get_custom_out_dir\(\) -> PathBuf \{$/ { p = 1 } p { print } p && /^    }$/ { exit }' \
        "$build_rs" >"$out_fn"
    [ -s "$out_fn" ] && [ "$(sha256_of "$out_fn")" = "$CRATE_OUT_DIR_FN_SHA256" ] \
        || die "$build_rs: get_custom_out_dir changed, so the build script may no longer read \$HOME/.tesseract-rs on Linux and would download unverified sources elsewhere; re-read it and $retry."
    rm -f "$out_fn"
    for marker in 'let third_party_dir = project_dir.join("third_party");' \
        'third_party_dir.join("leptonica").exists()' \
        'third_party_dir.join("tesseract").exists()' \
        'let tessdata_dir = project_dir.join("tessdata");' \
        'if !file_path.exists()'; do
        grep -qF "$marker" "$build_rs" \
            || die "$build_rs no longer contains \`$marker\`: the skip-if-present logic this layout relies on changed; re-read the build script and $retry."
    done
    writes=$(grep -c 'std::fs::write(' "$build_rs" || true)
    [ "$writes" -eq "$CRATE_IN_PLACE_WRITES" ] \
        || die "$build_rs has $writes in-place writes (std::fs::write), the pins expect $CRATE_IN_PLACE_WRITES: the build script edits the sources differently, so verify would reject them; re-derive EDITED_FILES and $retry."
    echo "tesseract-inputs: $build_rs matches the pinned inputs"
}

case "${1:-}" in
fetch) cmd_fetch ;;
check-crate)
    shift
    cmd_check_crate "$@"
    ;;
verify) cmd_verify ;;
verify-build)
    shift
    cmd_verify_build "$@"
    ;;
tree-digests)
    shift
    cmd_tree_digests "$@"
    ;;
*) die "usage: tesseract-inputs.sh fetch | check-crate [build.rs] | verify | verify-build TARGET_DIR | tree-digests NAME DIR" ;;
esac
