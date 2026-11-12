/**
 * `ocrCroppedRegion` contracts after the M6 swap to server-side OCR
 * (text-detection-1.0 plan).
 *
 * Pre-M6 this function loaded `tesseract.js` in the browser; now it
 * POSTs to `/api/me/issues/{id}/ocr` and returns the server's
 * response. These tests pin the wire shape, and that "the server found
 * no text" (`null`) stays distinct from "the request failed" (a rejected
 * `OcrRequestError` carrying the reason and whether a retry can help).
 *
 * The function lives in
 * `web/app/[locale]/read/[seriesSlug]/[issueSlug]/marker-selection.ts`
 * and is consumed by `MarkerEditor` + `MarkerOverlay`. We test it
 * directly — no React render — by mocking `apiFetch`.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/api/auth-refresh", () => ({
  apiFetch: vi.fn(),
  getCsrfToken: vi.fn(() => "csrf-fixture"),
}));

import { ocrCroppedRegion } from "@/app/[locale]/read/[seriesSlug]/[issueSlug]/marker-selection";
import { apiFetch } from "@/lib/api/auth-refresh";
import { OcrRequestError } from "@/lib/reader/ocr-errors";

const mockedFetch = vi.mocked(apiFetch);

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function input(
  overrides: Partial<Parameters<typeof ocrCroppedRegion>[0]> = {},
) {
  return {
    issueId: "abc-issue",
    pageIndex: 2,
    // Region is in 0-100% so 25/30/10/8 ⇒ pixels (250, 300, 100, 80) on a 1000×1000 page.
    region: { x: 25, y: 30, w: 10, h: 8, shape: "rect" as const },
    pageSize: { width: 1000, height: 1000 },
    ...overrides,
  };
}

beforeEach(() => {
  mockedFetch.mockReset();
});

afterEach(() => {
  vi.clearAllMocks();
});

describe("ocrCroppedRegion — server-side OCR", () => {
  it("POSTs pixel region to /me/issues/{id}/ocr", async () => {
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(200, { text: "POW!", confidence: 0.91 }),
    );

    const result = await ocrCroppedRegion(input());

    expect(result).toEqual({
      text: "POW!",
      confidence: 0.91,
      refinedBbox: null,
      lang: null,
      pageSize: null,
    });
    expect(mockedFetch).toHaveBeenCalledOnce();
    const [path, init] = mockedFetch.mock.calls[0]!;
    expect(path).toBe("/me/issues/abc-issue/ocr");
    expect(init?.method).toBe("POST");
    expect((init?.headers as Record<string, string>)["X-CSRF-Token"]).toBe(
      "csrf-fixture",
    );
    expect((init?.headers as Record<string, string>)["Content-Type"]).toBe(
      "application/json",
    );
    const body = JSON.parse(init?.body as string) as {
      page: number;
      region: { x: number; y: number; w: number; h: number };
      lang?: string;
      detect?: boolean;
    };
    expect(body.page).toBe(2);
    // No lang/detect by default — the server resolves the language
    // (series text_language → reading_direction → western) and the
    // detector only runs when the caller knows the cache is warm.
    expect(body.lang).toBeUndefined();
    expect(body.detect).toBeUndefined();
    // The percent region always goes (the server maps it onto the page it
    // decodes) …
    expect((body as { region_pct?: unknown }).region_pct).toEqual({
      x: 25,
      y: 30,
      w: 10,
      h: 8,
    });
    // … plus the pixel rectangle for an older server: 25% of 1000 = 250;
    // 30% = 300; 10% = 100; 8% = 80.
    expect(body.region).toEqual({ x: 250, y: 300, w: 100, h: 80 });
  });

  it("sends only region_pct when no page size is known, and reports the response's page size", async () => {
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(200, {
        text: "HI",
        confidence: 0.9,
        page_w: 1988,
        page_h: 3056,
      }),
    );
    const result = await ocrCroppedRegion(input({ pageSize: null }));
    const [, init] = mockedFetch.mock.calls[0]!;
    expect(JSON.parse(init?.body as string)).toEqual({
      page: 2,
      region_pct: { x: 25, y: 30, w: 10, h: 8 },
    });
    expect(result?.pageSize).toEqual({ width: 1988, height: 3056 });
  });

  it("forwards lang + detect options when set", async () => {
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(200, { text: "すごい", confidence: 1 }),
    );
    await ocrCroppedRegion(input(), { lang: "manga", detect: true });
    const [, init] = mockedFetch.mock.calls[0]!;
    const body = JSON.parse(init?.body as string) as {
      lang?: string;
      detect?: boolean;
    };
    expect(body.lang).toBe("manga");
    expect(body.detect).toBe(true);
  });

  it("passes through the server's refined_bbox", async () => {
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(200, {
        text: "SNAPPED",
        confidence: 0.8,
        refined_bbox: { x: 120, y: 240, w: 300, h: 180 },
      }),
    );
    const result = await ocrCroppedRegion(input(), { detect: true });
    expect(result?.refinedBbox).toEqual({ x: 120, y: 240, w: 300, h: 180 });
  });

  it("trims response text", async () => {
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(200, { text: "  CRASH!  \n", confidence: 0.5 }),
    );
    const result = await ocrCroppedRegion(input());
    expect(result?.text).toBe("CRASH!");
  });

  it("returns null when the server recognized no text", async () => {
    // Tesseract's "blank tile" output: empty string + 0 confidence.
    // The reader treats this as "Couldn't read any text" and the
    // caller falls back to a plain highlight.
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(200, { text: "", confidence: 0 }),
    );
    const result = await ocrCroppedRegion(input());
    expect(result).toBeNull();
  });

  it("rejects with the reason on a failure, never a silent null", async () => {
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(429, {
        error: { code: "rate_limited", message: "slow down" },
      }),
    );
    const err = await ocrCroppedRegion(input()).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(OcrRequestError);
    expect(err).toMatchObject({ status: 429, code: "rate_limited" });
    expect((err as OcrRequestError).transient).toBe(true);
  });

  it("carries Retry-After on a busy answer", async () => {
    mockedFetch.mockResolvedValueOnce(
      new Response(
        JSON.stringify({
          error: { code: "service_unavailable", message: "busy" },
        }),
        { status: 503, headers: { "retry-after": "7" } },
      ),
    );
    const err = (await ocrCroppedRegion(input()).catch(
      (e: unknown) => e,
    )) as OcrRequestError;
    expect(err.retryAfterMs).toBe(7000);
    expect(err.transient).toBe(true);
  });

  it("a 409 is a final failure with its reason", async () => {
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(409, {
        error: { code: "archive_needs_conversion", message: "convert" },
      }),
    );
    const err = (await ocrCroppedRegion(input()).catch(
      (e: unknown) => e,
    )) as OcrRequestError;
    expect(err.transient).toBe(false);
    expect(err.message).toContain("converted to CBZ");
  });

  it("rejects with a network failure when the request never answers", async () => {
    // Offline / DNS / TLS — apiFetch rejects.
    mockedFetch.mockRejectedValueOnce(new Error("offline"));
    const err = (await ocrCroppedRegion(input()).catch(
      (e: unknown) => e,
    )) as OcrRequestError;
    expect(err).toBeInstanceOf(OcrRequestError);
    expect(err.status).toBe(0);
    expect(err.transient).toBe(true);
  });

  it("clamps region to page bounds — rounding overshoot can't escape", async () => {
    // Region runs to the bottom-right edge of a 100×100 page.
    // 99.5% of 100 rounds to 100 → x+w would be 100+1; the helper
    // must clamp w/h so the rect stays inside the page.
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(200, { text: "EDGE", confidence: 1 }),
    );
    await ocrCroppedRegion(
      input({
        pageSize: { width: 100, height: 100 },
        region: { x: 99.5, y: 99.5, w: 1, h: 1, shape: "rect" },
      }),
    );
    const [, init] = mockedFetch.mock.calls[0]!;
    const body = JSON.parse(init?.body as string) as {
      region: { x: number; y: number; w: number; h: number };
    };
    expect(body.region.x + body.region.w).toBeLessThanOrEqual(100);
    expect(body.region.y + body.region.h).toBeLessThanOrEqual(100);
    expect(body.region.w).toBeGreaterThanOrEqual(1);
    expect(body.region.h).toBeGreaterThanOrEqual(1);
  });

  it("rejects when the server response isn't valid JSON", async () => {
    mockedFetch.mockResolvedValueOnce(
      new Response("not json", {
        status: 200,
        headers: { "content-type": "text/plain" },
      }),
    );
    await expect(ocrCroppedRegion(input())).rejects.toBeInstanceOf(
      OcrRequestError,
    );
  });
});
