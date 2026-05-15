/**
 * URL builders for every entity in the app — single source of truth so
 * route shape changes only require touching this file.
 *
 * Locale lives on the `NEXT_LOCALE` cookie (post-Human-URLs M3), not the
 * path, so none of these functions take a locale argument.
 *
 * Keep this in lockstep with the server-side route registrations under
 * `crates/server/src/api/`.
 */

import type {
  IssueDetailView,
  IssueSummaryView,
  LibraryView,
  SeriesView,
} from "@/lib/api/types";

// ───── Library ─────

export function libraryUrl(lib: { slug: string } | string): string {
  const slug = typeof lib === "string" ? lib : lib.slug;
  return `/libraries/${encodeURIComponent(slug)}`;
}

export function adminLibraryUrl(
  lib: { slug: string } | string,
  tab?: "settings" | "health" | "history" | "removed" | "scan" | "thumbnails",
): string {
  const slug = typeof lib === "string" ? lib : lib.slug;
  const base = `/admin/libraries/${encodeURIComponent(slug)}`;
  return tab ? `${base}/${tab}` : base;
}

// ───── Series ─────

export function seriesUrl(s: { slug: string } | string): string {
  const slug = typeof s === "string" ? s : s.slug;
  return `/series/${encodeURIComponent(slug)}`;
}

export function adminSeriesUrl(s: { slug: string } | string): string {
  const slug = typeof s === "string" ? s : s.slug;
  return `/admin/series/${encodeURIComponent(slug)}`;
}

// ───── Issue ─────

/**
 * Optional URL options that thread reading-context through to the
 * reader / issue detail. `cbl` is the saved-view id (kind=`cbl`) the
 * user is reading through — the reader uses it to pick "next" out of
 * the list instead of the parent series. Omit when the user is not in
 * a CBL context.
 */
export type IssueUrlOpts = { cbl?: string | null };

function appendOpts(base: string, opts?: IssueUrlOpts): string {
  if (!opts?.cbl) return base;
  const sep = base.includes("?") ? "&" : "?";
  return `${base}${sep}cbl=${encodeURIComponent(opts.cbl)}`;
}

/**
 * Issue detail URL, nested under the parent series. Accepts either a full
 * `IssueDetailView` / `IssueSummaryView` (which carry `series_slug`), or
 * an explicit `(seriesSlug, issueSlug)` pair when the caller has them
 * separately (e.g., URL params).
 */
export function issueUrl(
  i: Pick<IssueSummaryView, "slug" | "series_slug">,
  opts?: IssueUrlOpts,
): string;
export function issueUrl(
  seriesSlug: string,
  issueSlug: string,
  opts?: IssueUrlOpts,
): string;
export function issueUrl(
  a: string | Pick<IssueSummaryView, "slug" | "series_slug">,
  b?: string | IssueUrlOpts,
  c?: IssueUrlOpts,
): string {
  if (typeof a === "string") {
    const base = `/series/${encodeURIComponent(a)}/issues/${encodeURIComponent(b as string)}`;
    return appendOpts(base, c);
  }
  const base = `/series/${encodeURIComponent(a.series_slug)}/issues/${encodeURIComponent(a.slug)}`;
  return appendOpts(base, b as IssueUrlOpts | undefined);
}

/** Reader URL for an issue. Same nesting as `issueUrl` but rooted at `/read`. */
export function readerUrl(
  i: Pick<IssueSummaryView, "slug" | "series_slug">,
  opts?: IssueUrlOpts,
): string;
export function readerUrl(
  seriesSlug: string,
  issueSlug: string,
  opts?: IssueUrlOpts,
): string;
export function readerUrl(
  a: string | Pick<IssueSummaryView, "slug" | "series_slug">,
  b?: string | IssueUrlOpts,
  c?: IssueUrlOpts,
): string {
  if (typeof a === "string") {
    const base = `/read/${encodeURIComponent(a)}/${encodeURIComponent(b as string)}`;
    return appendOpts(base, c);
  }
  const base = `/read/${encodeURIComponent(a.series_slug)}/${encodeURIComponent(a.slug)}`;
  return appendOpts(base, b as IssueUrlOpts | undefined);
}

// ───── Page bytes (still UUID — internal/signed) ─────
//
// Page-byte URLs intentionally keep the BLAKE3 issue id since (a) they're
// not user-facing, (b) OPDS-PSE signatures bind to the id (spec §8.3),
// and (c) the reader fetches bytes directly from the API after reading
// the slug-shaped page URL.

/** Cover thumbnail URL for an issue. */
export function coverThumbUrl(issueId: string): string {
  return `/api/issues/${encodeURIComponent(issueId)}/pages/0/thumb`;
}

/** Strip thumbnail URL for a specific page. */
export function pageThumbUrl(issueId: string, page: number): string {
  return `/api/issues/${encodeURIComponent(issueId)}/pages/${page}/thumb`;
}

/** Full-resolution page bytes URL. */
export function pageBytesUrl(issueId: string, page: number): string {
  return `/api/issues/${encodeURIComponent(issueId)}/pages/${page}`;
}

// ───── Helper exports for caller convenience ─────

/** Type guard: most call sites pass a `Pick<>` to keep coupling loose. */
export type LibraryRef = Pick<LibraryView, "slug">;
export type SeriesRef = Pick<SeriesView, "slug">;
export type IssueRef = Pick<IssueSummaryView, "slug" | "series_slug">;
export type IssueDetailRef = Pick<IssueDetailView, "slug" | "series_slug">;
