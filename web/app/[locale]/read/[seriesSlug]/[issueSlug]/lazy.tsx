"use client";

/**
 * Lazy (code-split) reader surfaces — WP-4.4, reader bundle budget.
 *
 * The reader's first-load JS is gated in CI (`scripts/check-bundle-size.mjs`,
 * numbers in `docs/dev/pwa-performance.md`). Only what paints page one
 * belongs in it: `Reader`, `PageImage`, the page views, gestures, the store.
 * Everything else is declared here as a `next/dynamic` component and is
 * fetched as its own chunk after hydration (or on first use).
 *
 * The pattern, for anything new that is not needed to paint page one:
 *
 *  1. Add a `preloadX = () => import("./X")` loader and an
 *     `X = dynamic(() => import("./X").then((m) => m.X), { ssr: false })`
 *     pair below. Keep the `import()` inline in the `dynamic()` call — the
 *     Next compiler reads it from there.
 *  2. Import `X` from `./lazy` instead of `./X` at the call site.
 *  3. Decide when it mounts:
 *     - **Always-mounted overlays that start off-screen** (chrome, page
 *       strip, marker overlay): render it unconditionally. With
 *       `ssr: false` the server and the hydration pass render the (null)
 *       fallback and the chunk loads right after hydration. Both the
 *       chrome and the strip already start `data-state="closed"` and slide
 *       in after the first frame, so nothing visible is lost.
 *     - **Open-on-demand surfaces** (end-of-issue card, settings, editor,
 *       sheets): mount on first open and keep mounted so exit animations
 *       still play; call `preloadX()` on intent (hover / focus /
 *       pointerdown, or "user is near the end") so the chunk is warm by
 *       the time it is needed. Keyboard shortcuts that open one flip the
 *       same state, so they keep working — the surface just appears once
 *       its chunk resolves.
 *  4. Give it a fallback only when an empty frame would be visible (e.g.
 *     inside an already-open popover); otherwise `null`.
 *
 * `ssr: false` is deliberate everywhere here: a `dynamic()` with SSR would
 * be server-rendered and then *required for hydration*, which puts its
 * chunk straight back into first-load JS.
 */

import dynamic from "next/dynamic";

export const preloadReaderChrome = () => import("./ReaderChrome");
/** Top bar. Starts closed (translated above the viewport) and slides in
 *  after the first frame, so it is not part of the first paint. */
export const ReaderChrome = dynamic(
  () => import("./ReaderChrome").then((m) => m.ReaderChrome),
  { ssr: false },
);

export const preloadPageStrip = () => import("./PageStrip");
/** Bottom thumbnail strip. Same closed-then-slide-in entrance as the
 *  chrome; hidden entirely unless the user has it enabled. */
export const PageStrip = dynamic(
  () => import("./PageStrip").then((m) => m.PageStrip),
  { ssr: false },
);

export const preloadMarkerOverlay = () => import("./MarkerOverlay");
/** Saved-marker rects/pins + the marker/OCR capture surface. Marker data
 *  arrives from a client query anyway, so nothing is lost by painting the
 *  page before this chunk lands. */
export const MarkerOverlay = dynamic(
  () => import("./MarkerOverlay").then((m) => m.MarkerOverlay),
  { ssr: false },
);

export const preloadReaderSettings = () => import("./ReaderSettings");
/** Settings popover body. Only mounted while the popover is open; the
 *  chrome preloads it on hover / focus / pointerdown of the gear. */
export const ReaderSettings = dynamic(
  () => import("./ReaderSettings").then((m) => m.ReaderSettings),
  {
    ssr: false,
    loading: () => (
      <div
        className="h-72 animate-pulse rounded-md bg-neutral-900/60"
        aria-busy="true"
        aria-label="Loading reader settings"
      />
    ),
  },
);

/** End-of-issue panel. Mounted once the reader nears the last page so the
 *  slide-in animation still plays when it opens. */
export const EndOfIssueCard = dynamic(
  () => import("./EndOfIssueCard").then((m) => m.EndOfIssueCard),
  { ssr: false },
);

/** "Up next" footer at the bottom of a webtoon scroll (cover, link,
 *  button). Only rendered in webtoon mode, far below the first page. */
export const WebtoonEndFooter = dynamic(
  () => import("./WebtoonEndFooter").then((m) => m.WebtoonEndFooter),
  { ssr: false },
);

/** Marker editor sheet (Sheet + form). Mounted on the first marker edit. */
export const MarkerEditor = dynamic(
  () => import("./MarkerEditor").then((m) => m.MarkerEditor),
  { ssr: false },
);

/** Active-marker-mode indicator + touch cancel (audit C7); only shown
 *  while a marker mode is active. */
export const MarkerModePill = dynamic(
  () => import("./MarkerModePill").then((m) => m.MarkerModePill),
  { ssr: false },
);

/** One-time reader orientation overlay (audit C5); only mounted for
 *  genuine first-run users. */
export const ReaderFirstRunOverlay = dynamic(
  () => import("./ReaderFirstRunOverlay").then((m) => m.ReaderFirstRunOverlay),
  { ssr: false },
);

/** Page-text panel (WP-4.8): OCR text of the visible page for screen
 *  readers. Mounted only once opened — it drives server OCR, so neither
 *  its bytes nor its requests touch a reader that never opens it. */
export const PageTextPanel = dynamic(
  () => import("./PageTextPanel").then((m) => m.PageTextPanel),
  { ssr: false },
);
