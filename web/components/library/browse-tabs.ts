/** Server-safe (no `"use client"`) tab list + `?tab=` parser for
 *  `/browse` (WP-5.5). The page is a server component and parses
 *  `searchParams` itself; a function exported from the `"use client"`
 *  `BrowseTabs` module can't be called from the server, so it lives here
 *  (same split as `library-grid-filters`). */
export const BROWSE_TABS = [
  "characters",
  "teams",
  "arcs",
  "publishers",
  "creators",
] as const;
export type BrowseTab = (typeof BROWSE_TABS)[number];

export function parseBrowseTab(raw: string | undefined | null): BrowseTab {
  return (BROWSE_TABS as readonly string[]).includes(raw ?? "")
    ? (raw as BrowseTab)
    : "characters";
}
