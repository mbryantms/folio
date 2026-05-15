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
  | "Gauge"
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
  | "UserCog"
  | "Users";

export type NavItem = {
  href: string;
  label: string;
  icon: IconName;
  /** Marks pages that are placeholders shipped in M1 — drop this when M2–M6 fill them in. */
  placeholder?: boolean;
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
        { href: p(""), label: "Dashboard", icon: "Gauge" },
        {
          href: p("/libraries"),
          label: "Libraries",
          icon: "Library",
        },
      ],
    },
    {
      label: "People",
      items: [
        { href: p("/users"), label: "Users", icon: "Users" },
        { href: p("/audit"), label: "Audit log", icon: "FileClock" },
        { href: p("/activity"), label: "Activity", icon: "Activity" },
      ],
    },
    {
      label: "Insights",
      items: [{ href: p("/stats"), label: "Stats", icon: "BarChart3" }],
    },
    {
      label: "System",
      items: [
        { href: p("/server"), label: "Server info", icon: "Server" },
        { href: p("/auth"), label: "Auth config", icon: "Shield" },
        { href: p("/email"), label: "Email", icon: "Mail" },
        { href: p("/logs"), label: "Logs", icon: "ListChecks" },
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
