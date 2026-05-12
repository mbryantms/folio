/** Server-safe (no `"use client"`) types + URL parser for the library
 *  grid's filter state. Lives in its own module so the home page's
 *  server component can call `parseLibraryGridFilters` to compute the
 *  `initialFilters` prop — invoking a client-component export from a
 *  server component is a Next.js runtime error. */

/** Credit-role facets. Keys match the `/series` query params (plural)
 *  and the corresponding state fields in the grid; `role` is the
 *  singular value passed to the credits-options endpoint. The
 *  registry lives here so both the parser and the client component
 *  can iterate the same list. */
export const CREDIT_ROLES = [
  { key: "writers" as const, label: "Writers", role: "writer" },
  { key: "pencillers" as const, label: "Pencillers", role: "penciller" },
  { key: "inkers" as const, label: "Inkers", role: "inker" },
  { key: "colorists" as const, label: "Colorists", role: "colorist" },
  { key: "letterers" as const, label: "Letterers", role: "letterer" },
  {
    key: "cover_artists" as const,
    label: "Cover artists",
    role: "cover_artist",
  },
  { key: "editors" as const, label: "Editors", role: "editor" },
  { key: "translators" as const, label: "Translators", role: "translator" },
];

export type CreditKey = (typeof CREDIT_ROLES)[number]["key"];
export type CreditState = Record<CreditKey, string[]>;

export const EMPTY_CREDITS: CreditState = Object.fromEntries(
  CREDIT_ROLES.map((c) => [c.key, [] as string[]]),
) as CreditState;

export const RATING_MIN = 0;
export const RATING_MAX = 5;
export const RATING_STEP = 0.5;

/** Library grid mode — series-level (default) or issue-level. URL
 *  param `mode=issues` lands on the issues view; anything else falls
 *  through to series. */
export type LibraryGridMode = "series" | "issues";

/** URL-derived initial filter values. Page.tsx parses the request's
 *  query string into this shape and the grid seeds its local state
 *  from it on mount — so chip deep-links like
 *  `/?library=all&writers=Brian%20K.%20Vaughan` actually pre-apply.
 *  After mount the grid owns its state; URL is only the entry point. */
export type LibraryGridInitialFilters = {
  mode?: LibraryGridMode;
  status?: string;
  yearFrom?: string;
  yearTo?: string;
  publishers?: string[];
  languages?: string[];
  ageRatings?: string[];
  genres?: string[];
  tags?: string[];
  credits?: Partial<Record<CreditKey, string[]>>;
  characters?: string[];
  teams?: string[];
  locations?: string[];
  ratingRange?: [number, number];
};

/** Parse a `Record<string, string | undefined>` (the shape App
 *  Router's `searchParams` resolves to) into the grid's
 *  `initialFilters` prop. */
export function parseLibraryGridFilters(
  raw: Record<string, string | undefined>,
): LibraryGridInitialFilters | undefined {
  const csv = (key: string): string[] | undefined => {
    const v = raw[key];
    if (!v) return undefined;
    const parts = v
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    return parts.length ? parts : undefined;
  };
  const num = (key: string): number | undefined => {
    const v = raw[key];
    if (v == null || v === "") return undefined;
    const n = Number(v);
    return Number.isFinite(n) ? n : undefined;
  };
  const credits: Partial<Record<CreditKey, string[]>> = {};
  for (const c of CREDIT_ROLES) {
    const v = csv(c.key);
    if (v) credits[c.key] = v;
  }
  const min = num("user_rating_min");
  const max = num("user_rating_max");
  const ratingRange: [number, number] | undefined =
    min != null || max != null
      ? [min ?? RATING_MIN, max ?? RATING_MAX]
      : undefined;
  const mode: LibraryGridMode | undefined =
    raw.mode === "issues"
      ? "issues"
      : raw.mode === "series"
        ? "series"
        : undefined;
  const out: LibraryGridInitialFilters = {
    mode,
    status: raw.status && raw.status !== "any" ? raw.status : undefined,
    yearFrom: raw.year_from || undefined,
    yearTo: raw.year_to || undefined,
    publishers: csv("publisher"),
    languages: csv("language"),
    ageRatings: csv("age_rating"),
    genres: csv("genres"),
    tags: csv("tags"),
    credits: Object.keys(credits).length ? credits : undefined,
    characters: csv("characters"),
    teams: csv("teams"),
    locations: csv("locations"),
    ratingRange,
  };
  // Compact: return undefined when nothing was set, so the caller can
  // skip both the prop and the remount key.
  return Object.values(out).some((v) => v !== undefined) ? out : undefined;
}
