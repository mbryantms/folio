"use client";

import * as React from "react";

import { useMe } from "@/lib/api/queries";
import {
  actionForKey,
  resolveKeybinds,
  shouldSkipHotkey,
} from "@/lib/reader/keybinds";

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
 * the `toggleSidebar` keybind (default `Mod+B`) to toggle.
 */
export function useSidebarState(initial: SidebarState) {
  const [state, setState] = React.useState<SidebarState>(initial);
  const me = useMe();

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

  // `toggleSidebar` is a global-scoped registry action so users can
  // rebind it under Settings → Keybinds. Default is `Mod+B` (VS Code /
  // Cursor / GitHub convention). The shared `shouldSkipHotkey` gate
  // keeps typing "b" in a search field from collapsing the shell.
  const bindings = React.useMemo(() => {
    const stored = (me.data?.keybinds ?? null) as Record<string, string> | null;
    return resolveKeybinds(stored);
  }, [me.data?.keybinds]);

  React.useEffect(() => {
    if (typeof window === "undefined") return;
    const handler = (e: KeyboardEvent) => {
      if (shouldSkipHotkey(e)) return;
      if (actionForKey(e, bindings) !== "toggleSidebar") return;
      e.preventDefault();
      setState((s) => (s === "expanded" ? "collapsed" : "expanded"));
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [bindings]);

  const toggle = React.useCallback(
    () => setState((s) => (s === "expanded" ? "collapsed" : "expanded")),
    [],
  );
  return { state, setState, toggle, collapsed: state === "collapsed" };
}
