#!/usr/bin/env node
/**
 * Generate the library the Playwright reader-flow spec scans.
 *
 *   node web/tests/e2e/fixtures/make-library.mjs [outDir]
 *
 * Writes `<outDir>/Test Series (2020)/Test Series 001.cbz`: a STORED zip of
 * three real, decodable portrait PNGs (100x150, solid colour, distinct bytes
 * per page). Mirrors the Rust fixture helpers (crates/server/tests/
 * scanner_smoke.rs `write_minimal_cbz`) but with pixels the browser can draw:
 *   - stored, not deflated: the archive ratio guard drops entries whose
 *     compressed size is 0 or ratio > 200 (crates/archive/src/cbz.rs);
 *   - real PNG signature: readers content-sniff pages (image_sniff.rs);
 *   - portrait: detectViewMode() stays "single" (median w/h <= 1.2);
 *   - one series FOLDER: archives at the library root are ignored
 *     (scanner/enumerate.rs, spec §2.2);
 *   - distinct bytes per page: content dedupe hashes every entry.
 * Default outDir is web/tests/e2e/.library (gitignored); compose.test.yml
 * bind-mounts it at /library via SMOKE_LIBRARY_DIR.
 */
import { mkdirSync, writeFileSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { crc32, deflateSync } from "node:zlib";

const here = dirname(fileURLToPath(import.meta.url));
const outDir = process.argv[2] ?? join(here, "..", ".library");

function pngChunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const typeBuf = Buffer.from(type, "latin1");
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([typeBuf, data])) >>> 0);
  return Buffer.concat([len, typeBuf, data, crc]);
}

/** Solid-colour RGB PNG, width x height. */
function png(width, height, [r, g, b]) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // colour type: RGB
  ihdr[10] = 0; // compression
  ihdr[11] = 0; // filter
  ihdr[12] = 0; // interlace
  const row = Buffer.alloc(1 + width * 3);
  for (let x = 0; x < width; x++) row.set([r, g, b], 1 + x * 3);
  const raw = Buffer.concat(Array.from({ length: height }, () => row));
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk("IHDR", ihdr),
    pngChunk("IDAT", deflateSync(raw)),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

/** Minimal STORED zip writer (local headers + central directory + EOCD). */
function zipStored(entries) {
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const [name, data] of entries) {
    const nameBuf = Buffer.from(name, "utf8");
    const crc = crc32(data) >>> 0;
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4); // version needed
    local.writeUInt16LE(0x0800, 6); // flags: UTF-8 names
    local.writeUInt16LE(0, 8); // method: stored
    local.writeUInt16LE(0, 10); // mtime
    local.writeUInt16LE(0x21, 12); // mdate (1980-01-01)
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(nameBuf.length, 26);
    local.writeUInt16LE(0, 28);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4); // version made by
    central.writeUInt16LE(20, 6); // version needed
    central.writeUInt16LE(0x0800, 8);
    central.writeUInt16LE(0, 10);
    central.writeUInt16LE(0, 12);
    central.writeUInt16LE(0x21, 14);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(data.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(nameBuf.length, 28);
    central.writeUInt16LE(0, 30); // extra
    central.writeUInt16LE(0, 32); // comment
    central.writeUInt16LE(0, 34); // disk
    central.writeUInt16LE(0, 36); // internal attrs
    central.writeUInt32LE(0, 38); // external attrs
    central.writeUInt32LE(offset, 42);
    locals.push(local, nameBuf, data);
    centrals.push(central, nameBuf);
    offset += local.length + nameBuf.length + data.length;
  }
  const cd = Buffer.concat(centrals);
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(0, 4);
  eocd.writeUInt16LE(0, 6);
  eocd.writeUInt16LE(entries.length, 8);
  eocd.writeUInt16LE(entries.length, 10);
  eocd.writeUInt32LE(cd.length, 12);
  eocd.writeUInt32LE(offset, 16);
  eocd.writeUInt16LE(0, 20);
  return Buffer.concat([...locals, cd, eocd]);
}

const seriesDir = join(outDir, "Test Series (2020)");
rmSync(outDir, { recursive: true, force: true });
mkdirSync(seriesDir, { recursive: true });
const pages = [
  ["page-001.png", png(100, 150, [200, 40, 40])],
  ["page-002.png", png(100, 150, [40, 160, 60])],
  ["page-003.png", png(100, 150, [40, 80, 220])],
];
const cbz = join(seriesDir, "Test Series 001.cbz");
writeFileSync(cbz, zipStored(pages));
console.log(`wrote ${cbz} (${pages.length} pages)`);
