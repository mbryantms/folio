/**
 * Sidebar open state is persisted by the upstream shadcn `SidebarProvider`
 * (components/ui/sidebar.tsx, vendored verbatim) in the `sidebar_state`
 * cookie as "true" / "false". The server reads it before paint so the first
 * render already has the user's choice (no expanded→collapsed flash).
 * `comic_sidebar` is the pre-2026-10-26 cookie ("expanded" / "collapsed");
 * honour it until the new one has been written once.
 */
export const SIDEBAR_COOKIE = "sidebar_state";
export const LEGACY_SIDEBAR_COOKIE = "comic_sidebar";

export function parseSidebarOpen(
  raw: string | undefined,
  legacy?: string | undefined,
): boolean {
  if (raw === "true") return true;
  if (raw === "false") return false;
  return legacy !== "collapsed";
}
