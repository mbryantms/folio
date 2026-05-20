"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
import { useReadingSession } from "@/lib/reader/session";
import {
  computeSpreadGroups,
  firstPageOfGroup,
  groupIndexForPage,
  type SpreadGroup,
} from "@/lib/reader/spreads";
import { useReaderProgressWrite } from "@/lib/reader/use-progress-write";
import { useReaderPrefetch } from "@/lib/reader/use-prefetch";
import { useReaderSwipe } from "@/lib/reader/use-swipe";
import { useReaderKeymap } from "@/lib/reader/use-keymap";
import { useIssueMarkers, useNextUp, usePrevUp } from "@/lib/api/queries";
import { readerUrl } from "@/lib/urls";
import {
  useCreateMarker,
  useDeleteMarker,
} from "@/lib/api/mutations";
import { markerToCreateReq } from "@/lib/markers/recreate";
import { UNDO_TOAST_DURATION_MS } from "@/lib/api/toast-strings";
import type { PageInfo } from "@/lib/api/types";
import { EndOfIssueCard } from "./EndOfIssueCard";
import {
  usePageTransition,
  type PageTransitionResult,
} from "@/lib/reader/use-page-transition";

import { MarkerEditor } from "./MarkerEditor";
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
  /** v0.3.44 — `'off' | 'slide' | null`. Null falls back to the
   *  built-in default of `'slide'`. */
  userDefaultPageAnimation: "off" | "slide" | null;
  userDefaultCoverSolo: boolean;
  userKeybinds: Record<string, string>;
  activityTrackingEnabled: boolean;
  /** When true, suppress per-page progress writes and the reading-session
   *  tracker for this read. Set by `?incognito=1` on the read page. */
  incognito?: boolean;
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
  const toggleMarkersHidden = useReaderStore((s) => s.toggleMarkersHidden);
  const beginMarkerEdit = useReaderStore((s) => s.beginMarkerEdit);
  const setMarkerMode = useReaderStore((s) => s.setMarkerMode);
  const markerModeForKeybinds = useReaderStore((s) => s.markerMode);
  const pendingMarkerForKeybinds = useReaderStore((s) => s.pendingMarker);

  // Per-issue marker fetch — drives the overlay, the page-strip dots,
  // and the bookmark toggle's "is this page already bookmarked?"
  // lookup. One round-trip, shared via TanStack Query cache.
  const issueMarkers = useIssueMarkers(issueId);
  const createMarker = useCreateMarker();
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
  const handleNaturalSize = useCallback(
    (page: number) => (width: number, height: number) => {
      pageNaturalSize.current.set(page, { width, height });
    },
    [],
  );

  // Bookmark-toggle helper: looks up an existing page-level bookmark
  // for `currentPage` and creates/deletes accordingly. Used by the
  // `b` keybind so it mirrors the chrome's bookmark button.
  const existingPageBookmark = useMemo(
    () =>
      (issueMarkers.data?.items ?? []).find(
        (m) =>
          m.kind === "bookmark" && m.page_index === currentPage && !m.region,
      ),
    [issueMarkers.data, currentPage],
  );
  // v0.3.44: favorites are their own kind, fully decoupled from
  // bookmarks. The `s` keybind toggles a `kind='favorite'` row at
  // the current page in lockstep with `FavoriteToggleButton`.
  const existingPageFavorite = useMemo(
    () =>
      (issueMarkers.data?.items ?? []).find(
        (m) =>
          m.kind === "favorite" && m.page_index === currentPage && !m.region,
      ),
    [issueMarkers.data, currentPage],
  );
  // The delete mutation hook needs an id at construction time; mint
  // it only when there's something to delete. `silent: true` so the
  // keybind-specific toast below ("Removed bookmark on page X") is the
  // only success signal — without it, the hook's generic "Removed"
  // would fire alongside, double-toasting one click.
  const deleteExistingBookmark = useDeleteMarker(
    existingPageBookmark?.id ?? "",
    issueId,
    { silent: true },
  );
  const deleteFavoriteMarker = useDeleteMarker(
    existingPageFavorite?.id ?? "",
    issueId,
    { silent: true },
  );
  const toggleBookmark = useCallback(() => {
    if (existingPageBookmark) {
      const snapshot = existingPageBookmark;
      deleteExistingBookmark.mutate(undefined, {
        onSuccess: () =>
          toast.success(`Removed bookmark on page ${currentPage + 1}`, {
            duration: UNDO_TOAST_DURATION_MS,
            action: {
              label: "Undo",
              onClick: () => createMarker.mutate(markerToCreateReq(snapshot)),
            },
          }),
      });
      return;
    }
    createMarker.mutate(
      {
        issue_id: issueId,
        page_index: currentPage,
        kind: "bookmark",
      },
      {
        onSuccess: () => toast.success(`Bookmarked page ${currentPage + 1}`),
      },
    );
  }, [
    existingPageBookmark,
    deleteExistingBookmark,
    createMarker,
    issueId,
    currentPage,
  ]);
  // `s` keybind — mirrors FavoriteToggleButton in the chrome.
  // v0.3.44: favorites are their own kind now; toggling creates or
  // deletes a `kind='favorite'` row at the current page. No more
  // is_favorite flag dance, no more bookmark side-effects.
  const toggleFavorite = useCallback(() => {
    if (existingPageFavorite) {
      const snapshot = existingPageFavorite;
      deleteFavoriteMarker.mutate(undefined, {
        onSuccess: () =>
          toast.success(`Unstarred page ${currentPage + 1}`, {
            duration: UNDO_TOAST_DURATION_MS,
            action: {
              label: "Undo",
              onClick: () => createMarker.mutate(markerToCreateReq(snapshot)),
            },
          }),
      });
      return;
    }
    createMarker.mutate(
      {
        issue_id: issueId,
        page_index: currentPage,
        kind: "favorite",
      },
      {
        onSuccess: () => toast.success(`Starred page ${currentPage + 1}`),
      },
    );
  }, [
    existingPageFavorite,
    deleteFavoriteMarker,
    createMarker,
    issueId,
    currentPage,
  ]);

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

  // v0.3.44 page-turn slide. Webtoon mode skips entirely
  // (continuous scroll is its own animation). The hook gates
  // on `enabled` AND `prefers-reduced-motion` internally.
  const animationPref = userDefaultPageAnimation ?? "slide";
  const pageTransition = usePageTransition({
    currentPage,
    direction,
    enabled: animationPref === "slide" && viewMode !== "webtoon",
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
    bookmarkPages,
    setPage,
    onNext: goNext,
    onPrev: goPrev,
    onNextIssue: goNextIssue,
    onPrevIssue: goPrevIssue,
    toggleChrome,
    cycleFitMode,
    cycleViewMode,
    togglePageStrip,
    toggleBookmark,
    toggleFavorite,
    toggleMarkersHidden: toggleMarkersHiddenWithToast,
    beginAddNote,
    beginHighlight,
    onQuitReader: handleQuitReader,
    onDismissEndCard: dismissEndCard,
  });

  useReaderProgressWrite({ issueId, currentPage, totalPages, incognito });

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
    trackingEnabled: activityTrackingEnabled,
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
  useEffect(() => {
    if (viewMode === "webtoon") return;
    const reduced =
      typeof window !== "undefined" &&
      window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    window.scrollTo({
      top: 0,
      left: 0,
      behavior: reduced ? "auto" : "instant",
    });
  }, [currentPage, viewMode]);

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
        ? "h-screen w-auto max-w-none"
        : "max-w-none w-auto h-auto";
  // Double-page panes need different wrapper sizing depending on fitMode.
  // In width mode each pane is forced to share the viewport row (flex-1
  // with min-w-0 so the inner img can shrink); in height/original modes
  // the pane is content-sized so the natural image width drives layout.
  const doublePaneClass =
    fitMode === "width" ? "flex-1 min-w-0" : "inline-block";

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
  useReaderSwipe({
    target: gestureRef,
    enabled:
      markerModeForKeybinds === "idle" && pendingMarkerForKeybinds === null,
    viewMode,
    direction,
    onNext: goNext,
    onPrev: goPrev,
  });

  return (
    <div
      ref={gestureRef}
      // `touch-action: pan-y pinch-zoom` keeps native vertical
      // scroll AND native pinch-zoom, while leaving the horizontal
      // axis available for the JS swipe-to-turn handler above.
      style={{ touchAction: "pan-y pinch-zoom" }}
      className="min-h-screen bg-black text-neutral-200"
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
            fitClass={fitClass}
          />
        ) : viewMode === "double" ? (
          <DoublePageView
            issueId={issueId}
            visiblePages={visiblePages}
            direction={direction}
            fitClass={fitClass}
            paneClass={doublePaneClass}
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
      <MarkerEditor issueId={issueId} pageNaturalSize={pageNaturalSize} />
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
}) {
  const wrapperRef = useRef<HTMLDivElement>(null);
  const markerMode = useReaderStore((s) => s.markerMode);
  const natural = pageNaturalSize.current?.get(currentPage) ?? null;
  // The wrapper is a block-level <div> rather than the previous
  // `inline-block` <span> so it has no inline-baseline descender that
  // could leave the overlay's `absolute inset-0` covering a slightly
  // taller area than the rendered img. SVG percent coords + pointer
  // math both anchor here, so visual rect placement matches the user's
  // drag exactly.
  // Track the rendered image element separately from the wrapper so
  // the marker overlay can align to the actual image bounds. At
  // fit=height the wrapper is full-width but the image is centered
  // and narrower — without this ref the overlay would cover (and
  // capture pointer coords from) the empty band on each side.
  const imgRef = useRef<HTMLImageElement>(null);
  const exitAnim =
    transition.exitDir === "left"
      ? "page-slide-out-to-left"
      : transition.exitDir === "right"
        ? "page-slide-out-to-right"
        : "";
  return (
    <main className="relative grid min-h-screen place-items-center">
      <div
        ref={wrapperRef}
        className="relative w-full overflow-hidden"
        data-testid="reader-page-wrapper"
      >
        {transition.prevPage !== null && (
          // v0.3.44: outgoing page slide layer. Absolute-positioned
          // overlay of the previous page that animates off-screen
          // in lockstep with the incoming page sliding in below.
          // Keyed on prevPage so the animation restarts on every
          // navigation (rapid keypresses cleanly cancel + retrigger).
          // No MarkerOverlay on the outgoing layer — a slide-off
          // marker would look broken; the overlay snaps to the new
          // page below.
          <div
            key={`prev-${transition.prevPage}`}
            aria-hidden="true"
            className={`pointer-events-none absolute inset-0 flex w-full justify-center ${exitAnim}`}
          >
            <img
              src={`/issues/${issueId}/pages/${transition.prevPage}`}
              alt=""
              className={`block ${fitClass}`}
              decoding="async"
            />
          </div>
        )}
        <div
          className={transition.enterAnimClass ?? undefined}
          key={`enter-${currentPage}`}
        >
          <PageImage
            key={`${issueId}-${currentPage}`}
            src={`/issues/${issueId}/pages/${currentPage}`}
            alt={`Page ${currentPage + 1}`}
            fitClass={fitClass}
            onNaturalSize={onNaturalSize(currentPage)}
            imgRef={imgRef}
          />
          <MarkerOverlay
            issueId={issueId}
            pageIndex={currentPage}
            wrapperRef={wrapperRef}
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
    <main className="relative grid min-h-screen place-items-center">
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
              paneClass={paneClass}
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
  const wrapperRef = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  return (
    <div ref={wrapperRef} className={`relative align-top ${paneClass}`}>
      <PageImage
        src={`/issues/${issueId}/pages/${page}`}
        alt={`Page ${page + 1}`}
        fitClass={fitClass}
        onNaturalSize={onNaturalSize}
        imgRef={imgRef}
      />
      <MarkerOverlay
        issueId={issueId}
        pageIndex={page}
        wrapperRef={wrapperRef}
        imgRef={imgRef}
        naturalSize={naturalSize}
      />
    </div>
  );
}

function WebtoonView({
  issueId,
  totalPages,
  fitClass,
}: {
  issueId: string;
  totalPages: number;
  fitClass: string;
}) {
  const currentPage = useReaderStore((s) => s.currentPage);
  const setPage = useReaderStore((s) => s.setPage);
  const containerRef = useRef<HTMLElement>(null);
  // Tracks the most recent page index that came from the scroll observer
  // (vs. an external setPage call e.g. from PageStrip). When `currentPage`
  // diverges from this we treat it as an external jump and scroll the
  // matching page into view.
  const lastObservedPage = useRef<number>(-1);

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
    el.scrollIntoView({
      behavior: reduced ? "auto" : "smooth",
      block: "start",
    });
  }, [currentPage]);

  return (
    <main
      ref={containerRef}
      className="flex min-h-screen flex-col items-center"
    >
      {Array.from({ length: totalPages }, (_, i) => (
        <div key={`${issueId}-${i}`} data-page-idx={i}>
          <PageImage
            src={`/issues/${issueId}/pages/${i}`}
            alt={`Page ${i + 1}`}
            fitClass={fitClass}
            loading={i < 3 ? "eager" : "lazy"}
          />
        </div>
      ))}
    </main>
  );
}

function TapZones({
  onLeft,
  onRight,
  onChrome,
}: {
  onLeft: () => void;
  onRight: () => void;
  onChrome: () => void;
}) {
  return (
    <div className="absolute inset-0 z-10 grid grid-cols-3" aria-hidden="true">
      <button
        type="button"
        className="cursor-w-resize"
        onClick={onLeft}
        aria-label="Left zone"
      />
      <button type="button" onClick={onChrome} aria-label="Toggle controls" />
      <button
        type="button"
        className="cursor-e-resize"
        onClick={onRight}
        aria-label="Right zone"
      />
    </div>
  );
}
