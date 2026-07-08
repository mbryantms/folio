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
  return `/issues/${encodeURIComponent(issueId)}/pages/0/thumb`;
}

/**
 * Build a responsive `srcset` for a cover image (audit G9). Only the cover
 * **thumb route** (`…/pages/0/thumb`) has a small (`@sm`, 300px) variant —
 * provider cover URLs (`…/covers/{id}`) and anything else return `null`, so
 * the caller serves a plain `src`. The thumb route degrades gracefully:
 * `?variant=cover_small` falls back to the full 600px cover until the `@sm`
 * exists, so this never 404s. Pair with `sizes="auto"` (lazy imgs) for
 * per-card sizing, or the browser defaults to the 600px step.
 */
export function coverThumbSrcSet(src: string): string | null {
  if (!/\/pages\/0\/thumb(?:\?|$)/.test(src)) return null;
  const sep = src.includes("?") ? "&" : "?";
  return `${src}${sep}variant=cover_small 300w, ${src} 600w`;
}

/** Strip thumbnail URL for a specific page. */
export function pageThumbUrl(issueId: string, page: number): string {
  return `/issues/${encodeURIComponent(issueId)}/pages/${page}/thumb`;
}

/**
 * Append an archive-content version stamp (`v`) to a page/thumb URL.
 *
 * Page and thumbnail URLs are stable across archive rewrites, so the
 * browser's cache key never changes when the underlying bytes do. The
 * server now serves these revalidatable, but entries cached under the
 * old `immutable` policy are pinned for a year and never revalidate —
 * the only way past them is a different URL. Callers pass the issue's
 * `last_rewrite_at` (null until the archive is first rewritten, so
 * untouched issues keep their historical URLs and warm caches).
 */
export function withContentVersion(
  url: string,
  version: string | null | undefined,
): string {
  if (!version) return url;
  const sep = url.includes("?") ? "&" : "?";
  return `${url}${sep}v=${encodeURIComponent(version)}`;
}

/** Full-resolution page bytes URL. */
export function pageBytesUrl(issueId: string, page: number): string {
  return `/issues/${encodeURIComponent(issueId)}/pages/${page}`;
}

/** Width ladder for reader page variants (audit FEP-1). Mirrors
 *  `page_variants::TIERS` on the server — change both together. */
export const PAGE_VARIANT_TIERS = [480, 720, 1080, 1600] as const;

/** `?w=` variant of a page-bytes URL. */
export function pageVariantUrl(src: string, w: number): string {
  return `${src}${src.includes("?") ? "&" : "?"}w=${w}`;
}

/** The tier a `srcSet`+`sizes` browser pick resolves to for a given
 *  device-pixel target, or `null` when the target wants full-res (bigger
 *  than every useful tier, or at/above the intrinsic width). The reader
 *  prefetcher uses this so warmed URLs match the rendered `<img>`'s pick
 *  byte-for-byte — the decode-and-retain cache is keyed by exact URL. */
export function selectPageVariantTier(
  targetDevicePx: number,
  intrinsicWidth?: number | null,
): number | null {
  for (const t of PAGE_VARIANT_TIERS) {
    if (t >= targetDevicePx) {
      return intrinsicWidth != null && t >= intrinsicWidth ? null : t;
    }
  }
  return null;
}

/** `srcSet` for a reader page: one entry per tier below the intrinsic
 *  width plus the full-res URL anchored at the intrinsic width, so the
 *  browser can pick original pixels when the slot genuinely needs them.
 *  `undefined` when the page is already smaller than the smallest tier
 *  (or has no server-known width — no safe descriptor to anchor). */
export function pageBytesSrcSet(
  src: string,
  intrinsicWidth?: number | null,
): string | undefined {
  if (intrinsicWidth == null || intrinsicWidth <= PAGE_VARIANT_TIERS[0]) {
    return undefined;
  }
  const parts = PAGE_VARIANT_TIERS.filter((t) => t < intrinsicWidth).map(
    (t) => `${pageVariantUrl(src, t)} ${t}w`,
  );
  parts.push(`${src} ${intrinsicWidth}w`);
  return parts.join(", ");
}

// ───── Helper exports for caller convenience ─────

/** Type guard: most call sites pass a `Pick<>` to keep coupling loose. */
export type LibraryRef = Pick<LibraryView, "slug">;
export type SeriesRef = Pick<SeriesView, "slug">;
export type IssueRef = Pick<IssueSummaryView, "slug" | "series_slug">;
export type IssueDetailRef = Pick<IssueDetailView, "slug" | "series_slug">;
