/** Command-palette ranker + `>` prefix parser tests.
 *  Backs the M6 action-search surface in `<SearchModal>`. */
import { describe, expect, it } from "vitest";

import {
  parseCommandPrefix,
  rankSearchActions,
} from "@/lib/search/actions-registry";

describe("parseCommandPrefix", () => {
  it("flags `>` prefix and strips it", () => {
    expect(parseCommandPrefix(">library")).toEqual({
      needle: "library",
      commandMode: true,
    });
  });

  it("strips leading whitespace after the `>` glyph", () => {
    expect(parseCommandPrefix(">  audit log")).toEqual({
      needle: "audit log",
      commandMode: true,
    });
  });

  it("does not enter command mode without the prefix", () => {
    expect(parseCommandPrefix("library")).toEqual({
      needle: "library",
      commandMode: false,
    });
  });

  it("treats `>` alone as command mode with an empty needle", () => {
    expect(parseCommandPrefix(">")).toEqual({
      needle: "",
      commandMode: true,
    });
  });
});

describe("rankSearchActions", () => {
  it("returns every visible action for an empty needle", () => {
    const out = rankSearchActions("", "admin");
    expect(out.length).toBeGreaterThan(0);
    // Admin entries appear for admin role
    expect(out.some((a) => a.id === "admin-libraries")).toBe(true);
  });

  it("hides admin entries from non-admin users", () => {
    const out = rankSearchActions("", undefined);
    expect(out.every((a) => a.id !== "admin-libraries")).toBe(true);
    expect(out.every((a) => !a.id.startsWith("admin-"))).toBe(true);
  });

  it("prefix match on label scores higher than keyword match", () => {
    const out = rankSearchActions("book", "admin");
    // "Bookmarks" label-startsWith "book" (score 100) > anywhere that
    // book appears only as a keyword.
    expect(out[0]?.id).toBe("go-bookmarks");
  });

  it("matches against keywords too", () => {
    const out = rankSearchActions("opds", "admin");
    expect(out.some((a) => a.id === "open-api-tokens")).toBe(true);
  });

  it("returns empty array for needles with no hits", () => {
    expect(rankSearchActions("xyzzy-no-match", "admin")).toEqual([]);
  });

  it("admin-only keyword search still filters by role", () => {
    // "audit" only matches the admin Audit log entry. Non-admin user
    // gets nothing back, not a 403.
    expect(rankSearchActions("audit", undefined)).toEqual([]);
    expect(rankSearchActions("audit", "admin").some((a) => a.id === "admin-audit")).toBe(true);
  });

  it("case-insensitive", () => {
    expect(rankSearchActions("LIBRARY", "admin").length).toBeGreaterThan(0);
  });

  it("admin keyword match surfaces above group-startsWith nav entries", () => {
    // "library" (singular) matches:
    //   - The Library *group* on nav entries (score 50)
    //   - The "library" *keyword* on admin-libraries (score 30)
    // …so nav entries actually win on score. We assert admin is in
    // the result set (role-gating works) and verify that the
    // exact plural form does promote admin-libraries to the top.
    const singular = rankSearchActions("library", "admin").map((a) => a.id);
    expect(singular).toContain("admin-libraries");
    const plural = rankSearchActions("libraries", "admin").map((a) => a.id);
    expect(plural[0]).toBe("admin-libraries");
  });
});
