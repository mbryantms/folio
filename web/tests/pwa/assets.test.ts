import { readFileSync, statSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import manifest from "@/app/manifest";
import { appleStartupImages } from "@/lib/pwa/apple-splash";

/**
 * Static counterpart of `scripts/check-pwa-assets.mjs` (which checks the
 * booted origin in docker-smoke): every raster the manifest and root
 * layout declare must be committed under `public/` as a real PNG at the
 * declared size. Regenerate with `pnpm --filter web run build-icons`.
 */
const PUBLIC = resolve(__dirname, "../../public");
const PNG_SIGNATURE = Buffer.from([
  0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a,
]);

function png(src: string) {
  const bytes = readFileSync(resolve(PUBLIC, `.${src}`));
  expect(bytes.subarray(0, 8).equals(PNG_SIGNATURE), src).toBe(true);
  expect(bytes.toString("latin1", 12, 16), src).toBe("IHDR");
  return {
    width: bytes.readUInt32BE(16),
    height: bytes.readUInt32BE(20),
    // IHDR colour type: 2 = RGB, 3 = palette, 4/6 = with alpha.
    colorType: bytes[25],
    bytes: bytes.length,
  };
}

const m = manifest();

describe("web app manifest", () => {
  it("launches standalone with a stable identity", () => {
    expect(m.display).toBe("standalone");
    expect(m.id).toBe("/");
    expect(m.scope).toBe("/");
    expect(m.start_url).toBe("/");
  });

  it("splits `any` and `maskable` icons and ships both 192 and 512", () => {
    const icons = m.icons ?? [];
    const any = icons.filter((i) => i.purpose === "any");
    const maskable = icons.filter((i) => i.purpose === "maskable");
    expect(any.map((i) => i.sizes).sort()).toEqual(["192x192", "512x512"]);
    expect(maskable.map((i) => i.sizes)).toEqual(["512x512"]);
  });

  it("declares an icon for every shortcut", () => {
    for (const shortcut of m.shortcuts ?? [])
      expect(shortcut.icons?.length, shortcut.name).toBeGreaterThan(0);
  });

  it("every manifest icon is a committed PNG at its declared size", () => {
    const icons = [
      ...(m.icons ?? []),
      ...(m.shortcuts ?? []).flatMap((s) => s.icons ?? []),
    ];
    for (const icon of icons) {
      expect(icon.type).toBe("image/png");
      const [width, height] = icon.sizes!.split("x").map(Number);
      const file = png(icon.src);
      expect([file.width, file.height], icon.src).toEqual([width, height]);
      expect(file.bytes, icon.src).toBeLessThan(50 * 1024);
      // Maskable icons are cropped by the OS; a transparent bleed would
      // show as black/white corners, so the file must be opaque.
      if (icon.purpose === "maskable")
        expect([2, 3], icon.src).toContain(file.colorType);
    }
  });
});

describe("Apple / favicon assets", () => {
  it("apple-touch-icon is an opaque 180x180 PNG", () => {
    const file = png("/icons/apple-touch-icon.png");
    expect([file.width, file.height]).toEqual([180, 180]);
    expect([2, 3]).toContain(file.colorType);
  });

  it("every startup image exists at the resolution its media query targets", () => {
    const images = appleStartupImages();
    expect(images.length).toBeGreaterThan(0);
    for (const { url, media } of images) {
      const [, cssW, cssH, ratio, orientation] = media.match(
        /device-width: (\d+)px\) and \(device-height: (\d+)px\) and \(-webkit-device-pixel-ratio: (\d+)\) and \(orientation: (\w+)\)/,
      )!;
      const [w, h] = [
        Number(cssW) * Number(ratio),
        Number(cssH) * Number(ratio),
      ];
      const expected = orientation === "portrait" ? [w, h] : [h, w];
      const file = png(url);
      expect([file.width, file.height], url).toEqual(expected);
      expect(file.bytes, url).toBeLessThan(100 * 1024);
    }
  });

  it("covers portrait and landscape for every device", () => {
    const media = appleStartupImages().map((i) => i.media);
    expect(media.filter((q) => q.includes("portrait")).length).toBe(
      media.filter((q) => q.includes("landscape")).length,
    );
  });

  it("ships the favicon pair", () => {
    const ico = readFileSync(resolve(PUBLIC, "favicon.ico"));
    // ICONDIR: reserved 0, type 1 (icon), 3 images (16/32/48).
    expect([
      ico.readUInt16LE(0),
      ico.readUInt16LE(2),
      ico.readUInt16LE(4),
    ]).toEqual([0, 1, 3]);
    expect(statSync(resolve(PUBLIC, "icon.svg")).size).toBeGreaterThan(0);
  });
});
