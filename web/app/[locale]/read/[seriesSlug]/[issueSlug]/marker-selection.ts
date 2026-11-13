"use client";

import { apiFetch, getCsrfToken } from "@/lib/api/auth-refresh";
import { queryKeys } from "@/lib/api/query-keys";
import type { MarkerRegion, OcrRequest, OcrResponse } from "@/lib/api/types";
import {
  NETWORK_FAILURE,
  OcrRequestError,
  ocrErrorFromResponse,
} from "@/lib/reader/ocr-errors";
import {
  regionToPagePixels,
  type PageSize,
  type PercentBox,
} from "@/lib/reader/page-size";

import { PAGE_TEXT_GC_MS, textBoxKey } from "@/lib/reader/text-regions";

/** Promise-cached Image so concurrent overlays don't re-decode. Used
 *  by the image-hash path; the OCR path doesn't load the page bytes
 *  client-side (the server reads from the canonical archive). */
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

/** Canvas-crop the region from the ORIGINAL page bytes. The crop is
 *  computed against that image's own pixel size (it is loaded here
 *  without a `srcSet`, so its natural size is the file's), never against
 *  the size of whatever variant the reader's `<img>` shows — so the same
 *  marked area always hashes the same pixels, on any screen. */
async function cropToCanvas(input: {
  issueId: string;
  pageIndex: number;
  region: PercentBox;
}): Promise<HTMLCanvasElement> {
  const src = `/issues/${input.issueId}/pages/${input.pageIndex}`;
  const img = await loadImage(src);
  const w = img.naturalWidth;
  const h = img.naturalHeight;
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

/** Per-call knobs for [`ocrCroppedRegion`]. */
export type OcrOptions = {
  /** Recognizer override. Omitted → the server resolves a default
   *  (series `text_language`, falling back to western). */
  lang?: "western" | "manga";
  /** Snap the rectangle to the detected region it overlaps most. Only
   *  set this when the page's detect cache is known-warm (a text-regions
   *  fetch succeeded) — with the server's previous engines a cold
   *  detector run can take tens of seconds on weak hosts. Omitted → the
   *  server reads the rectangle it is given. One exception either way:
   *  for Japanese, a rectangle that is one of the page's balloons (a
   *  tapped box, or the box a drag snapped to) is narrowed by the server
   *  to the text box inside the balloon; a rectangle drawn by hand and
   *  not snapped is read as sent. */
  detect?: boolean;
  /** Aborts the request (the server still finishes its own work). */
  signal?: AbortSignal;
};

export type OcrResult = {
  text: string;
  confidence: number;
  /** The detected box the server read, in page pixels, when that is
   *  not the rectangle as sent: the region a `detect` request snapped
   *  to, or, for Japanese, the text box inside the balloon (also for a
   *  tap without `detect`). `null` when the rectangle was read as
   *  sent. */
  refinedBbox: { x: number; y: number; w: number; h: number } | null;
  /** Recognizer the server resolved (`western` / `manga`) — the page-text
   *  panel maps it to a `lang` attribute for screen readers. */
  lang: string | null;
  /** The decoded page's size the server reported with this answer —
   *  the size to convert `refinedBbox` back to percent with. `null` from
   *  a server that does not send it yet. */
  pageSize: PageSize | null;
};

export type OcrInput = {
  issueId: string;
  pageIndex: number;
  region: MarkerRegion | PercentBox;
  /** The page's pixel size when the client knows one
   *  (`resolvePageSize`). The region always goes as `region_pct`, which
   *  the server maps onto the page it decodes; the size only adds the
   *  equivalent pixel `region` for a server that predates `region_pct`. */
  pageSize: PageSize | null;
};

/** Read the text in one region on the server (`POST /me/issues/{id}/ocr`).
 *  The recognizer runs on the given rectangle, after an optional detector
 *  snap (`opts.detect`); results are cached server-side keyed by
 *  `(content_hash, page, lang, region)`, so the same rectangle a second
 *  time is a cache hit.
 *
 *  Resolves to the result, or `null` when the server read the region and
 *  found no text. Rejects with an {@link OcrRequestError} when the request
 *  failed (network, any non-2xx, an unreadable answer): a failure is never
 *  reported as "no text", and the error carries the reason
 *  (`ocrFailureMessage`) and whether retrying can help. */
export async function ocrCroppedRegion(
  input: OcrInput,
  opts: OcrOptions = {},
): Promise<OcrResult | null> {
  const { x, y, w, h } = input.region;
  // `region_pct` (percent of the decoded page) is what the server uses; it
  // clamps it to the page and shares the result cache with the equivalent
  // pixel rectangle. The pixel `region` rides along only when a size is
  // known, for a server that predates `region_pct`.
  const body: OcrRequest = {
    page: input.pageIndex,
    region_pct: { x, y, w, h },
    ...(input.pageSize
      ? { region: regionToPagePixels(input.region, input.pageSize) }
      : {}),
    ...(opts.lang ? { lang: opts.lang } : {}),
    ...(opts.detect ? { detect: true } : {}),
  };
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
      body: JSON.stringify(body),
      signal: opts.signal,
    });
  } catch (err) {
    if (opts.signal?.aborted) throw err;
    // Offline, DNS, TLS: no HTTP answer at all.
    throw new OcrRequestError(NETWORK_FAILURE, null);
  }
  if (!res.ok) throw await ocrErrorFromResponse(res);
  // Partial: a server that predates the page size in the response omits
  // `page_w` / `page_h`, and the size is then treated as unknown.
  let payload: Partial<OcrResponse>;
  try {
    payload = (await res.json()) as typeof payload;
  } catch {
    throw new OcrRequestError(res.status, "ocr_failed");
  }
  const text = (payload.text ?? "").trim();
  if (!text) return null;
  const confidence = Number(payload.confidence ?? 0);
  return {
    text,
    confidence,
    refinedBbox: payload.refined_bbox ?? null,
    lang: payload.lang ?? null,
    pageSize:
      payload.page_w && payload.page_h
        ? { width: payload.page_w, height: payload.page_h }
        : null,
  };
}

/** The query that reads one detected box with the default recognizer and
 *  no detector snap — shared by the page-text panel and a capture tap /
 *  key press, so reading a box the panel already read is instant and both
 *  hit one cache entry (client and server). Keyed by the percent box
 *  (`textBoxKey`), never by a pixel rectangle, so callers that knew
 *  different page sizes still share it. Failures are errors, never cached
 *  as a result. */
export function boxTextQueryOptions(
  issueId: string,
  pageIndex: number,
  box: PercentBox,
  pageSize: PageSize | null,
) {
  return {
    queryKey: queryKeys.issuePageRegionText(
      issueId,
      pageIndex,
      textBoxKey(box),
    ),
    queryFn: ({ signal }: { signal?: AbortSignal }) =>
      ocrCroppedRegion(
        {
          issueId,
          pageIndex,
          region: { x: box.x, y: box.y, w: box.w, h: box.h },
          pageSize,
        },
        { signal },
      ),
    staleTime: Infinity,
    gcTime: PAGE_TEXT_GC_MS,
  };
}

/** Compute a SHA-256 over the cropped pixel bytes of the original page.
 *  Used for the image-aware highlight mode so a future "find this panel"
 *  lookup can bucket matches by hash. Returns lowercase hex. */
export async function sha256CroppedRegion(input: {
  issueId: string;
  pageIndex: number;
  region: PercentBox;
}): Promise<string | null> {
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
