"use client";

import type { MarkerRegion } from "@/lib/api/types";
import { pageBytesUrl } from "@/lib/urls";

/** Client-side image-region crop helpers shared by the bookmarks page
 *  (Copy / Save actions). The reader's `marker-selection.ts` has its
 *  own copy of the crop primitive because it pre-processes the canvas
 *  for OCR / SHA-256 (upscale, binarize). We keep them separate so a
 *  change to either path doesn't ripple unexpectedly. */

const imageCache = new Map<string, Promise<HTMLImageElement>>();

function loadPageImage(src: string): Promise<HTMLImageElement> {
  const existing = imageCache.get(src);
  if (existing) return existing;
  const p = new Promise<HTMLImageElement>((resolve, reject) => {
    const img = new Image();
    // Comic pages are same-origin under /api so CORS shouldn't matter,
    // but the clipboard write requires a "clean" canvas — set crossOrigin
    // defensively so a future CDN move doesn't taint the canvas.
    img.crossOrigin = "anonymous";
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error(`failed to load ${src}`));
    img.src = src;
  });
  imageCache.set(src, p);
  return p;
}

/** Crop the marker's region against the full-resolution page image and
 *  return a PNG blob. Returns `null` if the browser can't render to
 *  canvas (SSR, missing 2d ctx) or if `toBlob` fails. */
export async function cropMarkerToBlob(
  issueId: string,
  pageIndex: number,
  region: MarkerRegion,
): Promise<Blob | null> {
  if (typeof document === "undefined") return null;
  const img = await loadPageImage(pageBytesUrl(issueId, pageIndex));
  const w = img.naturalWidth;
  const h = img.naturalHeight;
  const x = (region.x / 100) * w;
  const y = (region.y / 100) * h;
  const cw = Math.max(1, Math.round((region.w / 100) * w));
  const ch = Math.max(1, Math.round((region.h / 100) * h));
  const canvas = document.createElement("canvas");
  canvas.width = cw;
  canvas.height = ch;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  ctx.drawImage(img, x, y, cw, ch, 0, 0, cw, ch);
  return new Promise((resolve) =>
    canvas.toBlob((b) => resolve(b), "image/png"),
  );
}

/** Filename for a downloaded marker crop. Mirrors the issue page URL
 *  shape (`{series}-{issue}-p{N}.png`) so a user who saves several
 *  crops sees them sort naturally. Falls back to issue ID + page when
 *  the slugs aren't on the marker. */
export function markerCropFilename(opts: {
  seriesSlug?: string | null;
  issueSlug?: string | null;
  issueId: string;
  pageIndex: number;
}): string {
  const page = opts.pageIndex + 1;
  if (opts.seriesSlug && opts.issueSlug) {
    return `${opts.seriesSlug}-${opts.issueSlug}-p${page}.png`;
  }
  return `bookmark-${opts.issueId}-p${page}.png`;
}

/** Write the cropped region to the system clipboard as a PNG image.
 *  Throws when the browser doesn't expose the async clipboard API
 *  (legacy Firefox before 127, ancient Safari). Callers surface the
 *  error as a toast. */
export async function copyMarkerImageToClipboard(
  issueId: string,
  pageIndex: number,
  region: MarkerRegion,
): Promise<void> {
  if (
    typeof navigator === "undefined" ||
    !navigator.clipboard ||
    typeof ClipboardItem === "undefined"
  ) {
    throw new Error("clipboard-unsupported");
  }
  const blob = await cropMarkerToBlob(issueId, pageIndex, region);
  if (!blob) throw new Error("crop-failed");
  await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
}

/** Trigger a download of the cropped region as a PNG. */
export async function downloadMarkerImage(
  issueId: string,
  pageIndex: number,
  region: MarkerRegion,
  filename: string,
): Promise<void> {
  const blob = await cropMarkerToBlob(issueId, pageIndex, region);
  if (!blob) throw new Error("crop-failed");
  const url = URL.createObjectURL(blob);
  try {
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    a.remove();
  } finally {
    // Defer revoke so the browser has a chance to start the download.
    setTimeout(() => URL.revokeObjectURL(url), 1_000);
  }
}
