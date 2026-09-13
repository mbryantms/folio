#!/usr/bin/env python3
"""Write a RAR 5.0 archive with STORED (uncompressed) entries — no RAR
writer is available as free software, but the container format for method
0 is small enough to emit by hand. Spec: https://www.rarlab.com/technote.htm
"""
import struct, sys, zlib

def vint(n: int) -> bytes:
    out = bytearray()
    while True:
        b = n & 0x7F; n >>= 7
        if n: out.append(b | 0x80)
        else: out.append(b); return bytes(out)

def block(htype: int, hflags: int, body: bytes, data: bytes = b"") -> bytes:
    # header = size(vint) type(vint) flags(vint) [data_size(vint)] body
    rest = vint(htype) + vint(hflags) + (vint(len(data)) if hflags & 0x0002 else b"") + body
    hdr = vint(len(rest)) + rest
    return struct.pack("<I", zlib.crc32(hdr) & 0xFFFFFFFF) + hdr + data

def rar5(entries: list[tuple[str, bytes]]) -> bytes:
    out = bytearray(b"Rar!\x1a\x07\x01\x00")
    out += block(1, 0, vint(0))                      # main archive header, archive flags = 0
    for name, data in entries:
        nb = name.encode("utf-8")
        body = (vint(0x0004)                         # file flags: CRC32 present
                + vint(len(data))                    # unpacked size
                + vint(0x20)                         # attributes (archive bit)
                + struct.pack("<I", zlib.crc32(data) & 0xFFFFFFFF)
                + vint(0)                            # compression info: version 0, method 0 (store), dict 128K
                + vint(0)                            # host OS: Windows
                + vint(len(nb)) + nb)
        out += block(2, 0x0002, body, data)          # file header, data area present
    out += block(5, 0, vint(0))                      # end of archive, flags 0
    return bytes(out)

if __name__ == "__main__":
    # 3 tiny "JPEG" pages: a real SOI/APP0 JFIF prefix so content-sniffing
    # sees image magic, then distinct filler so bytes differ per page.
    jfif = bytes.fromhex("FFD8FFE000104A46494600010100000100010000")
    pages = [(f"page-{i:03}.jpg", jfif + bytes([i]) * 64 + b"\xFF\xD9") for i in (1, 2, 3)]
    sys.stdout.buffer.write(rar5(pages))
