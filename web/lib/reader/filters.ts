/**
 * Reader colour filters (audit 2026-11 §5 item 22, remediation F6).
 *
 * Six viewing-comfort adjustments compose into the one CSS `filter` the
 * reader puts on its page surface: brightness and sepia (the original
 * two), plus grayscale, invert ("night"), warmth and contrast. The store
 * owns the values; this module owns their ranges, clamping and the
 * composition, so the slider bounds, the persisted-value guard and the
 * rendered string can never disagree.
 *
 * Order matters — CSS applies filter functions left to right:
 *
 *   1. `invert(1)` first, so brightness still brightens and a warm tint
 *      still reads warm on the inverted page (tinting after inverting
 *      would turn warm into blue);
 *   2. `brightness` and `contrast` on the (possibly inverted) luminance;
 *   3. `grayscale`, then `sepia` — sepia on a desaturated page is the
 *      classic paper tone;
 *   4. warmth last. Warm is a partial `sepia()`; cool is the same sepia
 *      applied between two `invert(1)`s, which tints toward blue while
 *      leaving every other hue where it was (unlike `hue-rotate`, which
 *      would turn reds green).
 *
 * Only non-default parts are emitted, and `undefined` means "no filter"
 * so the style attribute is dropped entirely at the defaults.
 */

export type ReaderFilters = {
  /** CSS `brightness()` multiplier. 1 = unchanged. */
  brightness: number;
  /** CSS `sepia()` amount in [0, 1]. */
  sepia: number;
  /** CSS `grayscale()` percent, 0–100. */
  grayscale: number;
  /** Night mode: colour-inverted page surface. */
  invert: boolean;
  /** Colour temperature, −50 (cool) … 50 (warm); 0 = neutral. */
  warmth: number;
  /** CSS `contrast()` percent, 50–150. 100 = unchanged. */
  contrast: number;
};

/** The custom property the reader sets on its view wrapper to the composed
 *  filter (absent at the defaults). The page `<img>`s read it through
 *  {@link READER_FILTER_VAR}, so the filter lands on the page bitmap only —
 *  marker overlays and the webtoon footer keep their own colours, and no
 *  wrapper becomes a containing block for `position: fixed` children. */
export const READER_FILTER_PROPERTY = "--reader-filter";
/** `var()` with an empty fallback: unset, the declaration is invalid at
 *  computed-value time and `filter` falls back to `none`; combined with
 *  another function (`… blur(24px)`) the empty fallback leaves that
 *  function alone. */
export const READER_FILTER_VAR = `var(${READER_FILTER_PROPERTY},)`;

export type FilterRange = { min: number; max: number; step: number };

/** Slider ranges. The server's PATCH validation and DB CHECK constraints
 *  pin the same bounds (`crates/server/src/auth/local.rs`). */
export const FILTER_RANGES = {
  brightness: { min: 0.5, max: 1.5, step: 0.05 },
  sepia: { min: 0, max: 1, step: 0.05 },
  grayscale: { min: 0, max: 100, step: 5 },
  warmth: { min: -50, max: 50, step: 5 },
  contrast: { min: 50, max: 150, step: 5 },
} as const satisfies Record<
  Exclude<keyof ReaderFilters, "invert">,
  FilterRange
>;

export const DEFAULT_FILTERS: ReaderFilters = {
  brightness: 1,
  sepia: 0,
  grayscale: 0,
  invert: false,
  warmth: 0,
  contrast: 100,
};

const clampTo = (r: FilterRange, v: number) =>
  Math.max(r.min, Math.min(r.max, v));

/** Three decimals is finer than any slider step and keeps a value such
 *  as `0.1 + 0.2` from rendering as `0.30000000000000004`. */
const round3 = (v: number) => Math.round(v * 1000) / 1000;

export const clampBrightness = (v: number) =>
  round3(clampTo(FILTER_RANGES.brightness, v));
export const clampSepia = (v: number) =>
  round3(clampTo(FILTER_RANGES.sepia, v));
export const clampGrayscale = (v: number) =>
  Math.round(clampTo(FILTER_RANGES.grayscale, v));
export const clampWarmth = (v: number) =>
  Math.round(clampTo(FILTER_RANGES.warmth, v));
export const clampContrast = (v: number) =>
  Math.round(clampTo(FILTER_RANGES.contrast, v));

/** Every value at its default — the "Reset filters" control hides. */
export function isDefaultFilters(f: ReaderFilters): boolean {
  return (
    f.brightness === DEFAULT_FILTERS.brightness &&
    f.sepia === DEFAULT_FILTERS.sepia &&
    f.grayscale === DEFAULT_FILTERS.grayscale &&
    f.invert === DEFAULT_FILTERS.invert &&
    f.warmth === DEFAULT_FILTERS.warmth &&
    f.contrast === DEFAULT_FILTERS.contrast
  );
}

/** The composed CSS `filter` value, or `undefined` at the defaults. */
export function composeFilter(f: ReaderFilters): string | undefined {
  const parts: string[] = [];
  if (f.invert) parts.push("invert(1)");
  if (f.brightness !== 1) parts.push(`brightness(${f.brightness})`);
  if (f.contrast !== 100) parts.push(`contrast(${f.contrast / 100})`);
  if (f.grayscale !== 0) parts.push(`grayscale(${f.grayscale / 100})`);
  if (f.sepia !== 0) parts.push(`sepia(${f.sepia})`);
  if (f.warmth > 0) parts.push(`sepia(${f.warmth / 100})`);
  else if (f.warmth < 0)
    parts.push(`invert(1) sepia(${-f.warmth / 100}) invert(1)`);
  return parts.length > 0 ? parts.join(" ") : undefined;
}
