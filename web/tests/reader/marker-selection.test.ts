/**
 * `ocrCroppedRegion` contracts after the M6 swap to server-side OCR
 * (text-detection-1.0 plan).
 *
 * Pre-M6 this function loaded `tesseract.js` in the browser; now it
 * POSTs to `/api/me/issues/{id}/ocr` and returns the server's
 * response. These tests pin the wire shape and the fallback paths
 * so a future refactor can't silently regress the reader's
 * "Couldn't read any text" toast UX.
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
    naturalSize: { width: 1000, height: 1000 },
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
  it("POSTs pixel region + lang to /me/issues/{id}/ocr", async () => {
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(200, { text: "POW!", confidence: 0.91 }),
    );

    const result = await ocrCroppedRegion(input());

    expect(result).toEqual({ text: "POW!", confidence: 0.91 });
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
      lang: string;
    };
    expect(body.page).toBe(2);
    expect(body.lang).toBe("western");
    // 25% of 1000 = 250; 30% = 300; 10% = 100; 8% = 80.
    expect(body.region).toEqual({ x: 250, y: 300, w: 100, h: 80 });
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

  it("returns null + logs on a server error envelope", async () => {
    // Folio's standard error envelope: `{ error: { code, message } }`.
    const consoleWarn = vi.spyOn(console, "warn").mockImplementation(() => {});
    mockedFetch.mockResolvedValueOnce(
      jsonResponse(429, {
        error: { code: "rate_limited", message: "slow down" },
      }),
    );
    const result = await ocrCroppedRegion(input());
    expect(result).toBeNull();
    expect(consoleWarn).toHaveBeenCalledWith(expect.stringContaining("429"));
    consoleWarn.mockRestore();
  });

  it("returns null on a network rejection", async () => {
    // Offline / DNS / TLS — apiFetch rejects.
    const consoleWarn = vi.spyOn(console, "warn").mockImplementation(() => {});
    mockedFetch.mockRejectedValueOnce(new Error("offline"));
    const result = await ocrCroppedRegion(input());
    expect(result).toBeNull();
    expect(consoleWarn).toHaveBeenCalledWith(
      "markers: server OCR network error",
      expect.any(Error),
    );
    consoleWarn.mockRestore();
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
        naturalSize: { width: 100, height: 100 },
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

  it("returns null when the server response isn't valid JSON", async () => {
    const consoleWarn = vi.spyOn(console, "warn").mockImplementation(() => {});
    mockedFetch.mockResolvedValueOnce(
      new Response("not json", {
        status: 200,
        headers: { "content-type": "text/plain" },
      }),
    );
    const result = await ocrCroppedRegion(input());
    expect(result).toBeNull();
    expect(consoleWarn).toHaveBeenCalledWith(
      "markers: server OCR returned malformed JSON",
      expect.any(Error),
    );
    consoleWarn.mockRestore();
  });
});
