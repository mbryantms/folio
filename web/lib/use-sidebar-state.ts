"use client";

import * as React from "react";

import {
  SIDEBAR_COOKIE,
  SIDEBAR_COOKIE_MAX_AGE_SEC,
  type SidebarState,
} from "./sidebar-state";

/**
 * SSR-safe collapsible-sidebar hook, modeled on shadcn's sidebar block.
 *
 * Two states for desktop (mobile uses a Sheet, handled by the shell):
 *   - `expanded`  — full label + icon (w-64)
 *   - `collapsed` — icon-only with tooltips (w-14)
 *
 * Server-rendered initial state comes from the cookie via the layout's
 * `parseSidebarState(...)` call; the hook persists subsequent changes
 * back to the same cookie so a hard reload doesn't flash. Also wires
 * the `Mod+B` keyboard shortcut to toggle.
 */
export function useSidebarState(initial: SidebarState) {
  const [state, setState] = React.useState<SidebarState>(initial);

  // Persist on every change. `SameSite=Lax` keeps the cookie usable on
  // top-level navigation, which is what the layout's `cookies().get(...)`
  // reads from. `max-age` is a year — the user explicitly toggled it,
  // so respect that across sessions.
  React.useEffect(() => {
    if (typeof document === "undefined") return;
    document.cookie =
      `${SIDEBAR_COOKIE}=${state}; path=/; max-age=${SIDEBAR_COOKIE_MAX_AGE_SEC};` +
      ` SameSite=Lax`;
  }, [state]);

  // Keyboard shortcut. Modifier is Cmd on Mac, Ctrl elsewhere — matches
  // VS Code, Cursor, GitHub. Skip when an input has focus so typing "b"
  // in a search field doesn't jolt the layout.
  React.useEffect(() => {
    if (typeof window === "undefined") return;
    const handler = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return;
      if (e.shiftKey || e.altKey) return;
      if (e.key !== "b" && e.key !== "B") return;
      const target = e.target as HTMLElement | null;
      if (
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable)
      ) {
        return;
      }
      e.preventDefault();
      setState((s) => (s === "expanded" ? "collapsed" : "expanded"));
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  const toggle = React.useCallback(
    () => setState((s) => (s === "expanded" ? "collapsed" : "expanded")),
    [],
  );
  return { state, setState, toggle, collapsed: state === "collapsed" };
}
