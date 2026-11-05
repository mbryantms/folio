"use client";

import { paletteFromImage, type CoverPalette } from "@/lib/ui/cover-palette";

/**
 * Remembered cover colours (folio-flair-1.0: "blur-up cover placeholders"
 * and "cover colour extraction", both standard).
 *
 * When a card cover decodes, its average and most vibrant colours are
 * read from the image already on screen (a 24×36 canvas sample, scheduled
 * for idle time) and remembered per URL in localStorage. Next time that
 * cover is drawn, its placeholder box is already the cover's own colour
 * while the image loads, and the card can tint itself from the vibrant
 * colour on hover. Storage is a convenience: if it is unavailable the
 * colours only last for the tab.
 */
export type Tone = { average: string; vibrant: string };

const KEY = "folio:cover-tones:v1";
/** Most-recent covers kept; ~30 bytes each. */
const MAX = 2500;

let tones: Map<string, string> | null = null;
let saveTimer: ReturnType<typeof setTimeout> | null = null;

function load(): Map<string, string> {
  if (tones) return tones;
  tones = new Map();
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) {
      for (const [k, v] of JSON.parse(raw) as Array<[string, string]>) {
        tones.set(k, v);
      }
    }
  } catch {
    // Private mode, quota, or junk: start empty.
  }
  return tones;
}

function save() {
  if (saveTimer) return;
  saveTimer = setTimeout(() => {
    saveTimer = null;
    try {
      localStorage.setItem(KEY, JSON.stringify([...load()]));
    } catch {
      // Ignore: the in-memory map still serves this tab.
    }
  }, 1500);
}

/** Cache key: the cover URL without its query (size variants share). */
export function toneKey(src: string): string {
  const i = src.indexOf("?");
  return i === -1 ? src : src.slice(0, i);
}

const hex = ([r, g, b]: [number, number, number]) =>
  ((1 << 24) | (r << 16) | (g << 8) | b).toString(16).slice(1);

const toRgb = (h: string) => {
  const n = parseInt(h, 16);
  return `${(n >> 16) & 255} ${(n >> 8) & 255} ${n & 255}`;
};

/** Pure: encode / decode one entry. Exported for tests. */
export function encodeTone(p: CoverPalette): string {
  return `${hex(p.average)}${hex(p.vibrant)}`;
}
export function decodeTone(v: string | undefined): Tone | null {
  if (!v || !/^[0-9a-f]{12}$/.test(v)) return null;
  return { average: toRgb(v.slice(0, 6)), vibrant: toRgb(v.slice(6)) };
}

/** The remembered colours for a cover, as `"r g b"` triplets. */
export function getTone(src: string): Tone | null {
  if (typeof window === "undefined") return null;
  return decodeTone(load().get(toneKey(src)));
}

/** Read and remember the colours of a decoded cover, off the main path. */
export function rememberTone(src: string, img: HTMLImageElement): void {
  const key = toneKey(src);
  const map = load();
  if (map.has(key)) {
    // Refresh recency (Map keeps insertion order; oldest drop first).
    const v = map.get(key)!;
    map.delete(key);
    map.set(key, v);
    return;
  }
  const run = () => {
    const p = paletteFromImage(img);
    if (!p) return;
    map.set(key, encodeTone(p));
    while (map.size > MAX) {
      const oldest = map.keys().next().value;
      if (oldest === undefined) break;
      map.delete(oldest);
    }
    save();
    applyTint(img, decodeTone(map.get(key)));
  };
  if ("requestIdleCallback" in window) {
    window.requestIdleCallback(run, { timeout: 2000 });
  } else {
    setTimeout(run, 50);
  }
}

/** Hand the vibrant colour to the enclosing card (`[data-card]`), which
 *  uses `--cover-tint` for its hover wash (globals.css). */
export function applyTint(el: Element | null, tone: Tone | null): void {
  const card = el?.closest<HTMLElement>("[data-card]");
  if (card && tone) {
    card.style.setProperty("--cover-tint", `rgb(${tone.vibrant})`);
  }
}
