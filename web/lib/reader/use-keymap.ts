import { useEffect, useRef } from "react";
import type { Direction, ViewMode } from "@/lib/reader/detect";
import {
  actionForKey,
  shouldSkipHotkey,
  type KeybindAction,
} from "@/lib/reader/keybinds";
import { firstPageOfGroup, type SpreadGroup } from "@/lib/reader/spreads";

const GG_LEADER_MS = 500;

/**
 * Reader keyboard map (§7.4). Listens on `window` so the user can
 * page-flip without focusing any specific element. Bindings come
 * from the caller (already resolved against user prefs via
 * `resolveKeybinds`).
 *
 * Special-cases preserved from the inline implementation:
 *
 *   - Spacebar is hardcoded to `nextPage` (the OS steals it from
 *     focused buttons, so it isn't user-rebindable).
 *   - `g g` within `GG_LEADER_MS` → first-page; `Shift+G` → last-page.
 *     Vim conventions, intentionally outside the rebind registry.
 *   - In RTL, arrow keys flip direction so the visual swipe-to-advance
 *     feels right.
 *   - `markerActive` short-circuits everything — arrow keys nudging
 *     a highlight don't fall through to page-flip; the marker
 *     overlay's capture-phase handler ran first.
 *   - `quitReader` while the end-of-issue card is open dismisses the
 *     card; second Esc exits the reader via `onQuitReader`.
 */
export function useReaderKeymap(opts: {
  bindings: Record<KeybindAction, string>;
  viewMode: ViewMode;
  direction: Direction;
  groups: ReadonlyArray<SpreadGroup>;
  totalPages: number;
  currentPage: number;
  /** Reader-actions are suppressed when the user is mid-marker-edit
   *  (highlight-rect drag in progress, or pending-marker editor
   *  open). Esc still exits via the marker overlay's capture-phase
   *  handler. */
  markerActive: boolean;
  /** When true, Esc dismisses the end-card and *does not* exit. */
  showEndCard: boolean;
  /** Precomputed sorted page indices of bookmark markers. */
  bookmarkPages: readonly number[];
  setPage: (p: number) => void;
  onNext: () => void;
  onPrev: () => void;
  onNextIssue: () => void;
  onPrevIssue: () => void;
  toggleChrome: () => void;
  cycleFitMode: () => void;
  cycleViewMode: () => void;
  zoomIn: () => void;
  zoomOut: () => void;
  zoomReset: () => void;
  togglePageStrip: () => void;
  toggleBookmark: () => void;
  toggleFavorite: () => void;
  toggleMarkersHidden: () => void;
  beginAddNote: () => void;
  beginHighlight: () => void;
  beginCaptureText: () => void;
  /** Called for `quitReader` when no end-card override fires. The
   *  caller decides router push, exit URL, etc. */
  onQuitReader: () => void;
  /** Called for `quitReader` while `showEndCard` is true. The hook
   *  itself does not own end-card visibility state. */
  onDismissEndCard: () => void;
}): void {
  const {
    bindings,
    viewMode,
    direction,
    groups,
    totalPages,
    currentPage,
    markerActive,
    showEndCard,
    bookmarkPages,
    setPage,
    onNext,
    onPrev,
    onNextIssue,
    onPrevIssue,
    toggleChrome,
    cycleFitMode,
    cycleViewMode,
    zoomIn,
    zoomOut,
    zoomReset,
    togglePageStrip,
    toggleBookmark,
    toggleFavorite,
    toggleMarkersHidden,
    beginAddNote,
    beginHighlight,
    beginCaptureText,
    onQuitReader,
    onDismissEndCard,
  } = opts;
  // `g g` leader state. Bare `g` arms; a second `g` within
  // GG_LEADER_MS fires firstPage. Ref so the listener can mutate
  // without forcing a re-render between strokes.
  const ggLeaderRef = useRef<number | null>(null);
  useEffect(() => {
    const goFirstPage = () => {
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
      if (markerActive) return;
      if (e.key === " ") {
        e.preventDefault();
        onNext();
        return;
      }
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
            // RTL flip — visual right-arrow goes backwards.
            if (e.key === "ArrowRight") onPrev();
            else onNext();
          } else {
            onNext();
          }
          break;
        case "prevPage":
          if (
            direction === "rtl" &&
            (e.key === "ArrowRight" || e.key === "ArrowLeft")
          ) {
            if (e.key === "ArrowLeft") onNext();
            else onPrev();
          } else {
            onPrev();
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
        case "zoomIn":
          zoomIn();
          break;
        case "zoomOut":
          zoomOut();
          break;
        case "zoomReset":
          zoomReset();
          break;
        case "togglePageStrip":
          togglePageStrip();
          break;
        case "quitReader":
          if (showEndCard) {
            onDismissEndCard();
            break;
          }
          onQuitReader();
          break;
        case "bookmarkPage":
          toggleBookmark();
          break;
        case "addNote":
          beginAddNote();
          break;
        case "startHighlight":
          beginHighlight();
          break;
        case "captureText":
          beginCaptureText();
          break;
        case "favoritePage":
          toggleFavorite();
          break;
        case "toggleMarkersHidden":
          toggleMarkersHidden();
          break;
        case "nextBookmark": {
          const next = bookmarkPages.find((p) => p > currentPage);
          if (next != null) setPage(next);
          break;
        }
        case "prevBookmark": {
          // `findLast` would be cleaner but the tsconfig target lags
          // the ES2023 lib.
          let prev: number | undefined;
          for (const p of bookmarkPages) {
            if (p < currentPage) prev = p;
            else break;
          }
          if (prev != null) setPage(prev);
          break;
        }
        case "nextIssue":
          onNextIssue();
          break;
        case "prevIssue":
          onPrevIssue();
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    beginAddNote,
    beginCaptureText,
    beginHighlight,
    bindings,
    bookmarkPages,
    currentPage,
    cycleFitMode,
    cycleViewMode,
    direction,
    groups,
    markerActive,
    onDismissEndCard,
    onNext,
    onNextIssue,
    onPrev,
    onPrevIssue,
    onQuitReader,
    setPage,
    showEndCard,
    toggleBookmark,
    toggleChrome,
    toggleFavorite,
    toggleMarkersHidden,
    togglePageStrip,
    totalPages,
    viewMode,
    zoomIn,
    zoomOut,
    zoomReset,
  ]);
}
