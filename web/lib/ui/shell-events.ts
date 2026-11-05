/**
 * Window-level requests to the app shell from surfaces mounted outside it.
 *
 * The search palette lives at the root layout, above the shells'
 * `SidebarProvider`, so it can't call `useSidebar()`. It dispatches a
 * request instead; the shell's provider (`AppSidebarProvider`) listens.
 * With no shell mounted (the reader) nothing listens and the request is a
 * no-op, which is why the palette also hides the command there.
 */
export const SIDEBAR_TOGGLE_EVENT = "folio:toggle-sidebar";

/** Same idea for the keyboard-shortcuts sheet: its provider wraps the
 *  page tree *inside* the search provider, so the palette can't reach its
 *  context. `GlobalShortcutsSheet` listens. */
export const SHORTCUTS_OPEN_EVENT = "folio:open-shortcuts";

export function requestShortcutsSheet(): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new Event(SHORTCUTS_OPEN_EVENT));
}

export function requestSidebarToggle(): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new Event(SIDEBAR_TOGGLE_EVENT));
}
