import { redirect } from "next/navigation";

import { LibraryGridView } from "@/components/library/LibraryGridView";
import { parseLibraryGridFilters } from "@/components/library/library-grid-filters";
import { ScrollToTopOnMount } from "@/components/ScrollToTopOnMount";
import { PageRails } from "@/components/saved-views/PageRails";
import { apiGet } from "@/lib/api/fetch";
import type { LibraryView, PageListView } from "@/lib/api/types";

/** App-Router resolves `searchParams` to this shape; the library grid
 *  funnels every filter dimension through the URL (chip deep-links,
 *  bookmarkable views) so the catch-all index keeps the page typed
 *  without enumerating each parameter. */
type LibraryHomeSearchParams = Record<string, string | undefined>;

export default async function HomePage({
  searchParams,
}: {
  searchParams: Promise<LibraryHomeSearchParams>;
}) {
  const params = await searchParams;
  const query = (params.q ?? "").trim();
  const libraryParam = (params.library ?? "").trim();

  // Legacy `/?q=` retired (audit E2 / 1.6): the home route's old
  // series-only search is superseded by the dedicated multi-category
  // `/search` page — the single search surface after the cmdk
  // consolidation. The grid's in-grid toolbar search keeps its query in
  // local state and never emits `?q=`, so this redirect only fires for an
  // external or bookmarked `/?q=` link.
  if (query) {
    redirect(`/search?q=${encodeURIComponent(query)}`);
  }

  // `?library=all` and `?library=<uuid>` both render the metadata
  // library grid; bare `/` keeps the pinned saved-views home (M7).
  if (libraryParam) {
    const libraries = await apiGet<LibraryView[]>("/libraries").catch(
      () => [] as LibraryView[],
    );
    const initialFilters = parseLibraryGridFilters(params);
    // Key on the library only — NOT the filter signature. The grid now
    // syncs facets ↔ URL itself (audit B2, `useLibraryGridFilters`):
    // it applies external chip deep-links onto its own state and writes
    // its own changes back, so it must NOT remount when its own URL
    // write-back lands. A library switch (`all` ↔ a uuid) still changes
    // the key and remounts, giving the new library a fresh grid.
    if (libraryParam === "all") {
      return (
        <LibraryGridView
          key="all"
          libraryId={null}
          libraryName="All Libraries"
          libraryCount={libraries.length}
          initialFilters={initialFilters}
        />
      );
    }
    const lib = libraries.find((l) => l.id === libraryParam);
    return (
      <LibraryGridView
        key={libraryParam}
        libraryId={libraryParam}
        libraryName={lib?.name ?? "Library"}
        initialFilters={initialFilters}
      />
    );
  }

  // Multi-page rails M5: the bare `/` route resolves to the user's
  // system "Home" page. Server-side fetch avoids the loading flash the
  // old client-only path produced. `/me/pages` lazy-creates the system
  // row on first access, so we're guaranteed to find one — the fallback
  // is defensive for failed/unauthed fetches where middleware will be
  // redirecting anyway. Audit-remediation M4 wrapped the endpoint in
  // `CursorPage<PageView>`; pages are bounded per-user so we just read
  // `.items`.
  const pages = await apiGet<PageListView>("/me/pages").catch(
    () => ({ items: [], total: 0, next_cursor: null }) as PageListView,
  );
  const system = pages.items.find((p) => p.is_system);
  if (!system) {
    return (
      <div className="text-muted-foreground py-12 text-sm">
        Couldn&apos;t load your home page. Reload to try again.
      </div>
    );
  }
  return (
    <>
      {/* Home shares the `/` pathname with the grid + search, so the App
          Router won't auto-reset scroll when arriving here from those
          views — reset it on mount. */}
      <ScrollToTopOnMount />
      <PageRails
        pageId={system.id}
        pageName={system.name}
        pageDescription={system.description ?? null}
        isSystem
        showInSidebar={system.show_in_sidebar}
      />
    </>
  );
}
