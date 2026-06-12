"use client";

import { apiFetch, getCsrfToken } from "@/lib/api/auth-refresh";
import type { MarkerRegion } from "@/lib/api/types";

/** Inputs shared by both OCR and image-hash paths. `naturalSize`
 *  is still required because the image-hash path crops pixels in
 *  the browser; the OCR path uses it only to map the region's
 *  0–100 percent fields into integer pixel coordinates that the
 *  server-side handler expects. */
type CropInput = {
  issueId: string;
  pageIndex: number;
  region: MarkerRegion;
  naturalSize: { width: number; height: number };
};

/** Promise-cached Image so concurrent overlays don't re-decode. Used
 *  by the image-hash path; the OCR path doesn't load the page bytes
 *  client-side any more (the server reads from the canonical archive). */
const imageCache = new Map<string, Promise<HTMLImageElement>>();

function loadImage(src: string): Promise<HTMLImageElement> {
  const existing = imageCache.get(src);
  if (existing) return existing;
  const p = new Promise<HTMLImageElement>((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error(`failed to load ${src}`));
    img.src = src;
  });
  imageCache.set(src, p);
  return p;
}

/** Canvas-crop the region against the source image at native
 *  resolution. Image-hash uses this directly; the OCR path doesn't
 *  any more (server-side now). */
async function cropToCanvas(input: CropInput): Promise<HTMLCanvasElement> {
  const src = `/issues/${input.issueId}/pages/${input.pageIndex}`;
  const img = await loadImage(src);
  const w = input.naturalSize.width;
  const h = input.naturalSize.height;
  const x = (input.region.x / 100) * w;
  const y = (input.region.y / 100) * h;
  const cw = Math.max(1, Math.round((input.region.w / 100) * w));
  const ch = Math.max(1, Math.round((input.region.h / 100) * h));
  const canvas = document.createElement("canvas");
  canvas.width = cw;
  canvas.height = ch;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("no 2d context");
  ctx.drawImage(img, x, y, cw, ch, 0, 0, cw, ch);
  return canvas;
}

/** Translate the marker's percent-based region into the integer
 *  pixel rect the server expects. Clamped to page bounds so a
 *  rounding overshoot near the right/bottom edges can't push us
 *  into the handler's `invalid_region` branch. */
function regionToPixels(
  region: MarkerRegion,
  natural: { width: number; height: number },
): { x: number; y: number; w: number; h: number } {
  const W = natural.width;
  const H = natural.height;
  const x = Math.max(0, Math.min(W - 1, Math.round((region.x / 100) * W)));
  const y = Math.max(0, Math.min(H - 1, Math.round((region.y / 100) * H)));
  const w = Math.max(1, Math.min(W - x, Math.round((region.w / 100) * W)));
  const h = Math.max(1, Math.min(H - y, Math.round((region.h / 100) * H)));
  return { x, y, w, h };
}

/** Per-call knobs for [`ocrCroppedRegion`]. */
export type OcrOptions = {
  /** Recognizer override. Omitted → the server resolves a default
   *  (series `text_language`, falling back to western). */
  lang?: "western" | "manga";
  /** Run the server-side bubble detector and snap the region to the
   *  tightest enclosing bubble. Only set this when the page's
   *  detect cache is known-warm (a text-regions fetch succeeded) —
   *  a cold detector run can take tens of seconds on weak hosts. */
  detect?: boolean;
};

export type OcrResult = {
  text: string;
  confidence: number;
  /** Detector's snap-to-bubble rect in page pixels; `null` when the
   *  detector didn't run or nothing overlapped the region. */
  refinedBbox: { x: number; y: number; w: number; h: number } | null;
};

/** Run server-side OCR over the cropped region. The pipeline runs
 *  `comic-text-detector` → snap-to-bubble → Tesseract LSTM (or
 *  manga-ocr for `lang: "manga"`) → a junk-stripping postprocess
 *  pass; results are cached server-side keyed by
 *  `(content_hash, page, lang, region_hash)` so a re-OCR on the
 *  same region is a Redis round-trip.
 *
 *  Returns `null` when the server failed (non-2xx, network error)
 *  or recognized no text. The caller (`MarkerEditor` /
 *  `MarkerOverlay`) decides how to surface that — usually a
 *  "Couldn't read any text" toast and a fallback to a plain
 *  highlight.
 *
 *  Pre-M6 this ran tesseract.js entirely in the browser. The new
 *  path drops the 6 MB WASM bundle + 2 s cold-boot in exchange for
 *  a network round-trip; quality is materially better because the
 *  server pairs a real bubble detector with `tessdata_best`. */
export async function ocrCroppedRegion(
  input: CropInput,
  opts: OcrOptions = {},
): Promise<OcrResult | null> {
  // Unlike pre-M6 — no canvas / DOM touch any more, so no SSR
  // guard. The file is `"use client"` for the other helpers; the
  // function itself is safe to call from any context that has
  // `fetch` (i.e. Node 18+ as well as the browser).
  const region = regionToPixels(input.region, input.naturalSize);
  const csrf = getCsrfToken();
  let res: Response;
  try {
    res = await apiFetch(`/me/issues/${input.issueId}/ocr`, {
      method: "POST",
      headers: {
        Accept: "application/json",
        "Content-Type": "application/json",
        ...(csrf ? { "X-CSRF-Token": csrf } : {}),
      },
      body: JSON.stringify({
        page: input.pageIndex,
        region,
        ...(opts.lang ? { lang: opts.lang } : {}),
        ...(opts.detect ? { detect: true } : {}),
      }),
    });
  } catch (err) {
    // Network failure (offline, DNS, TLS). Same null-return contract
    // as before so the call site treats it like "couldn't OCR" —
    // toast strings are owned there.
    console.warn("markers: server OCR network error", err);
    return null;
  }
  if (!res.ok) {
    let detail = `${res.status}`;
    try {
      const body = (await res.json()) as { error?: { message?: string } };
      detail = body.error?.message ?? detail;
    } catch {
      // Body wasn't JSON — keep the status code message.
    }
    console.warn(`markers: server OCR failed (${res.status}): ${detail}`);
    return null;
  }
  let payload: {
    text?: string;
    confidence?: number;
    refined_bbox?: { x: number; y: number; w: number; h: number } | null;
  };
  try {
    payload = (await res.json()) as typeof payload;
  } catch (err) {
    console.warn("markers: server OCR returned malformed JSON", err);
    return null;
  }
  const text = (payload.text ?? "").trim();
  if (!text) return null;
  const confidence = Number(payload.confidence ?? 0);
  return { text, confidence, refinedBbox: payload.refined_bbox ?? null };
}

/** Compute a SHA-256 over the cropped pixel bytes. Used for the
 *  image-aware highlight mode so a future "find this panel" lookup
 *  can bucket matches by hash. Returns lowercase hex. */
export async function sha256CroppedRegion(
  input: CropInput,
): Promise<string | null> {
  if (typeof document === "undefined") return null;
  if (typeof crypto === "undefined" || !crypto.subtle) return null;
  const canvas = await cropToCanvas(input);
  const blob: Blob | null = await new Promise((resolve) =>
    canvas.toBlob((b) => resolve(b), "image/png"),
  );
  if (!blob) return null;
  const buf = await blob.arrayBuffer();
  const digest = await crypto.subtle.digest("SHA-256", buf);
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}
