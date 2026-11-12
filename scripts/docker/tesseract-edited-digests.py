#!/usr/bin/env python3
"""Expected digests of the source files tesseract-rs's build.rs edits in place.

Usage: tesseract-edited-digests.py THIRD_PARTY_DIR
  THIRD_PARTY_DIR holds the unpacked `leptonica/` and `tesseract/` trees
  (each archive's top-level folder renamed, as the build script does).

Prints `<tree> ./<path> <pristine sha256> <edited sha256>` lines in the
format of EDITED_FILES in tesseract-inputs.sh.

The edits reproduce tesseract-rs 0.4.0 build.rs exactly: lines 111-123
(environ.h), 128-136 (makefile.static) and 201-206 (tesseract
CMakeLists.txt). Each is a chain of Rust `str::replace` calls (every
non-overlapping occurrence, left to right) whose result is written back
even when no pattern matched; `bytes.replace` behaves identically for
these ASCII patterns. When a tesseract-rs bump changes those calls, change
EDITS to match and re-run this.
"""

import hashlib
import pathlib
import sys

EDITS = {
    ("leptonica", "src/environ.h"): [
        (b"#define  HAVE_LIBZ          1", b"#define  HAVE_LIBZ          0"),
        (b"#ifdef  NO_CONSOLE_IO", b"#define NO_CONSOLE_IO\n#ifdef  NO_CONSOLE_IO"),
    ],
    ("leptonica", "prog/makefile.static"): [
        (
            b"ALL_LIBS =\t$(LEPTLIB) -ltiff -ljpeg -lpng -lz -lm",
            b"ALL_LIBS =\t$(LEPTLIB) -lm",
        ),
    ],
    ("tesseract", "CMakeLists.txt"): [(b"set(HAVE_TIFFIO_H ON)", b"")],
}


def main() -> None:
    root = pathlib.Path(sys.argv[1])
    for (tree, rel), replacements in EDITS.items():
        data = (root / tree / rel).read_bytes()
        data.decode("utf-8")  # build.rs reads with read_to_string
        edited = data
        for old, new in replacements:
            edited = edited.replace(old, new)
        print(
            tree,
            f"./{rel}",
            hashlib.sha256(data).hexdigest(),
            hashlib.sha256(edited).hexdigest(),
        )


if __name__ == "__main__":
    main()
