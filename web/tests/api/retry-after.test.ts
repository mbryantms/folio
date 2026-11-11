import { describe, expect, it } from "vitest";

import {
  MAX_RETRY_AFTER_MS,
  MIN_RETRY_AFTER_MS,
  retryAfterMs,
  sleep,
} from "@/lib/api/retry-after";

describe("retryAfterMs", () => {
  it("reads delta-seconds", () => {
    expect(retryAfterMs("4")).toBe(4000);
  });
  it("clamps to the floor and the cap", () => {
    expect(retryAfterMs("0")).toBe(MIN_RETRY_AFTER_MS);
    expect(retryAfterMs(null)).toBe(MIN_RETRY_AFTER_MS);
    expect(retryAfterMs("soon")).toBe(MIN_RETRY_AFTER_MS);
    expect(retryAfterMs("3600")).toBe(MAX_RETRY_AFTER_MS);
  });
  it("reads an HTTP date", () => {
    const now = Date.parse("2026-11-11T12:00:00Z");
    expect(retryAfterMs("Wed, 11 Nov 2026 12:00:05 GMT", now)).toBe(5000);
  });
});

describe("sleep", () => {
  it("rejects when aborted", async () => {
    const c = new AbortController();
    const p = sleep(10_000, c.signal);
    c.abort();
    await expect(p).rejects.toMatchObject({ name: "AbortError" });
  });
});
