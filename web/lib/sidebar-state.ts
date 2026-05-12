/**
 * Server-safe helpers for the collapsible sidebar.
 *
 * No `"use client"` here — layouts (server components) read the sidebar
 * cookie via `parseSidebarState` to render the correct width on first
 * paint. The matching React hook lives in `./use-sidebar-state.ts` and
 * imports these constants.
 */

export type SidebarState = "expanded" | "collapsed";

export const SIDEBAR_COOKIE = "comic_sidebar";
export const SIDEBAR_COOKIE_MAX_AGE_SEC = 60 * 60 * 24 * 365;

/** Parse a cookie value (or undefined) into a SidebarState. Defaults to
 *  `"expanded"` for unknown / missing values. */
export function parseSidebarState(raw: string | undefined): SidebarState {
  return raw === "collapsed" ? "collapsed" : "expanded";
}
