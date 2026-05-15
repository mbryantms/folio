/**
 * Unit tests for `web/lib/urls.ts` — the URL builders that thread
 * reading-context (`?cbl=`) onto reader / issue links. Other URL builders
 * are trivial enough that the typed signatures + grep-able call sites are
 * the safety net; the optional-opts shape is the part worth exercising.
 */
import { describe, expect, it } from "vitest";

import { issueUrl, readerUrl } from "@/lib/urls";

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
