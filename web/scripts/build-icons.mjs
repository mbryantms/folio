/**
 * Regenerate every PWA / favicon / Apple raster asset from the SVG masters
 * in `public/brand/`. Re-runnable and deterministic: run it after replacing
 * a master (`pnpm --filter web run build-icons`) and commit the outputs.
 *
 *   node scripts/build-icons.mjs [path/to/icon-master.svg]
 *
 * Outputs (all referenced from `app/manifest.ts` / `app/layout.tsx`):
 *   public/icons/icon-192.png, icon-512.png        manifest `any`
 *   public/icons/icon-512-maskable.png             manifest `maskable`
 *   public/icons/apple-touch-icon.png              iOS Home Screen (180)
 *   public/icons/shortcut-{library,bookmarks}-96.png manifest shortcuts
 *   public/icons/splash-<w>x<h>.png                iOS startup images,
 *                                                  portrait + landscape
 *   public/favicon.ico (16/32/48), public/icon.svg browser tab
 *
 * The background colour is read from `lib/pwa/theme-colors.ts` (the dark
 * `--background` token) so the maskable bleed, Apple icon, and splash
 * screens always match the manifest's `background_color`.
 */
import { copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import sharp from "sharp";
import devices from "../lib/pwa/apple-splash-devices.json" with { type: "json" };

const web = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const brand = path.join(web, "public/brand");
const icons = path.join(web, "public/icons");
const master = path.resolve(
  process.argv[2] ?? path.join(brand, "icon-master.svg"),
);

const themeSource = await readFile(
  path.join(web, "lib/pwa/theme-colors.ts"),
  "utf8",
);
const background = themeSource.match(/dark:\s*"(#[0-9a-f]{6})"/i)?.[1];
if (!background) throw new Error("dark theme colour not found");

/** Maskable safe zone is the centred circle of radius 40 % (204.8 px at 512).
 * At 0.88 the interim glyph's farthest corner sits ~176 px from centre, and
 * the master's tile corners fall on the same-colour bleed. Re-check this
 * when the master changes (the README has the recipe). */
const MASKABLE_SCALE = 0.88;
/** iOS applies its own squircle mask to an opaque square. */
const APPLE_SCALE = 0.9;
/** Splash mark size as a fraction of the shorter screen edge. */
const SPLASH_SCALE = 0.28;

const masterSvg = await readFile(master);

/** Rasterise an SVG at exactly `size`×`size` (density keeps it crisp). */
function render(svg, size) {
  return sharp(svg, { density: Math.max(72, (72 * size) / 96) })
    .resize(size, size)
    .png()
    .toBuffer();
}

/** Icons keep full-colour anti-aliasing (they are a few KiB anyway); the
 * large, mostly-flat splash screens are palette-quantised. */
function encode(image, { palette = false } = {}) {
  return image
    .png(
      palette
        ? { palette: true, quality: 100, effort: 10, compressionLevel: 9 }
        : { compressionLevel: 9, adaptiveFiltering: true },
    )
    .toBuffer();
}

/** Master centred on an opaque `background` canvas (no alpha channel:
 * iOS and maskable crops must never reveal transparency). */
async function onCanvas(width, height, markSize, options) {
  const mark = await render(masterSvg, markSize);
  const composed = await sharp({
    create: { width, height, channels: 3, background },
  })
    .composite([
      {
        input: mark,
        left: Math.round((width - markSize) / 2),
        top: Math.round((height - markSize) / 2),
      },
    ])
    .raw()
    .toBuffer({ resolveWithObject: true });
  return encode(
    sharp(composed.data, { raw: composed.info }).removeAlpha(),
    options,
  );
}

const outputs = new Map();

for (const size of [192, 512])
  outputs.set(
    `icons/icon-${size}.png`,
    await encode(sharp(await render(masterSvg, size))),
  );
outputs.set(
  "icons/icon-512-maskable.png",
  await onCanvas(512, 512, Math.round(512 * MASKABLE_SCALE)),
);
outputs.set(
  "icons/apple-touch-icon.png",
  await onCanvas(180, 180, Math.round(180 * APPLE_SCALE)),
);
for (const name of ["library", "bookmarks"])
  outputs.set(
    `icons/shortcut-${name}-96.png`,
    await encode(
      sharp(
        await render(
          await readFile(path.join(brand, `shortcut-${name}.svg`)),
          96,
        ),
      ),
    ),
  );
for (const { width, height, ratio } of devices) {
  const [w, h] = [width * ratio, height * ratio];
  for (const [cw, ch] of [
    [w, h],
    [h, w],
  ])
    outputs.set(
      `icons/splash-${cw}x${ch}.png`,
      await onCanvas(cw, ch, Math.round(Math.min(cw, ch) * SPLASH_SCALE), {
        palette: true,
      }),
    );
}

// favicon.ico with PNG payloads (supported by every browser since IE Vista).
const favicons = await Promise.all(
  [16, 32, 48].map(async (size) => ({
    size,
    data: await sharp(await render(masterSvg, size))
      .png({ compressionLevel: 9 })
      .toBuffer(),
  })),
);
const header = Buffer.alloc(6 + 16 * favicons.length);
header.writeUInt16LE(0, 0);
header.writeUInt16LE(1, 2);
header.writeUInt16LE(favicons.length, 4);
let offset = header.length;
favicons.forEach(({ size, data }, i) => {
  const entry = 6 + 16 * i;
  header.writeUInt8(size, entry);
  header.writeUInt8(size, entry + 1);
  header.writeUInt16LE(1, entry + 4); // colour planes
  header.writeUInt16LE(32, entry + 6); // bits per pixel
  header.writeUInt32LE(data.length, entry + 8);
  header.writeUInt32LE(offset, entry + 12);
  offset += data.length;
});
outputs.set(
  "favicon.ico",
  Buffer.concat([header, ...favicons.map(({ data }) => data)]),
);

await mkdir(icons, { recursive: true });
for (const [file, data] of outputs)
  await writeFile(path.join(web, "public", file), data);
await copyFile(master, path.join(web, "public/icon.svg"));

const total = [...outputs.values()].reduce((sum, b) => sum + b.length, 0);
console.warn(
  `Wrote ${outputs.size + 1} assets (${(total / 1024).toFixed(0)} KiB raster) from ${path.relative(web, master)}`,
);
