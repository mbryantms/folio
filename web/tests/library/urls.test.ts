/**
 * Unit tests for `web/lib/urls.ts` — the URL builders that thread
 * reading-context (`?cbl=`) onto reader / issue links. Other URL builders
 * are trivial enough that the typed signatures + grep-able call sites are
 * the safety net; the optional-opts shape is the part worth exercising.
 */
import { describe, expect, it } from "vitest";

import {
  coverThumbSrcSet,
  issueUrl,
  pageBytesSrcSet,
  readerUrl,
  withContentVersion,
} from "@/lib/urls";

const ISSUE = { slug: "issue-1", series_slug: "invincible" };
const SV_ID = "00000000-0000-0000-0000-00000000abcd";

describe("readerUrl", () => {
  it("returns the canonical /read path for an issue with no opts", () => {
    expect(readerUrl(ISSUE)).toBe("/read/invincible/issue-1");
  });

  it("appends ?cbl=<id> when a saved-view id is passed", () => {
    expect(readerUrl(ISSUE, { cbl: SV_ID })).toBe(
      `/read/invincible/issue-1?cbl=${SV_ID}`,
    );
  });

  it("omits ?cbl= when the opt is null or empty", () => {
    expect(readerUrl(ISSUE, { cbl: null })).toBe("/read/invincible/issue-1");
    expect(readerUrl(ISSUE, {})).toBe("/read/invincible/issue-1");
  });

  it("works via the explicit (seriesSlug, issueSlug) overload too", () => {
    expect(readerUrl("invincible", "issue-1", { cbl: SV_ID })).toBe(
      `/read/invincible/issue-1?cbl=${SV_ID}`,
    );
  });

  it("URL-encodes saved-view ids with special characters", () => {
    // Defense-in-depth — saved-view ids are UUIDs in practice, but the
    // helper should not break if a caller forwards a value with an
    // ampersand or space (e.g., a future opaque pagination token).
    expect(readerUrl(ISSUE, { cbl: "a&b c" })).toBe(
      "/read/invincible/issue-1?cbl=a%26b%20c",
    );
  });
});

describe("issueUrl", () => {
  it("appends ?cbl= the same way as readerUrl", () => {
    expect(issueUrl(ISSUE, { cbl: SV_ID })).toBe(
      `/series/invincible/issues/issue-1?cbl=${SV_ID}`,
    );
  });

  it("omits ?cbl= without opts", () => {
    expect(issueUrl(ISSUE)).toBe("/series/invincible/issues/issue-1");
  });
});

describe("coverThumbSrcSet", () => {
  it("builds a 300w/600w srcset for a cover thumb URL", () => {
    expect(coverThumbSrcSet("/issues/abc/pages/0/thumb")).toBe(
      "/issues/abc/pages/0/thumb?variant=cover_small 300w, /issues/abc/pages/0/thumb 600w",
    );
  });

  it("preserves an existing query with &", () => {
    expect(coverThumbSrcSet("/issues/abc/pages/0/thumb?v=2")).toBe(
      "/issues/abc/pages/0/thumb?v=2&variant=cover_small 300w, /issues/abc/pages/0/thumb?v=2 600w",
    );
  });

  it("returns null for non-cover-thumb URLs (provider covers, strips)", () => {
    expect(coverThumbSrcSet("/issues/abc/covers/xyz")).toBeNull();
    // strip page (n > 0) has no small variant
    expect(coverThumbSrcSet("/issues/abc/pages/3/thumb")).toBeNull();
    expect(coverThumbSrcSet("https://cdn.example.com/cover.jpg")).toBeNull();
  });
});

describe("withContentVersion", () => {
  it("appends v to a bare URL and &v to a URL with a query", () => {
    expect(
      withContentVersion("/issues/a/pages/3", "2026-07-04T00:00:00Z"),
    ).toBe("/issues/a/pages/3?v=2026-07-04T00%3A00%3A00Z");
    expect(
      withContentVersion("/issues/a/pages/3/thumb?variant=strip", "s1"),
    ).toBe("/issues/a/pages/3/thumb?variant=strip&v=s1");
  });

  it("passes the URL through untouched when there is no version", () => {
    expect(withContentVersion("/issues/a/pages/3", null)).toBe(
      "/issues/a/pages/3",
    );
    expect(withContentVersion("/issues/a/pages/3", undefined)).toBe(
      "/issues/a/pages/3",
    );
  });

  it("composes with pageBytesSrcSet so variant URLs inherit the stamp", () => {
    const src = withContentVersion("/issues/a/pages/3", "s1");
    expect(pageBytesSrcSet(src, 1000)).toBe(
      "/issues/a/pages/3?v=s1&w=480 480w, /issues/a/pages/3?v=s1&w=720 720w, /issues/a/pages/3?v=s1 1000w",
    );
  });
});
