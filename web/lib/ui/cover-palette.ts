"use client";

import { useEffect, useState } from "react";

/**
 * Colours read from a cover image in the browser (folio-flair-1.0).
 *
 * Covers and thumbnails are served from Folio's own origin, so a canvas can
 * read their pixels without CORS headers. The image is scaled down to a
 * 24×36 sample before reading, which keeps the work well under a
 * millisecond. Results are cached per URL for the life of the tab.
 */
export interface CoverPalette {
  /** Average colour of the whole cover. */
  average: [number, number, number];
  /** The most saturated, mid-brightness colour (falls back to `average`). */
  vibrant: [number, number, number];
}

const cache = new Map<string, Promise<CoverPalette | null>>();

const SAMPLE_W = 24;
const SAMPLE_H = 36;

function rgbToHsl(r: number, g: number, b: number): [number, number, number] {
  const rn = r / 255,
    gn = g / 255,
    bn = b / 255;
  const max = Math.max(rn, gn, bn),
    min = Math.min(rn, gn, bn);
  const l = (max + min) / 2;
  if (max === min) return [0, 0, l];
  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  let h: number;
  if (max === rn) h = (gn - bn) / d + (gn < bn ? 6 : 0);
  else if (max === gn) h = (bn - rn) / d + 2;
  else h = (rn - gn) / d + 4;
  return [h / 6, s, l];
}

/** Pure: pick the palette from RGBA pixel data. Exported for tests. */
export function paletteFromPixels(
  data: Uint8ClampedArray,
): CoverPalette | null {
  let r = 0,
    g = 0,
    b = 0,
    n = 0;
  let best: [number, number, number] | null = null;
  let bestScore = -1;
  for (let i = 0; i + 3 < data.length; i += 4) {
    if ((data[i + 3] ?? 0) < 128) continue;
    const pr = data[i] ?? 0,
      pg = data[i + 1] ?? 0,
      pb = data[i + 2] ?? 0;
    r += pr;
    g += pg;
    b += pb;
    n++;
    const [, s, l] = rgbToHsl(pr, pg, pb);
    // Favour strong colour at a usable brightness; near-black and
    // near-white pixels (borders, logos, paper) score low.
    const score = s * (1 - Math.abs(l - 0.5) * 1.6);
    if (score > bestScore) {
      bestScore = score;
      best = [pr, pg, pb];
    }
  }
  if (n === 0) return null;
  const average: [number, number, number] = [
    Math.round(r / n),
    Math.round(g / n),
    Math.round(b / n),
  ];
  return { average, vibrant: bestScore > 0.15 && best ? best : average };
}

function extract(url: string): Promise<CoverPalette | null> {
  return new Promise((resolve) => {
    const img = new Image();
    img.decoding = "async";
    img.onload = () => {
      try {
        const canvas = document.createElement("canvas");
        canvas.width = SAMPLE_W;
        canvas.height = SAMPLE_H;
        const ctx = canvas.getContext("2d", { willReadFrequently: true });
        if (!ctx) return resolve(null);
        ctx.drawImage(img, 0, 0, SAMPLE_W, SAMPLE_H);
        resolve(
          paletteFromPixels(ctx.getImageData(0, 0, SAMPLE_W, SAMPLE_H).data),
        );
      } catch {
        // A tainted canvas (cross-origin provider cover) can't be read.
        resolve(null);
      }
    };
    img.onerror = () => resolve(null);
    img.src = url;
  });
}

export function coverPalette(url: string): Promise<CoverPalette | null> {
  let p = cache.get(url);
  if (!p) {
    p = extract(url);
    cache.set(url, p);
  }
  return p;
}

/** The palette for `url`, or null while loading / when unreadable. */
export function useCoverPalette(
  url: string | null | undefined,
): CoverPalette | null {
  const [palette, setPalette] = useState<CoverPalette | null>(null);
  useEffect(() => {
    if (!url) return;
    let alive = true;
    void coverPalette(url).then((p) => {
      if (alive) setPalette(p);
    });
    return () => {
      alive = false;
    };
  }, [url]);
  return url ? palette : null;
}

export const rgb = ([r, g, b]: [number, number, number], alpha = 1) =>
  `rgb(${r} ${g} ${b} / ${alpha})`;
