/**
 * Reader-local state.
 *
 * Per-series prefs (`fitMode`, `viewMode`, `direction`, `coverSolo`) are
 * persisted in `localStorage` so the choice survives across issues in the
 * same series, and follow the account through
 * `/me/series/{id}/reader-prefs` (post-audit T4.4, see
 * `series-prefs-sync`): `init` mirrors the server row into the local
 * cache first, and every per-series write is pushed back. The global
 * display prefs (brightness, sepia, grayscale, invert, warmth, contrast,
 * zoom persist) follow the account the same way through
 * `/me/preferences` (`account-prefs-sync`, audit 2026-11 F7). Other state
 * is per-issue and is reset each time the reader mounts.
 */
import { create } from "zustand";
import type { Direction, ViewMode } from "@/lib/reader/detect";
import { primaryPointerIsCoarse } from "@/lib/reader/coarse-pointer";
import {
  DEFAULT_SCOPE,
  readerPrefGet,
  readerPrefSet,
} from "@/lib/reader/prefs-storage";
import { DOUBLE_TAP_MS } from "@/lib/reader/zoom";
import {
  type AccountPrefSlice,
  type AccountReaderPrefs,
  clearAccountAdoption,
  mirrorAccountPrefs,
  noteAccountAdoption,
  pushAccountPref,
} from "@/lib/reader/account-prefs-sync";
import {
  DEFAULT_FILTERS,
  clampBrightness,
  clampContrast,
  clampGrayscale,
  clampSepia,
  clampWarmth,
} from "@/lib/reader/filters";
import {
  mirrorServerPrefs,
  pendingAdoption,
  pushSeriesPref,
  pushSeriesPrefs,
  type SeriesPrefSlice,
} from "@/lib/reader/series-prefs-sync";
import type {
  MarkerKind,
  MarkerRegion,
  MarkerSelection,
  SeriesReaderPrefsView,
} from "@/lib/api/types";

/** `contain` = fit the whole page on screen (both axes, up- or down-scaled;
 *  audit UX-4). */
export type FitMode = "width" | "height" | "original" | "contain";

/** State the marker overlay is in.
 *
 *   - `idle` — overlay is read-only; rects render for hover/inspection.
 *   - `select-rect` — pointer-drag captures a region rect, opens the
 *     editor on release with `{ shape: 'rect' }`.
 *   - `select-text` — same drag, but the region is POSTed to
 *     `/api/me/issues/{id}/ocr` to populate `selection.text`.
 *   - `select-image` — same drag, but the cropped pixels feed
 *     `crypto.subtle.digest` to populate `selection.image_hash`. */
export type MarkerMode =
  "idle" | "select-rect" | "select-text" | "select-image";

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
// `contain` is appended (not slotted in) so the `f` cycle keeps its
// established width → height → original order for existing muscle memory.
export const FIT_MODES: FitMode[] = ["width", "height", "original", "contain"];
export const VIEW_MODES: ViewMode[] = ["single", "double", "webtoon"];

type PersistedSlice =
  | "fitMode"
  | "viewMode"
  | "direction"
  | "coverSolo"
  | "markersHidden"
  | "brightness"
  | "sepia"
  | "grayscale"
  | "invert"
  | "warmth"
  | "contrast"
  | "zoomPersist"
  | "chromeDismissed";

const isFitMode = (v: unknown): v is FitMode =>
  v === "width" || v === "height" || v === "original" || v === "contain";
const isViewMode = (v: unknown): v is ViewMode =>
  v === "single" || v === "double" || v === "webtoon";
const isDirection = (v: unknown): v is Direction => v === "ltr" || v === "rtl";
const isBoolFlag = (v: unknown): v is "true" | "false" =>
  v === "true" || v === "false";
const isNumericString = (v: unknown): v is string =>
  typeof v === "string" && v.trim() !== "" && Number.isFinite(Number(v));

/** `null` seriesId → the global `_default` scope (markersHidden /
 *  brightness / sepia, which persist library-wide). Per-series slices pass
 *  the real id and participate in the storage LRU. */
const scopeOf = (seriesId: string | null) => seriesId ?? DEFAULT_SCOPE;

function load<T>(
  slice: PersistedSlice,
  seriesId: string | null,
  guard: (v: unknown) => v is T,
): T | null {
  const raw = readerPrefGet(slice, scopeOf(seriesId));
  return guard(raw) ? raw : null;
}

const isSeriesSlice = (s: PersistedSlice): s is SeriesPrefSlice =>
  s === "fitMode" || s === "viewMode" || s === "direction" || s === "coverSolo";
const isAccountSlice = (s: PersistedSlice): s is AccountPrefSlice =>
  s === "brightness" ||
  s === "sepia" ||
  s === "grayscale" ||
  s === "invert" ||
  s === "warmth" ||
  s === "contrast" ||
  s === "zoomPersist";

function save(slice: PersistedSlice, seriesId: string | null, value: string) {
  readerPrefSet(slice, scopeOf(seriesId), value);
  // Per-series slices also follow the account (coalesced PUT; no-op
  // until the Reader installs the transport); so do the global display
  // slices (coalesced PATCH `/me/preferences`, `account-prefs-sync`).
  if (seriesId && isSeriesSlice(slice)) pushSeriesPref(seriesId, slice, value);
  else if (isAccountSlice(slice)) pushAccountPref(slice, value);
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

/** "Keep zoom between pages" (WP-4.2). Global like
 *  {@link loadMarkersHidden}: it's about how the reader likes to read, not
 *  about one series. Defaults to off — zoom is transient per page. */
export const loadZoomPersist = (): boolean =>
  load("zoomPersist", null, isBoolFlag) === "true";

/** Whether the user has ever hidden the reader chrome on purpose (a tap
 *  on the page, the `t` key, Esc). Global, like {@link loadMarkersHidden}. */
export const loadChromeDismissed = (): boolean =>
  load("chromeDismissed", null, isBoolFlag) === "true";

/**
 * Chrome visibility when the reader opens (audit N-R3 / AC-2, remediation
 * C5). Touch devices have no `t` key and the tap zones are invisible, so
 * on a coarse primary pointer the chrome starts **shown** until the user
 * hides it once — that first explicit hide is persisted and every later
 * open starts hidden, as before. Fine-pointer devices keep today's
 * pure-image first paint (the first-run overlay and the skip link say how
 * to reveal the controls). The 4 s idle auto-hide is not a choice and
 * never persists.
 */
export function initialChromeVisible(
  coarse: boolean = primaryPointerIsCoarse(),
): boolean {
  return coarse && !loadChromeDismissed();
}

// The tap-zone double-tap (zoom) toggles the chrome optimistically on the
// first tap and toggles it straight back on the second (`TapZones`), in
// either order — hide-then-show when the chrome was up, show-then-hide
// when it was down. So a hide counts as the user's choice only once it
// has outlived the double-tap window, and a hide that *follows* a toggle
// inside that window is the second half of a double-tap, not a choice.
// `dismissChrome` (Esc) records at once.
let pendingDismiss: ReturnType<typeof setTimeout> | null = null;
let lastChromeToggleAt = Number.NEGATIVE_INFINITY;
const CHROME_DISMISS_SETTLE_MS = DOUBLE_TAP_MS + 50;
function recordChromeDismissed() {
  save("chromeDismissed", null, "true");
}
function noteChromeToggle(hidden: boolean) {
  const now = Date.now();
  const insideDoubleTap = now - lastChromeToggleAt <= CHROME_DISMISS_SETTLE_MS;
  lastChromeToggleAt = now;
  if (pendingDismiss) {
    clearTimeout(pendingDismiss);
    pendingDismiss = null;
  }
  if (!hidden || insideDoubleTap) return;
  pendingDismiss = setTimeout(() => {
    pendingDismiss = null;
    recordChromeDismissed();
  }, CHROME_DISMISS_SETTLE_MS);
}

/** The colour filters persist globally (not per-series): they're an
 *  eye-comfort setting about the viewer's screen and room, not the
 *  content, so they carry across the whole library — same reasoning as
 *  {@link loadMarkersHidden}. Null when unset so the built-in default
 *  still applies. The clamps (`filters.ts`) are shared with the setters so
 *  a persisted value can never re-hydrate outside the slider's range. */
export const loadBrightness = (): number | null => {
  const raw = load("brightness", null, isNumericString);
  return raw === null ? null : clampBrightness(Number(raw));
};
export const loadSepia = (): number | null => {
  const raw = load("sepia", null, isNumericString);
  return raw === null ? null : clampSepia(Number(raw));
};
export const loadGrayscale = (): number | null => {
  const raw = load("grayscale", null, isNumericString);
  return raw === null ? null : clampGrayscale(Number(raw));
};
export const loadInvert = (): boolean | null => {
  const raw = load("invert", null, isBoolFlag);
  return raw === null ? null : raw === "true";
};
export const loadWarmth = (): number | null => {
  const raw = load("warmth", null, isNumericString);
  return raw === null ? null : clampWarmth(Number(raw));
};
export const loadContrast = (): number | null => {
  const raw = load("contrast", null, isNumericString);
  return raw === null ? null : clampContrast(Number(raw));
};

export interface ReaderState {
  issueId: string;
  /** Bumped by every `init`. `Reader` compares it with the value it saw
   *  when its issue last changed to know the store has been seeded for
   *  *that* issue — `issueId` alone can't tell a fresh seed from the
   *  stale state of a previous visit to the same issue. */
  initSeq: number;
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
  /** CSS `grayscale()` percent, 0–100 (F6). */
  grayscale: number;
  /** Night mode: the page surface is colour-inverted (F6). */
  invert: boolean;
  /** Colour temperature, −50 (cool) … 50 (warm); 0 = neutral (F6). */
  warmth: number;
  /** CSS `contrast()` percent, 50–150; 100 = unchanged (F6). */
  contrast: number;
  /** Render the first page solo in double-page view, like the front cover
   * of a printed comic. Defaults to true; user-toggleable per-series. */
  coverSolo: boolean;
  /** When true, the reader hides every marker overlay (region rects,
   *  page-level pins, dots on the page strip) so the user can read
   *  without annotations on top. Persists across sessions globally
   *  via localStorage; toggled from the reader settings popover or
   *  the `o` keybind. Does not delete any marker data. */
  markersHidden: boolean;
  /** When true, transform zoom carries across page turns (landing on the
   *  new page's reading-order start corner) instead of resetting to 1×.
   *  Global, persisted in localStorage; toggled from reader settings. */
  zoomPersist: boolean;
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
  /** Hide the chrome as an explicit choice (Esc): recorded at once, so
   *  later opens on a touch device start hidden. */
  dismissChrome: () => void;
  setChromeVisible: (v: boolean) => void;
  setChromeAutoHide: (v: boolean) => void;
  setChromePinned: (v: boolean) => void;
  togglePageStrip: () => void;
  setBrightness: (v: number) => void;
  setSepia: (v: number) => void;
  setGrayscale: (v: number) => void;
  setInvert: (v: boolean) => void;
  setWarmth: (v: number) => void;
  setContrast: (v: number) => void;
  /** Every colour filter back to its default (one coalesced account
   *  write). */
  resetFilters: () => void;
  setCoverSolo: (v: boolean) => void;
  setMarkersHidden: (v: boolean) => void;
  toggleMarkersHidden: () => void;
  setZoomPersist: (v: boolean) => void;
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
    /** The account's per-series overrides (`/me/series/{id}/reader-prefs`),
     *  mirrored into the local cache before resolution so they beat a
     *  stale local value. `null` = the fetch failed (offline): the local
     *  cache stands. Omit in tests that want pure local resolution. */
    serverSeriesPrefs?: SeriesReaderPrefsView | null;
    /** The account's global display prefs (`/auth/me`, F7): set slices
     *  are mirrored into the local cache before resolution; unset ones
     *  leave the local value in charge and are adopted on the first
     *  change. `null` / omitted = unknown (offline, tests): local stands
     *  and nothing is adopted. */
    serverAccountPrefs?: AccountReaderPrefs | null;
  }) => void;
}

export const useReaderStore = create<ReaderState>((set, get) => ({
  issueId: "",
  initSeq: 0,
  seriesId: null,
  currentPage: 0,
  totalPages: 0,
  fitMode: "width",
  viewMode: "single",
  direction: "ltr",
  // Resolved per mount in `init` (`initialChromeVisible`): hidden on fine
  // pointers, shown on touch until the user hides it once.
  chromeVisible: false,
  chromeAutoHide: true,
  chromePinned: false,
  pageStripVisible: false,
  brightness: DEFAULT_FILTERS.brightness,
  sepia: DEFAULT_FILTERS.sepia,
  grayscale: DEFAULT_FILTERS.grayscale,
  invert: DEFAULT_FILTERS.invert,
  warmth: DEFAULT_FILTERS.warmth,
  contrast: DEFAULT_FILTERS.contrast,
  coverSolo: true,
  markersHidden: false,
  zoomPersist: false,
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
    const next = FIT_MODES[(idx + 1) % FIT_MODES.length]!;
    save("fitMode", get().seriesId, next);
    set({ fitMode: next });
  },
  setFitMode: (m: FitMode) => {
    save("fitMode", get().seriesId, m);
    set({ fitMode: m });
  },

  cycleViewMode: () => {
    const idx = VIEW_MODES.indexOf(get().viewMode);
    const next = VIEW_MODES[(idx + 1) % VIEW_MODES.length]!;
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
  // A toggle is always the user's doing (tap on the page, the `t` key, the
  // chrome button): hiding records the choice once it has settled past
  // the double-tap window (`noteChromeToggle`). `setChromeVisible` is the
  // programmatic path (auto-hide, the skip link) and records nothing.
  toggleChrome: () => {
    const next = !get().chromeVisible;
    noteChromeToggle(!next);
    set({ chromeVisible: next, pageStripVisible: next });
  },
  // Esc closes whatever is up; it is a chrome choice only when the chrome
  // itself was showing (not when only the `m` page strip was).
  dismissChrome: () => {
    if (get().chromeVisible) {
      if (pendingDismiss) clearTimeout(pendingDismiss);
      pendingDismiss = null;
      recordChromeDismissed();
    }
    set({ chromeVisible: false, pageStripVisible: false });
  },
  setChromeVisible: (v: boolean) =>
    set({ chromeVisible: v, pageStripVisible: v }),
  setChromeAutoHide: (v: boolean) => set({ chromeAutoHide: v }),
  setChromePinned: (v: boolean) => set({ chromePinned: v }),
  togglePageStrip: () => set({ pageStripVisible: !get().pageStripVisible }),

  // Brightness 0.5–1.5 keeps the slider sane (no pure black, no flooded
  // whites). Sepia 0–1 matches the CSS function's input domain. Every
  // filter persists globally so the comfort setting survives reloads and
  // issue switches (resolved back in `init`) and follows the account.
  setBrightness: (v: number) => {
    const clamped = clampBrightness(v);
    save("brightness", null, String(clamped));
    set({ brightness: clamped });
  },
  setSepia: (v: number) => {
    const clamped = clampSepia(v);
    save("sepia", null, String(clamped));
    set({ sepia: clamped });
  },
  setGrayscale: (v: number) => {
    const clamped = clampGrayscale(v);
    save("grayscale", null, String(clamped));
    set({ grayscale: clamped });
  },
  setInvert: (v: boolean) => {
    save("invert", null, v ? "true" : "false");
    set({ invert: v });
  },
  setWarmth: (v: number) => {
    const clamped = clampWarmth(v);
    save("warmth", null, String(clamped));
    set({ warmth: clamped });
  },
  setContrast: (v: number) => {
    const clamped = clampContrast(v);
    save("contrast", null, String(clamped));
    set({ contrast: clamped });
  },
  resetFilters: () => {
    const d = DEFAULT_FILTERS;
    save("brightness", null, String(d.brightness));
    save("sepia", null, String(d.sepia));
    save("grayscale", null, String(d.grayscale));
    save("invert", null, d.invert ? "true" : "false");
    save("warmth", null, String(d.warmth));
    save("contrast", null, String(d.contrast));
    set({ ...d });
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

  setZoomPersist: (v: boolean) => {
    save("zoomPersist", null, v ? "true" : "false");
    set({ zoomPersist: v });
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
    serverSeriesPrefs,
    serverAccountPrefs,
  }) => {
    // Account overrides first (post-audit T4.4): the server row overwrites
    // the local cache, and local overrides the server has never seen are
    // adopted once so they follow the account from here on.
    if (seriesId && serverSeriesPrefs) {
      mirrorServerPrefs(seriesId, serverSeriesPrefs);
      const adopt = pendingAdoption(seriesId, serverSeriesPrefs);
      if (adopt) pushSeriesPrefs(seriesId, adopt);
    }
    // Global display prefs (F7): the account's set slices win over the
    // local cache; slices it has never set keep the local value and ride
    // along with the first change (write-through, not on read).
    if (serverAccountPrefs) {
      mirrorAccountPrefs(serverAccountPrefs);
      noteAccountAdoption(serverAccountPrefs);
    } else {
      clearAccountAdoption();
    }
    // Stored per-series choice always wins over the auto-detected default and
    // over the user's global preference. Resolution order:
    //   server(per-series) > localStorage(per-series) > user default > built-in.
    const stored = load("fitMode", seriesId, isFitMode);
    const fit: FitMode = stored ?? initialFitMode ?? "width";
    set({
      initSeq: get().initSeq + 1,
      issueId,
      seriesId,
      totalPages,
      currentPage: Math.max(0, Math.min(totalPages - 1, initialPage)),
      fitMode: fit,
      viewMode: loadViewMode(seriesId) ?? initialViewMode,
      direction: loadDirection(seriesId) ?? initialDirection,
      // Fine pointers open with chrome hidden so each new issue lands on
      // a pure-image surface (a tap on the page or `t` reveals it). Touch
      // devices start with it shown until the user has hidden it once —
      // see `initialChromeVisible`.
      chromeVisible: initialChromeVisible(),
      chromeAutoHide: get().chromeAutoHide,
      chromePinned: false,
      pageStripVisible: initialPageStripVisible ?? false,
      // Vision adjustments are a viewing-comfort setting, not a per-issue
      // choice. Persisted globally, so re-hydrate from localStorage on
      // every issue switch / reload (falling back to the live value, then
      // the built-in default).
      brightness: loadBrightness() ?? get().brightness,
      sepia: loadSepia() ?? get().sepia,
      grayscale: loadGrayscale() ?? get().grayscale,
      invert: loadInvert() ?? get().invert,
      warmth: loadWarmth() ?? get().warmth,
      contrast: loadContrast() ?? get().contrast,
      // Resolution order: per-series localStorage > user default (from
      // MeView) > built-in fallback (true).
      coverSolo: loadCoverSolo(seriesId) ?? initialCoverSolo ?? true,
      // Marker overlay visibility is global (not per-series), so we
      // re-hydrate from the same `_default` key on every issue switch.
      markersHidden: loadMarkersHidden(),
      zoomPersist: loadZoomPersist(),
      // Marker state never persists across issue changes.
      markerMode: "idle",
      pendingMarker: null,
      editingMarkerId: null,
    });
  },
}));
