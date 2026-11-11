/**
 * Multi-page rails M5 — `<PageRails>` prop wiring smoke test.
 *
 * Vitest's node env can't fully render the rail tree (TanStack hooks +
 * dnd children need a DOM), so `PageRails` is called as a plain function
 * and the element tree it returns is walked. Two things are pinned:
 *
 *  - each call site asks the server for "pins on this page";
 *  - cover size is the app-wide one (lib/library/cover-size.ts): the page
 *    renders the shared `CoverSizeOptions` / `CoverSizeFields` controls,
 *    holds no size state, and hands no size to the rails or the loading
 *    skeleton. Home and a custom page are identical here — a page no
 *    longer has a density key of its own.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type * as React from "react";

import type { SavedViewView } from "@/lib/api/types";

type CapturedFilters = {
  pinned?: boolean;
  pinnedOn?: string;
} | null;

let captured: CapturedFilters = null;
let pinned: { items: SavedViewView[]; isLoading: boolean } = {
  items: [],
  isLoading: false,
};

vi.mock("@/lib/api/queries", () => ({
  useSavedViews: (filters: CapturedFilters) => {
    captured = filters;
    return pinned.isLoading
      ? { data: undefined, isLoading: true, isError: false }
      : { data: { items: pinned.items }, isLoading: false, isError: false };
  },
}));

// Identity stubs: the walk matches elements by component type, so the
// real popover / slider and the rail's data hooks never have to load.
vi.mock("@/components/library/CoverSizeOptions", () => ({
  CoverSizeOptions: () => null,
  CoverSizeFields: () => null,
}));
vi.mock("@/components/saved-views/LazyRail", () => ({
  LazyRail: () => null,
}));

import {
  CoverSizeFields,
  CoverSizeOptions,
} from "@/components/library/CoverSizeOptions";
import { PhoneControlsBar } from "@/components/library/PhoneControlsBar";
import { StickyToolbarRow } from "@/components/library/StickyToolbarRow";
import { LazyRail } from "@/components/saved-views/LazyRail";
import { PageRails } from "@/components/saved-views/PageRails";
import { RailsSkeleton } from "@/components/skeletons/LoadingShapes";

const customProps = {
  pageId: "page-marvel",
  pageName: "Marvel",
  pageDescription: null,
  isSystem: false,
  showInSidebar: true,
} as const;
const systemProps = {
  pageId: "sys-home",
  pageName: "Home",
  pageDescription: null,
  isSystem: true,
  showInSidebar: true,
} as const;

function view(id: string): SavedViewView {
  return {
    id,
    kind: "filter_series",
    user_id: "u1",
    is_system: false,
    name: `View ${id}`,
    description: null,
    custom_year_start: null,
    custom_year_end: null,
    custom_tags: [],
    match_mode: "all",
    conditions: [],
    sort_field: "name",
    sort_order: "asc",
    result_limit: 12,
    cbl_list_id: null,
    pinned: true,
    pinned_position: 0,
    show_in_sidebar: false,
    pinned_on_pages: [],
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  };
}

/** Every element of `type` under `node`, following `children` only. */
function collectByType<P = Record<string, unknown>>(
  node: React.ReactNode,
  type: unknown,
): Array<React.ReactElement<P>> {
  const found: Array<React.ReactElement<P>> = [];
  const stack: React.ReactNode[] = [node];
  while (stack.length) {
    const cur = stack.shift();
    if (Array.isArray(cur)) {
      stack.push(...cur);
      continue;
    }
    if (!cur || typeof cur !== "object" || !("type" in cur)) continue;
    const el = cur as React.ReactElement<{ children?: React.ReactNode }>;
    if (el.type === type) found.push(el as unknown as React.ReactElement<P>);
    if (el.props && el.props.children) stack.push(el.props.children);
  }
  return found;
}

describe("PageRails prop wiring", () => {
  beforeEach(() => {
    captured = null;
    pinned = { items: [], isLoading: false };
  });

  it("queries pins scoped to the supplied page id", () => {
    PageRails(customProps);
    expect(captured).toEqual({ pinnedOn: "page-marvel" });
  });

  describe.each([
    ["a custom page", customProps],
    ["the system Home page", systemProps],
  ] as const)("cover size on %s", (_label, props) => {
    it("renders the shared CoverSizeOptions in the pinned row, with no size of its own", () => {
      const tree = PageRails(props);
      const rows = collectByType(tree, StickyToolbarRow);
      expect(rows).toHaveLength(1);
      const controls = collectByType(rows[0], CoverSizeOptions);
      expect(controls).toHaveLength(1);
      // No value, setter, range or storage key: the control reads and
      // writes the app-wide size itself.
      expect(controls[0]!.props).toEqual({});
      // …and the row's is the only one on the page.
      expect(collectByType(tree, CoverSizeOptions)).toHaveLength(1);
    });

    it("offers the same shared size in the phone View options sheet", () => {
      const bars = collectByType<{
        menu?: (close: () => void) => React.ReactNode;
      }>(PageRails(props), PhoneControlsBar);
      expect(bars).toHaveLength(1);
      const sheet = bars[0]!.props.menu?.(() => undefined);
      const fields = collectByType(sheet, CoverSizeFields);
      expect(fields).toHaveLength(1);
      expect(fields[0]!.props).toEqual({ fieldId: "page-phone-card-size" });
    });

    it("hands each rail its view and priority, and no size", () => {
      const items = [view("v1"), view("v2")];
      pinned = { items, isLoading: false };
      const rails = collectByType(PageRails(props), LazyRail);
      expect(rails.map((rail) => rail.props)).toEqual([
        { view: items[0], priority: true, eager: true },
        { view: items[1], priority: false, eager: false },
      ]);
    });

    it("prioritizes the rail the server prefetch settled on, mounting the hidden ones before it", () => {
      const items = [view("v1"), view("v2"), view("v3")];
      pinned = { items, isLoading: false };
      const rails = collectByType(
        PageRails({ ...props, priorityViewId: "v2" }),
        LazyRail,
      );
      expect(rails.map((rail) => rail.props)).toEqual([
        { view: items[0], priority: false, eager: true },
        { view: items[1], priority: true, eager: true },
        { view: items[2], priority: false, eager: false },
      ]);
    });

    it("passes no size to the loading skeleton", () => {
      pinned = { items: [], isLoading: true };
      const skeletons = collectByType(PageRails(props), RailsSkeleton);
      expect(skeletons).toHaveLength(1);
      expect(skeletons[0]!.props).toEqual({
        rails: 3,
        card: props.isSystem ? "issue" : "series",
        label: "Loading views…",
      });
    });
  });
});
