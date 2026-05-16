"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import { useGesture } from "@use-gesture/react";
import { toast } from "sonner";
import { useReaderStore, type FitMode } from "@/lib/reader/store";
import {
  detectDirection,
  detectViewMode,
  type Direction,
  type ViewMode,
} from "@/lib/reader/detect";
import {
  actionForKey,
  resolveKeybinds,
  shouldSkipHotkey,
} from "@/lib/reader/keybinds";
import { useReadingSession } from "@/lib/reader/session";
import {
  computeSpreadGroups,
  firstPageOfGroup,
  groupIndexForPage,
  type SpreadGroup,
} from "@/lib/reader/spreads";
import { useIssueMarkers, useNextUp, usePrevUp } from "@/lib/api/queries";
import { readerUrl } from "@/lib/urls";
import {
  useCreateMarker,
  useDeleteMarker,
  useUpdateMarker,
} from "@/lib/api/mutations";
import { markerToCreateReq } from "@/lib/markers/recreate";
import type { PageInfo } from "@/lib/api/types";
import { EndOfIssueCard } from "./EndOfIssueCard";
import { MarkerEditor } from "./MarkerEditor";
import { MarkerOverlay } from "./MarkerOverlay";
import { PageStrip } from "./PageStrip";
import { PageImage } from "./PageImage";
import { ReaderChrome } from "./ReaderChrome";

const PROGRESS_DEBOUNCE_MS = 300;
const PREFETCH_AHEAD = 2;
const SWIPE_THRESHOLD_PX = 30;

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
  userDefaultFitMode,
  userDefaultViewMode,
  userDefaultPageStrip,
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
  userDefaultFitMode: FitMode | null;
  userDefaultViewMode: ViewMode | null;
  userDefaultPageStrip: boolean;
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
  // Any page-level marker on the current page (bookmark / note) can
  // carry the favorite star. Picks the same row order as the chrome's
  // FavoriteToggleButton so keybind + UI stay in lockstep.
  const existingPageMarkerForFav = useMemo(
    () =>
      (issueMarkers.data?.items ?? []).find(
        (m) => m.page_index === currentPage && !m.region,
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
  const updateFavoriteMarker = useUpdateMarker(
    existingPageMarkerForFav?.id ?? "",
    issueId,
  );
  const deleteFavoriteMarker = useDeleteMarker(
    existingPageMarkerForFav?.id ?? "",
    issueId,
    { silent: true },
  );
  const toggleBookmark = useCallback(() => {
    if (existingPageBookmark) {
      const snapshot = existingPageBookmark;
      deleteExistingBookmark.mutate(undefined, {
        onSuccess: () =>
          toast.success(`Removed bookmark on page ${currentPage + 1}`, {
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
  const toggleFavorite = useCallback(() => {
    if (existingPageMarkerForFav) {
      if (existingPageMarkerForFav.is_favorite) {
        // If the marker only exists to carry the star, drop it.
        // Otherwise just clear the flag so the user's note stays.
        const hasOtherContent =
          (existingPageMarkerForFav.body &&
            existingPageMarkerForFav.body.length > 0) ||
          existingPageMarkerForFav.kind !== "bookmark";
        if (hasOtherContent) {
          updateFavoriteMarker.mutate(
            { is_favorite: false },
            {
              onSuccess: () =>
                toast.success(`Unstarred page ${currentPage + 1}`),
            },
          );
        } else {
          const snapshot = existingPageMarkerForFav;
          deleteFavoriteMarker.mutate(undefined, {
            onSuccess: () =>
              toast.success(`Unstarred page ${currentPage + 1}`, {
                action: {
                  label: "Undo",
                  onClick: () => createMarker.mutate(markerToCreateReq(snapshot)),
                },
              }),
          });
        }
      } else {
        updateFavoriteMarker.mutate(
          { is_favorite: true },
          { onSuccess: () => toast.success(`Starred page ${currentPage + 1}`) },
        );
      }
      return;
    }
    createMarker.mutate(
      {
        issue_id: issueId,
        page_index: currentPage,
        kind: "bookmark",
        is_favorite: true,
      },
      {
        onSuccess: () => toast.success(`Starred page ${currentPage + 1}`),
      },
    );
  }, [
    existingPageMarkerForFav,
    updateFavoriteMarker,
    deleteFavoriteMarker,
    createMarker,
    issueId,
    currentPage,
  ]);

  const initialDirection = useMemo<Direction>(
    () => detectDirection(manga, userDefaultDirection),
    [manga, userDefaultDirection],
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
    () => computeSpreadGroups(pages, { coverSolo }),
    [pages, coverSolo],
  );
  const currentGroupIdx = useMemo(
    () => groupIndexForPage(groups, currentPage),
    [groups, currentPage],
  );
  const visiblePages = useMemo<readonly number[]>(
    () => groups[currentGroupIdx] ?? [currentPage],
    [groups, currentGroupIdx, currentPage],
  );

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

  // Keyboard nav (§7.4). Bindings are resolved from user prefs (M4) so the
  // user's /settings/keybinds page can rebind every action. Spacebar always
  // pages forward — it's not user-rebindable since the OS already steals it
  // when a button has focus. The `?` shortcut-sheet toggle lives in
  // `<GlobalShortcutsSheet>` at the root so it works everywhere; the
  // sheet picks a Reader-first section ordering on this route.

  // `g g` leader state. Bare `g` arms the leader; a second `g` within
  // 500 ms fires firstPage. Stored in a ref so the listener can read
  // and clear it without forcing a re-render between strokes.
  const ggLeaderRef = useRef<number | null>(null);
  const GG_LEADER_MS = 500;

  useEffect(() => {
    const goFirstPage = () => {
      // Jump to the first spread group in double-page mode (cover-solo
      // when enabled), or page 0 otherwise. RTL doesn't flip here:
      // "first" is always page 0 regardless of reading direction.
      if (viewMode === "double" && groups.length > 0) {
        setPage(firstPageOfGroup(groups, 0));
      } else {
        setPage(0);
      }
    };
    const goLastPage = () => {
      if (viewMode === "double" && groups.length > 0) {
        setPage(firstPageOfGroup(groups, groups.length - 1));
      } else {
        setPage(totalPages - 1);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (shouldSkipHotkey(e)) return;
      // While the user is mid-selection or editing a pending marker,
      // swallow page-nav + most reader actions so a stray arrow / space
      // doesn't flip the page out from under the highlight. ESC still
      // exits via MarkerOverlay's capture-phase handler. The arrow-key
      // nudge in MarkerOverlay also runs in capture-phase, so it gets
      // first crack at the event before we ever reach this branch.
      const markerActive =
        markerModeForKeybinds !== "idle" || pendingMarkerForKeybinds !== null;
      if (markerActive) return;
      // Spacebar always advances (regardless of binding).
      if (e.key === " ") {
        e.preventDefault();
        goNext();
        return;
      }
      // Vim aliases (not in the keybind registry — they're convention,
      // not customization). `g g` → firstPage; `Shift+G` → lastPage.
      // Aliases survive a user rebinding `firstPage` / `lastPage` away
      // from Home / End.
      const noChord = !e.metaKey && !e.ctrlKey && !e.altKey;
      if (noChord && e.key === "g" && !e.shiftKey) {
        e.preventDefault();
        const now = Date.now();
        const lead = ggLeaderRef.current;
        if (lead != null && now - lead < GG_LEADER_MS) {
          ggLeaderRef.current = null;
          goFirstPage();
        } else {
          ggLeaderRef.current = now;
        }
        return;
      }
      // Any other key resets the leader so `g x` doesn't carry state.
      ggLeaderRef.current = null;
      if (noChord && (e.key === "G" || (e.key === "g" && e.shiftKey))) {
        e.preventDefault();
        goLastPage();
        return;
      }
      const action = actionForKey(e, bindings);
      if (!action) return;
      e.preventDefault();
      switch (action) {
        case "nextPage":
          if (
            direction === "rtl" &&
            (e.key === "ArrowRight" || e.key === "ArrowLeft")
          ) {
            // Arrow keys flip in RTL so the visual swipe-to-advance feels right.
            if (e.key === "ArrowRight") goPrev();
            else goNext();
          } else {
            goNext();
          }
          break;
        case "prevPage":
          if (
            direction === "rtl" &&
            (e.key === "ArrowRight" || e.key === "ArrowLeft")
          ) {
            if (e.key === "ArrowLeft") goNext();
            else goPrev();
          } else {
            goPrev();
          }
          break;
        case "firstPage":
          goFirstPage();
          break;
        case "lastPage":
          goLastPage();
          break;
        case "toggleChrome":
          toggleChrome();
          break;
        case "cycleFit":
          cycleFitMode();
          break;
        case "cycleViewMode":
          cycleViewMode();
          break;
        case "togglePageStrip":
          togglePageStrip();
          break;
        case "quitReader":
          // End-of-issue card swallows the first Esc — second one exits
          // as normal. Keeps "Stay here" reachable from the keyboard
          // without a separate keybind.
          if (showEndCard) {
            setShowEndCard(false);
            break;
          }
          router.push(exitUrl);
          break;
        case "bookmarkPage":
          toggleBookmark();
          break;
        case "addNote":
          beginMarkerEdit({
            kind: "note",
            page_index: currentPage,
            region: null,
            selection: null,
            body: "",
            is_favorite: false,
            tags: [],
          });
          break;
        case "startHighlight":
          setMarkerMode("select-rect");
          break;
        case "favoritePage":
          toggleFavorite();
          break;
        case "toggleMarkersHidden": {
          // Toggle returns the next value via the store; surface a
          // small toast so it's clear what just flipped (the visual
          // delta isn't always obvious if there are few markers on the
          // current page).
          toggleMarkersHidden();
          const nowHidden = useReaderStore.getState().markersHidden;
          toast.message(nowHidden ? "Markers hidden" : "Markers shown");
          break;
        }
        case "nextBookmark": {
          const pages = (issueMarkers.data?.items ?? [])
            .filter((m) => m.kind === "bookmark")
            .map((m) => m.page_index)
            .sort((a, b) => a - b);
          const next = pages.find((p) => p > currentPage);
          if (next != null) setPage(next);
          break;
        }
        case "prevBookmark": {
          const pages = (issueMarkers.data?.items ?? [])
            .filter((m) => m.kind === "bookmark")
            .map((m) => m.page_index)
            .sort((a, b) => a - b);
          // Walk backwards to find the largest page-index strictly less
          // than the current — `findLast` would be cleaner but the
          // tsconfig target lags the ES2023 lib.
          let prev: number | undefined;
          for (const p of pages) {
            if (p < currentPage) prev = p;
            else break;
          }
          if (prev != null) setPage(prev);
          break;
        }
        case "nextIssue":
          goNextIssue();
          break;
        case "prevIssue":
          goPrevIssue();
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    beginMarkerEdit,
    bindings,
    currentPage,
    cycleFitMode,
    cycleViewMode,
    direction,
    exitUrl,
    goNext,
    goNextIssue,
    goPrev,
    goPrevIssue,
    groups,
    issueId,
    issueMarkers.data,
    showEndCard,
    markerModeForKeybinds,
    pendingMarkerForKeybinds,
    router,
    setMarkerMode,
    setPage,
    toggleBookmark,
    toggleFavorite,
    toggleChrome,
    toggleMarkersHidden,
    togglePageStrip,
    totalPages,
    viewMode,
  ]);

  // Progress write — debounced so a fast page-flip doesn't hammer the server.
  const progressTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const csrfToken = useMemo(() => {
    if (typeof document === "undefined") return "";
    const m = document.cookie.match(/(?:^|;\s*)(?:__Host-)?comic_csrf=([^;]+)/);
    return m ? decodeURIComponent(m[1]) : "";
  }, []);

  useEffect(() => {
    if (!issueId) return;
    // Incognito skips the per-page progress write entirely. The
    // reading-session tracker is also disabled by the parent (it gates on
    // `activityTrackingEnabled`), so the server is never told the issue
    // was opened in this mode.
    if (incognito) return;
    if (progressTimer.current) clearTimeout(progressTimer.current);
    progressTimer.current = setTimeout(() => {
      // `finished` is sticky on the server: omit it on mid-issue writes
      // so a jump to a bookmark (or any non-last page) can't clear a
      // previously-marked-read issue. We only assert `finished: true`
      // when the user genuinely lands on the last page — explicit
      // "Mark as unread" goes through the mutation hook with its own
      // explicit `finished: false`.
      const onLastPage = currentPage >= totalPages - 1;
      const body: Record<string, unknown> = {
        issue_id: issueId,
        page: currentPage,
      };
      if (onLastPage) body.finished = true;
      void fetch("/api/progress", {
        method: "POST",
        credentials: "include",
        headers: {
          "Content-Type": "application/json",
          ...(csrfToken ? { "X-CSRF-Token": csrfToken } : {}),
        },
        body: JSON.stringify(body),
      }).catch(() => {
        /* best-effort; will retry on next page change */
      });
    }, PROGRESS_DEBOUNCE_MS);
    return () => {
      if (progressTimer.current) clearTimeout(progressTimer.current);
    };
  }, [csrfToken, currentPage, incognito, issueId, totalPages]);

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

  // Prefetch upcoming pages by seeding the browser HTTP cache. In
  // double-page mode we advance by group, not by page index, so we don't
  // waste requests on the back of a pair we just rendered.
  useEffect(() => {
    if (viewMode === "webtoon") return; // webtoon renders the whole stack
    if (viewMode === "double" && groups.length > 0) {
      for (let g = 1; g <= PREFETCH_AHEAD; g += 1) {
        const grp = groups[currentGroupIdx + g];
        if (!grp) break;
        for (const p of grp) {
          const img = new Image();
          img.src = `/issues/${issueId}/pages/${p}`;
        }
      }
      return;
    }
    for (let i = 1; i <= PREFETCH_AHEAD; i += 1) {
      const next = currentPage + i;
      if (next >= totalPages) break;
      const img = new Image();
      img.src = `/issues/${issueId}/pages/${next}`;
    }
  }, [currentPage, currentGroupIdx, groups, issueId, totalPages, viewMode]);

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

  // Gestures: drag (swipe) for page nav; pinch to cycle fit modes.
  // Webtoon mode skips swipe — vertical scroll is the native interaction there.
  const gestureRef = useRef<HTMLDivElement>(null);
  // Disable swipe/pinch while the user is drawing a highlight or has a
  // pending marker editor open: `@use-gesture/react` attaches native
  // pointer listeners on this container that fire BEFORE React's
  // synthetic handlers on the SVG overlay, so a horizontal drag in
  // highlight mode was being interpreted as a page-flip swipe.
  // Switching off the gesture entirely is cleaner than racing
  // `stopPropagation` on the native handlers.
  const gesturesEnabled =
    markerModeForKeybinds === "idle" && pendingMarkerForKeybinds === null;
  useGesture(
    {
      onDragEnd: ({ movement: [mx], cancel }) => {
        if (viewMode === "webtoon") {
          cancel();
          return;
        }
        if (Math.abs(mx) < SWIPE_THRESHOLD_PX) return;
        // Swipe-right (positive mx) → previous page in LTR, next in RTL.
        const swipeIsForward = direction === "rtl" ? mx > 0 : mx < 0;
        if (swipeIsForward) goNext();
        else goPrev();
      },
      onPinchEnd: ({ movement: [mScale], cancel }) => {
        if (viewMode === "webtoon") {
          cancel();
          return;
        }
        if (Math.abs(mScale) < 0.05) return;
        cycleFitMode();
      },
    },
    {
      target: gestureRef,
      drag: {
        axis: "x",
        filterTaps: true,
        threshold: 10,
        enabled: gesturesEnabled,
      },
      pinch: { enabled: gesturesEnabled },
      eventOptions: { passive: false },
    },
  );

  return (
    <div
      ref={gestureRef}
      className="min-h-screen touch-pan-y bg-black text-neutral-200"
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
  return (
    <main className="relative grid min-h-screen place-items-center">
      <div ref={wrapperRef} className="relative w-full">
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
}) {
  // RTL pairs render right-to-left; reuse `flex-row-reverse` to flip ordering.
  const flexClass =
    direction === "rtl" ? "flex flex-row-reverse" : "flex flex-row";
  const markerMode = useReaderStore((s) => s.markerMode);
  // In width-fit mode each pane is forced to share viewport width 50/50,
  // so the flex container itself needs to span the viewport. In other
  // modes it sizes to the natural image widths.
  const containerWidthClass = paneClass.includes("flex-1") ? "w-screen" : "";

  return (
    <main className="relative grid min-h-screen place-items-center">
      <div
        className={`${flexClass} ${containerWidthClass} items-center justify-center gap-1`}
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
    <div
      ref={wrapperRef}
      className={`relative align-top ${paneClass}`}
    >
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
