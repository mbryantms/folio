import { LibraryGridView } from "@/components/library/LibraryGridView";
import { parseLibraryGridFilters } from "@/components/library/library-grid-filters";
import { SeriesCard } from "@/components/library/SeriesCard";
import { ScrollToTopOnMount } from "@/components/ScrollToTopOnMount";
import { PageRails } from "@/components/saved-views/PageRails";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type {
  LibraryView,
  PageListView,
  SeriesListView,
} from "@/lib/api/types";

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

  // Search keeps its existing flat-grid view — saved-view rails are
  // about discovery; search is about finding a thing right now.
  if (query) {
    const libraries = await apiGet<LibraryView[]>("/libraries").catch(
      () => [] as LibraryView[],
    );
    return <SearchView query={query} libraries={libraries} />;
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

async function SearchView({
  query,
  libraries,
}: {
  query: string;
  libraries: LibraryView[] | null;
}) {
  const result = await fetchSeries(
    `/series?q=${encodeURIComponent(query)}&limit=100`,
  );
  const items = result?.items ?? [];

  return (
    <>
      <div className="mb-6">
        <h1 className="text-2xl font-semibold tracking-tight">Search</h1>
        <p className="text-muted-foreground mt-1 text-sm">
          {libraries?.length ?? 0}{" "}
          {libraries?.length === 1 ? "library" : "libraries"} · {items.length}{" "}
          {items.length === 1 ? "match" : "matches"} for{" "}
          <span className="text-foreground font-medium">
            &ldquo;{query}&rdquo;
          </span>
        </p>
      </div>
      {items.length === 0 ? (
        <p className="text-muted-foreground text-sm">
          No series matched <code className="text-foreground">{query}</code>.
        </p>
      ) : (
        <ul className="grid grid-cols-2 gap-4 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5">
          {items.map((s) => (
            <li key={s.id}>
              <SeriesCard series={s} size="md" />
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

// Hide individual rail failures behind an empty result so one library
// hiccup doesn't blank the whole search page.
async function fetchSeries(path: string): Promise<SeriesListView | null> {
  try {
    return await apiGet<SeriesListView>(path);
  } catch (e) {
    if (e instanceof ApiError) return null;
    throw e;
  }
}
