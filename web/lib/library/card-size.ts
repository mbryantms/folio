/**
 * Card sizes as CSS variables, so a cover grid or rail is the reader's
 * size from the first frame — including the route skeletons, which are
 * server components with no access to the preference.
 *
 * The sizes stay where they have always lived (`localStorage`, one key
 * per surface, written by `useCardSize`). `syncCardSizeVars` mirrors every
 * stored `folio.*.cardSize*` key onto `<html>` as `--card-size-<key>`:
 * the root layout runs it as an inline script before first paint, and
 * `useCardSize` re-runs it after every change. Grids and skeletons size
 * through `cardGridColumns` / `cardWidth`, which read the variable and
 * fall back to the default (or the component's own state) when the
 * reader never moved the slider.
 *
 * Custom pages key their size by page id, but `/pages/<slug>` only knows
 * the slug while it loads. `PageRails` records `folio.page.slugId.<slug>`
 * → id, and the sync also publishes that page's size under the slug
 * (`CARD_SIZE_KEYS.pageSlug`).
 */

/** Every surface's storage key. */
export const CARD_SIZE_KEYS = {
  library: "folio.libraryGrid.cardSize",
  series: "folio.series.cardSize",
  savedView: "folio.savedView.cardSize",
  systemView: "folio.systemView.cardSize",
  search: "folio.search.cardSize",
  bookmarks: "folio.bookmarks.cardSize",
  home: "folio.home.cardSize",
  page: (pageId: string) => `folio.page.cardSize.${pageId}`,
  /** Variable-only alias (never stored): a custom page's size by slug. */
  pageSlug: (slug: string) => `folio.page.cardSize.slug.${slug}`,
} as const;

/** `folio.page.slugId.<slug>` → page id, written by `PageRails`. */
export const PAGE_SLUG_ID_PREFIX = "folio.page.slugId.";

/** The default card width every slider starts at. */
export const DEFAULT_CARD_SIZE_PX = 160;

/** `folio.series.cardSize` → `--card-size-series-cardSize`. Keep in sync
 *  with `varName` inside `syncCardSizeVars` (a test pins them together). */
export function cardSizeVarName(storageKey: string): string {
  return (
    "--card-size-" +
    storageKey.replace(/^folio\./, "").replace(/[^A-Za-z0-9_-]/g, "-")
  );
}

/** A card's width: the reader's size for `storageKey`, else `fallbackPx`
 *  (always `fallbackPx` without a key). */
export function cardWidth(
  storageKey: string | undefined,
  fallbackPx: number = DEFAULT_CARD_SIZE_PX,
): string {
  return storageKey
    ? `var(${cardSizeVarName(storageKey)}, ${fallbackPx}px)`
    : `${fallbackPx}px`;
}

/** The cover-grid template every card-size surface shares: as many
 *  columns as fit at the reader's size, never fewer than two
 *  (docs/dev/phone-layout.md), stretched to fill the row. */
export function cardGridColumns(
  storageKey: string | undefined,
  fallbackPx: number = DEFAULT_CARD_SIZE_PX,
): string {
  return `repeat(auto-fill, minmax(min(${cardWidth(storageKey, fallbackPx)}, calc(50% - 0.5rem)), 1fr))`;
}

/**
 * Mirror stored card sizes onto `<html>` as CSS variables, and drop the
 * variables of keys that are gone. Serialized into the pre-paint inline
 * script, so it must stay self-contained: no imports, no
 * closures over module scope, no syntax a compiler would rewrite into a
 * helper call.
 */
export function syncCardSizeVars(): void {
  try {
    const ls = window.localStorage;
    const style = document.documentElement.style;
    const seen: Record<string, 1> = {};
    const varName = function (k: string) {
      return (
        "--card-size-" +
        k.replace(/^folio\./, "").replace(/[^A-Za-z0-9_-]/g, "-")
      );
    };
    const put = function (k: string, raw: string | null) {
      const n = Number(raw);
      // Sliders store 120–280; anything wildly outside is junk.
      if (raw === null || raw === "" || !isFinite(n) || n < 80 || n > 480)
        return;
      const name = varName(k);
      style.setProperty(name, n + "px");
      seen[name] = 1;
    };
    for (let i = 0; i < ls.length; i++) {
      const k = ls.key(i);
      if (!k || k.indexOf("folio.") !== 0) continue;
      if (/\.cardSize(\.|$)/.test(k)) put(k, ls.getItem(k));
      if (k.indexOf("folio.page.slugId.") === 0) {
        const id = ls.getItem(k);
        if (id)
          put(
            "folio.page.cardSize.slug." + k.slice(18),
            ls.getItem("folio.page.cardSize." + id),
          );
      }
    }
    for (let j = style.length - 1; j >= 0; j--) {
      const p = style[j];
      if (p && p.indexOf("--card-size-") === 0 && !seen[p])
        style.removeProperty(p);
    }
  } catch {
    // Storage blocked (private mode): grids use their defaults.
  }
}

/** Inline-script body for the root layout's `<head>`. */
export const CARD_SIZE_BOOTSTRAP = `(${syncCardSizeVars.toString()})()`;
