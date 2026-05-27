/**
 * Sidebar nav data. Icons are referenced by *string name* — not component
 * reference — because these structures are produced by Server Components
 * (`(admin)/layout.tsx`, `(settings)/layout.tsx`) and consumed by the client
 * `AdminSidebar`. React 19 / Next 15 cannot serialize function references
 * across the RSC boundary, so the client resolves the icon via
 * `nav-icons.tsx`.
 */

export type IconName =
  | "Activity"
  | "BarChart3"
  | "BookOpen"
  | "Cog"
  | "FileClock"
  | "FileText"
  | "Gauge"
  | "HeartPulse"
  | "Key"
  | "KeyRound"
  | "Keyboard"
  | "LayoutGrid"
  | "Library"
  | "ListChecks"
  | "Mail"
  | "Palette"
  | "PanelLeft"
  | "Search"
  | "Server"
  | "Shield"
  | "Sparkles"
  | "UserCog"
  | "Users";

/** Discriminator for live count badges rendered next to a nav label.
 *  Resolved client-side in `<AdminSidebar>` so the server layout can
 *  stay synchronous. Add a case here + a render branch in the
 *  sidebar when introducing a new dynamic-count surface. */
export type DynamicBadge = "metadata-unmatched";

export type NavItem = {
  href: string;
  label: string;
  icon: IconName;
  /** Marks pages that are placeholders shipped in M1 — drop this when M2–M6 fill them in. */
  placeholder?: boolean;
  /** Render a live count pill (e.g. unmatched-series total for the Metadata entry). */
  dynamicBadge?: DynamicBadge;
  /**
   * When true, only highlight this nav item when the current path
   * matches `href` exactly — no descendant match. Use for "index"
   * entries whose href is the layout root (e.g. `/admin` for the
   * Dashboard); without this flag, descendant-matching would light
   * Dashboard up on every admin sub-page since they're all `/admin/…`.
   */
  exact?: boolean;
};

export type NavSection = {
  label: string;
  items: NavItem[];
};

export function adminNav(localePrefix: string): NavSection[] {
  const p = (path: string) => `${localePrefix}/admin${path}`;
  return [
    {
      label: "Overview",
      items: [
        { href: p(""), label: "Dashboard", icon: "Gauge", exact: true },
        {
          href: p("/libraries"),
          label: "Libraries",
          icon: "Library",
        },
        {
          href: p("/findings"),
          label: "Findings",
          icon: "HeartPulse",
        },
      ],
    },
    {
      label: "People",
      items: [
        { href: p("/users"), label: "Users", icon: "Users" },
        // Audit-log entries are surfaced via the unified Activity
        // feed (filter chip = "Audit"). The dedicated /admin/audit
        // route still resolves — it redirects to
        // `/admin/activity?kinds=audit` — so bookmarks aren't broken,
        // but the sidebar collapses to one entry per data source.
        { href: p("/activity"), label: "Activity", icon: "Activity" },
      ],
    },
    {
      label: "Insights",
      items: [{ href: p("/stats"), label: "Stats", icon: "BarChart3" }],
    },
    {
      label: "Content",
      items: [
        // metadata-providers-1.0 M6 — provider/quota dashboard +
        // review queue + run history at `/admin/metadata`. The
        // `metadata-unmatched` badge surfaces the dashboard's
        // `series_unmatched` count so operators see backlog at a
        // glance without opening the page.
        {
          href: p("/metadata"),
          label: "Metadata",
          icon: "Sparkles",
          dynamicBadge: "metadata-unmatched",
        },
      ],
    },
    {
      label: "System",
      items: [
        { href: p("/server"), label: "Server info", icon: "Server" },
        { href: p("/auth"), label: "Auth config", icon: "Shield" },
        { href: p("/email"), label: "Email", icon: "Mail" },
        { href: p("/logs"), label: "Logs", icon: "ListChecks" },
        { href: p("/api-docs"), label: "API reference", icon: "FileText" },
      ],
    },
  ];
}

export function settingsNav(localePrefix: string): NavSection[] {
  const p = (path: string) => `${localePrefix}/settings${path}`;
  return [
    {
      label: "Reader",
      items: [
        { href: p("/reading"), label: "Reading", icon: "BookOpen" },
        { href: p("/keybinds"), label: "Key binds", icon: "Keyboard" },
        { href: p("/theme"), label: "Theme", icon: "Palette" },
        { href: p("/activity"), label: "Activity", icon: "Activity" },
      ],
    },
    {
      label: "Library",
      items: [
        { href: p("/views"), label: "Saved views", icon: "ListChecks" },
        { href: p("/pages"), label: "Pages", icon: "LayoutGrid" },
        { href: p("/navigation"), label: "Sidebar", icon: "PanelLeft" },
      ],
    },
    {
      label: "Account",
      items: [
        { href: p("/account"), label: "Account", icon: "UserCog" },
        {
          href: p("/api-tokens"),
          label: "App passwords",
          icon: "KeyRound",
        },
      ],
    },
  ];
}
