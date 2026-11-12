// @vitest-environment jsdom
/**
 * A busy OCR server (503 + Retry-After from the OCR routes) is reachable:
 * the page-text panel retries those reads, and counting them as a
 * connectivity loss made the indicator flicker. Other 5xx still count.
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  CONNECTIVITY_EVENT,
  connectionFetch,
  reportConnectivity,
} from "@/lib/pwa/connectivity";

function seen() {
  const events: string[] = [];
  const on = (e: Event) => events.push((e as CustomEvent<string>).detail);
  window.addEventListener(CONNECTIVITY_EVENT, on);
  return {
    events,
    off: () => window.removeEventListener(CONNECTIVITY_EVENT, on),
  };
}

function answer(status: number, headers: Record<string, string> = {}) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response("{}", { status, headers })),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
  reportConnectivity("reachable");
});

describe("connectivity and busy OCR answers", () => {
  it.each([
    "/api/me/issues/abc/ocr",
    "/api/me/issues/abc/pages/3/text-regions",
  ])("503 + Retry-After from %s is not 'unreachable'", async (url) => {
    reportConnectivity("offline");
    const s = seen();
    answer(503, { "retry-after": "5" });
    await connectionFetch(url);
    s.off();
    expect(s.events).toEqual(["reachable"]);
  });

  it("a 503 elsewhere, or without Retry-After, still counts", async () => {
    const s = seen();
    answer(503, { "retry-after": "5" });
    await connectionFetch("/api/me/markers");
    reportConnectivity("reachable");
    answer(503);
    await connectionFetch("/api/me/issues/abc/ocr");
    s.off();
    expect(s.events).toEqual(["unreachable", "reachable", "unreachable"]);
  });
});
