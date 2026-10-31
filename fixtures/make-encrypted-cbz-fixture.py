#!/usr/bin/env python3
"""Write the committed password-protected CBZ fixture by hand.

`fixtures/synthetic-encrypted.cbz` is a ZIP whose single page entry is
stored (method 0) under traditional PKWARE "ZipCrypto" encryption
(general-purpose flag bit 0, 12-byte encryption header, password
`secret`). Python's `zipfile` can *read* ZipCrypto but not write it, and
the repo must not depend on a host `7z`/`zip` binary, so the cipher and the
container are emitted here; the output is byte-stable across runs.

The page carries the real 8-byte PNG signature so the fixture exercises the
scanner's encryption branch, not the image content-sniff: a reader that
ignored the encryption flag would still see an image-named entry whose
plaintext starts with image magic. The CBZ reader refuses the archive as
`ArchiveError::Encrypted`; the scanner ingests it as a page-less
`encrypted`-state issue and raises the `EncryptedArchive` health finding.

Usage: python3 fixtures/make-encrypted-cbz-fixture.py
"""
from __future__ import annotations

import io
import os
import struct
import sys
import zipfile
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "synthetic-encrypted.cbz")
PASSWORD = b"secret"
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"

ENTRIES: list[tuple[str, bytes]] = [
    ("page-001.png", PNG_SIGNATURE + bytes([0x01]) * 64),
    (
        "ComicInfo.xml",
        b'<?xml version="1.0" encoding="utf-8"?>\n'
        b"<ComicInfo><Series>Synthetic Encrypted</Series><Number>1</Number>"
        b"<PageCount>1</PageCount></ComicInfo>\n",
    ),
]

# Fixed DOS date/time (2020-01-01 00:00:00) so re-runs are byte-identical.
DOS_TIME = 0
DOS_DATE = (40 << 9) | (1 << 5) | 1


class ZipCrypto:
    """PKWARE traditional encryption (APPNOTE 6.1), encrypt direction."""

    def __init__(self, password: bytes) -> None:
        self.k0, self.k1, self.k2 = 0x12345678, 0x23456789, 0x34567890
        for b in password:
            self._update(b)

    @staticmethod
    def _crc32_byte(crc: int, b: int) -> int:
        return (zlib.crc32(bytes([b]), crc ^ 0xFFFFFFFF) ^ 0xFFFFFFFF) & 0xFFFFFFFF

    def _update(self, b: int) -> None:
        self.k0 = self._crc32_byte(self.k0, b)
        self.k1 = (self.k1 + (self.k0 & 0xFF)) & 0xFFFFFFFF
        self.k1 = (self.k1 * 134775813 + 1) & 0xFFFFFFFF
        self.k2 = self._crc32_byte(self.k2, (self.k1 >> 24) & 0xFF)

    def _stream_byte(self) -> int:
        t = (self.k2 | 2) & 0xFFFF
        return ((t * (t ^ 1)) >> 8) & 0xFF

    def encrypt(self, plain: bytes) -> bytes:
        out = bytearray()
        for b in plain:
            c = b ^ self._stream_byte()
            self._update(b)
            out.append(c)
        return bytes(out)


def encrypt_entry(data: bytes, crc: int) -> bytes:
    """12-byte header (11 fixed bytes + CRC high byte for the password
    check) followed by the ciphertext of `data`."""
    header = bytes(range(0xA0, 0xAB)) + bytes([(crc >> 24) & 0xFF])
    cipher = ZipCrypto(PASSWORD)
    return cipher.encrypt(header + data)


def build() -> bytes:
    out = bytearray()
    central = bytearray()
    for name, data in ENTRIES:
        nb = name.encode("utf-8")
        crc = zlib.crc32(data) & 0xFFFFFFFF
        payload = encrypt_entry(data, crc)
        flags = 0x0001  # bit 0: encrypted
        method = 0  # stored
        offset = len(out)
        out += struct.pack(
            "<IHHHHHIIIHH",
            0x04034B50,
            20,
            flags,
            method,
            DOS_TIME,
            DOS_DATE,
            crc,
            len(payload),
            len(data),
            len(nb),
            0,
        )
        out += nb + payload
        central += struct.pack(
            "<IHHHHHHIIIHHHHHII",
            0x02014B50,
            20,
            20,
            flags,
            method,
            DOS_TIME,
            DOS_DATE,
            crc,
            len(payload),
            len(data),
            len(nb),
            0,
            0,
            0,
            0,
            0x20,
            offset,
        )
        central += nb
    cd_offset = len(out)
    out += central
    out += struct.pack(
        "<IHHHHIIH",
        0x06054B50,
        0,
        0,
        len(ENTRIES),
        len(ENTRIES),
        len(central),
        cd_offset,
        0,
    )
    return bytes(out)


def verify(blob: bytes) -> None:
    """Round-trip through the stdlib reader: the entries must be flagged
    encrypted, refuse to open without the password, and decrypt to the
    staged plaintext with it."""
    with zipfile.ZipFile(io.BytesIO(blob)) as zf:
        for name, data in ENTRIES:
            info = zf.getinfo(name)
            assert info.flag_bits & 0x1, f"{name}: encryption flag not set"
            try:
                zf.read(name)
            except RuntimeError:
                pass
            else:
                raise AssertionError(f"{name}: opened without a password")
            assert zf.read(name, pwd=PASSWORD) == data, f"{name}: bad decrypt"


def main() -> int:
    blob = build()
    verify(blob)
    with open(OUT, "wb") as f:
        f.write(blob)
    print(f"wrote {OUT} ({len(blob)} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
