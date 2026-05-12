import { SEARCH_CATEGORIES, type SearchCategory } from "@/lib/search/types";

import { SearchView } from "./SearchView";

/**
 * Dedicated full-results search page. Linked from the global search modal
 * footer (or `Mod+Enter` on the modal). Reads the initial query from the
 * URL so a deep-link like `/search?q=geiger` lands in a populated state.
 *
 * Default view renders one horizontal-scroll rail per category, mirroring
 * the home page's saved-view rails. The `?category=` query param flips
 * the page to a full-grid view of just that category — the destination
 * of each rail's "View all" trailing tile.
 *
 * The actual rendering is a client component because (a) we want
 * keystroke-live results and (b) the layout already enforces auth.
 */
export default async function SearchPage({
  searchParams,
}: {
  searchParams: Promise<{ q?: string; category?: string }>;
}) {
  const { q, category } = await searchParams;
  const validCategory = isSearchCategory(category) ? category : null;
  // `key` makes SearchView remount whenever the URL query or category
  // changes (e.g. the user clicks a rail's "View all" → `category=series`).
  // Without it the client component reuses its previous `raw`/`debounced`
  // state and the old results stick around. Typing inside SearchView
  // itself uses `history.replaceState`, which bypasses Next's router and
  // doesn't re-key — so we don't pay for a remount on each keystroke.
  const key = `${q ?? ""}|${validCategory ?? ""}`;
  return (
    <SearchView key={key} initialQuery={q ?? ""} category={validCategory} />
  );
}

function isSearchCategory(v: string | undefined): v is SearchCategory {
  if (!v) return false;
  return SEARCH_CATEGORIES.some((c) => c.key === v);
}
