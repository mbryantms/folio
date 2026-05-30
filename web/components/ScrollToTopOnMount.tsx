"use client";

import { useEffect } from "react";

/**
 * Scrolls the window to the top once, on mount.
 *
 * Used on the bare-home rails view. Home (`/`), the library grid
 * (`/?library=…`), and search (`/?q=…`) all share the `/` pathname, and the
 * App Router only auto-resets scroll on a *pathname* change — a query-only
 * change reuses the page and preserves scroll. So arriving at Home from the
 * grid or search (same pathname) would otherwise inherit that view's scroll
 * position. The rails branch mounts a fresh subtree on that transition, so a
 * mount-time reset fires exactly then — bringing Home in line with how every
 * distinct-pathname page already behaves. It does NOT fire on in-grid filter
 * changes (the grid stays mounted), so filtering keeps its scroll position.
 */
export function ScrollToTopOnMount() {
  useEffect(() => {
    window.scrollTo(0, 0);
  }, []);
  return null;
}
