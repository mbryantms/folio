#!/usr/bin/env python3
"""Write the committed CB7 (7z) test fixtures with the host `7z` binary.

Unlike RAR, 7z has a free writer, so this just stages a few files in a temp
dir and shells out to `7z a`. Pages carry a real JPEG SOI/APP0 JFIF prefix so
the archive readers' content sniff (`archive::image_sniff`) accepts them, then
distinct filler so every page's bytes differ.

Outputs (next to this script):

  synthetic-3page.cb7            non-solid, COPY (stored) — one block per file
  synthetic-3page-solid.cb7      solid LZMA2 -mx9 — every file in one block,
                                 so reads must decode from the block start
  synthetic-3page-encrypted.cb7  AES-256 with encrypted headers (-mhe=on);
                                 the reader must refuse it as `Encrypted`
  synthetic-3page-encrypted-data.cb7
                                 AES-256 data, plaintext headers (-mhe=off):
                                 lists fine, but must still be `Encrypted`

Each non-encrypted archive also holds a root ComicInfo.xml and a foreign
`notes.txt` so conversion tests can check the rewrite policy (sidecars +
foreign entries preserved verbatim).

Usage: python3 fixtures/make-cb7-fixture.py   (needs `7z` on PATH)
"""
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
JFIF = bytes.fromhex("FFD8FFE000104A46494600010100000100010000")
COMICINFO = (
    b'<?xml version="1.0" encoding="utf-8"?>\n'
    b"<ComicInfo><Series>Synthetic CB7</Series><Number>1</Number>"
    b"<PageCount>3</PageCount></ComicInfo>\n"
)
NOTES = b"foreign entry: must survive CB7 -> CBZ conversion byte-for-byte\n"

# 7z switches shared by every variant: no timestamps/attrs so re-runs are as
# stable as 7-Zip allows, quiet output.
COMMON = ["-mtm=off", "-mtc=off", "-mta=off", "-mtr=off", "-bso0", "-bsp0"]

VARIANTS = {
    "synthetic-3page.cb7": ["-m0=Copy", "-ms=off"],
    "synthetic-3page-solid.cb7": ["-m0=LZMA2", "-mx9", "-ms=on"],
    "synthetic-3page-encrypted.cb7": ["-m0=LZMA2", "-ms=on", "-psecret", "-mhe=on"],
    "synthetic-3page-encrypted-data.cb7": ["-m0=LZMA2", "-ms=on", "-psecret", "-mhe=off"],
}


def stage(dirpath: str) -> list[str]:
    names = []
    for i in (1, 2, 3):
        name = f"page-{i:03}.jpg"
        with open(os.path.join(dirpath, name), "wb") as f:
            f.write(JFIF + bytes([i]) * 64 + b"\xFF\xD9")
        names.append(name)
    for name, data in (("ComicInfo.xml", COMICINFO), ("notes.txt", NOTES)):
        with open(os.path.join(dirpath, name), "wb") as f:
            f.write(data)
        names.append(name)
    return names


def main() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        names = stage(tmp)
        for out, switches in VARIANTS.items():
            dst = os.path.join(HERE, out)
            if os.path.exists(dst):
                os.remove(dst)
            subprocess.run(
                ["7z", "a", "-t7z", *COMMON, *switches, dst, *names],
                cwd=tmp,
                check=True,
            )
            print(f"wrote {dst} ({os.path.getsize(dst)} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
