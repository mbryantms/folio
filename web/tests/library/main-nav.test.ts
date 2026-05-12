/**
 * Snapshot tests for the sidebar nav builder. Locks the Browse section
 * shape so the markers + collections M3 changes (drop Favorites, add
 * Collections + Want to Read) don't regress quietly.
 */
import { describe, expect, it } from "vitest";

import { mainNav } from "@/components/library/main-nav";
import type { LibraryView, SavedViewView } from "@/lib/api/types";

function lib(overrides: Partial<LibraryView> = {}): LibraryView {
  return {
    id: "lib-1",
    slug: "lib-1",
    name: "Main",
    root_path: "/library",
    default_language: "en",
    default_reading_direction: "ltr",
    dedupe_by_content: false,
    scan_schedule_cron: null,
    last_scan_at: null,
    file_watch_enabled: true,
    soft_delete_days: 30,
    ignore_globs: [],
    report_missing_comicinfo: false,
    generate_page_thumbs_on_scan: false,
    ...overrides,
  };
}

describe("mainNav Browse section", () => {
  it("contains Home / Bookmarks / Collections / Want to Read in order", () => {
    const sections = mainNav("", [lib()]);
    const browse = sections.find((s) => s.label === "Browse");
    expect(browse).toBeDefined();
    expect(browse!.items.map((i) => i.label)).toEqual([
      "Home",
      "Bookmarks",
      "Collections",
      "Want to Read",
    ]);
  });

  it("Favorites is gone", () => {
    const sections = mainNav("", []);
    const labels = sections.flatMap((s) => s.items.map((i) => i.label));
    expect(labels).not.toContain("Favorites");
  });

  it("Bookmarks is no longer a placeholder once markers M6 has shipped", () => {
    const sections = mainNav("", []);
    const bookmarks = sections
      .find((s) => s.label === "Browse")!
      .items.find((i) => i.label === "Bookmarks");
    expect(bookmarks?.placeholder).not.toBe(true);
    expect(bookmarks?.href).toBe("/bookmarks");
  });

  it("Collections + Want to Read are NOT placeholders", () => {
    const sections = mainNav("", []);
    const browse = sections.find((s) => s.label === "Browse")!;
    expect(
      browse.items.find((i) => i.label === "Collections")?.placeholder,
    ).not.toBe(true);
    expect(
      browse.items.find((i) => i.label === "Want to Read")?.placeholder,
    ).not.toBe(true);
  });

  it("Want to Read links to the kebab-case alias /views/want-to-read", () => {
    const sections = mainNav("/en", []);
    const wtr = sections
      .find((s) => s.label === "Browse")!
      .items.find((i) => i.label === "Want to Read");
    expect(wtr?.href).toBe("/en/views/want-to-read");
  });

  it("Collections links to /collections", () => {
    const sections = mainNav("/en", []);
    const collections = sections
      .find((s) => s.label === "Browse")!
      .items.find((i) => i.label === "Collections");
    expect(collections?.href).toBe("/en/collections");
  });

  it("matches the locked Browse shape (snapshot)", () => {
    // Markers M7 acceptance: fresh sign-in shows Home / Bookmarks /
    // Libraries / Collections / Want to Read. No Favorites. No
    // placeholders. The exact shape lives here so future nav tweaks
    // surface a deliberate test update instead of silent drift.
    const sections = mainNav("", [lib({ id: "lib-1", name: "Main" })]);
    expect(sections).toMatchInlineSnapshot(`
      [
        {
          "items": [
            {
              "href": "/",
              "icon": "Home",
              "label": "Home",
            },
            {
              "href": "/bookmarks",
              "icon": "Bookmark",
              "label": "Bookmarks",
            },
            {
              "href": "/collections",
              "icon": "Folder",
              "label": "Collections",
            },
            {
              "href": "/views/want-to-read",
              "icon": "ListPlus",
              "label": "Want to Read",
            },
          ],
          "label": "Browse",
        },
        {
          "items": [
            {
              "href": "/?library=all",
              "icon": "Library",
              "label": "All Libraries",
            },
            {
              "href": "/?library=lib-1",
              "icon": "Library",
              "label": "Main",
            },
          ],
          "label": "Libraries",
        },
      ]
    `);
  });

  it("with sidebar views appended, adds a Saved views section but Browse is unchanged", () => {
    const view: SavedViewView = {
      id: "v-pinned",
      kind: "filter_series",
      user_id: "u1",
      is_system: false,
      name: "My Filter",
      description: null,
      custom_year_start: null,
      custom_year_end: null,
      custom_tags: [],
      match_mode: "all",
      conditions: [],
      sort_field: "created_at",
      sort_order: "desc",
      result_limit: 12,
      cbl_list_id: null,
      pinned: false,
      pinned_position: null,
      show_in_sidebar: true,
      icon: null,
      system_key: null,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
    };
    const sections = mainNav("", [], [view]);
    expect(sections.map((s) => s.label)).toEqual([
      "Browse",
      "Libraries",
      "Saved views",
    ]);
    const browse = sections.find((s) => s.label === "Browse")!;
    expect(browse.items.map((i) => i.label)).toEqual([
      "Home",
      "Bookmarks",
      "Collections",
      "Want to Read",
    ]);
  });

  it("kind='collection' saved views map to the Folder default icon", () => {
    const view: SavedViewView = {
      id: "c1",
      kind: "collection",
      user_id: "u1",
      is_system: false,
      name: "My Capes",
      description: null,
      custom_year_start: null,
      custom_year_end: null,
      custom_tags: [],
      match_mode: null,
      conditions: null,
      sort_field: null,
      sort_order: null,
      result_limit: null,
      cbl_list_id: null,
      pinned: true,
      pinned_position: 0,
      show_in_sidebar: true,
      icon: null,
      system_key: null,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
    };
    const sections = mainNav("/en", [], [view]);
    const savedSection = sections.find((s) => s.label === "Saved views");
    expect(savedSection?.items[0]?.icon).toBe("Folder");
  });
});
