/**
 * Heuristics for picking the initial reader direction and view mode based on
 * what the publisher tells us (`Manga` flag, `DoublePage` per-page flag) plus
 * the page dimensions when present.
 *
 * These run only on first mount for a series with no per-series localStorage
 * choice — user toggles always win.
 */
import type { PageInfo } from "@/lib/api/types";

export type Direction = "ltr" | "rtl";
export type ViewMode = "single" | "double" | "webtoon";
/** `series.reading_direction` values. `ttb` (top-to-bottom, set from the
 *  series editor's "Vertical (webtoon)" option) is a *layout* signal, not a
 *  page-turn direction: it selects webtoon view (see
 *  {@link detectInitialViewMode}) and is "no opinion" for
 *  {@link detectDirection}. */
export type SeriesDirection = Direction | "ttb";

const WEBTOON_ASPECT_THRESHOLD = 2.5; // h/w
const SPREAD_ASPECT_THRESHOLD = 1.2; // w/h
const DOUBLE_PAGE_FLAG_RATIO = 0.1;

/**
 * Detect reading direction. Manga marked `YesAndRightToLeft` always
 * wins. Falls back through the chain documented in
 * `~/.claude/plans/manga-and-bulk-metadata-1.0.md`:
 *
 *   1. ComicInfo `<Manga>` on the issue (highest — author intent)
 *   2. `series.reading_direction` (M2 — admin / scanner-heuristic set)
 *   3. user's `default_reading_direction`
 *   4. library's `default_reading_direction` (M1 — newly consulted)
 *   5. `ltr` (final default)
 *
 * Each layer accepts `null` / `undefined` meaning "no opinion, defer
 * to the next layer." Non-recognized strings ("auto", etc.) and the
 * series-level `ttb` (a layout choice — it picks webtoon view instead,
 * see {@link detectInitialViewMode}) are treated as "no opinion" at
 * this layer — the next-most-specific signal wins.
 */
export function detectDirection(
  manga: string | null | undefined,
  userDefault: Direction | null | undefined,
  libraryDefault?: Direction | null | undefined,
  seriesOverride?: SeriesDirection | null | undefined,
): Direction {
  if (manga === "YesAndRightToLeft") return "rtl";
  if (seriesOverride === "ltr" || seriesOverride === "rtl")
    return seriesOverride;
  if (userDefault === "ltr" || userDefault === "rtl") return userDefault;
  if (libraryDefault === "ltr" || libraryDefault === "rtl")
    return libraryDefault;
  return "ltr";
}

/**
 * Detect view mode from per-page metadata.
 *
 * - `webtoon` when median page aspect (h/w) ≥ 2.5 — strong indicator of
 *   tall/strip content like webcomics or vertical scrolls.
 * - `double` when ≥ 10% of pages carry the `DoublePage` flag, OR when median
 *   aspect indicates landscape spreads (w/h > 1.2 — typical for two-page
 *   publishing layouts).
 * - `single` otherwise.
 *
 * Pages without dimensions don't contribute to the median; if no page has
 * dimensions, falls back to `single` unless the `DoublePage` flag tips it.
 */
export function detectViewMode(pages: PageInfo[]): ViewMode {
  if (pages.length === 0) return "single";

  const sized = pages.filter(
    (p): p is PageInfo & { image_width: number; image_height: number } =>
      typeof p.image_width === "number" &&
      typeof p.image_height === "number" &&
      p.image_width > 0 &&
      p.image_height > 0,
  );

  // Aspect = height / width (so taller-than-wide > 1)
  const heightOverWidth = sized
    .map((p) => p.image_height / p.image_width)
    .sort((a, b) => a - b);
  const medianHW =
    heightOverWidth[Math.floor(heightOverWidth.length / 2)] ?? null;

  if (medianHW !== null && medianHW >= WEBTOON_ASPECT_THRESHOLD) {
    return "webtoon";
  }

  const flagged = pages.filter((p) => p.double_page === true).length;
  if (pages.length > 0 && flagged / pages.length >= DOUBLE_PAGE_FLAG_RATIO) {
    return "double";
  }

  if (medianHW !== null && 1 / medianHW > SPREAD_ASPECT_THRESHOLD) {
    return "double";
  }

  return "single";
}

/**
 * Initial view mode for an issue with no per-series localStorage choice
 * (which `store.init` still lets win over this). Resolution order:
 *
 *   1. `series.reading_direction === "ttb"` → `webtoon` (admin intent for
 *      this series — beats the user's global default, audit UX-5)
 *   2. the user's `default_view_mode`
 *   3. {@link detectViewMode} from page metadata
 */
export function detectInitialViewMode(
  pages: PageInfo[],
  userDefault: ViewMode | null | undefined,
  seriesDirection?: SeriesDirection | null | undefined,
): ViewMode {
  if (seriesDirection === "ttb") return "webtoon";
  if (
    userDefault === "single" ||
    userDefault === "double" ||
    userDefault === "webtoon"
  ) {
    return userDefault;
  }
  return detectViewMode(pages);
}
