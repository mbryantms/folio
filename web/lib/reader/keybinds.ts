/**
 * Hotkey registry. Hosts both the **reader** scope (only active inside the
 * reader route, dispatched by `Reader.tsx`) and the **global** scope
 * (dispatched by the `<GlobalHotkeys />` component mounted in the root
 * layout). Each action maps to a default key spec.
 *
 * Key spec format: a `+`-joined chord, modifier prefixes followed by the
 * `KeyboardEvent.key` token. Example specs:
 *
 *   `"k"`            — bare letter key
 *   `"ArrowLeft"`    — special token
 *   `"Ctrl+k"`       — Ctrl chord
 *   `"Mod+,"`        — Cmd on macOS, Ctrl elsewhere (cross-platform default)
 *   `"Ctrl+Shift+f"` — multiple modifiers
 *
 * Recognized modifiers (case-insensitive): `Ctrl` / `Control`, `Shift`,
 * `Alt`, `Meta` / `Cmd`, and the platform-aware `Mod` alias.
 *
 * User preferences (persisted on the `users.keybinds` JSONB column via
 * `PATCH /me/preferences`) override the defaults action-by-action. Missing
 * actions fall back to defaults.
 */

export type KeybindScope = "global" | "reader";

export type KeybindAction =
  // global
  | "openSettings"
  | "openSearch"
  | "toggleSidebar"
  // reader
  | "nextPage"
  | "prevPage"
  | "firstPage"
  | "lastPage"
  | "toggleChrome"
  | "cycleFit"
  | "cycleViewMode"
  | "togglePageStrip"
  | "quitReader"
  // markers
  | "bookmarkPage"
  | "addNote"
  | "startHighlight"
  | "favoritePage"
  | "toggleMarkersHidden"
  | "nextBookmark"
  | "prevBookmark"
  | "nextIssue"
  | "prevIssue";

export const GLOBAL_KEYBIND_ACTIONS: readonly KeybindAction[] = [
  "openSettings",
  "openSearch",
  "toggleSidebar",
] as const;

export const READER_KEYBIND_ACTIONS: readonly KeybindAction[] = [
  "nextPage",
  "prevPage",
  "firstPage",
  "lastPage",
  "toggleChrome",
  "cycleFit",
  "cycleViewMode",
  "togglePageStrip",
  "quitReader",
  "bookmarkPage",
  "addNote",
  "startHighlight",
  "favoritePage",
  "toggleMarkersHidden",
  "nextBookmark",
  "prevBookmark",
  "nextIssue",
  "prevIssue",
] as const;

/** All actions in display order: global first, then reader. */
export const KEYBIND_ACTIONS: readonly KeybindAction[] = [
  ...GLOBAL_KEYBIND_ACTIONS,
  ...READER_KEYBIND_ACTIONS,
] as const;

export const KEYBIND_SCOPES: Record<KeybindAction, KeybindScope> = {
  openSettings: "global",
  openSearch: "global",
  toggleSidebar: "global",
  nextPage: "reader",
  prevPage: "reader",
  firstPage: "reader",
  lastPage: "reader",
  toggleChrome: "reader",
  cycleFit: "reader",
  cycleViewMode: "reader",
  togglePageStrip: "reader",
  quitReader: "reader",
  bookmarkPage: "reader",
  addNote: "reader",
  startHighlight: "reader",
  favoritePage: "reader",
  toggleMarkersHidden: "reader",
  nextBookmark: "reader",
  prevBookmark: "reader",
  nextIssue: "reader",
  prevIssue: "reader",
};

export const KEYBIND_LABELS: Record<KeybindAction, string> = {
  openSettings: "Open settings",
  openSearch: "Open search",
  toggleSidebar: "Toggle sidebar",
  nextPage: "Next page",
  prevPage: "Previous page",
  firstPage: "First page",
  lastPage: "Last page",
  toggleChrome: "Toggle controls",
  cycleFit: "Cycle fit mode",
  cycleViewMode: "Cycle view mode",
  togglePageStrip: "Toggle page strip",
  quitReader: "Exit reader",
  bookmarkPage: "Bookmark this page",
  addNote: "Add note",
  startHighlight: "Start highlight",
  favoritePage: "Favorite this page",
  toggleMarkersHidden: "Show / hide markers",
  nextBookmark: "Next bookmark",
  prevBookmark: "Previous bookmark",
  nextIssue: "Next issue",
  prevIssue: "Previous issue",
};

export const KEYBIND_DEFAULTS: Record<KeybindAction, string> = {
  // `Mod+` is Ctrl on Linux/Windows and Cmd on macOS. The user asked for
  // `Ctrl + K` / `Ctrl + ,` as the defaults; using Mod keeps the muscle
  // memory consistent on each platform.
  openSettings: "Mod+,",
  openSearch: "Mod+k",
  toggleSidebar: "Mod+b",
  nextPage: "ArrowRight",
  prevPage: "ArrowLeft",
  firstPage: "Home",
  lastPage: "End",
  toggleChrome: "t",
  cycleFit: "f",
  cycleViewMode: "d",
  togglePageStrip: "m",
  quitReader: "Escape",
  bookmarkPage: "b",
  addNote: "n",
  startHighlight: "h",
  // `s` for star — `f` is taken by `cycleFit`. Toggles the favorite
  // flag on the current page's bookmark, creating one if needed.
  favoritePage: "s",
  // `o` for overlays — toggles every marker overlay (regions, pins,
  // page-strip dots) without touching the saved data.
  toggleMarkersHidden: "o",
  // Vim-flavored bookmark navigation. `]` jumps forward to the next
  // bookmark-kind marker on a later page; `[` jumps back.
  nextBookmark: "]",
  prevBookmark: "[",
  // `n` is taken by `addNote` so we use the shifted variant; same
  // "N for next" muscle memory. Calls the next-up resolver and
  // navigates to whatever it picks (CBL > series).
  nextIssue: "Shift+N",
  // Symmetric with nextIssue; "P for prev." Pure sequential nav (no
  // finished-state filter) — back-by-one regardless of read state.
  prevIssue: "Shift+P",
};

/**
 * Merge user overrides into the defaults. Any action absent from `overrides`
 * keeps its default binding.
 */
export function resolveKeybinds(
  overrides: Record<string, string> | undefined | null,
): Record<KeybindAction, string> {
  const out = { ...KEYBIND_DEFAULTS };
  if (!overrides) return out;
  for (const action of KEYBIND_ACTIONS) {
    const v = overrides[action];
    if (typeof v === "string" && v.length > 0) {
      out[action] = v;
    }
  }
  return out;
}

/**
 * Should this keystroke be ignored by global / reader hotkey dispatchers?
 * Returns true when focus is inside an input-like surface so that typing
 * "b" in a search field doesn't fire the bookmark-page action. Uses
 * `closest()` so contentEditable + nested inputs inside custom components
 * are caught even when `e.target` isn't the input element itself.
 */
export function shouldSkipHotkey(e: KeyboardEvent): boolean {
  // Defensive: `HTMLElement` is undefined in the SSR / test (node) env;
  // there are no key events there anyway, so "don't skip" is the safe
  // default and lets node-env vitests exercise the function.
  if (typeof HTMLElement === "undefined") return false;
  const t = e.target;
  if (!(t instanceof HTMLElement)) return false;
  return !!t.closest(
    'input, textarea, select, [contenteditable="true"], [contenteditable=""]',
  );
}

// ─────────────────────────── chord parsing ──────────────────────────

interface Chord {
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
  meta: boolean;
  /** `KeyboardEvent.key` value, lowercased for single-character keys. */
  key: string;
}

function isMacPlatform(): boolean {
  if (typeof navigator === "undefined") return false;
  const ua = navigator.userAgent || "";
  return /Mac|iPhone|iPad|iPod/i.test(ua);
}

function normalizeKey(raw: string): string {
  if (raw === " ") return "Space";
  return raw.length === 1 ? raw.toLowerCase() : raw;
}

/**
 * Parse a key spec (`"Ctrl+k"`, `"Mod+,"`, `"ArrowLeft"`, `"Space"`, …)
 * into a `Chord`. The `Mod` token is expanded to Meta on macOS and Ctrl
 * elsewhere so a single default works cross-platform.
 */
export function parseChord(spec: string): Chord {
  const out: Chord = {
    ctrl: false,
    shift: false,
    alt: false,
    meta: false,
    key: "",
  };
  if (!spec) return out;
  // Special-case bare `+` so a literal "+" key works as a binding.
  if (spec === "+") {
    out.key = "+";
    return out;
  }
  const parts = spec.split("+").filter((s) => s.length > 0);
  const mac = isMacPlatform();
  for (const part of parts) {
    const tag = part.toLowerCase();
    switch (tag) {
      case "ctrl":
      case "control":
        out.ctrl = true;
        break;
      case "shift":
        out.shift = true;
        break;
      case "alt":
      case "option":
        out.alt = true;
        break;
      case "meta":
      case "cmd":
      case "command":
        out.meta = true;
        break;
      case "mod":
        if (mac) out.meta = true;
        else out.ctrl = true;
        break;
      default:
        out.key = normalizeKey(part);
        break;
    }
  }
  return out;
}

/** Snapshot the relevant fields of a `KeyboardEvent` into a `Chord`. */
function chordFromEvent(e: {
  key: string;
  ctrlKey?: boolean;
  shiftKey?: boolean;
  altKey?: boolean;
  metaKey?: boolean;
}): Chord {
  return {
    ctrl: !!e.ctrlKey,
    shift: !!e.shiftKey,
    alt: !!e.altKey,
    meta: !!e.metaKey,
    key: normalizeKey(e.key),
  };
}

function chordsMatch(a: Chord, b: Chord): boolean {
  return (
    a.ctrl === b.ctrl &&
    a.shift === b.shift &&
    a.alt === b.alt &&
    a.meta === b.meta &&
    a.key === b.key
  );
}

/**
 * Build a key spec from a captured `KeyboardEvent`. Modifier order is
 * stable (`Ctrl`, `Alt`, `Shift`, `Meta`) so two captures of the same
 * combo round-trip identically.
 */
export function comboFromEvent(e: KeyboardEvent): string {
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Meta");
  parts.push(normalizeKey(e.key));
  return parts.join("+");
}

/**
 * Render a key spec as a human-readable label. Modifier glyphs follow
 * platform convention: `⌘ ⌥ ⇧ ⌃` on macOS, full words on Linux/Windows.
 */
export function formatKey(spec: string): string {
  if (!spec) return "—";
  if (spec === "+") return "+";
  const parts = spec.split("+").filter((s) => s.length > 0);
  if (parts.length === 0) return "—";
  const mac = isMacPlatform();
  const out: string[] = [];
  for (const part of parts) {
    const tag = part.toLowerCase();
    if (tag === "ctrl" || tag === "control") out.push(mac ? "⌃" : "Ctrl");
    else if (tag === "alt" || tag === "option") out.push(mac ? "⌥" : "Alt");
    else if (tag === "shift") out.push(mac ? "⇧" : "Shift");
    else if (tag === "meta" || tag === "cmd" || tag === "command")
      out.push(mac ? "⌘" : "Meta");
    else if (tag === "mod") out.push(mac ? "⌘" : "Ctrl");
    else out.push(formatSingleKey(part));
  }
  return out.join(mac ? " " : " + ");
}

function formatSingleKey(key: string): string {
  switch (key) {
    case " ":
    case "Space":
      return "Space";
    case "ArrowLeft":
      return "←";
    case "ArrowRight":
      return "→";
    case "ArrowUp":
      return "↑";
    case "ArrowDown":
      return "↓";
    case "Escape":
      return "Esc";
    case "Enter":
      return "Enter";
    case "Tab":
      return "Tab";
    case "Backspace":
      return "Backspace";
    case "Home":
      return "Home";
    case "End":
      return "End";
    default:
      return key.length === 1 ? key.toUpperCase() : key;
  }
}

/**
 * Find an action whose binding collides with the candidate chord, ignoring
 * `excludeAction` (the row currently being edited). Used by the settings
 * editor to warn before two actions silently share a chord — `actionForKey`
 * resolves to the first match in registry order, so a collision would just
 * make the lower-priority action dead. Comparison is scope-blind: a global
 * chord that matches a reader chord still counts, because the global
 * dispatcher fires inside the reader too.
 */
export function findConflict(
  chord: string,
  excludeAction: KeybindAction,
  resolved: Record<KeybindAction, string>,
): KeybindAction | null {
  const target = parseChord(chord);
  if (!target.key) return null;
  for (const action of KEYBIND_ACTIONS) {
    if (action === excludeAction) continue;
    const other = parseChord(resolved[action]);
    if (chordsMatch(other, target)) return action;
  }
  return null;
}

/**
 * Reverse-lookup: given a `KeyboardEvent` (or a bare key string for the
 * legacy single-key call sites), which action does the bindings table
 * fire? Returns `null` for keys that aren't bound.
 */
export function actionForKey(
  input:
    | string
    | {
        key: string;
        ctrlKey?: boolean;
        shiftKey?: boolean;
        altKey?: boolean;
        metaKey?: boolean;
      },
  bindings: Record<KeybindAction, string>,
): KeybindAction | null {
  const target = chordFromEvent(
    typeof input === "string" ? { key: input } : input,
  );
  for (const action of KEYBIND_ACTIONS) {
    const want = parseChord(bindings[action]);
    if (chordsMatch(want, target)) return action;
  }
  return null;
}
