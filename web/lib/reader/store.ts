/**
 * Reader-local state. Cross-device sync arrives in Phase 4 (Automerge);
 * the network drain is a separate concern from this store.
 *
 * Per-series prefs (`fitMode`, `viewMode`, `direction`) are persisted in
 * `localStorage` so the choice survives across issues in the same series.
 * Other state is per-issue and is reset each time the reader mounts.
 */
import { create } from "zustand";
import type { Direction, ViewMode } from "@/lib/reader/detect";
import type {
  MarkerKind,
  MarkerRegion,
  MarkerSelection,
} from "@/lib/api/types";

export type FitMode = "width" | "height" | "original";

/** State the marker overlay is in.
 *
 *   - `idle` — overlay is read-only; rects render for hover/inspection.
 *   - `select-rect` — pointer-drag captures a region rect, opens the
 *     editor on release with `{ shape: 'rect' }`.
 *   - `select-text` — same drag, but the cropped pixels also feed
 *     `tesseract.js` to populate `selection.text`.
 *   - `select-image` — same drag, but the cropped pixels feed
 *     `crypto.subtle.digest` to populate `selection.image_hash`. */
export type MarkerMode =
  | "idle"
  | "select-rect"
  | "select-text"
  | "select-image";

/** Sketch of a marker the user is about to save. Lives off the store
 *  while the editor is open so the page-flip listener can clear it on
 *  abort. */
export type PendingMarker = {
  kind: MarkerKind;
  page_index: number;
  region: MarkerRegion | null;
  selection: MarkerSelection | null;
  body: string;
  is_favorite: boolean;
  tags: string[];
};
export const FIT_MODES: FitMode[] = ["width", "height", "original"];
export const VIEW_MODES: ViewMode[] = ["single", "double", "webtoon"];

type PersistedSlice =
  | "fitMode"
  | "viewMode"
  | "direction"
  | "coverSolo"
  | "markersHidden";

const storageKey = (slice: PersistedSlice, seriesId: string | null) =>
  `reader:${slice}:${seriesId ?? "_default"}`;

const isFitMode = (v: unknown): v is FitMode =>
  v === "width" || v === "height" || v === "original";
const isViewMode = (v: unknown): v is ViewMode =>
  v === "single" || v === "double" || v === "webtoon";
const isDirection = (v: unknown): v is Direction => v === "ltr" || v === "rtl";
const isBoolFlag = (v: unknown): v is "true" | "false" =>
  v === "true" || v === "false";

function load<T>(
  slice: PersistedSlice,
  seriesId: string | null,
  guard: (v: unknown) => v is T,
): T | null {
  if (typeof window === "undefined") return null;
  const raw = window.localStorage.getItem(storageKey(slice, seriesId));
  return guard(raw) ? raw : null;
}

function save(slice: PersistedSlice, seriesId: string | null, value: string) {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(storageKey(slice, seriesId), value);
}

export const loadFitMode = (seriesId: string | null): FitMode =>
  load("fitMode", seriesId, isFitMode) ?? "width";
export const loadViewMode = (seriesId: string | null): ViewMode | null =>
  load("viewMode", seriesId, isViewMode);
export const loadDirection = (seriesId: string | null): Direction | null =>
  load("direction", seriesId, isDirection);
export const loadCoverSolo = (seriesId: string | null): boolean | null => {
  const raw = load("coverSolo", seriesId, isBoolFlag);
  return raw === null ? null : raw === "true";
};
/** Markers-hidden flag is global (not per-series), so we ignore the
 *  passed seriesId and load against the `_default` bucket. Defaults to
 *  false when the key is absent so first-time users still see their
 *  markers. */
export const loadMarkersHidden = (): boolean => {
  const raw = load("markersHidden", null, isBoolFlag);
  return raw === "true";
};

export interface ReaderState {
  issueId: string;
  seriesId: string | null;
  currentPage: number; // 0-indexed
  totalPages: number;
  fitMode: FitMode;
  viewMode: ViewMode;
  direction: Direction;
  chromeVisible: boolean;
  /** When true, chrome auto-hides after a period of input idle. */
  chromeAutoHide: boolean;
  /** Held by interactive surfaces (open popover, focused input) to prevent
   * the auto-hide timer from collapsing chrome out from under the user. */
  chromePinned: boolean;
  pageStripVisible: boolean;
  /** Multiplier applied to a CSS `filter: brightness(...)` over the page
   * surface. 1 = unchanged. Range used by the slider is 0.5–1.5. */
  brightness: number;
  /** Sepia tone amount in [0,1]. 0 = off, 1 = full sepia. */
  sepia: number;
  /** Render the first page solo in double-page view, like the front cover
   * of a printed comic. Defaults to true; user-toggleable per-series. */
  coverSolo: boolean;
  /** When true, the reader hides every marker overlay (region rects,
   *  page-level pins, dots on the page strip) so the user can read
   *  without annotations on top. Persists across sessions globally
   *  via localStorage; toggled from the reader settings popover or
   *  the `o` keybind. Does not delete any marker data. */
  markersHidden: boolean;
  /** Marker overlay's selection mode. `idle` is the default; the
   *  reader chrome flips it into a select-* mode while the user picks
   *  a region. The overlay returns it to `idle` once the drag commits
   *  or aborts. */
  markerMode: MarkerMode;
  /** Sketch of a marker the user is composing. Populated by the
   *  overlay when the drag completes (for highlight/note) or by the
   *  reader chrome (for whole-page notes). `null` while no editor is
   *  open. */
  pendingMarker: PendingMarker | null;
  /** Existing marker id when the user opens the editor on a saved
   *  row (e.g. "edit note"). `null` for a brand-new pending marker. */
  editingMarkerId: string | null;

  setPage: (n: number) => void;
  nextPage: () => void;
  prevPage: () => void;
  cycleFitMode: () => void;
  setFitMode: (m: FitMode) => void;
  cycleViewMode: () => void;
  setViewMode: (m: ViewMode) => void;
  setDirection: (d: Direction) => void;
  toggleChrome: () => void;
  setChromeVisible: (v: boolean) => void;
  setChromeAutoHide: (v: boolean) => void;
  setChromePinned: (v: boolean) => void;
  togglePageStrip: () => void;
  setBrightness: (v: number) => void;
  setSepia: (v: number) => void;
  setCoverSolo: (v: boolean) => void;
  setMarkersHidden: (v: boolean) => void;
  toggleMarkersHidden: () => void;
  setMarkerMode: (mode: MarkerMode) => void;
  /** Start the marker editor. Pass `null` to close it (also resets
   *  `markerMode` to `idle` so escape-cancel paths converge here). */
  beginMarkerEdit: (
    pending: PendingMarker | null,
    existingId?: string | null,
  ) => void;
  init: (args: {
    issueId: string;
    seriesId: string | null;
    totalPages: number;
    initialPage: number;
    initialDirection: Direction;
    initialViewMode: ViewMode;
    /** M4: user-default fit mode; per-series localStorage still wins. */
    initialFitMode?: FitMode;
    /** M4: user preference for whether the page strip starts visible. */
    initialPageStripVisible?: boolean;
    /** Per-user default for cover-solo in double-page view. Per-series
     * localStorage still wins; built-in fallback is `true`. */
    initialCoverSolo?: boolean;
  }) => void;
}

export const useReaderStore = create<ReaderState>((set, get) => ({
  issueId: "",
  seriesId: null,
  currentPage: 0,
  totalPages: 0,
  fitMode: "width",
  viewMode: "single",
  direction: "ltr",
  chromeVisible: true,
  chromeAutoHide: true,
  chromePinned: false,
  pageStripVisible: false,
  brightness: 1,
  sepia: 0,
  coverSolo: true,
  markersHidden: false,
  markerMode: "idle",
  pendingMarker: null,
  editingMarkerId: null,

  setPage: (n: number) => {
    const total = get().totalPages;
    if (total === 0) return;
    const clamped = Math.max(0, Math.min(total - 1, n));
    set({ currentPage: clamped });
  },
  nextPage: () => get().setPage(get().currentPage + 1),
  prevPage: () => get().setPage(get().currentPage - 1),

  cycleFitMode: () => {
    const idx = FIT_MODES.indexOf(get().fitMode);
    const next = FIT_MODES[(idx + 1) % FIT_MODES.length];
    save("fitMode", get().seriesId, next);
    set({ fitMode: next });
  },
  setFitMode: (m: FitMode) => {
    save("fitMode", get().seriesId, m);
    set({ fitMode: m });
  },

  cycleViewMode: () => {
    const idx = VIEW_MODES.indexOf(get().viewMode);
    const next = VIEW_MODES[(idx + 1) % VIEW_MODES.length];
    save("viewMode", get().seriesId, next);
    set({ viewMode: next });
  },
  setViewMode: (m: ViewMode) => {
    save("viewMode", get().seriesId, m);
    set({ viewMode: m });
  },

  setDirection: (d: Direction) => {
    save("direction", get().seriesId, d);
    set({ direction: d });
  },

  // Chrome and the page strip travel together: showing chrome reveals the
  // strip, hiding it (auto-hide or manual toggle) fades both at the same
  // frequency. The `m` keybind hits `togglePageStrip` instead, so users can
  // still flip the strip independently when chrome is hidden.
  toggleChrome: () => {
    const next = !get().chromeVisible;
    set({ chromeVisible: next, pageStripVisible: next });
  },
  setChromeVisible: (v: boolean) =>
    set({ chromeVisible: v, pageStripVisible: v }),
  setChromeAutoHide: (v: boolean) => set({ chromeAutoHide: v }),
  setChromePinned: (v: boolean) => set({ chromePinned: v }),
  togglePageStrip: () => set({ pageStripVisible: !get().pageStripVisible }),

  // Brightness 0.5–1.5 keeps the slider sane (no pure black, no flooded
  // whites). Sepia 0–1 matches the CSS function's input domain.
  setBrightness: (v: number) => {
    const clamped = Math.max(0.5, Math.min(1.5, v));
    set({ brightness: clamped });
  },
  setSepia: (v: number) => {
    const clamped = Math.max(0, Math.min(1, v));
    set({ sepia: clamped });
  },
  setCoverSolo: (v: boolean) => {
    save("coverSolo", get().seriesId, v ? "true" : "false");
    set({ coverSolo: v });
  },
  setMarkersHidden: (v: boolean) => {
    // Persist globally (seriesId = null) so toggling once and switching
    // series doesn't unhide overlays. Matches how the user reasons
    // about the toggle: "I want a clean read across the whole library."
    save("markersHidden", null, v ? "true" : "false");
    set({ markersHidden: v });
  },
  toggleMarkersHidden: () => {
    const next = !get().markersHidden;
    save("markersHidden", null, next ? "true" : "false");
    set({ markersHidden: next });
  },

  setMarkerMode: (mode: MarkerMode) => set({ markerMode: mode }),
  beginMarkerEdit: (pending, existingId = null) =>
    set({
      pendingMarker: pending,
      editingMarkerId: existingId,
      // Closing the editor (pending = null) drops back to idle so
      // ESC-cancel and successful save funnel through one path.
      markerMode: pending ? get().markerMode : "idle",
    }),

  init: ({
    issueId,
    seriesId,
    totalPages,
    initialPage,
    initialDirection,
    initialViewMode,
    initialFitMode,
    initialPageStripVisible,
    initialCoverSolo,
  }) => {
    // Stored per-series choice always wins over the auto-detected default and
    // over the user's global preference. Resolution order:
    //   localStorage(per-series) > user default > built-in fallback.
    const stored = load("fitMode", seriesId, isFitMode);
    const fit: FitMode = stored ?? initialFitMode ?? "width";
    set({
      issueId,
      seriesId,
      totalPages,
      currentPage: Math.max(0, Math.min(totalPages - 1, initialPage)),
      fitMode: fit,
      viewMode: loadViewMode(seriesId) ?? initialViewMode,
      direction: loadDirection(seriesId) ?? initialDirection,
      chromeVisible: true,
      chromeAutoHide: get().chromeAutoHide,
      chromePinned: false,
      pageStripVisible: initialPageStripVisible ?? false,
      // Carry forward per-tab vision adjustments — they're a viewing-comfort
      // setting, not a per-issue choice. Reset only on full reload.
      brightness: get().brightness,
      sepia: get().sepia,
      // Resolution order: per-series localStorage > user default (from
      // MeView) > built-in fallback (true).
      coverSolo: loadCoverSolo(seriesId) ?? initialCoverSolo ?? true,
      // Marker overlay visibility is global (not per-series), so we
      // re-hydrate from the same `_default` key on every issue switch.
      markersHidden: loadMarkersHidden(),
      // Marker state never persists across issue changes.
      markerMode: "idle",
      pendingMarker: null,
      editingMarkerId: null,
    });
  },
}));
