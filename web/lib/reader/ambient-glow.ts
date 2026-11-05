/**
 * Ambient page glow (folio-flair-1.0, effect `ambient_glow`).
 *
 * The empty reader background around the page glows softly with the
 * colours at the page's four edges. Colours are read in the browser from
 * the page's strip thumbnail (same-origin, usually already warm in cache
 * from the blur-up placeholder and the page strip): drawn into a tiny
 * canvas, then a strip along each edge is averaged. Results are cached per
 * URL, so revisiting a page costs nothing.
 *
 * The pure pieces (`edgeColorsFromPixels`, `combineSpreadEdges`,
 * `glowBackground`) are exported for tests; `sampleEdgeColors` is the only
 * part that touches the DOM.
 */

export type Rgb = readonly [number, number, number];

export interface EdgeColors {
  top: Rgb;
  bottom: Rgb;
  left: Rgb;
  right: Rgb;
}

export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** Sample canvas size. Pages are portrait; 16×24 keeps their rough shape
 *  while costing a few hundred pixels to read back. */
export const SAMPLE_W = 16;
export const SAMPLE_H = 24;
/** Edge strip thickness, in sample pixels. */
export const SAMPLE_STRIP = 2;

/**
 * Average an RGBA pixel buffer (`ImageData.data` layout) along each edge.
 * Each strip is `strip` pixels deep and spans the full edge; pixels are
 * weighted by alpha so a transparent margin doesn't drag colours to black.
 * A fully transparent strip averages to black.
 */
export function edgeColorsFromPixels(
  data: ArrayLike<number>,
  width: number,
  height: number,
  strip: number = SAMPLE_STRIP,
): EdgeColors {
  const s = Math.max(1, Math.min(strip, Math.floor(width / 2), height));
  const avg = (x0: number, y0: number, x1: number, y1: number): Rgb => {
    let r = 0;
    let g = 0;
    let b = 0;
    let w = 0;
    for (let y = y0; y < y1; y++) {
      for (let x = x0; x < x1; x++) {
        const i = (y * width + x) * 4;
        const a = (data[i + 3] ?? 255) / 255;
        r += (data[i] ?? 0) * a;
        g += (data[i + 1] ?? 0) * a;
        b += (data[i + 2] ?? 0) * a;
        w += a;
      }
    }
    if (w === 0) return [0, 0, 0];
    return [Math.round(r / w), Math.round(g / w), Math.round(b / w)];
  };
  const sv = Math.max(1, Math.min(strip, Math.floor(height / 2)));
  return {
    top: avg(0, 0, width, sv),
    bottom: avg(0, height - sv, width, height),
    left: avg(0, 0, s, height),
    right: avg(width - s, 0, width, height),
  };
}

/** Edges of a spread, given each page's edges in visual (left-to-right)
 *  order: outer sides from the outer pages, top/bottom averaged. */
export function combineSpreadEdges(
  pages: readonly EdgeColors[],
): EdgeColors | null {
  if (pages.length === 0) return null;
  if (pages.length === 1) return pages[0]!;
  const mean = (pick: (e: EdgeColors) => Rgb): Rgb => {
    const sum = [0, 0, 0];
    for (const p of pages) {
      const c = pick(p);
      sum[0]! += c[0];
      sum[1]! += c[1];
      sum[2]! += c[2];
    }
    return [
      Math.round(sum[0]! / pages.length),
      Math.round(sum[1]! / pages.length),
      Math.round(sum[2]! / pages.length),
    ];
  };
  return {
    left: pages[0]!.left,
    right: pages[pages.length - 1]!.right,
    top: mean((e) => e.top),
    bottom: mean((e) => e.bottom),
  };
}

/** Brightest channel a glow colour may reach. Comic pages often have white
 *  margins; a full-white glow on the dark reader surface would glare and
 *  pull the eye off the art, so bright colours are scaled down (hue kept). */
const GLOW_MAX_CHANNEL = 170;

export function tameGlowColor(c: Rgb): Rgb {
  const max = Math.max(c[0], c[1], c[2]);
  if (max <= GLOW_MAX_CHANNEL) return c;
  const k = GLOW_MAX_CHANNEL / max;
  return [Math.round(c[0] * k), Math.round(c[1] * k), Math.round(c[2] * k)];
}

const css = (c: Rgb) => {
  const t = tameGlowColor(c);
  return `rgb(${t[0]} ${t[1]} ${t[2]})`;
};

/**
 * CSS `background` for the glow layer: one soft radial gradient centred on
 * the middle of each page edge, sized from the page box. The page itself
 * is opaque and sits above the layer, so only the empty space around it
 * shows the glow.
 */
export function glowBackground(edges: EdgeColors, page: Box): string {
  const hw = page.width / 2;
  const hh = page.height / 2;
  const cx = page.left + hw;
  const cy = page.top + hh;
  const g = (c: Rgb, x: number, y: number, rx: number, ry: number) =>
    `radial-gradient(${Math.round(rx)}px ${Math.round(ry)}px at ${Math.round(x)}px ${Math.round(y)}px, ${css(c)}, transparent)`;
  return [
    g(edges.left, page.left, cy, hw + 140, hh * 1.25),
    g(edges.right, page.left + page.width, cy, hw + 140, hh * 1.25),
    g(edges.top, cx, page.top, hw * 1.7, hh * 0.6 + 90),
    g(edges.bottom, cx, page.top + page.height, hw * 1.7, hh * 0.6 + 90),
  ].join(", ");
}

/** Union of boxes (the rendered spread), or null for none / empty. */
export function unionBox(boxes: readonly Box[]): Box | null {
  const real = boxes.filter((b) => b.width > 0 && b.height > 0);
  if (real.length === 0) return null;
  const left = Math.min(...real.map((b) => b.left));
  const top = Math.min(...real.map((b) => b.top));
  const right = Math.max(...real.map((b) => b.left + b.width));
  const bottom = Math.max(...real.map((b) => b.top + b.height));
  return { left, top, width: right - left, height: bottom - top };
}

// ---------------------------------------------------------------------------
// Browser sampling

/** Bounded per-URL cache. Holds the promise so concurrent requests for the
 *  same page share one decode; a failed sample caches `null` (no glow for
 *  that page rather than a retry loop). */
const CACHE_LIMIT = 96;
const cache = new Map<string, Promise<EdgeColors | null>>();

let canvas: HTMLCanvasElement | null = null;

function readEdges(img: HTMLImageElement): EdgeColors | null {
  canvas ??= document.createElement("canvas");
  canvas.width = SAMPLE_W;
  canvas.height = SAMPLE_H;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) return null;
  ctx.clearRect(0, 0, SAMPLE_W, SAMPLE_H);
  ctx.drawImage(img, 0, 0, SAMPLE_W, SAMPLE_H);
  try {
    const { data } = ctx.getImageData(0, 0, SAMPLE_W, SAMPLE_H);
    return edgeColorsFromPixels(data, SAMPLE_W, SAMPLE_H);
  } catch {
    // Tainted canvas (a cross-origin redirect) — no glow for this page.
    return null;
  }
}

/** Edge colours of the image at `url` (same-origin). Cached per URL. */
export function sampleEdgeColors(url: string): Promise<EdgeColors | null> {
  const hit = cache.get(url);
  if (hit) {
    // Refresh recency.
    cache.delete(url);
    cache.set(url, hit);
    return hit;
  }
  const p = new Promise<EdgeColors | null>((resolve) => {
    const img = new Image();
    img.decoding = "async";
    img.onload = () => resolve(readEdges(img));
    img.onerror = () => resolve(null);
    img.src = url;
  });
  cache.set(url, p);
  if (cache.size > CACHE_LIMIT) {
    const oldest = cache.keys().next().value;
    if (oldest !== undefined) cache.delete(oldest);
  }
  return p;
}

/** Test hook: drop every cached sample. */
export function clearEdgeColorCache(): void {
  cache.clear();
}
