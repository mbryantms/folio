import { LibrarySearch } from "@/components/LibrarySearch";
import { LibraryGridView } from "@/components/library/LibraryGridView";
import { parseLibraryGridFilters } from "@/components/library/library-grid-filters";
import { SeriesCard } from "@/components/library/SeriesCard";
import { PinnedViewsHome } from "@/components/saved-views/PinnedViewsHome";
import { apiGet, ApiError } from "@/lib/api/fetch";
import type { LibraryView, SeriesListView } from "@/lib/api/types";

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
    // Force a remount when the URL filter signature changes — the
    // grid's filter state is local once mounted, so a chip click that
    // navigates to a different filter URL would otherwise reuse the
    // prior component instance and ignore the new initial values.
    const filterKey = filterSignature(params);
    if (libraryParam === "all") {
      return (
        <LibraryGridView
          key={`all|${filterKey}`}
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
        key={`${libraryParam}|${filterKey}`}
        libraryId={libraryParam}
        libraryName={lib?.name ?? "Library"}
        initialFilters={initialFilters}
      />
    );
  }

  return <PinnedViewsHome />;
}

/** Stable string key over the filter-relevant query params. Excludes
 *  `library` (used in the prefix) and `q` (search has its own page).
 *  Sorting the entries makes the key insensitive to URL ordering, so
 *  `?genres=a&tags=b` and `?tags=b&genres=a` produce the same key. */
function filterSignature(params: LibraryHomeSearchParams): string {
  return Object.entries(params)
    .filter(([k, v]) => k !== "library" && k !== "q" && v !== undefined)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([k, v]) => `${k}=${v}`)
    .join("&");
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
      <div className="mb-6 flex flex-wrap items-baseline justify-between gap-4">
        <div>
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
        <LibrarySearch initial={query} basePath="/" />
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
