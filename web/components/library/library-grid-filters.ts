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
  /** Per-user read state — CSV subset of `unread,in_progress,read`
   *  (series mode only). */
  readStatus?: string[];
  yearFrom?: string;
  yearTo?: string;
  publishers?: string[];
  languages?: string[];
  ageRatings?: string[];
  genres?: string[];
  tags?: string[];
  credits?: Partial<Record<CreditKey, string[]>>;
  /** Any-role credit filter (the `?credits=<name>` query param). Matches
   *  series where the named person holds *any* credit role — used by
   *  the people-search click-through so creators with mixed roles
   *  (writer + cover artist + …) surface every series they touched
   *  rather than only the intersection of their roles. */
  anyCredits?: string[];
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
    readStatus: csv("read_status"),
    yearFrom: raw.year_from || undefined,
    yearTo: raw.year_to || undefined,
    publishers: csv("publisher"),
    languages: csv("language"),
    ageRatings: csv("age_rating"),
    genres: csv("genres"),
    tags: csv("tags"),
    credits: Object.keys(credits).length ? credits : undefined,
    anyCredits: csv("credits"),
    characters: csv("characters"),
    teams: csv("teams"),
    locations: csv("locations"),
    ratingRange,
  };
  // Compact: return undefined when nothing was set, so the caller can
  // skip both the prop and the remount key.
  return Object.values(out).some((v) => v !== undefined) ? out : undefined;
}

/** The serializable slice of grid state. `library` is the entry param
 *  (preserved verbatim); the rest mirror `parseLibraryGridFilters`'s
 *  inputs so `parse(serialize(x))` round-trips. Sort/order/mode that are
 *  per-user rather than per-link live in localStorage, not here — except
 *  `mode`, which is cheap to share and the parser already reads. */
export type LibraryGridUrlState = {
  library: string;
  mode: LibraryGridMode;
  status?: string;
  readStatus: string[];
  yearFrom?: string;
  yearTo?: string;
  publishers: string[];
  languages: string[];
  ageRatings: string[];
  genres: string[];
  tags: string[];
  credits: CreditState;
  anyCredits: string[];
  characters: string[];
  teams: string[];
  locations: string[];
  ratingRange: [number, number] | null;
};

/** Serialize grid state into a query string (no leading `?`), the
 *  inverse of {@link parseLibraryGridFilters}. Default/empty values are
 *  omitted so the URL stays short and a pristine grid produces just
 *  `library=…`. Keys are emitted in a stable order for deterministic
 *  URLs (so an unchanged grid never rewrites history). */
export function serializeLibraryGridFilters(
  state: LibraryGridUrlState,
): string {
  const sp = new URLSearchParams();
  sp.set("library", state.library);
  // Only emit `mode` when it differs from the default (series) — keeps
  // the common case URL clean. In-grid search (`q`) is intentionally
  // NOT serialized: `?q=` on `/` routes to the dedicated SearchView,
  // so the grid's toolbar search stays local state.
  if (state.mode === "issues") sp.set("mode", "issues");
  if (state.status && state.status !== "any") sp.set("status", state.status);
  if (state.readStatus.length)
    sp.set("read_status", state.readStatus.join(","));
  if (state.yearFrom?.trim()) sp.set("year_from", state.yearFrom.trim());
  if (state.yearTo?.trim()) sp.set("year_to", state.yearTo.trim());
  const csv = (key: string, values: string[]) => {
    if (values.length) sp.set(key, values.join(","));
  };
  csv("publisher", state.publishers);
  csv("language", state.languages);
  csv("age_rating", state.ageRatings);
  csv("genres", state.genres);
  csv("tags", state.tags);
  for (const c of CREDIT_ROLES) csv(c.key, state.credits[c.key]);
  csv("credits", state.anyCredits);
  csv("characters", state.characters);
  csv("teams", state.teams);
  csv("locations", state.locations);
  if (state.ratingRange) {
    sp.set("user_rating_min", String(state.ratingRange[0]));
    sp.set("user_rating_max", String(state.ratingRange[1]));
  }
  return sp.toString();
}
