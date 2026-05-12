"use client";

import type { MarkerRegion } from "@/lib/api/types";

/** Inputs shared by both OCR and image-hash paths. The cropped pixels
 *  come from the same image URL the `<img>` is rendering, sampled at
 *  the image's natural resolution so the OCR engine sees the highest
 *  fidelity available without re-fetching. */
type CropInput = {
  issueId: string;
  pageIndex: number;
  region: MarkerRegion;
  naturalSize: { width: number; height: number };
};

/** Promise-cached Image so concurrent overlays don't re-decode. */
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

/** Canvas-crop the region against the source image at native resolution,
 *  optionally upsampled by `scale`. Tesseract works best with text at
 *  ~300 DPI, and a comic page rendered at ~100 DPI on screen
 *  ([typical], doesn't account for image post-processing) is too small
 *  for the engine — upsampling 2-3x before OCR more than doubles
 *  legibility. */
async function cropToCanvas(
  input: CropInput,
  scale = 1,
): Promise<HTMLCanvasElement> {
  const src = `/api/issues/${input.issueId}/pages/${input.pageIndex}`;
  const img = await loadImage(src);
  const w = input.naturalSize.width;
  const h = input.naturalSize.height;
  const x = (input.region.x / 100) * w;
  const y = (input.region.y / 100) * h;
  const cw = Math.max(1, Math.round((input.region.w / 100) * w));
  const ch = Math.max(1, Math.round((input.region.h / 100) * h));
  const canvas = document.createElement("canvas");
  canvas.width = Math.round(cw * scale);
  canvas.height = Math.round(ch * scale);
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("no 2d context");
  // `imageSmoothingQuality = 'high'` matters when we upsample —
  // bilinear interpolation keeps the text edges crisp where the
  // default 'low' would alias. Off when scale is 1, on otherwise.
  if (scale !== 1) {
    ctx.imageSmoothingEnabled = true;
    ctx.imageSmoothingQuality = "high";
  }
  ctx.drawImage(img, x, y, cw, ch, 0, 0, canvas.width, canvas.height);
  return canvas;
}

/** Mutate the canvas in place: convert to grayscale and binarize at a
 *  threshold derived from the per-region mean luma. Output is pure
 *  black-and-white pixels — Tesseract's text engine is calibrated for
 *  this kind of crisp two-tone image and accuracy jumps significantly
 *  vs. raw colored comic art with halftone screens / gradients. */
function binarize(canvas: HTMLCanvasElement): void {
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const data = ctx.getImageData(0, 0, canvas.width, canvas.height);
  const pixels = data.data;
  // First pass: compute mean luma.
  let sum = 0;
  for (let i = 0; i < pixels.length; i += 4) {
    sum += 0.299 * pixels[i]! + 0.587 * pixels[i + 1]! + 0.114 * pixels[i + 2]!;
  }
  const mean = sum / (pixels.length / 4);
  // Bias the threshold a bit toward the brighter side — speech bubbles
  // are usually mostly white with dark text, so a mean-based cut
  // would push too many white pixels to black on bright bubbles. Bias
  // -20 leaves the white background intact while still binarizing the
  // text glyphs cleanly.
  const threshold = Math.max(120, Math.min(220, mean - 20));
  for (let i = 0; i < pixels.length; i += 4) {
    const luma =
      0.299 * pixels[i]! + 0.587 * pixels[i + 1]! + 0.114 * pixels[i + 2]!;
    const v = luma > threshold ? 255 : 0;
    pixels[i] = v;
    pixels[i + 1] = v;
    pixels[i + 2] = v;
  }
  ctx.putImageData(data, 0, 0);
}

type TesseractWorker = {
  setParameters: (params: Record<string, string | number>) => Promise<unknown>;
  recognize: (
    img: Blob | string,
  ) => Promise<{ data: { text?: string; confidence?: number } }>;
  terminate: () => Promise<unknown>;
};
type TesseractModule = {
  createWorker: (
    lang?: string,
    oem?: number,
    options?: Record<string, unknown>,
  ) => Promise<TesseractWorker>;
  recognize: (
    img: Blob | string,
    lang?: string,
  ) => Promise<{ data: { text?: string; confidence?: number } }>;
};

/** Lazy-singleton tesseract worker shared across every OCR call in the
 *  reader. Loading the WASM + English language data is the slow part
 *  (~2s on a warm cache); reusing the worker across pages drops the
 *  per-OCR cost to roughly the recognize() call itself.
 *
 *  We hold the worker as a Promise so concurrent first-call sites await
 *  the same initialization without racing to spawn duplicates. The
 *  module reference is kept separately for the no-worker fallback
 *  path on older tesseract.js builds. */
let workerSingleton: Promise<TesseractWorker> | null = null;
let moduleSingleton: TesseractModule | null = null;

async function loadTesseract(): Promise<TesseractModule | null> {
  if (moduleSingleton) return moduleSingleton;
  try {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const dyn = (await import(
      /* @vite-ignore */ "tesseract.js" as any
    )) as unknown;
    moduleSingleton = dyn as TesseractModule;
    return moduleSingleton;
  } catch (err) {
    console.warn("markers: tesseract.js unavailable, falling back", err);
    return null;
  }
}

async function getOcrWorker(
  mod: TesseractModule,
): Promise<TesseractWorker | null> {
  if (typeof mod.createWorker !== "function") return null;
  if (!workerSingleton) {
    // Stash the promise immediately so concurrent callers await the
    // same boot — if initialization fails we clear the slot so the
    // next call can retry.
    workerSingleton = (async () => {
      const worker = await mod.createWorker("eng");
      await worker.setParameters({
        tessedit_pageseg_mode: "6",
      });
      return worker;
    })();
    workerSingleton.catch(() => {
      workerSingleton = null;
    });
  }
  return workerSingleton;
}

/** Run client-side OCR on the cropped region. Pre-processes the crop
 *  with a 3x upscale + grayscale binarization to give the engine a
 *  fighting chance against stylized comic lettering — accuracy without
 *  that pre-pass is poor in practice. Uses a singleton worker so
 *  subsequent calls in the same session skip the WASM-load. */
export async function ocrCroppedRegion(
  input: CropInput,
): Promise<{ text: string; confidence: number } | null> {
  if (typeof document === "undefined") return null;
  // 3x upscale brings a typical 60×40 px speech-bubble crop to
  // 180×120, comfortably above the 30+ px per character minimum
  // tesseract wants. Larger scales (>4x) start to inflate without
  // adding detail.
  const canvas = await cropToCanvas(input, 3);
  binarize(canvas);
  const blob: Blob | null = await new Promise((resolve) =>
    canvas.toBlob((b) => resolve(b), "image/png"),
  );
  if (!blob) return null;

  const mod = await loadTesseract();
  if (!mod) return null;

  try {
    const worker = await getOcrWorker(mod);
    if (worker) {
      const result = await worker.recognize(blob);
      const text = (result.data?.text ?? "").trim();
      const confidence = Number(result.data?.confidence ?? 0);
      return text ? { text, confidence } : null;
    }
    // Fallback for older tesseract.js versions without the worker API.
    const result = await mod.recognize(blob, "eng");
    const text = (result.data?.text ?? "").trim();
    const confidence = Number(result.data?.confidence ?? 0);
    return text ? { text, confidence } : null;
  } catch (err) {
    console.warn("markers: tesseract recognize failed", err);
    return null;
  }
}

/** Compute a SHA-256 over the cropped pixel bytes. Used for the
 *  image-aware highlight mode so a future "find this panel" lookup can
 *  bucket matches by hash. Returns lowercase hex. */
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
