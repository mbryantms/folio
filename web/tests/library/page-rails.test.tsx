/**
 * Multi-page rails M5 — `<PageRails>` prop wiring smoke test.
 *
 * Vitest's node env can't fully render the rail tree (TanStack hooks +
 * dnd children need a DOM), so we mock `useSavedViews` to capture the
 * filters the component passes through. That's the surface we care
 * about for M5: each call site asks the server for "pins on this
 * page", with a per-page density key that doesn't clobber Home's.
 */
import { describe, expect, it, vi } from "vitest";

import type { SavedViewListView } from "@/lib/api/types";

type CapturedFilters = {
  pinned?: boolean;
  pinnedOn?: string;
} | null;

let captured: CapturedFilters = null;
let lastStorageKey: string | null = null;

vi.mock("@/lib/api/queries", () => ({
  useSavedViews: (filters: CapturedFilters) => {
    captured = filters;
    const items: SavedViewListView["items"] = [];
    return { data: { items }, isLoading: false };
  },
}));

vi.mock("@/components/library/use-card-size", () => ({
  useCardSize: ({ storageKey }: { storageKey: string }) => {
    lastStorageKey = storageKey;
    return [160, () => undefined] as const;
  },
}));

vi.mock("@/components/LibrarySearch", () => ({
  LibrarySearch: () => null,
}));
vi.mock("@/components/library/CardSizeOptions", () => ({
  CardSizeOptions: () => null,
}));
vi.mock("./SavedViewRail", () => ({
  SavedViewRail: () => null,
}));

import { PageRails } from "@/components/saved-views/PageRails";

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

describe("PageRails prop wiring", () => {
  it("queries pins scoped to the supplied page id", () => {
    captured = null;
    PageRails(customProps);
    expect(captured).toEqual({ pinnedOn: "page-marvel" });
  });

  it("custom pages use a per-page localStorage key for density", () => {
    lastStorageKey = null;
    PageRails(customProps);
    expect(lastStorageKey).toBe("folio.page.cardSize.page-marvel");
  });

  it("system page keeps the legacy folio.home.cardSize key", () => {
    lastStorageKey = null;
    PageRails(systemProps);
    expect(lastStorageKey).toBe("folio.home.cardSize");
  });
});
