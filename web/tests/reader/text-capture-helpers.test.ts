/**
 * Pure helpers behind text capture and the page-text panel:
 *  - the page's real size and the percent <-> pixel conversions (D1),
 *  - the one definition of a page's text boxes and the hit-test (D2),
 *  - the words for every failure, and which failures are transient (D5).
 */
import { describe, expect, it } from "vitest";
import type { TextRegionView } from "@/lib/api/types";
import {
  measuredOriginalSize,
  pagePixelsToRegion,
  regionToPagePixels,
  resolvePageSize,
  serverPageSize,
} from "@/lib/reader/page-size";
import {
  hitTestBox,
  pageTextBoxes,
  readingOrderRegions,
} from "@/lib/reader/text-regions";
import {
  OcrRequestError,
  isTransientOcrFailure,
  ocrFailureMessage,
  ocrRetryDelayMs,
  retryTransientOcr,
  toOcrRequestError,
} from "@/lib/reader/ocr-errors";

const SERVER = { width: 1988, height: 3056 };
const VARIANT = { width: 960, height: 1476 };

describe("page size (D1)", () => {
  it("prefers the detection's size, then the server's, then a measured original", () => {
    expect(
      resolvePageSize({
        detected: { width: 2000, height: 3000 },
        server: SERVER,
        measured: VARIANT,
      }),
    ).toEqual({ width: 2000, height: 3000 });
    expect(resolvePageSize({ server: SERVER, measured: VARIANT })).toEqual(
      SERVER,
    );
    expect(resolvePageSize({ measured: VARIANT })).toEqual(VARIANT);
    expect(resolvePageSize({})).toBeNull();
    expect(
      resolvePageSize({ server: { width: 0, height: 10 }, measured: null }),
    ).toBeNull();
  });

  it("reads the server size only when both dimensions are known", () => {
    expect(serverPageSize({ image_width: 1988, image_height: 3056 })).toEqual(
      SERVER,
    );
    expect(
      serverPageSize({ image_width: 1988, image_height: null }),
    ).toBeNull();
    expect(serverPageSize(undefined)).toBeNull();
  });

  it("never trusts the natural size of an <img> that has a srcset", () => {
    const img = {
      complete: true,
      naturalWidth: 960,
      naturalHeight: 1476,
      getAttribute: (n: string) => (n === "srcset" ? "/p?w=1080 1080w" : null),
    } as unknown as HTMLImageElement;
    expect(measuredOriginalSize(img)).toBeNull();
    const original = {
      ...img,
      complete: true,
      naturalWidth: 1988,
      naturalHeight: 3056,
      getAttribute: () => null,
    } as unknown as HTMLImageElement;
    expect(measuredOriginalSize(original)).toEqual(SERVER);
  });

  it("a box at 50% / 50% maps to the original page's pixels, not the variant's", () => {
    const box = { x: 40, y: 40, w: 20, h: 20 };
    expect(regionToPagePixels(box, SERVER)).toEqual({
      x: 795,
      y: 1222,
      w: 398,
      h: 611,
    });
    // What the variant size used to produce: smaller and shifted.
    expect(regionToPagePixels(box, VARIANT)).toEqual({
      x: 384,
      y: 590,
      w: 192,
      h: 295,
    });
  });

  it("clamps rounding overshoot at the page edge", () => {
    const px = regionToPagePixels(
      { x: 99.5, y: 99.5, w: 1, h: 1 },
      {
        width: 100,
        height: 100,
      },
    );
    expect(px.x + px.w).toBeLessThanOrEqual(100);
    expect(px.y + px.h).toBeLessThanOrEqual(100);
    expect(px.w).toBeGreaterThanOrEqual(1);
  });

  it("round-trips a snapped pixel box back to percent with the same size", () => {
    const px = { x: 398, y: 611, w: 597, h: 917 };
    const pct = pagePixelsToRegion(px, SERVER);
    expect(regionToPagePixels(pct, SERVER)).toEqual(px);
    // Never past the page.
    const edge = pagePixelsToRegion(
      { x: 1900, y: 3000, w: 500, h: 500 },
      SERVER,
    );
    expect(edge.x + edge.w).toBeLessThanOrEqual(100);
    expect(edge.y + edge.h).toBeLessThanOrEqual(100);
  });
});

function r(x: number, y: number, w: number, h: number): TextRegionView {
  return { x, y, w, h, confidence: 0.9, class: 0 };
}

describe("the page's text boxes (D2)", () => {
  // The server sends the final set: a large region, a balloon inside it,
  // and a box elsewhere are three regions.
  const large = r(30, 30, 40, 40);
  const inner = r(35, 45, 30, 8);
  const other = r(5, 80, 10, 10);

  it("keeps a small box inside a large one", () => {
    expect(pageTextBoxes([inner, large, other])).toEqual([inner, large, other]);
  });

  it("drops only boxes with no area", () => {
    expect(pageTextBoxes([r(50, 50, 0, 10), large, r(60, 60, 10, 0)])).toEqual([
      large,
    ]);
  });

  it("a point inside the inner box hits it; a point beside it hits the large one", () => {
    const boxes = pageTextBoxes([large, inner]);
    expect(hitTestBox(boxes, 50, 49)).toBe(inner);
    expect(hitTestBox(boxes, 50, 35)).toBe(large);
    expect(hitTestBox(boxes, 1, 1)).toBeNull();
  });

  it("reading order is the same set, ordered", () => {
    expect(new Set(readingOrderRegions([inner, large, other], "ltr"))).toEqual(
      new Set(pageTextBoxes([inner, large, other])),
    );
  });
});

describe("failure words (D5)", () => {
  const cases: [number, string | null, RegExp, boolean][] = [
    [409, "archive_needs_conversion", /converted to CBZ/, false],
    [410, "archive_missing", /file is missing/, false],
    [410, "archive_unreadable", /can't be opened/, false],
    [503, "library_offline", /folder is offline/, true],
    [503, "service_unavailable", /busy/, true],
    [429, "rate_limited", /Too many text requests/, true],
    [408, null, /took too long/, true],
    [500, "ocr_model_unverified", /model couldn't be downloaded/, false],
    [500, "ocr_failed", /recognition failed/, false],
    [415, "decode_failed", /isn't an image/, false],
    [404, "not_found", /couldn't be found/, false],
    [422, "invalid_region", /isn't on the page/, false],
    [0, null, /Couldn't reach the server/, true],
    [503, null, /busy/, true],
    [418, null, /error 418/, false],
  ];
  it.each(cases)("status %i code %s", (status, code, words, transient) => {
    expect(ocrFailureMessage(status, code)).toMatch(words);
    expect(isTransientOcrFailure(status, code)).toBe(transient);
    expect(new OcrRequestError(status, code).transient).toBe(transient);
  });

  it("normalises a query HttpError, keeping Retry-After", () => {
    const e = toOcrRequestError({
      status: 503,
      code: "service_unavailable",
      retryAfter: "5",
    });
    expect(e).toMatchObject({ status: 503, code: "service_unavailable" });
    expect(e.retryAfterMs).toBe(5000);
    expect(toOcrRequestError(new TypeError("fetch failed")).status).toBe(0);
  });

  it("retries only transient failures, a bounded number of times", () => {
    const retry = retryTransientOcr(3);
    const busy = new OcrRequestError(503, "service_unavailable");
    const final = new OcrRequestError(409, "archive_needs_conversion");
    expect(retry(0, busy)).toBe(true);
    expect(retry(2, busy)).toBe(true);
    expect(retry(3, busy)).toBe(false);
    expect(retry(0, final)).toBe(false);
  });

  it("waits Retry-After when given, else backs off, never over 30 s", () => {
    expect(ocrRetryDelayMs(0, new OcrRequestError(503, null, 7000))).toBe(7000);
    // TanStack passes 0 for the first retry: 2 s, 4 s, 8 s.
    expect(ocrRetryDelayMs(0, new OcrRequestError(503, null))).toBe(2000);
    expect(ocrRetryDelayMs(1, new OcrRequestError(503, null))).toBe(4000);
    expect(ocrRetryDelayMs(2, new OcrRequestError(503, null))).toBe(8000);
    expect(ocrRetryDelayMs(10, new OcrRequestError(503, null))).toBe(30_000);
  });
});
