"use client";

import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { useRouter } from "next/navigation";
import { toast } from "sonner";
import { useReaderStore, type FitMode } from "@/lib/reader/store";
import {
  detectDirection,
  detectViewMode,
  type Direction,
  type ViewMode,
} from "@/lib/reader/detect";
import { resolveKeybinds } from "@/lib/reader/keybinds";
import {
  hasSeenReaderFirstRun,
  markReaderFirstRunSeen,
  readerFirstRunServerSnapshot,
  subscribeReaderFirstRun,
} from "@/lib/reader/first-run";
import { useReadingSession } from "@/lib/reader/session";
import {
  computeSpreadGroups,
  firstPageOfGroup,
  groupIndexForPage,
  isSpreadPage,
  type SpreadGroup,
} from "@/lib/reader/spreads";
import { useReaderProgressWrite } from "@/lib/reader/use-progress-write";
import { useReaderPrefetch } from "@/lib/reader/use-prefetch";
import { useReaderGestures } from "@/lib/reader/use-swipe";
import { useReaderKeymap } from "@/lib/reader/use-keymap";
import {
  DOUBLE_TAP_MS,
  DOUBLE_TAP_ZOOM,
  clampPan,
  nextZoomStep,
  zoomOriginPercent,
} from "@/lib/reader/zoom";
import { useIssueMarkers, useNextUp, usePrevUp } from "@/lib/api/queries";
import { readerUrl } from "@/lib/urls";
import { usePageMarkerToggle } from "@/lib/markers/use-page-marker-toggle";
import type { NextUpView, PageInfo } from "@/lib/api/types";
import {
  computeWebtoonWindow,
  placeholderAspectRatio,
} from "@/lib/reader/webtoon-window";
import { EndOfIssueCard } from "./EndOfIssueCard";
import { WebtoonEndFooter } from "./WebtoonEndFooter";
import {
  usePageTransition,
  type PageTransitionResult,
} from "@/lib/reader/use-page-transition";

import dynamic from "next/dynamic";

// Lazy-load the marker editor (Sheet + form + its deps) so its bytes
// leave the reader's first-load JS — it's only needed once the user
// actually starts a marker edit (audit 2.5 bundle ratchet). `ssr: false`
// because it's interactive client UI gated on a store flag.
const MarkerEditor = dynamic(
  () => import("./MarkerEditor").then((m) => m.MarkerEditor),
  { ssr: false },
);
// Active-marker-mode indicator + touch cancel (audit C7); lazy since it's
// only shown while a marker mode is active.
const MarkerModePill = dynamic(
  () => import("./MarkerModePill").then((m) => m.MarkerModePill),
  { ssr: false },
);
// One-time reader orientation overlay (audit C5); lazy + only mounted for
// genuine first-run users, so its bytes never touch the steady-state
// first-load JS budget.
const ReaderFirstRunOverlay = dynamic(
  () => import("./ReaderFirstRunOverlay").then((m) => m.ReaderFirstRunOverlay),
  { ssr: false },
);
import { MarkerOverlay } from "./MarkerOverlay";
import { PageStrip } from "./PageStrip";
import { PageImage } from "./PageImage";
import { ReaderChrome } from "./ReaderChrome";

export function Reader({
  issueId,
  seriesId,
  cblSavedViewId,
  exitUrl,
  totalPages,
  initialPage,
  pages,
  manga,
  userDefaultDirection,
  libraryDefaultDirection,
  seriesReadingDirection,
  userDefaultFitMode,
  userDefaultViewMode,
  userDefaultPageStrip,
  userDefaultPageAnimation,
  userDefaultCoverSolo,
  userKeybinds,
  activityTrackingEnabled,
  incognito = false,
  initialPeek = false,
  readingMinActiveMs,
  readingMinPages,
  readingIdleMs,
}: {
  issueId: string;
  seriesId: string | null;
  /** Saved-view id of the CBL the user is reading through. When set,
   *  the next-up resolver picks from that list instead of the parent
   *  series. Forwarded onto the next-issue URL when source === "cbl"
   *  so the CBL session persists across issues. */
  cblSavedViewId: string | null;
  /** URL to exit the reader to (the issue detail page). Computed by the
   * page wrapper from the slug params. */
  exitUrl: string;
  totalPages: number;
  initialPage: number;
  pages: PageInfo[];
  manga: string | null;
  userDefaultDirection: Direction | null;
  /** Parent library's `default_reading_direction`. Fallback in the
   *  resolution chain below `userDefaultDirection` but above LTR.
   *  See `manga-and-bulk-metadata-1.0` M1. */
  libraryDefaultDirection: Direction | null;
  /** Parent series' `reading_direction` override. Sits in the
   *  resolution chain between ComicInfo `<Manga>` and the user pref.
   *  See `manga-and-bulk-metadata-1.0` M2. */
  seriesReadingDirection: Direction | null;
  userDefaultFitMode: FitMode | null;
  userDefaultViewMode: ViewMode | null;
  userDefaultPageStrip: boolean;
  /** v0.3.44 / v0.3.45 — `'off' | 'slide' | 'fade' | null`. Null
   *  falls back to the built-in default of `'slide'` (fresh
   *  users start here). */
  userDefaultPageAnimation: "off" | "slide" | "fade" | null;
  userDefaultCoverSolo: boolean;
  userKeybinds: Record<string, string>;
  activityTrackingEnabled: boolean;
  /** When true, suppress per-page progress writes and the reading-session
   *  tracker for this read. Set by `?incognito=1` on the read page. */
  incognito?: boolean;
  /** When true on mount, the reader starts in **peek mode** — like
   *  incognito but flippable by the user via the peek banner's
   *  "Continue from here" button. Used by bookmark "Jump to page"
   *  navigation (`?peek=1`) so glancing at a marker doesn't generate
   *  activity. No timeout; the user controls when peek ends. */
  initialPeek?: boolean;
  readingMinActiveMs: number;
  readingMinPages: number;
  readingIdleMs: number;
}) {
  const router = useRouter();
  const init = useReaderStore((s) => s.init);
  const currentPage = useReaderStore((s) => s.currentPage);
  const fitMode = useReaderStore((s) => s.fitMode);
  const viewMode = useReaderStore((s) => s.viewMode);
  const direction = useReaderStore((s) => s.direction);
  const brightness = useReaderStore((s) => s.brightness);
  const sepia = useReaderStore((s) => s.sepia);
  const coverSolo = useReaderStore((s) => s.coverSolo);
  const setPage = useReaderStore((s) => s.setPage);
  const nextPage = useReaderStore((s) => s.nextPage);
  const prevPage = useReaderStore((s) => s.prevPage);
  const cycleFitMode = useReaderStore((s) => s.cycleFitMode);
  const cycleViewMode = useReaderStore((s) => s.cycleViewMode);
  const toggleChrome = useReaderStore((s) => s.toggleChrome);
  const togglePageStrip = useReaderStore((s) => s.togglePageStrip);
  const chromeVisible = useReaderStore((s) => s.chromeVisible);
  const pageStripVisible = useReaderStore((s) => s.pageStripVisible);
  const setChromeVisible = useReaderStore((s) => s.setChromeVisible);
  const toggleMarkersHidden = useReaderStore((s) => s.toggleMarkersHidden);
  const beginMarkerEdit = useReaderStore((s) => s.beginMarkerEdit);
  const setMarkerMode = useReaderStore((s) => s.setMarkerMode);
  const markerModeForKeybinds = useReaderStore((s) => s.markerMode);
  const pendingMarkerForKeybinds = useReaderStore((s) => s.pendingMarker);
  // Mount the lazy marker editor once a marker edit first opens, then keep
  // it mounted so the Sheet's close animation plays and re-opens are
  // instant ("adjust state during render" recipe — no effect).
  const [markerEditorMounted, setMarkerEditorMounted] = useState(false);
  if (pendingMarkerForKeybinds !== null && !markerEditorMounted) {
    setMarkerEditorMounted(true);
  }

  // First-run orientation overlay (audit C5). Read the localStorage flag
  // via useSyncExternalStore so it's SSR-safe (server snapshot = "seen",
  // no hydration flash, no setState-in-effect). Dismissal marks the
  // global flag and flips local state so it disappears immediately and
  // never returns.
  const firstRunSeen = useSyncExternalStore(
    subscribeReaderFirstRun,
    hasSeenReaderFirstRun,
    readerFirstRunServerSnapshot,
  );
  const [firstRunDismissed, setFirstRunDismissed] = useState(false);
  const showFirstRun = !firstRunSeen && !firstRunDismissed;
  const dismissFirstRun = useCallback(() => {
    markReaderFirstRunSeen();
    setFirstRunDismissed(true);
  }, []);

  // Per-issue marker fetch — drives the overlay, the page-strip dots,
  // and the bookmark toggle's "is this page already bookmarked?"
  // lookup. One round-trip, shared via TanStack Query cache.
  const issueMarkers = useIssueMarkers(issueId);
  // Prefetch the "what's next?" target on mount so the `Shift+N` keybind
  // (and the M4 end-of-issue card) can navigate without waiting on a
  // round-trip. The hook handles the CBL > series > none resolution
  // server-side; the client just consumes the result.
  const nextUp = useNextUp(issueId, cblSavedViewId);
  // CBL self-healing: when the server tells us `?cbl=<id>` pointed at a
  // CBL the current issue isn't in (entry deleted, shared link from a
  // sibling list, etc.), strip the dead param from the URL so a refresh
  // / shared link doesn't carry it forward. `router.replace` keeps the
  // browser history clean. Render-phase setState-style guard: a state
  // flag prevents repeated replaces if the resolver's data sticks
  // around in cache across renders.
  const [cblParamScrubbed, setCblParamScrubbed] = useState(false);
  if (
    !cblParamScrubbed &&
    cblSavedViewId &&
    nextUp.data?.cbl_param_was_stale === true
  ) {
    setCblParamScrubbed(true);
    if (typeof window !== "undefined") {
      const url = new URL(window.location.href);
      url.searchParams.delete("cbl");
      router.replace(url.pathname + url.search);
    }
  }
  const goNextIssue = useCallback(() => {
    const data = nextUp.data;
    if (!data || !data.target) {
      // Either the resolver hasn't loaded yet, or it returned source =
      // "none" — both translate to "nothing to do" from the user's POV.
      // A soft toast keeps the action discoverable instead of feeling
      // broken when the user mashes the key on the last issue.
      if (data && data.source === "none") {
        toast.message("You're caught up — no next issue.");
      }
      return;
    }
    // Forward the CBL context only when the resolver also picked CBL —
    // a series fallback means the user is no longer following the list
    // and the next read should reset to series-only context.
    const forwardCbl = data.source === "cbl" ? cblSavedViewId : null;
    router.push(readerUrl(data.target, { cbl: forwardCbl }));
  }, [cblSavedViewId, nextUp.data, router]);

  // Symmetric to useNextUp for the `Shift+P` keybind. Pure sequential
  // back-navigation (no finished-state filter); the resolver returns
  // source=none when the user is already at the first issue.
  const prevUp = usePrevUp(issueId, cblSavedViewId);
  const goPrevIssue = useCallback(() => {
    const data = prevUp.data;
    if (!data || !data.target) {
      if (data && data.source === "none") {
        toast.message("Already at the first issue.");
      }
      return;
    }
    const forwardCbl = data.source === "cbl" ? cblSavedViewId : null;
    router.push(readerUrl(data.target, { cbl: forwardCbl }));
  }, [cblSavedViewId, prevUp.data, router]);

  // End-of-issue side panel. Opens only when the user *attempts to
  // advance past* the last page (e.g., right-arrow / spacebar / swipe
  // forward / right-tap zone). Reaching the last page is not by itself
  // a trigger — the panel must never sit on top of the comic the user
  // is still reading. `goNext` does the intercept (see below).
  //
  // Auto-dismiss when the page state moves back below the last page so
  // a `goPrev` after the panel opens closes it cleanly. No once-per-
  // mount gate is needed: the trigger is user-initiated, not implicit.
  const [showEndCard, setShowEndCard] = useState(false);
  if (showEndCard && currentPage < totalPages - 1) {
    setShowEndCard(false);
  }
  const dismissEndCard = useCallback(() => setShowEndCard(false), []);
  const continueFromEndCard = useCallback(() => {
    setShowEndCard(false);
    goNextIssue();
  }, [goNextIssue]);
  // Per-page natural dimensions, populated as pages load. Stored in a
  // ref so a slider that rebuilds PageImage doesn't trigger a re-render
  // here — the overlay reads it directly.
  const pageNaturalSize = useRef<
    Map<number, { width: number; height: number }>
  >(new Map());
  // Top-of-page snap state for single/double mode (see the page-change
  // effect below for the full rationale). Declared here so the natural-
  // size handler can re-assert the top once a freshly-turned page
  // decodes. `snapTopRef` stays armed from the page turn until the page
  // loads or the reader scrolls on purpose; `programmaticScrollRef`
  // marks our own `scrollTo` so it isn't mistaken for a user scroll.
  const snapTopRef = useRef(false);
  const programmaticScrollRef = useRef(false);
  const scrollWindowToTop = useCallback(() => {
    if (typeof window === "undefined") return;
    const reduced = window.matchMedia?.(
      "(prefers-reduced-motion: reduce)",
    ).matches;
    programmaticScrollRef.current = true;
    window.scrollTo({
      top: 0,
      left: 0,
      behavior: reduced ? "auto" : "instant",
    });
    requestAnimationFrame(() => {
      programmaticScrollRef.current = false;
    });
  }, []);
  const handleNaturalSize = useCallback(
    (page: number) => (width: number, height: number) => {
      pageNaturalSize.current.set(page, { width, height });
      // The image just decoded and the wrapper jumped from ~0 to full
      // height. If this is the page we just turned to and the reader
      // hasn't scrolled away on purpose, snap the (possibly drifted)
      // viewport back to the top so every fresh page starts at the top.
      if (
        snapTopRef.current &&
        page === useReaderStore.getState().currentPage
      ) {
        scrollWindowToTop();
      }
    },
    [scrollWindowToTop],
  );

  // Bookmark-toggle helper: looks up an existing page-level bookmark
  // for `currentPage` and creates/deletes accordingly. Used by the
  // `b` keybind so it mirrors the chrome's bookmark button.
  // `b` / `s` keybinds — share one toggle hook with the chrome's
  // BookmarkToggleButton / FavoriteToggleButton (kills the duplicated
  // find-existing → create/delete-with-Undo logic, audit G9). The
  // page-specific toast copy stays here so the keybind path keeps its
  // exact wording ("Bookmarked page N" / "Removed bookmark on page N").
  const bookmarkToggle = usePageMarkerToggle(issueId, currentPage, "bookmark");
  const favoriteToggle = usePageMarkerToggle(issueId, currentPage, "favorite");
  const toggleBookmark = useCallback(
    () =>
      bookmarkToggle.toggle({
        created: `Bookmarked page ${currentPage + 1}`,
        removed: `Removed bookmark on page ${currentPage + 1}`,
      }),
    [bookmarkToggle, currentPage],
  );
  const toggleFavorite = useCallback(
    () =>
      favoriteToggle.toggle({
        created: `Starred page ${currentPage + 1}`,
        removed: `Unstarred page ${currentPage + 1}`,
      }),
    [favoriteToggle, currentPage],
  );

  const initialDirection = useMemo<Direction>(
    () =>
      detectDirection(
        manga,
        userDefaultDirection,
        libraryDefaultDirection,
        seriesReadingDirection,
      ),
    [
      manga,
      userDefaultDirection,
      libraryDefaultDirection,
      seriesReadingDirection,
    ],
  );
  // User defaults take precedence over auto-detection on first mount; per-series
  // localStorage still wins over both (see store.init).
  const initialViewMode = useMemo<ViewMode>(
    () => userDefaultViewMode ?? detectViewMode(pages),
    [pages, userDefaultViewMode],
  );
  const initialFitMode = useMemo<FitMode>(
    () => userDefaultFitMode ?? "width",
    [userDefaultFitMode],
  );

  // Initial mount: hydrate the store from props.
  useEffect(() => {
    init({
      issueId,
      seriesId,
      totalPages,
      initialPage,
      initialDirection,
      initialViewMode,
      initialFitMode,
      initialPageStripVisible: userDefaultPageStrip,
      initialCoverSolo: userDefaultCoverSolo,
    });
  }, [
    init,
    issueId,
    seriesId,
    totalPages,
    initialPage,
    initialDirection,
    initialViewMode,
    initialFitMode,
    userDefaultPageStrip,
    userDefaultCoverSolo,
  ]);

  // Resolve user → defaults at mount; pinned in a ref so the listener below
  // doesn't churn when the keymap object identity changes.
  const bindings = useMemo(() => resolveKeybinds(userKeybinds), [userKeybinds]);

  // Spread-group derivation (Phase B). In double-page view we navigate
  // between groups (a solo cover, a pair, a solo spread) instead of raw
  // page indices, so a {4,5} pair never lands on {5,6} on the next flip.
  // Single + webtoon modes don't pair pages and use the raw `currentPage`
  // for navigation as before.
  const groups = useMemo<ReadonlyArray<SpreadGroup>>(
    () => computeSpreadGroups(pages, { coverSolo, totalPages }),
    [pages, coverSolo, totalPages],
  );
  const currentGroupIdx = useMemo(
    () => groupIndexForPage(groups, currentPage),
    [groups, currentPage],
  );
  const visiblePages = useMemo<readonly number[]>(
    () => groups[currentGroupIdx] ?? [currentPage],
    [groups, currentGroupIdx, currentPage],
  );

  // v0.3.44 / v0.3.45 page-turn animation. Webtoon mode skips
  // entirely (continuous scroll is its own animation). The hook
  // also gates on `prefers-reduced-motion` internally.
  const pageTransition = usePageTransition({
    currentPage,
    direction,
    mode:
      viewMode === "webtoon" ? "off" : (userDefaultPageAnimation ?? "slide"),
  });

  // Direction-aware navigation. In RTL, "next" should respond to ← and the
  // right tap zone (so a swipe-right feels like turning the page forward).
  //
  // End-of-issue intercept: when the user attempts to advance past the
  // last page (or last spread-group in double mode), open the
  // EndOfIssueCard side panel instead of no-opping. Don't hijack while
  // a marker is in flight — same suppression rule the rest of the
  // reader uses for incidental UI.
  const goNext = useCallback(() => {
    const markerActive =
      markerModeForKeybinds !== "idle" || pendingMarkerForKeybinds !== null;
    if (viewMode === "double" && groups.length > 0) {
      const atLastGroup = currentGroupIdx >= groups.length - 1;
      if (atLastGroup) {
        if (!markerActive) setShowEndCard(true);
        return;
      }
      const target = currentGroupIdx + 1;
      setPage(firstPageOfGroup(groups, target));
    } else {
      const atLastPage = totalPages > 0 && currentPage >= totalPages - 1;
      if (atLastPage) {
        if (!markerActive) setShowEndCard(true);
        return;
      }
      nextPage();
    }
  }, [
    viewMode,
    groups,
    currentGroupIdx,
    currentPage,
    totalPages,
    markerModeForKeybinds,
    pendingMarkerForKeybinds,
    setPage,
    nextPage,
  ]);
  const goPrev = useCallback(() => {
    if (viewMode === "double" && groups.length > 0) {
      const target = Math.max(0, currentGroupIdx - 1);
      setPage(firstPageOfGroup(groups, target));
    } else {
      prevPage();
    }
  }, [viewMode, groups, currentGroupIdx, setPage, prevPage]);
  const onLeftZone = direction === "rtl" ? goNext : goPrev;
  const onRightZone = direction === "rtl" ? goPrev : goNext;

  // Bookmark-page list precomputed for the keymap hook's
  // `nextBookmark` / `prevBookmark` jumps. Sorted ascending so the
  // `find` / linear walk inside the hook can short-circuit.
  const bookmarkPages = useMemo<readonly number[]>(
    () =>
      (issueMarkers.data?.items ?? [])
        .filter((m) => m.kind === "bookmark")
        .map((m) => m.page_index)
        .sort((a, b) => a - b),
    [issueMarkers.data],
  );

  // The keymap's `quitReader` action either dismisses the end-card
  // (first Esc) or exits the reader (subsequent Esc). The hook only
  // owns dispatch; this component owns the end-card visibility and
  // router push.
  const handleQuitReader = useCallback(() => {
    router.push(exitUrl);
  }, [exitUrl, router]);

  // `addNote` / `startHighlight` keymap actions are forwarded through
  // these wrappers so the hook doesn't need to know about the marker
  // store's `beginMarkerEdit` / `setMarkerMode` shape.
  const beginAddNote = useCallback(() => {
    beginMarkerEdit({
      kind: "note",
      page_index: currentPage,
      region: null,
      selection: null,
      body: "",
      is_favorite: false,
      tags: [],
    });
  }, [beginMarkerEdit, currentPage]);
  const beginHighlight = useCallback(() => {
    setMarkerMode("select-rect");
  }, [setMarkerMode]);
  const beginCaptureText = useCallback(() => {
    setMarkerMode("select-text");
  }, [setMarkerMode]);

  // `toggleMarkersHidden` from the store flips visibility silently;
  // the keymap wants a toast confirming the new state (the visual
  // delta isn't obvious with few markers on the current page).
  const toggleMarkersHiddenWithToast = useCallback(() => {
    toggleMarkersHidden();
    const nowHidden = useReaderStore.getState().markersHidden;
    toast.message(nowHidden ? "Markers hidden" : "Markers shown");
  }, [toggleMarkersHidden]);

  // Reader keymap — §7.4 (M7.5). Bindings already resolved against
  // user prefs above; the hook owns the global keydown listener,
  // `g g` / `Shift+G` vim aliases, RTL arrow-key flip, and end-card
  // Esc-then-exit two-step. Marker-mode active state suppresses
  // page-nav so a highlight drag isn't interrupted; the marker
  // overlay's capture-phase handler still owns Esc in that mode.
  // Transform zoom (audit C9). Single-page only; transient per-page
  // (resets on page / fit / view / marker-mode change — the last so a
  // CSS transform never desyncs the offset-positioned MarkerOverlay
  // while drawing). `+`/`-` walk a discrete ladder and re-center; the
  // drag-to-pan in `SinglePageView` clamps the offset to the page edges.
  const [zoom, setZoom] = useState<{
    scale: number;
    offset: { x: number; y: number };
    origin: { x: number; y: number };
  }>({ scale: 1, offset: { x: 0, y: 0 }, origin: { x: 50, y: 50 } });
  // Overflow (audit C4): a fit=height/original page rendered wider/taller
  // than the viewport. Reported up from SinglePageView; makes a drag pan
  // the page (rather than turn it) so the cropped sides are reachable.
  const [overflowing, setOverflowing] = useState(false);
  // Kept fresh for the gesture callbacks (which close over stale state).
  const zoomRef = useRef(zoom);
  useEffect(() => {
    zoomRef.current = zoom;
  }, [zoom]);
  // Rendered content vs visible-box sizes for clamping the pan — owned
  // by SinglePageView (it has the img + wrapper refs), read here.
  const panMetricsRef = useRef<{
    content: { w: number; h: number };
    container: { w: number; h: number };
  }>({ content: { w: 0, h: 0 }, container: { w: 0, h: 0 } });
  const panStartRef = useRef<{ x: number; y: number }>({ x: 0, y: 0 });

  const RECENTER = { offset: { x: 0, y: 0 }, origin: { x: 50, y: 50 } };
  const zoomIn = useCallback(
    () => setZoom((z) => ({ scale: nextZoomStep(z.scale, "in"), ...RECENTER })),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
  const zoomOut = useCallback(
    () =>
      setZoom((z) => ({ scale: nextZoomStep(z.scale, "out"), ...RECENTER })),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
  const zoomReset = useCallback(
    () => setZoom({ scale: 1, ...RECENTER }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
  // Double-tap / double-click toggles 1× ↔ 2× at the tapped point.
  const zoomToggleAt = useCallback(
    (rectX: number, rectY: number, rect: { w: number; h: number }) => {
      setZoom((z) =>
        z.scale > 1
          ? { scale: 1, ...RECENTER }
          : {
              scale: DOUBLE_TAP_ZOOM,
              offset: { x: 0, y: 0 },
              origin: zoomOriginPercent(rectX, rectY, rect),
            },
      );
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );

  // Pan (C4/C9). The gesture hook forwards drag movement here so panning
  // works *through* the TapZones overlay: drags bubble up to the gesture
  // container while taps stay with the zones.
  const panActive =
    (zoom.scale > 1 || overflowing) && markerModeForKeybinds === "idle";
  const onPanStart = useCallback(() => {
    panStartRef.current = zoomRef.current.offset;
  }, []);
  const onPan = useCallback((dx: number, dy: number) => {
    const { content, container } = panMetricsRef.current;
    const next = clampPan(
      { x: panStartRef.current.x + dx, y: panStartRef.current.y + dy },
      content,
      container,
    );
    setZoom((z) => ({ ...z, offset: next }));
  }, []);

  // Reset transient zoom/pan when the page / fit / view / marker-mode key
  // changes — React's "adjust state during render" recipe (no effect, no
  // extra paint). The marker-mode reset also sidesteps the
  // CSS-transform-vs-offset-positioned-overlay desync while drawing.
  const zoomResetKey = `${currentPage}|${fitMode}|${viewMode}|${markerModeForKeybinds}`;
  const [zoomKey, setZoomKey] = useState(zoomResetKey);
  if (zoomKey !== zoomResetKey) {
    setZoomKey(zoomResetKey);
    if (zoom.scale !== 1 || zoom.offset.x !== 0 || zoom.offset.y !== 0) {
      setZoom({ scale: 1, ...RECENTER });
    }
  }

  useReaderKeymap({
    bindings,
    viewMode,
    direction,
    groups,
    totalPages,
    currentPage,
    markerActive:
      markerModeForKeybinds !== "idle" || pendingMarkerForKeybinds !== null,
    showEndCard,
    chromeOrStripVisible: chromeVisible || pageStripVisible,
    bookmarkPages,
    setPage,
    onNext: goNext,
    onPrev: goPrev,
    onNextIssue: goNextIssue,
    onPrevIssue: goPrevIssue,
    toggleChrome,
    cycleFitMode,
    cycleViewMode,
    zoomIn,
    zoomOut,
    zoomReset,
    togglePageStrip,
    toggleBookmark,
    toggleFavorite,
    toggleMarkersHidden: toggleMarkersHiddenWithToast,
    beginAddNote,
    beginHighlight,
    beginCaptureText,
    onQuitReader: handleQuitReader,
    onDismissEndCard: dismissEndCard,
    onCollapseChrome: () => setChromeVisible(false),
  });

  // Peek mode: when on, both the progress write and the session
  // tracker no-op. Starts from the `?peek=1` URL flag (set by bookmark
  // "Jump to page" navigation) and stays on until the user clicks
  // "Continue from here" in the peek banner — no timeout, fully
  // user-controlled. Flipping it off re-runs the progress effect
  // (peek is in its deps), which fires a fresh write at the current
  // page so the user's resume position is captured immediately.
  const [peekActive, setPeekActive] = useState<boolean>(initialPeek);
  const exitPeek = useCallback(() => {
    setPeekActive(false);
    // Drop `?peek=1` from the URL so a refresh / shared link doesn't
    // re-enable peek for the same session. `router.replace` keeps the
    // browser history clean.
    if (typeof window !== "undefined") {
      const url = new URL(window.location.href);
      if (url.searchParams.has("peek")) {
        url.searchParams.delete("peek");
        router.replace(url.pathname + url.search);
      }
    }
  }, [router]);
  const suppressWrites = incognito || peekActive;

  useReaderProgressWrite({
    issueId,
    currentPage,
    initialPage,
    totalPages,
    incognito: suppressWrites,
    // Webtoon scroll-tracking drives `currentPage` both ways; persist a
    // monotonic high-water page so a scroll-up can't regress progress
    // (audit risk #5). Single/double keep raw writes — jumps are intent.
    monotonic: viewMode === "webtoon",
  });

  // M6a — capture the reading session (idempotent 30s heartbeat + final
  // flush). Coexists with the per-page progress write above; one source
  // (currentPage), two sinks (progress immediate / session aggregator).
  // Pass `visiblePages` so paired pages both count toward the session
  // envelope (start/end/distinct) in double-page mode.
  useReadingSession({
    issueId,
    totalPages,
    currentPage,
    viewMode,
    visiblePages,
    trackingEnabled: activityTrackingEnabled && !suppressWrites,
    minActiveMs: readingMinActiveMs,
    minPages: readingMinPages,
    idleMs: readingIdleMs,
  });

  useReaderPrefetch({
    issueId,
    totalPages,
    currentPage,
    currentGroupIdx,
    groups,
    viewMode,
  });

  // Reset scroll on page change in single/double mode so each new page
  // starts at the top. Webtoon manages its own scroll position via the
  // continuous-scroll layout.
  //
  // `snapTopRef` stays armed from the page turn until either (a) the new
  // page's image finishes decoding — at which point `handleNaturalSize`
  // re-asserts the top, because the wrapper only grows to full height on
  // load and a slow decode could otherwise leave the viewport parked
  // mid-page — or (b) the reader scrolls on purpose, which disarms it so
  // we never yank a deliberately-scrolled reader back to the top.
  useEffect(() => {
    if (viewMode === "webtoon") return;
    snapTopRef.current = true;
    scrollWindowToTop();
  }, [currentPage, viewMode, scrollWindowToTop]);
  useEffect(() => {
    if (viewMode === "webtoon") return;
    const onScroll = () => {
      if (programmaticScrollRef.current) return;
      snapTopRef.current = false;
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, [viewMode]);

  // Tailwind v4 preflight applies `max-width: 100%; height: auto` to every
  // `<img>`, which makes a naive empty-class "original" identical to
  // "max-w-full h-auto" for any page narrower than the viewport. Each mode
  // therefore overrides the preflight explicitly so the three behaviors
  // are actually distinct:
  //
  //   - width   → always fills viewport width (scales up if narrower).
  //               In double-page view the pair fills viewport width 50/50
  //               (via `paneClass` on the pane wrapper); in webtoon each
  //               image fills viewport width.
  //   - height  → always fills viewport height (overflows horizontally
  //               for wide spreads — body scrolls).
  //   - original → image at its intrinsic pixel size, no constraints.
  const fitClass =
    fitMode === "width"
      ? "w-full h-auto max-w-none"
      : fitMode === "height"
        ? // Fit-height fills the *safe* viewport, not the raw 100vh — on an
          // iOS PWA (status-bar-translucent + viewport-fit=cover) the top/bottom
          // insets are non-zero, so the page sits in the safe band and the
          // status bar / home indicator land on the black letterbox instead of
          // the art. Off-iOS the insets are 0, so this is exactly 100dvh.
          "h-[calc(100dvh_-_var(--safe-top)_-_var(--safe-bottom))] w-auto max-w-none"
        : "max-w-none w-auto h-auto";
  // Double-page panes need different wrapper sizing depending on fitMode.
  // In width mode each pane is forced to share the viewport row (flex-1
  // with min-w-0 so the inner img can shrink); in height/original modes
  // the pane is content-sized so the natural image width drives layout.
  const doublePaneClass =
    fitMode === "width" ? "flex-1 min-w-0" : "inline-block";
  // Solo-page width cap (audit C8): in double-page width-fit mode a lone
  // page (the solo cover or a trailing odd page) otherwise stretches across
  // the whole spread — twice the scale of the paired pages around it, which
  // reads as a jarring zoom on every cover. Cap it to one-page width UNLESS
  // the lone page is itself a spread (wide art that genuinely wants both
  // halves of the opening).
  const soloPaneCapped =
    viewMode === "double" &&
    fitMode === "width" &&
    visiblePages.length === 1 &&
    !isSpreadPage(pages[visiblePages[0]!]);

  // Gestures: horizontal drag (swipe) for page nav. Pinch is left
  // to the browser as native pinch-to-zoom so mobile users can
  // zoom into small letterer text. Pinch used to cycle fit modes
  // (v0.3.x and earlier), but a discoverable hidden gesture isn't
  // worth blocking the platform zoom — fit-mode toggling stays on
  // the `f` key and the chrome toggle button.
  //
  // Webtoon mode skips swipe — vertical scroll is the native
  // interaction there.
  const gestureRef = useRef<HTMLDivElement>(null);
  // Disable swipe while the user is drawing a highlight or has a
  // pending marker editor open: `@use-gesture/react` attaches
  // native pointer listeners on this container that fire BEFORE
  // React's synthetic handlers on the SVG overlay, so a horizontal
  // drag in highlight mode was being interpreted as a page-flip
  // swipe. Switching off the gesture entirely is cleaner than
  // racing `stopPropagation` on the native handlers.
  useReaderGestures({
    target: gestureRef,
    enabled:
      markerModeForKeybinds === "idle" && pendingMarkerForKeybinds === null,
    viewMode,
    direction,
    onNext: goNext,
    onPrev: goPrev,
    // When zoomed or a page overflows the viewport, the drag pans the
    // page (the gesture container receives the drag even though the
    // TapZones overlay is the pointer target); otherwise it turns pages.
    panActive,
    onPanStart,
    onPan,
  });

  return (
    <div
      ref={gestureRef}
      // `touch-action: pan-y pinch-zoom` keeps native vertical
      // scroll AND native pinch-zoom, while leaving the horizontal
      // axis available for the JS swipe-to-turn handler above.
      //
      // `overflow-anchor: none` opts the whole reader subtree out of
      // the browser's scroll-anchoring heuristic. Page images carry no
      // reserved height, so a freshly-turned page's wrapper grows from
      // ~0 to full height as the bytes decode; with anchoring on, the
      // browser would scroll *down* to "preserve" the content it had
      // anchored around the (vertically-centered) collapsed placeholder,
      // landing the viewport mid-page. Off, our explicit scroll-to-top
      // stays put and new pages always start at the top.
      // While panning (zoom/overflow) the JS gesture owns the drag, so
      // drop native pan/scroll to "none"; otherwise keep native vertical
      // scroll + pinch-zoom and leave the horizontal axis for swipe.
      style={{
        touchAction: panActive ? "none" : "pan-y pinch-zoom",
        overflowAnchor: "none",
      }}
      // Reader surface token (see globals.css `--reader-bg`): the
      // route-level loading skeleton consumes the same token so the
      // fallback never flashes white before the reader paints.
      className="bg-reader-bg min-h-screen text-neutral-200"
    >
      <ReaderChrome
        seriesId={seriesId}
        issueId={issueId}
        exitUrl={exitUrl}
        totalPages={totalPages}
        visiblePages={viewMode === "double" ? visiblePages : undefined}
        progressCurrent={viewMode === "double" ? currentGroupIdx : currentPage}
        progressTotal={viewMode === "double" ? groups.length : totalPages}
        incognito={incognito}
      />

      {peekActive && (
        // Peek-mode banner. Fixed at the top, below the safe-area
        // inset so it clears the iPhone notch / Dynamic Island. z-30
        // keeps it above the reader content but below modals
        // (MarkerEditor uses z-50). `pointer-events-auto` on the
        // banner so clicks reach the button even when the outer
        // wrapper sets pointer-events for the chrome.
        <div
          className="pointer-events-none fixed inset-x-0 top-0 z-30 flex justify-center px-3 pt-(--safe-top)"
          aria-live="polite"
        >
          <div className="border-border bg-background/95 text-foreground pointer-events-auto mt-3 flex w-full max-w-md items-center gap-3 rounded-lg border px-3 py-2 shadow-lg backdrop-blur">
            <p className="flex-1 text-xs leading-tight">
              <span className="font-medium">Peek mode.</span>{" "}
              <span className="text-muted-foreground">
                Your reading isn&apos;t being tracked.
              </span>
            </p>
            <button
              type="button"
              onClick={exitPeek}
              className="bg-primary text-primary-foreground hover:bg-primary/90 shrink-0 rounded-md px-3 py-1.5 text-xs font-medium"
            >
              Continue from here
            </button>
          </div>
        </div>
      )}

      <div aria-live="polite" aria-atomic="true" className="sr-only">
        {viewMode === "double" && visiblePages.length === 2
          ? `Pages ${visiblePages[0]! + 1} and ${visiblePages[1]! + 1} of ${totalPages}`
          : `Page ${currentPage + 1} of ${totalPages}`}
      </div>

      <div
        style={{
          filter:
            brightness !== 1 || sepia !== 0
              ? `brightness(${brightness}) sepia(${sepia})`
              : undefined,
        }}
      >
        {viewMode === "webtoon" ? (
          <WebtoonView
            issueId={issueId}
            totalPages={totalPages}
            pages={pages}
            fitClass={fitClass}
            onChromeZone={toggleChrome}
            nextUpData={nextUp.data}
            nextUpLoading={nextUp.isLoading}
            onReadNext={continueFromEndCard}
            exitUrl={exitUrl}
          />
        ) : viewMode === "double" ? (
          <DoublePageView
            issueId={issueId}
            visiblePages={visiblePages}
            direction={direction}
            fitClass={fitClass}
            paneClass={doublePaneClass}
            soloCapped={soloPaneCapped}
            onLeftZone={onLeftZone}
            onRightZone={onRightZone}
            onChromeZone={toggleChrome}
            onNaturalSize={handleNaturalSize}
            pageNaturalSize={pageNaturalSize}
            transition={pageTransition}
          />
        ) : (
          <SinglePageView
            issueId={issueId}
            currentPage={currentPage}
            fitClass={fitClass}
            onLeftZone={onLeftZone}
            onRightZone={onRightZone}
            onChromeZone={toggleChrome}
            onNaturalSize={handleNaturalSize}
            pageNaturalSize={pageNaturalSize}
            transition={pageTransition}
            zoom={zoom}
            overflowing={overflowing}
            onOverflowChange={setOverflowing}
            panMetricsRef={panMetricsRef}
            onZoomToggle={zoomToggleAt}
          />
        )}
      </div>

      <PageStrip
        issueId={issueId}
        totalPages={totalPages}
        currentPage={currentPage}
        direction={direction}
        pages={pages}
      />
      {markerEditorMounted ? (
        <MarkerEditor issueId={issueId} pageNaturalSize={pageNaturalSize} />
      ) : null}
      {markerModeForKeybinds !== "idle" ? (
        <MarkerModePill
          mode={markerModeForKeybinds}
          onCancel={() => setMarkerMode("idle")}
        />
      ) : null}
      {showFirstRun ? (
        <ReaderFirstRunOverlay
          direction={direction}
          onDismiss={dismissFirstRun}
        />
      ) : null}
      <EndOfIssueCard
        open={showEndCard}
        data={nextUp.data}
        isLoading={nextUp.isLoading}
        direction={direction}
        exitUrl={exitUrl}
        onContinue={continueFromEndCard}
        onDismiss={dismissEndCard}
      />
    </div>
  );
}

function SinglePageView({
  issueId,
  currentPage,
  fitClass,
  onLeftZone,
  onRightZone,
  onChromeZone,
  onNaturalSize,
  pageNaturalSize,
  transition,
  zoom,
  overflowing,
  onOverflowChange,
  panMetricsRef,
  onZoomToggle,
}: {
  issueId: string;
  currentPage: number;
  fitClass: string;
  onLeftZone: () => void;
  onRightZone: () => void;
  onChromeZone: () => void;
  onNaturalSize: (page: number) => (w: number, h: number) => void;
  pageNaturalSize: React.RefObject<
    Map<number, { width: number; height: number }>
  >;
  transition: PageTransitionResult;
  /** Transform zoom + pan (audit C4/C9). Pan offset is screen-px. */
  zoom: {
    scale: number;
    offset: { x: number; y: number };
    origin: { x: number; y: number };
  };
  /** Whether the rendered page overflows the viewport (drives pan). */
  overflowing: boolean;
  onOverflowChange: (overflowing: boolean) => void;
  /** Reader-owned clamp metrics; populated here from the live refs. */
  panMetricsRef: React.RefObject<{
    content: { w: number; h: number };
    container: { w: number; h: number };
  }>;
  /** Double-tap / double-click toggle, with the tap point + page rect. */
  onZoomToggle: (
    rectX: number,
    rectY: number,
    rect: { w: number; h: number },
  ) => void;
}) {
  const markerMode = useReaderStore((s) => s.markerMode);
  const panActive = (zoom.scale > 1 || overflowing) && markerMode === "idle";
  // The wrapper is a block-level <div> rather than the previous
  // `inline-block` <span> so it has no inline-baseline descender that
  // could leave the overlay's `absolute inset-0` covering a slightly
  // taller area than the rendered img.
  const pageWrapRef = useRef<HTMLDivElement>(null);
  // Track the rendered image separately so the overlay aligns to the
  // actual image bounds (at fit=height the wrapper is full-width but the
  // image is centered and narrower).
  const imgRef = useRef<HTMLImageElement>(null);

  // Measure rendered image vs wrapper: (a) report horizontal/vertical
  // overflow up so the gesture layer pans (C4), and (b) keep the
  // pan-clamp metrics fresh (content = scaled wrapper when zoomed, else
  // the rendered image; container = the visible wrapper box).
  useEffect(() => {
    const wrap = pageWrapRef.current;
    if (!wrap) return;
    const measure = () => {
      const img = imgRef.current;
      const cw = wrap.clientWidth;
      const ch = wrap.clientHeight;
      const iw = img?.offsetWidth ?? cw;
      const ih = img?.offsetHeight ?? ch;
      panMetricsRef.current = {
        content:
          zoom.scale > 1
            ? { w: cw * zoom.scale, h: ch * zoom.scale }
            : { w: iw, h: ih },
        container: { w: cw, h: ch },
      };
      onOverflowChange(iw > cw + 1 || ih > ch + 1);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(wrap);
    if (imgRef.current) ro.observe(imgRef.current);
    return () => ro.disconnect();
  }, [currentPage, fitClass, zoom.scale, onOverflowChange, panMetricsRef]);
  const natural = pageNaturalSize.current?.get(currentPage) ?? null;
  return (
    <main className="relative grid min-h-screen place-items-center pt-(--safe-top) pb-(--safe-bottom)">
      <div
        className="relative w-full overflow-hidden"
        data-testid="reader-page-wrapper"
      >
        {transition.prevPage !== null && (
          // v0.3.44 (extended v0.3.45 for fade + curl): outgoing
          // page overlay. Absolute-positioned image of the previous
          // page that animates off-screen via the CSS class the
          // hook selected for the current mode. Keyed on prevPage
          // so the animation restarts cleanly on rapid navigation.
          // No MarkerOverlay on the outgoing layer — a sliding /
          // curling marker would look broken; the overlay snaps to
          // the new page below.
          <div
            key={`prev-${transition.prevPage}`}
            aria-hidden="true"
            className={`pointer-events-none absolute inset-0 flex w-full justify-center ${transition.exitAnimClass ?? ""}`}
          >
            {/* Raw <img>: reader page bytes are served by the Rust origin
                with their own cache headers; next/image's optimizer + URL
                rewriting buys nothing here and pays a per-request CPU
                cost. eslint-disable applies only to this element. */}
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img
              src={`/issues/${issueId}/pages/${transition.prevPage}`}
              alt=""
              className={`block ${fitClass}`}
              decoding="async"
            />
          </div>
        )}
        <div
          ref={pageWrapRef}
          className={transition.enterAnimClass ?? undefined}
          key={`enter-${currentPage}`}
          // Transform zoom + pan (C4/C9). `translate` first (screen px)
          // then `scale`, so the offset is in screen space and matches
          // the clamp bounds. PageImage + MarkerOverlay transform together
          // (visually locked). The pan/double-tap themselves are handled
          // by the gesture hook + TapZones (which sit above this wrapper
          // and so actually receive the pointers); this element only
          // *renders* the resulting transform. `transition: none` so a
          // pan tracks the pointer 1:1 (the page-turn anim class only
          // matters at 1×).
          style={
            panActive
              ? {
                  transform: `translate(${zoom.offset.x}px, ${zoom.offset.y}px) scale(${zoom.scale})`,
                  transformOrigin: `${zoom.origin.x}% ${zoom.origin.y}%`,
                  transition: "none",
                  willChange: "transform",
                  cursor: zoom.scale > 1 ? "grab" : undefined,
                }
              : undefined
          }
        >
          <PageImage
            key={`${issueId}-${currentPage}`}
            src={`/issues/${issueId}/pages/${currentPage}`}
            thumbSrc={`/issues/${issueId}/pages/${currentPage}/thumb?variant=strip`}
            alt={`Page ${currentPage + 1}`}
            fitClass={fitClass}
            fetchPriority="high"
            onNaturalSize={onNaturalSize(currentPage)}
            imgRef={imgRef}
          />
          <MarkerOverlay
            issueId={issueId}
            pageIndex={currentPage}
            imgRef={imgRef}
            naturalSize={natural}
          />
        </div>
      </div>
      {markerMode === "idle" ? (
        <TapZones
          onLeft={onLeftZone}
          onRight={onRightZone}
          onChrome={onChromeZone}
          // Double-tap / double-click the center zone toggles zoom at the
          // tapped point (single-page only). The zones receive the taps;
          // the page wrapper beneath them never would.
          onCenterDoubleTap={(cx, cy) => {
            const r = pageWrapRef.current?.getBoundingClientRect();
            if (!r) return;
            onZoomToggle(cx - r.left, cy - r.top, { w: r.width, h: r.height });
          }}
        />
      ) : null}
    </main>
  );
}

function DoublePageView({
  issueId,
  visiblePages,
  direction,
  fitClass,
  paneClass,
  soloCapped,
  onLeftZone,
  onRightZone,
  onChromeZone,
  onNaturalSize,
  pageNaturalSize,
  transition,
}: {
  issueId: string;
  visiblePages: readonly number[];
  direction: Direction;
  fitClass: string;
  paneClass: string;
  /** Solo, non-spread page in width-fit: cap the pane to one-page width
   *  instead of letting flex-1 stretch it across the full spread (C8). */
  soloCapped: boolean;
  onLeftZone: () => void;
  onRightZone: () => void;
  onChromeZone: () => void;
  onNaturalSize: (page: number) => (w: number, h: number) => void;
  pageNaturalSize: React.RefObject<
    Map<number, { width: number; height: number }>
  >;
  transition: PageTransitionResult;
}) {
  // RTL pairs render right-to-left; reuse `flex-row-reverse` to flip ordering.
  const flexClass =
    direction === "rtl" ? "flex flex-row-reverse" : "flex flex-row";
  const markerMode = useReaderStore((s) => s.markerMode);
  // In width-fit mode each pane is forced to share viewport width 50/50,
  // so the flex container itself needs to span the viewport. In other
  // modes it sizes to the natural image widths.
  const containerWidthClass = paneClass.includes("flex-1") ? "w-screen" : "";

  // v0.3.44: double-page slide is enter-only. Rendering the
  // outgoing spread as a pair of absolutely-positioned panes adds
  // a lot of layout complexity (panes can be flex-1 OR
  // inline-block depending on fit-mode) for marginal UX win — the
  // overflow-hidden wrapper + new spread sliding in from the
  // correct edge reads as a page-turn already. Single mode does
  // the retain-old-page version for the more nuanced single-image
  // case.
  return (
    <main className="relative grid min-h-screen place-items-center pt-(--safe-top) pb-(--safe-bottom)">
      <div className="relative w-full overflow-hidden">
        <div
          key={`enter-${visiblePages.join("-")}`}
          className={`${transition.enterAnimClass ?? ""} ${flexClass} ${containerWidthClass} items-center justify-center gap-1`}
        >
          {visiblePages.map((p) => (
            <DoublePagePane
              key={`${issueId}-${p}`}
              issueId={issueId}
              page={p}
              fitClass={fitClass}
              // Capped solo page: half the spread width (one page), centered
              // by the container's `justify-center`, so the cover/last page
              // sits at the same scale as the paired pages (C8).
              paneClass={soloCapped ? "w-1/2 min-w-0" : paneClass}
              onNaturalSize={onNaturalSize(p)}
              naturalSize={pageNaturalSize.current?.get(p) ?? null}
            />
          ))}
        </div>
      </div>
      {markerMode === "idle" ? (
        <TapZones
          onLeft={onLeftZone}
          onRight={onRightZone}
          onChrome={onChromeZone}
        />
      ) : null}
    </main>
  );
}

function DoublePagePane({
  issueId,
  page,
  fitClass,
  paneClass,
  onNaturalSize,
  naturalSize,
}: {
  issueId: string;
  page: number;
  fitClass: string;
  paneClass: string;
  onNaturalSize: (w: number, h: number) => void;
  naturalSize: { width: number; height: number } | null;
}) {
  // Pane sizing depends on fitMode (passed in as `paneClass`):
  //  - "flex-1 min-w-0"  → width-fit: each pane shares the viewport row
  //                       50/50; `min-w-0` lets the inner img shrink.
  //  - "inline-block"    → height/original-fit: pane sizes to image
  //                       content so two panes fit side-by-side.
  // `align-top` (kept for the inline-block path) kills the inline-baseline
  // descender so the SVG overlay's `absolute inset-0` covers the img box
  // exactly.
  const imgRef = useRef<HTMLImageElement>(null);
  return (
    <div className={`relative align-top ${paneClass}`}>
      <PageImage
        src={`/issues/${issueId}/pages/${page}`}
        thumbSrc={`/issues/${issueId}/pages/${page}/thumb?variant=strip`}
        alt={`Page ${page + 1}`}
        fitClass={fitClass}
        fetchPriority="high"
        onNaturalSize={onNaturalSize}
        imgRef={imgRef}
      />
      <MarkerOverlay
        issueId={issueId}
        pageIndex={page}
        imgRef={imgRef}
        naturalSize={naturalSize}
      />
    </div>
  );
}

function WebtoonView({
  issueId,
  totalPages,
  pages,
  fitClass,
  onChromeZone,
  nextUpData,
  nextUpLoading,
  onReadNext,
  exitUrl,
}: {
  issueId: string;
  totalPages: number;
  /** Server-known page dims — used to reserve each page's layout
   *  height (`aspect-ratio`) before the bytes arrive. */
  pages: PageInfo[];
  fitClass: string;
  /** Called when the user taps to toggle the reader chrome. Mirrors
   *  the middle-tap behavior of `<TapZones>` in single / double
   *  view; webtoon mode handles its own left/right (vertical scroll
   *  is the navigation), so we only need the chrome path. Without
   *  this, touch users in webtoon had no way to bring back the
   *  auto-hidden chrome short of pressing the `c` keybind. */
  onChromeZone: () => void;
  /** Next-up resolver data for the inline end-of-chapter footer (C2). */
  nextUpData: NextUpView | undefined;
  nextUpLoading: boolean;
  onReadNext: () => void;
  exitUrl: string;
}) {
  const markerMode = useReaderStore((s) => s.markerMode);
  const currentPage = useReaderStore((s) => s.currentPage);
  const setPage = useReaderStore((s) => s.setPage);
  const containerRef = useRef<HTMLElement>(null);
  // Tracks the most recent page index that came from the scroll observer
  // (vs. an external setPage call e.g. from PageStrip). When `currentPage`
  // diverges from this we treat it as an external jump and scroll the
  // matching page into view.
  const lastObservedPage = useRef<number>(-1);
  // While a programmatic `scrollIntoView` jump animates, the observer
  // sweeps through every page between origin and target and would
  // otherwise drag `currentPage` along for the ride — flickering the
  // progress write + chrome counter and, if the smooth scroll is
  // interrupted, stranding the reader on an intermediate page. This
  // timestamp suppresses observer-driven updates until the jump settles.
  const suppressObserverUntil = useRef<number>(0);

  // IntersectionObserver tracks which page is most visible as the user
  // scrolls. The store's `currentPage` then drives ReadingProgress + the
  // PageStrip highlight + the chrome counter, all of which were frozen at
  // the issue's initial page before this.
  useEffect(() => {
    if (totalPages === 0) return;
    const container = containerRef.current;
    if (!container) return;
    const items = Array.from(
      container.querySelectorAll<HTMLElement>("[data-page-idx]"),
    );
    if (items.length === 0) return;
    const ratios = new Map<number, number>();
    const observer = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          const raw = e.target.getAttribute("data-page-idx");
          if (raw === null) continue;
          ratios.set(Number(raw), e.intersectionRatio);
        }
        let bestIdx = -1;
        let bestRatio = 0;
        for (const [idx, r] of ratios) {
          if (r > bestRatio) {
            bestRatio = r;
            bestIdx = idx;
          }
        }
        // Ignore the interim pages a programmatic jump scrolls past.
        if (performance.now() < suppressObserverUntil.current) return;
        if (bestIdx >= 0) {
          lastObservedPage.current = bestIdx;
          setPage(bestIdx);
        }
      },
      { threshold: [0, 0.25, 0.5, 0.75, 1] },
    );
    for (const el of items) observer.observe(el);
    return () => observer.disconnect();
  }, [issueId, totalPages, setPage]);

  // External `setPage` calls (PageStrip click, keyboard jump, resumed
  // progress on first paint) should scroll the matching page into view.
  // The `lastObservedPage` ref discriminates these from observer-driven
  // updates so we don't fight our own scroll-tracking.
  useEffect(() => {
    if (currentPage === lastObservedPage.current) return;
    const el = containerRef.current?.querySelector<HTMLElement>(
      `[data-page-idx="${currentPage}"]`,
    );
    if (!el) return;
    const reduced =
      typeof window !== "undefined" &&
      window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    // Claim the target as the observed page up front and mute the
    // observer for the duration of the (smooth) scroll so the pages it
    // sweeps past don't reset `currentPage` to an intermediate value.
    // The window self-heals: once it lapses the observer re-syncs to
    // whatever is actually on screen. Reduced-motion jumps are instant,
    // so no muting is needed.
    lastObservedPage.current = currentPage;
    suppressObserverUntil.current = reduced ? 0 : performance.now() + 700;
    el.scrollIntoView({
      behavior: reduced ? "auto" : "smooth",
      block: "start",
    });
  }, [currentPage]);

  // Window rendering to ±N pages around the current one (audit C1b).
  // Every page keeps a stable `data-page-idx` wrapper mounted (so the
  // IntersectionObserver above never loses an element and the observed
  // set never churns); only the heavy body — `<PageImage>` +
  // `<MarkerOverlay>` — mounts inside the window. Off-window slots are
  // sized placeholders (same `fitClass` + the page's aspect-ratio) so the
  // scroll height is exact: resume lands right and the placeholder→image
  // swap doesn't shift layout.
  const mountWindow = computeWebtoonWindow(currentPage, totalPages);

  return (
    <main
      ref={containerRef}
      className="flex min-h-screen flex-col items-center pt-(--safe-top) pb-(--safe-bottom)"
    >
      {Array.from({ length: totalPages }, (_, i) => {
        const within = i >= mountWindow.start && i <= mountWindow.end;
        return (
          <div key={`${issueId}-${i}`} data-page-idx={i} className="relative">
            {within ? (
              <WebtoonPage
                issueId={issueId}
                pageIndex={i}
                pageInfo={pages[i]}
                fitClass={fitClass}
                eager={i < 3 || Math.abs(i - currentPage) <= 1}
              />
            ) : (
              <span className="flex w-full justify-center">
                <span
                  aria-hidden="true"
                  className={`block ${fitClass}`}
                  style={{ aspectRatio: placeholderAspectRatio(pages[i]) }}
                />
              </span>
            )}
          </div>
        );
      })}
      <WebtoonEndFooter
        data={nextUpData}
        isLoading={nextUpLoading}
        onReadNext={onReadNext}
        exitUrl={exitUrl}
      />
      {markerMode === "idle" ? (
        // Chrome-toggle tap region for webtoon. `<TapZones>` in
        // single/double covers the page with three columns
        // (prev / chrome / next) — webtoon doesn't need
        // left/right because native vertical scroll handles
        // navigation, but it still needs a touch path to the
        // chrome since auto-hide leaves no other way back on
        // mobile. `fixed inset-0` covers the viewport regardless
        // of how far the user has scrolled. `touch-action: pan-y`
        // tells the browser to keep handling vertical pans (so
        // native scroll continues to work) while still firing
        // click on a tap. `z-10` keeps it behind the marker
        // SVGs (z-20) and behind the chrome itself (z-30).
        // Hidden in marker mode so highlight-drag has unimpeded
        // access to the SVG overlays.
        <button
          type="button"
          // Pointer-only like <TapZones> — a full-viewport invisible
          // button as the page's first tab stop (with no focus ring)
          // was a keyboard trap; `t` toggles chrome for keyboards.
          tabIndex={-1}
          aria-hidden="true"
          aria-label="Toggle controls"
          onClick={onChromeZone}
          className="pointer-events-auto fixed inset-0 z-10 cursor-pointer touch-pan-y bg-transparent"
        />
      ) : null}
    </main>
  );
}

/** One page in `WebtoonView`. Wraps `<PageImage>` with a relative
 *  positioning context and mounts `<MarkerOverlay>` per page so saved
 *  highlights / notes render at their stored coordinates and the
 *  highlight-mode drag affordance works on whichever page the user
 *  starts dragging on. Without this, the webtoon view rendered bare
 *  `<PageImage>`s and had no marker surface at all — saved highlights
 *  didn't show and the highlight keybind / chrome menu silently did
 *  nothing because there was no overlay to drag on.
 */
const WebtoonPage = memo(function WebtoonPage({
  issueId,
  pageIndex,
  pageInfo,
  fitClass,
  eager,
}: {
  issueId: string;
  pageIndex: number;
  pageInfo?: PageInfo;
  fitClass: string;
  eager: boolean;
}) {
  const imgRef = useRef<HTMLImageElement>(null);
  const [naturalSize, setNaturalSize] = useState<{
    width: number;
    height: number;
  } | null>(null);
  // Reserve height before load via the img width/height attributes
  // (exact — the intrinsic ratio takes over on decode, so no shift
  // and no distortion risk). Pages without server-known dims keep
  // today's behavior; the scanner records dims for the common case.
  const dimensions =
    pageInfo?.image_width && pageInfo?.image_height
      ? { width: pageInfo.image_width, height: pageInfo.image_height }
      : undefined;
  // No own wrapper: the parent `WebtoonView` renders the stable
  // `data-page-idx` `relative` slot (so the observer never loses it
  // across the windowing swap), which also serves as `<MarkerOverlay>`'s
  // positioned ancestor.
  return (
    <>
      <PageImage
        src={`/issues/${issueId}/pages/${pageIndex}`}
        thumbSrc={`/issues/${issueId}/pages/${pageIndex}/thumb?variant=strip`}
        alt={`Page ${pageIndex + 1}`}
        fitClass={fitClass}
        loading={eager ? "eager" : "lazy"}
        imgRef={imgRef}
        onNaturalSize={(w, h) => setNaturalSize({ width: w, height: h })}
        dimensions={dimensions}
      />
      <MarkerOverlay
        issueId={issueId}
        pageIndex={pageIndex}
        imgRef={imgRef}
        naturalSize={naturalSize}
      />
    </>
  );
});

/** Pointer-only navigation zones. The whole surface is `aria-hidden`
 *  AND its buttons are `tabIndex={-1}`: focusable descendants inside
 *  an aria-hidden subtree are an outright ARIA violation, and
 *  keyboard users already page via the arrow-key keymap — three
 *  invisible full-height tab stops helped nobody. */
function TapZones({
  onLeft,
  onRight,
  onChrome,
  onCenterDoubleTap,
}: {
  onLeft: () => void;
  onRight: () => void;
  onChrome: () => void;
  /** When set (single-page only), the center zone distinguishes a single
   *  tap (chrome toggle, debounced) from a double tap (zoom at point). */
  onCenterDoubleTap?: (clientX: number, clientY: number) => void;
}) {
  // Single/double-click arbitration for the center zone: a single tap
  // fires `onChrome` after a short delay; a second tap inside the window
  // cancels it and fires `onCenterDoubleTap` instead. Without the delay,
  // a double-tap-to-zoom would also toggle the chrome on its first click.
  const centerTapTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const onCenterClick = (e: React.MouseEvent) => {
    if (!onCenterDoubleTap) {
      onChrome();
      return;
    }
    if (centerTapTimer.current) {
      // Second click → double tap: cancel the pending chrome toggle, zoom.
      clearTimeout(centerTapTimer.current);
      centerTapTimer.current = null;
      onCenterDoubleTap(e.clientX, e.clientY);
      return;
    }
    centerTapTimer.current = setTimeout(() => {
      centerTapTimer.current = null;
      onChrome();
    }, DOUBLE_TAP_MS);
  };
  return (
    <div className="absolute inset-0 z-10 grid grid-cols-3" aria-hidden="true">
      <button
        type="button"
        tabIndex={-1}
        className="cursor-w-resize"
        onClick={onLeft}
        aria-label="Left zone"
      />
      <button
        type="button"
        tabIndex={-1}
        onClick={onCenterClick}
        aria-label="Toggle controls"
      />
      <button
        type="button"
        tabIndex={-1}
        className="cursor-e-resize"
        onClick={onRight}
        aria-label="Right zone"
      />
    </div>
  );
}
