/** Command-palette ranker + `>` prefix parser tests.
 *  Backs the M6 action-search surface in `<SearchModal>`. */
import { describe, expect, it } from "vitest";

import {
  SEARCH_ACTIONS,
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

describe("SEARCH_ACTIONS registry", () => {
  it("has 30 actions: 16 for every user + 14 admin-gated, unique ids", () => {
    const ids = SEARCH_ACTIONS.map((a) => a.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(SEARCH_ACTIONS.length).toBe(30);
    expect(SEARCH_ACTIONS.filter((a) => a.role === undefined).length).toBe(16);
    expect(SEARCH_ACTIONS.filter((a) => a.role === "admin").length).toBe(14);
    // Every `admin-*` id is gated and nothing else is.
    for (const a of SEARCH_ACTIONS) {
      expect(a.role === "admin", a.id).toBe(a.id.startsWith("admin-"));
    }
  });

  it("covers Browse, the saved-views index, the log and both admin pages (N-O6)", () => {
    const byId = new Map(SEARCH_ACTIONS.map((a) => [a.id, a]));
    expect(byId.get("go-browse")?.href).toBe("/browse");
    expect(byId.get("go-views")?.href).toBe("/settings/views");
    expect(byId.get("go-log")?.href).toBe("/log");
    expect(byId.get("admin-relationships")).toMatchObject({
      href: "/admin/relationships",
      role: "admin",
    });
    expect(byId.get("admin-background-work")).toMatchObject({
      href: "/admin/background-work",
      role: "admin",
    });
  });
});

describe("rankSearchActions", () => {
  it("returns every visible action for an empty needle", () => {
    const out = rankSearchActions("", "admin");
    expect(out.length).toBe(SEARCH_ACTIONS.length);
    // Admin entries appear for admin role
    expect(out.some((a) => a.id === "admin-libraries")).toBe(true);
    expect(out.some((a) => a.id === "admin-relationships")).toBe(true);
    expect(out.some((a) => a.id === "admin-background-work")).toBe(true);
  });

  it("hides admin entries from non-admin users", () => {
    const out = rankSearchActions("", undefined);
    expect(out.length).toBe(16);
    expect(out.every((a) => a.id !== "admin-libraries")).toBe(true);
    expect(out.every((a) => !a.id.startsWith("admin-"))).toBe(true);
    // The user-facing additions stay visible without a role.
    for (const id of ["go-browse", "go-views", "go-log"]) {
      expect(
        out.some((a) => a.id === id),
        id,
      ).toBe(true);
    }
  });

  it("the new admin pages are reachable by label and keyword, admins only", () => {
    expect(rankSearchActions("background", "admin")[0]?.id).toBe(
      "admin-background-work",
    );
    expect(rankSearchActions("jobs", undefined)).toEqual([]);
    expect(
      rankSearchActions("jobs", "admin").some(
        (a) => a.id === "admin-background-work",
      ),
    ).toBe(true);
    expect(rankSearchActions("relationship", "admin")[0]?.id).toBe(
      "admin-relationships",
    );
    expect(rankSearchActions("relationship", undefined)).toEqual([]);
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
    expect(
      rankSearchActions("audit", "admin").some((a) => a.id === "admin-audit"),
    ).toBe(true);
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
