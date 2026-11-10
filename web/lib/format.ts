/**
 * Display formatters used across the library / series / issue pages.
 *
 * Reading time is estimated from `page_count` at a fixed seconds-per-page
 * baseline (24 s ≈ a comfortable read). The constant is exported so callers
 * can override it in tests or future user-pref work without re-deriving the
 * formatter logic.
 */

export const SECONDS_PER_PAGE = 24;

export function formatReadingTime(
  pageCount: number | null | undefined,
  secondsPerPage = SECONDS_PER_PAGE,
): string | null {
  if (!pageCount || pageCount <= 0) return null;
  const totalSec = pageCount * secondsPerPage;
  const totalMin = Math.round(totalSec / 60);
  if (totalMin < 1) return "<1 min";
  if (totalMin < 60) return `${totalMin} min`;
  const hours = Math.floor(totalMin / 60);
  const mins = totalMin - hours * 60;
  if (mins === 0) return `${hours} hr`;
  return `${hours} hr ${mins} min`;
}

/**
 * Compact reading-time formatter for stat-card values. Uses single-letter
 * unit suffixes ("h"/"m") so the value fits on one line in a narrow card,
 * and prepends a tilde so the figure reads as an estimate (matches the
 * "~24h 56m" wording on the series page).
 */
export function formatReadingTimeCompact(
  pageCount: number | null | undefined,
  secondsPerPage = SECONDS_PER_PAGE,
): string | null {
  if (!pageCount || pageCount <= 0) return null;
  const totalSec = pageCount * secondsPerPage;
  const totalMin = Math.round(totalSec / 60);
  if (totalMin < 1) return "~<1m";
  if (totalMin < 60) return `~${totalMin}m`;
  const hours = Math.floor(totalMin / 60);
  const mins = totalMin - hours * 60;
  if (mins === 0) return `~${hours}h`;
  return `~${hours}h ${mins}m`;
}

/**
 * Compact thousands separator for page counts. 4,200 → "4.2K". Used by
 * stat cards where horizontal space is tight enough that a four- or
 * five-digit number breaks layout on long-running series.
 */
export function formatCompactPages(n: number | null | undefined): string {
  if (n == null) return "—";
  if (n < 1000) return n.toString();
  if (n < 10_000) return `${(n / 1000).toFixed(1)}K`;
  return `${Math.round(n / 1000)}K`;
}

/**
 * Render an absolute timestamp as a relative phrase like "3 hours ago" or
 * "yesterday". For dates older than a year we fall back to the locale date.
 * Pure: doesn't care about the user's preferred locale beyond `toLocaleDateString`.
 */
export function formatRelativeDate(
  iso: string | null | undefined,
  now: Date = new Date(),
): string | null {
  if (!iso) return null;
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return null;
  const diffMs = now.getTime() - d.getTime();
  const diffSec = Math.round(diffMs / 1000);
  if (diffSec < 60) return "just now";
  const diffMin = Math.round(diffSec / 60);
  if (diffMin < 60) return `${diffMin} min ago`;
  const diffHr = Math.round(diffMin / 60);
  if (diffHr < 24) return `${diffHr} hr ago`;
  const diffDay = Math.round(diffHr / 24);
  if (diffDay === 1) return "yesterday";
  if (diffDay < 7) return `${diffDay} days ago`;
  if (diffDay < 30) return `${Math.round(diffDay / 7)} wk ago`;
  if (diffDay < 365) return `${Math.round(diffDay / 30)} mo ago`;
  return d.toLocaleDateString();
}

/** Format a `(year, month?, day?)` tuple from ComicInfo into "MMMM YYYY" or
 *  "DD MMM YYYY". Returns null if `year` is missing. */
export function formatPublicationDate(
  year: number | null | undefined,
  month?: number | null,
  day?: number | null,
): string | null {
  if (!year) return null;
  if (month && day) {
    const d = new Date(year, month - 1, day);
    return d.toLocaleDateString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  }
  if (month) {
    const d = new Date(year, month - 1, 1);
    return d.toLocaleDateString(undefined, {
      year: "numeric",
      month: "long",
    });
  }
  return String(year);
}

/** Format an integer page count as "12 pages" / "1 page". */
export function formatPageCount(
  pageCount: number | null | undefined,
): string | null {
  if (pageCount == null || pageCount < 0) return null;
  return pageCount === 1 ? "1 page" : `${pageCount} pages`;
}

/** Convert "active" | "completed" | "ongoing" | "hiatus" | "cancelled" etc.
 *  into a friendly title-cased label. Falls back to the raw string. */
export function formatPublicationStatus(
  status: string | null | undefined,
): string | null {
  if (!status) return null;
  const normalized = status.replace(/[_-]+/g, " ").toLowerCase();
  return normalized.replace(/\b\w/g, (c) => c.toUpperCase());
}

/** Fields `issueLabel` reads. Every issue-shaped view (summary, detail,
 *  rail card, next-up target, offline record) carries these. */
export type IssueLabelSource = {
  title?: string | null;
  number?: string | null;
  series_name?: string | null;
};

/** The one display label for an issue — cards, toasts, dialogs, alt
 *  text, aria labels. Never repeats the number.
 *
 *  Rules:
 *    1. a real title → the title; with `withNumber`, `"<title> #<n>"`
 *       (for menus/dialogs that name the issue in one string)
 *    2. no title → `"<series> #<n>"` when the series name is known,
 *       else `"#<n>"`
 *    3. no title, no number → the series name, else `"Untitled"`
 *
 *  Pass `seriesName` when the caller knows it from a sibling field
 *  (e.g. `ContinueReadingCard.series_name`); otherwise it is read off
 *  `issue.series_name`, which only some endpoints populate.
 *
 *  Pass `numberLabel` when the surface has a richer number text than
 *  `"#<n>"` — a special's `"Annual #1"` — so the label never reads like
 *  the run's #1. It is used only for an issue that has a number. */
export function issueLabel(
  issue: IssueLabelSource,
  opts: {
    seriesName?: string | null;
    withNumber?: boolean;
    numberLabel?: string | null;
  } = {},
): string {
  const title = issue.title?.trim();
  const number = issue.number?.trim();
  const numberText = number
    ? opts.numberLabel?.trim() || `#${number}`
    : undefined;
  const series = opts.seriesName?.trim() || issue.series_name?.trim() || null;
  if (title) {
    return opts.withNumber && numberText ? `${title} ${numberText}` : title;
  }
  if (numberText) return series ? `${series} ${numberText}` : numberText;
  return series ?? "Untitled";
}

/** The title line for a card that already prints the issue number as its
 *  own element: `issueLabel`, or `null` when that label would only be the
 *  bare `"#<n>"` the card already shows (an untitled issue on a surface
 *  that doesn't know the series name). */
export function issueCardTitle(
  issue: IssueLabelSource,
  seriesName?: string | null,
): string | null {
  const label = issueLabel(issue, { seriesName });
  const number = issue.number?.trim();
  const titled = !!issue.title?.trim();
  return !titled && number && label === `#${number}` ? null : label;
}

/** Human byte size: `0 B`, `512 KB`, `1.4 MB`, `2.1 GB` (decimal units,
 *  matching what browsers and OS storage settings report). */
export function formatBytes(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n) || n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = n;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  return `${value.toFixed(digits)} ${units[unit]}`;
}
