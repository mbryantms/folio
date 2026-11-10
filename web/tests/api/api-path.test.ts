/**
 * `apiPath` encodes every interpolated value as one URI component, so a
 * route param can never address a different API path than intended.
 */
import { describe, expect, it } from "vitest";

import { apiPath, routeParam } from "@/lib/api/path";

describe("apiPath", () => {
  it("keeps the literal parts as written", () => {
    expect(apiPath`/me/pages`).toBe("/me/pages");
    expect(apiPath`/series/${"saga"}/issues?limit=5`).toBe(
      "/series/saga/issues?limit=5",
    );
  });

  it.each([
    ["slash", "a/b", "a%2Fb"],
    ["question mark", "a?b=1", "a%3Fb%3D1"],
    ["hash", "a#b", "a%23b"],
    ["percent", "100%", "100%25"],
    ["unicode", "café ☕", "caf%C3%A9%20%E2%98%95"],
    ["dot segment", "..", ".."],
  ])("encodes a %s", (_name, value, encoded) => {
    expect(apiPath`/series/${value}/resume`).toBe(`/series/${encoded}/resume`);
  });

  it("encodes already-encoded input again, by design", () => {
    expect(apiPath`/series/${"a%2Fb"}`).toBe("/series/a%252Fb");
  });

  it("passes numbers through", () => {
    expect(apiPath`/me/issues/${"i-1"}/pages/${3}/text-regions`).toBe(
      "/me/issues/i-1/pages/3/text-regions",
    );
  });

  it("encodes a query value placed after a literal ?", () => {
    expect(apiPath`/progress?issue_id=${"a&b=c"}`).toBe(
      "/progress?issue_id=a%26b%3Dc",
    );
  });

  it("encodes every segment independently", () => {
    expect(apiPath`/series/${"x/y"}/issues/${"#1"}`).toBe(
      "/series/x%2Fy/issues/%231",
    );
  });
});

describe("routeParam", () => {
  it("is the identity for an ASCII slug", () => {
    expect(routeParam("saga-2012")).toBe("saga-2012");
  });

  it("decodes what the App Router encoded, so apiPath encodes it once", () => {
    // The router hands a page `caf%C3%A9`; the API path must not become
    // `caf%25C3%25A9`.
    const param = encodeURIComponent("café au lait");
    expect(routeParam(param)).toBe("café au lait");
    expect(apiPath`/series/${routeParam(param)}`).toBe(`/series/${param}`);
  });

  it("returns a value that is not valid percent-encoding unchanged", () => {
    expect(routeParam("100%")).toBe("100%");
  });
});
