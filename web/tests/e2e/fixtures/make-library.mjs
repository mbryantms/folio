#!/usr/bin/env node
/**
 * Generate the library the Playwright docker-smoke suite scans.
 *
 *   node web/tests/e2e/fixtures/make-library.mjs [outDir]
 *
 * Writes three series folders, one issue each:
 *
 *   - `Test Series (2020)/Test Series 001.cbz` — what `reader-flow.spec.ts`
 *     reads: three pages, no ComicInfo (folder-name inference only).
 *   - `Relay (2011)/Relay (2011) 001.cbz` and `Relay (2016)/Relay (2016)
 *     001.cbz` — the same title in two volumes, with a ComicInfo.xml
 *     (Series `Relay`, Volume 1 / 2, Year 2011 / 2016, a shared publisher
 *     and writer). The post-scan relationship-suggestion run's
 *     name-continuation detector proposes "Relay (2016) continues Relay
 *     (2011)" (consecutive volumes → 0.9, the high bucket), which
 *     `relationship-review.spec.ts` accepts from `/admin/relationships`.
 *
 * Each archive is a STORED zip of real, decodable portrait PNGs (100x150,
 * solid colour, distinct bytes per page and per archive). Mirrors the Rust
 * fixture helpers (crates/server/tests/scanner_smoke.rs `write_minimal_cbz`)
 * but with pixels the browser can draw:
 *   - stored, not deflated: the archive ratio guard drops entries whose
 *     compressed size is 0 or ratio > 200 (crates/archive/src/cbz.rs);
 *   - real PNG signature: readers content-sniff pages (image_sniff.rs);
 *   - portrait: detectViewMode() stays "single" (median w/h <= 1.2);
 *   - one series FOLDER: archives at the library root are ignored
 *     (scanner/enumerate.rs, spec §2.2);
 *   - distinct bytes per page: content dedupe hashes every entry.
 * The output is deterministic (fixed zip timestamps, no randomness), so a
 * rerun writes byte-identical archives.
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

/** ComicInfo.xml for one issue (only the fields the scanner needs). */
function comicInfo({ series, volume, year, number, publisher, writer, genre }) {
  return Buffer.from(
    `<?xml version="1.0" encoding="utf-8"?>
<ComicInfo xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema">
  <Series>${series}</Series>
  <Number>${number}</Number>
  <Volume>${volume}</Volume>
  <Year>${year}</Year>
  <Month>1</Month>
  <Publisher>${publisher}</Publisher>
  <Writer>${writer}</Writer>
  <Genre>${genre}</Genre>
  <LanguageISO>en</LanguageISO>
  <PageCount>2</PageCount>
</ComicInfo>
`,
    "utf8",
  );
}

/** Write `<outDir>/<folder>/<file>` as a stored CBZ of `entries`. */
function writeCbz(folder, file, entries) {
  const dir = join(outDir, folder);
  mkdirSync(dir, { recursive: true });
  const path = join(dir, file);
  writeFileSync(path, zipStored(entries));
  const pages = entries.filter(([name]) => name.endsWith(".png")).length;
  console.log(`wrote ${path} (${pages} pages)`);
}

rmSync(outDir, { recursive: true, force: true });

writeCbz("Test Series (2020)", "Test Series 001.cbz", [
  ["page-001.png", png(100, 150, [200, 40, 40])],
  ["page-002.png", png(100, 150, [40, 160, 60])],
  ["page-003.png", png(100, 150, [40, 80, 220])],
]);

// Two volumes of one title → a pending `continues` suggestion after the scan.
for (const [volume, year, colours] of [
  [
    1,
    2011,
    [
      [180, 120, 30],
      [30, 120, 180],
    ],
  ],
  [
    2,
    2016,
    [
      [120, 30, 180],
      [30, 180, 120],
    ],
  ],
]) {
  writeCbz(`Relay (${year})`, `Relay (${year}) 001.cbz`, [
    [
      "ComicInfo.xml",
      comicInfo({
        series: "Relay",
        volume,
        year,
        number: 1,
        publisher: "Folio Test Comics",
        writer: "Ada Fixture",
        genre: "Science Fiction",
      }),
    ],
    ["page-001.png", png(100, 150, colours[0])],
    ["page-002.png", png(100, 150, colours[1])],
  ]);
}
