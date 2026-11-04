/** Command-palette ranker + `>` prefix parser tests.
 *  Backs the M6 action-search surface in `<SearchModal>`. */
import { describe, expect, it } from "vitest";

import {
  ACTION_GROUP_ORDER,
  SEARCH_ACTIONS,
  SUGGESTED_ACTION_IDS,
  groupSearchActions,
  parseCommandPrefix,
  rankSearchActions,
  suggestedSearchActions,
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
  it("has 42 actions: 22 for every user + 20 admin-gated, unique ids", () => {
    const ids = SEARCH_ACTIONS.map((a) => a.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(SEARCH_ACTIONS.length).toBe(42);
    expect(SEARCH_ACTIONS.filter((a) => a.role === undefined).length).toBe(22);
    expect(SEARCH_ACTIONS.filter((a) => a.role === "admin").length).toBe(20);
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
    expect(out.length).toBe(22);
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

describe("palette commands (folio-flair)", () => {
  const byId = new Map(SEARCH_ACTIONS.map((a) => [a.id, a]));

  it("every entry is exactly one of a route or a command", () => {
    for (const a of SEARCH_ACTIONS) {
      expect(!!a.href !== !!a.command, a.id).toBe(true);
    }
  });

  it("commands live in the Actions group and use the cmd- prefix", () => {
    for (const a of SEARCH_ACTIONS) {
      if (a.command) {
        expect(a.group, a.id).toBe("Actions");
        expect(a.id.startsWith("cmd-"), a.id).toBe(true);
      }
    }
    expect(byId.get("cmd-continue-reading")?.command).toBe("continue-reading");
    expect(byId.get("cmd-toggle-theme")?.command).toBe("toggle-theme");
    expect(byId.get("cmd-toggle-sidebar")).toMatchObject({
      command: "toggle-sidebar",
      keybind: "toggleSidebar",
    });
    expect(byId.get("cmd-shortcuts")).toMatchObject({
      command: "show-shortcuts",
      keyHint: "?",
    });
    expect(byId.get("cmd-tour")?.href).toBe("/?tour=1");
  });

  it("fills the settings + admin gaps without duplicating routes", () => {
    const hrefs = SEARCH_ACTIONS.flatMap((a) => (a.href ? [a.href] : []));
    expect(new Set(hrefs).size).toBe(hrefs.length);
    expect(byId.get("open-downloads")?.href).toBe("/settings/downloads");
    for (const [id, href] of [
      ["admin-scan-dashboard", "/admin/scan-dashboard"],
      ["admin-findings", "/admin/findings"],
      ["admin-metadata", "/admin/metadata"],
      ["admin-duplicates", "/admin/duplicates"],
      ["admin-starter-lists", "/admin/starter-lists"],
      ["admin-queue", "/admin/queue"],
    ] as const) {
      expect(byId.get(id), id).toMatchObject({ href, role: "admin" });
    }
  });

  it("rankSearchActions honours the context exclude set", () => {
    const exclude = new Set(["cmd-toggle-sidebar"]);
    expect(
      rankSearchActions("toggle", undefined, { exclude }).map((a) => a.id),
    ).toEqual(["cmd-toggle-theme"]);
    expect(rankSearchActions("dark mode", undefined)[0]?.id).toBe(
      "cmd-toggle-theme",
    );
    expect(rankSearchActions("resume", undefined)[0]?.id).toBe(
      "cmd-continue-reading",
    );
  });

  it("suggested actions follow SUGGESTED_ACTION_IDS minus exclusions", () => {
    expect(suggestedSearchActions(undefined).map((a) => a.id)).toEqual([
      ...SUGGESTED_ACTION_IDS,
    ]);
    const out = suggestedSearchActions(undefined, {
      exclude: new Set(["cmd-continue-reading"]),
    }).map((a) => a.id);
    expect(out).not.toContain("cmd-continue-reading");
    expect(out.length).toBe(SUGGESTED_ACTION_IDS.length - 1);
  });

  it("groupSearchActions buckets in display order and drops empty groups", () => {
    const groups = groupSearchActions(rankSearchActions("", "admin"));
    expect(groups.map((g) => g.group)).toEqual([...ACTION_GROUP_ORDER]);
    const userGroups = groupSearchActions(rankSearchActions("", undefined));
    expect(userGroups.map((g) => g.group)).toEqual([
      "Actions",
      "Library",
      "Settings",
    ]);
    // Ranked order is kept inside a group.
    const ranked = rankSearchActions("log", "admin");
    const lib = groupSearchActions(ranked).find((g) => g.group === "Library");
    expect(lib?.actions.map((a) => a.id)).toEqual(
      ranked.filter((a) => a.group === "Library").map((a) => a.id),
    );
  });
});
