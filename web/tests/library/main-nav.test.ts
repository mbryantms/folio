/**
 * Snapshot tests for the sidebar nav builder. After navigation
 * customization M1 `mainNav()` consumes a `SidebarLayoutView` from
 * `/me/sidebar-layout` — the server already resolves order,
 * visibility, label, icon, and href, so the function's job is just to
 * group consecutive same-kind entries into the legacy
 * `MainNavSection[]` shape that [`MainSidebar`](../../components/library/MainSidebar.tsx)
 * still consumes.
 *
 * These tests pin: (a) the default ordering matches the legacy
 * three-section layout, (b) hidden entries don't render, (c) interleaved
 * kinds split into multiple sections instead of being silently merged.
 */
import { describe, expect, it } from "vitest";

import { mainNav } from "@/components/library/main-nav";
import type {
  SidebarEntryView,
  SidebarLayoutView,
} from "@/lib/api/types";

function entry(
  overrides: Partial<SidebarEntryView> & Pick<SidebarEntryView, "kind" | "ref_id">,
): SidebarEntryView {
  return {
    label: overrides.ref_id,
    icon: "Sparkles",
    href: `/${overrides.ref_id}`,
    visible: true,
    position: 0,
    ...overrides,
  };
}

/** Mirror of the server `compute_layout` output — explicit header rows
 *  bookend each group, matching the new layout contract where section
 *  labels are server-supplied rather than client-inferred from kind. */
function defaultLayout(): SidebarLayoutView {
  return {
    entries: [
      {
        kind: "header",
        ref_id: "default:browse",
        label: "Browse",
        icon: "",
        href: "",
        visible: true,
        position: 0,
      },
      {
        kind: "builtin",
        ref_id: "home",
        label: "Home",
        icon: "Home",
        href: "/",
        visible: true,
        position: 1,
      },
      {
        kind: "builtin",
        ref_id: "bookmarks",
        label: "Bookmarks",
        icon: "Bookmark",
        href: "/bookmarks",
        visible: true,
        position: 2,
      },
      {
        kind: "builtin",
        ref_id: "collections",
        label: "Collections",
        icon: "Folder",
        href: "/collections",
        visible: true,
        position: 3,
      },
      {
        kind: "builtin",
        ref_id: "want_to_read",
        label: "Want to Read",
        icon: "ListPlus",
        href: "/views/want-to-read",
        visible: true,
        position: 4,
      },
      {
        kind: "header",
        ref_id: "default:libraries",
        label: "Libraries",
        icon: "",
        href: "",
        visible: true,
        position: 5,
      },
      {
        kind: "library",
        ref_id: "all",
        label: "All Libraries",
        icon: "Library",
        href: "/?library=all",
        visible: true,
        position: 6,
      },
      {
        kind: "library",
        ref_id: "lib-1",
        label: "Main",
        icon: "Library",
        href: "/?library=lib-1",
        visible: true,
        position: 7,
      },
    ],
  };
}

describe("mainNav default layout", () => {
  it("contains Home / Bookmarks / Collections / Want to Read in order", () => {
    const sections = mainNav("", defaultLayout());
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
    const sections = mainNav("", defaultLayout());
    const labels = sections.flatMap((s) => s.items.map((i) => i.label));
    expect(labels).not.toContain("Favorites");
  });

  it("Bookmarks links to /bookmarks", () => {
    const sections = mainNav("", defaultLayout());
    const bookmarks = sections
      .find((s) => s.label === "Browse")!
      .items.find((i) => i.label === "Bookmarks");
    expect(bookmarks?.href).toBe("/bookmarks");
  });

  it("Want to Read links to the kebab-case alias /views/want-to-read", () => {
    const sections = mainNav("/en", defaultLayout());
    const wtr = sections
      .find((s) => s.label === "Browse")!
      .items.find((i) => i.label === "Want to Read");
    expect(wtr?.href).toBe("/en/views/want-to-read");
  });

  it("Collections links to /collections", () => {
    const sections = mainNav("/en", defaultLayout());
    const collections = sections
      .find((s) => s.label === "Browse")!
      .items.find((i) => i.label === "Collections");
    expect(collections?.href).toBe("/en/collections");
  });

  it("matches the locked default shape (snapshot)", () => {
    // Built-ins (Browse) followed by the libraries group with the
    // synthetic "All Libraries" leading the real libraries. Any change
    // to BUILTIN_REGISTRY or the synthetic-entry order surfaces here as
    // a deliberate test update.
    const sections = mainNav("", defaultLayout());
    expect(sections).toMatchInlineSnapshot(`
      [
        {
          "items": [
            {
              "href": "/",
              "icon": "Home",
              "label": "Home",
              "pageId": undefined,
            },
            {
              "href": "/bookmarks",
              "icon": "Bookmark",
              "label": "Bookmarks",
              "pageId": undefined,
            },
            {
              "href": "/collections",
              "icon": "Folder",
              "label": "Collections",
              "pageId": undefined,
            },
            {
              "href": "/views/want-to-read",
              "icon": "ListPlus",
              "label": "Want to Read",
              "pageId": undefined,
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
              "pageId": undefined,
            },
            {
              "href": "/?library=lib-1",
              "icon": "Library",
              "label": "Main",
              "pageId": undefined,
            },
          ],
          "label": "Libraries",
        },
      ]
    `);
  });
});

describe("mainNav with saved views and overrides", () => {
  it("adds a Saved views section when a header + view follow the libraries", () => {
    const layout = defaultLayout();
    layout.entries.push(
      {
        kind: "header",
        ref_id: "default:views",
        label: "Saved views",
        icon: "",
        href: "",
        visible: true,
        position: 8,
      },
      {
        kind: "view",
        ref_id: "v-pinned",
        label: "My Filter",
        icon: "filter",
        href: "/views/v-pinned",
        visible: true,
        position: 9,
      },
    );
    const sections = mainNav("", layout);
    expect(sections.map((s) => s.label)).toEqual([
      "Browse",
      "Libraries",
      "Saved views",
    ]);
    const saved = sections.find((s) => s.label === "Saved views")!;
    expect(saved.items.map((i) => i.label)).toEqual(["My Filter"]);
  });

  it("hidden entries are dropped before sections are built", () => {
    const layout = defaultLayout();
    // Hide Collections; Browse should still be a contiguous run, just
    // shorter. "All Libraries" stays in the Libraries section.
    const collections = layout.entries.find(
      (e) => e.ref_id === "collections",
    )!;
    collections.visible = false;
    const sections = mainNav("", layout);
    const browse = sections.find((s) => s.label === "Browse")!;
    expect(browse.items.map((i) => i.label)).toEqual([
      "Home",
      "Bookmarks",
      "Want to Read",
    ]);
  });

  it("custom headers split groups even when interleaved kinds appear", () => {
    // With server-supplied headers driving section boundaries, the
    // user can deliberately break a run of same-kind items into two
    // headed groups (or vice versa).
    const layout: SidebarLayoutView = {
      entries: [
        entry({
          kind: "header",
          ref_id: "default:browse",
          label: "Browse",
          icon: "",
          href: "",
          position: 0,
        }),
        entry({
          kind: "builtin",
          ref_id: "home",
          label: "Home",
          icon: "Home",
          href: "/",
          position: 1,
        }),
        entry({
          kind: "builtin",
          ref_id: "bookmarks",
          label: "Bookmarks",
          icon: "Bookmark",
          href: "/bookmarks",
          position: 2,
        }),
        entry({
          kind: "header",
          ref_id: "custom:filters",
          label: "Filters",
          icon: "",
          href: "",
          position: 3,
        }),
        entry({
          kind: "view",
          ref_id: "v-1",
          label: "My View",
          icon: "filter",
          href: "/views/v-1",
          position: 4,
        }),
        entry({
          kind: "header",
          ref_id: "custom:more",
          label: "More",
          icon: "",
          href: "",
          position: 5,
        }),
        entry({
          kind: "builtin",
          ref_id: "collections",
          label: "Collections",
          icon: "Folder",
          href: "/collections",
          position: 6,
        }),
      ],
    };
    const sections = mainNav("", layout);
    expect(sections.map((s) => [s.label, s.items.map((i) => i.label)]))
      .toEqual([
        ["Browse", ["Home", "Bookmarks"]],
        ["Filters", ["My View"]],
        ["More", ["Collections"]],
      ]);
  });

  it("spacer entries emit a spacer section between content groups", () => {
    const layout: SidebarLayoutView = {
      entries: [
        entry({
          kind: "header",
          ref_id: "h1",
          label: "Top",
          icon: "",
          href: "",
          position: 0,
        }),
        entry({
          kind: "builtin",
          ref_id: "home",
          label: "Home",
          icon: "Home",
          href: "/",
          position: 1,
        }),
        entry({
          kind: "spacer",
          ref_id: "s1",
          label: "",
          icon: "",
          href: "",
          position: 2,
        }),
        entry({
          kind: "header",
          ref_id: "h2",
          label: "Bottom",
          icon: "",
          href: "",
          position: 3,
        }),
        entry({
          kind: "builtin",
          ref_id: "bookmarks",
          label: "Bookmarks",
          icon: "Bookmark",
          href: "/bookmarks",
          position: 4,
        }),
      ],
    };
    const sections = mainNav("", layout);
    expect(sections.length).toBe(3);
    expect(sections[0]).toMatchObject({
      label: "Top",
      items: [{ label: "Home" }],
    });
    expect(sections[1]).toMatchObject({ isSpacer: true });
    expect(sections[2]).toMatchObject({
      label: "Bottom",
      items: [{ label: "Bookmarks" }],
    });
  });

  it("locale prefix is applied to every href", () => {
    const sections = mainNav("/en", defaultLayout());
    const hrefs = sections.flatMap((s) => s.items.map((i) => i.href));
    expect(hrefs).toEqual([
      "/en/",
      "/en/bookmarks",
      "/en/collections",
      "/en/views/want-to-read",
      "/en/?library=all",
      "/en/?library=lib-1",
    ]);
  });

  it("empty layout returns no sections", () => {
    expect(mainNav("", { entries: [] })).toEqual([]);
  });

  it("kind='page' entries surface in a server-headered 'Pages' section", () => {
    // Multi-page rails M4: the server emits a "Pages" header before
    // the custom-page rows when at least one exists. Client renders
    // them under that section verbatim.
    const layout: SidebarLayoutView = {
      entries: [
        entry({
          kind: "header",
          ref_id: "default:browse",
          label: "Browse",
          icon: "",
          href: "",
          position: 0,
        }),
        entry({
          kind: "builtin",
          ref_id: "home",
          label: "Library", // renamed system page bleeds into the label
          icon: "Home",
          href: "/",
          position: 1,
        }),
        entry({
          kind: "header",
          ref_id: "default:pages",
          label: "Pages",
          icon: "",
          href: "",
          position: 2,
        }),
        entry({
          kind: "page",
          ref_id: "page-1",
          label: "Marvel",
          icon: "LayoutGrid",
          href: "/pages/marvel",
          position: 3,
        }),
      ],
    };
    const sections = mainNav("", layout);
    expect(sections.map((s) => [s.label, s.items.map((i) => i.label)]))
      .toEqual([
        ["Browse", ["Library"]],
        ["Pages", ["Marvel"]],
      ]);
    const marvel = sections
      .find((s) => s.label === "Pages")!
      .items.find((i) => i.label === "Marvel")!;
    expect(marvel.href).toBe("/pages/marvel");
    expect(marvel.icon).toBe("LayoutGrid");
  });
});
