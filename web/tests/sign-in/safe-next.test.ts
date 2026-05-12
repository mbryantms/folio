import { describe, expect, it } from "vitest";

import { isSafeNextPath } from "@/app/[locale]/sign-in/safe-next";

describe("isSafeNextPath", () => {
  it("accepts simple absolute paths", () => {
    expect(isSafeNextPath("/")).toBe(true);
    expect(isSafeNextPath("/library")).toBe(true);
    expect(isSafeNextPath("/series/x-men/issues/1")).toBe(true);
    expect(isSafeNextPath("/views/abc?sort=name")).toBe(true);
    expect(isSafeNextPath("/page?ratio=16:9")).toBe(true);
  });

  it("rejects null / undefined / empty", () => {
    expect(isSafeNextPath(null)).toBe(false);
    expect(isSafeNextPath(undefined)).toBe(false);
    expect(isSafeNextPath("")).toBe(false);
  });

  it("rejects absolute URLs", () => {
    expect(isSafeNextPath("https://evil.tld/")).toBe(false);
    expect(isSafeNextPath("http://evil.tld/")).toBe(false);
    expect(isSafeNextPath("//evil.tld/path")).toBe(false);
  });

  it("rejects backslash / double-slash smuggling", () => {
    expect(isSafeNextPath("/\\evil.tld")).toBe(false);
    expect(isSafeNextPath("/path\\back")).toBe(false);
    expect(isSafeNextPath("\\\\evil.tld")).toBe(false);
  });

  it("rejects embedded schemes", () => {
    expect(isSafeNextPath("/redirect=https://evil.tld")).toBe(false);
    expect(isSafeNextPath("/javascript:alert(1)")).toBe(false);
  });

  it("rejects relative paths", () => {
    expect(isSafeNextPath("relative/path")).toBe(false);
    expect(isSafeNextPath("javascript:alert(1)")).toBe(false);
  });

  it("rejects control characters", () => {
    expect(isSafeNextPath("/path\n")).toBe(false);
    expect(isSafeNextPath("/path\r\nLocation: evil")).toBe(false);
    expect(isSafeNextPath("/path\t")).toBe(false);
    expect(isSafeNextPath("/del")).toBe(false);
  });

  it("type-narrows to string on success", () => {
    const candidate: string | null = "/library";
    if (isSafeNextPath(candidate)) {
      // Compile-time check: candidate is `string` here, not `string | null`.
      const _x: string = candidate;
      expect(_x).toBe("/library");
    }
  });
});
