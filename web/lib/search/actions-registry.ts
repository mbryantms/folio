/** Static action registry for the search modal's command-palette
 *  surface (M6 of the search-improvements plan). Each entry is one
 *  thing a user can jump to or toggle — "Open Settings", "View audit
 *  log", "Manage libraries", etc.
 *
 *  Actions render alongside content-search hits in the modal. Typing
 *  a `>` prefix in the input hides content categories entirely so the
 *  modal becomes a pure command surface, à la Linear / VS Code.
 *
 *  The registry is intentionally static (not server-driven) because
 *  every action either navigates to a known route or toggles UI state
 *  — both of which are evergreen client concerns. Per-action route
 *  changes happen here, not on a roundtrip. */

import type { LucideIcon } from "lucide-react";
import {
  Activity,
  BarChart3,
  BookmarkCheck,
  BookOpen,
  BookOpenCheck,
  Compass,
  Copy,
  Download,
  FileText,
  FolderClosed,
  Gauge,
  History,
  Image as ImageIcon,
  Keyboard,
  LayoutDashboard,
  Library,
  ListChecks,
  ListOrdered,
  Lock,
  Mail,
  Map as MapIcon,
  Network,
  Palette,
  PanelLeft,
  PlusSquare,
  Radar,
  ScrollText,
  Search,
  Server,
  ShieldCheck,
  Sparkles,
  SunMoon,
  Tags,
  Users,
  Wrench,
} from "lucide-react";

import type { KeybindAction } from "@/lib/reader/keybinds";

/** Role gate. `undefined` means visible to every signed-in user. */
export type ActionRole = undefined | "admin";

/** Non-navigation commands. The registry stays static and pure: an entry
 *  names its command, and the surface's runner (`useSearchActions`)
 *  carries it out with the hooks it needs (theme, sidebar, the
 *  continue-reading query, the shortcuts sheet). */
export type ShellCommand =
  "continue-reading" | "toggle-theme" | "toggle-sidebar" | "show-shortcuts";

/** Display group. Rendered as section headings in command mode, in this
 *  order (`ACTION_GROUP_ORDER`). */
export type ActionGroup = "Actions" | "Library" | "Settings" | "Admin";
export const ACTION_GROUP_ORDER: readonly ActionGroup[] = [
  "Actions",
  "Library",
  "Settings",
  "Admin",
];

export interface SearchAction {
  /** Stable id — used as the React key + the "recent commands" cache
   *  key when we add it. Don't reuse across registry entries. */
  id: string;
  /** Display label. Doubles as the primary fuzzy-match target. */
  label: string;
  /** Short hint shown under the label (e.g. "Settings", "Admin",
   *  "Library"). Helps disambiguate when the label is short. */
  group: ActionGroup;
  /** Lucide icon to render in the thumb slot. */
  icon: LucideIcon;
  /** Destination route. Most actions are pure navigation. Exactly one of
   *  `href` / `command` is set (pinned by the registry test). */
  href?: string;
  /** A UI command instead of a route (theme, sidebar, …). */
  command?: ShellCommand;
  /** Keybind whose (user-resolved) chord the row shows as a hint. */
  keybind?: KeybindAction;
  /** A fixed key hint for shortcuts outside the rebindable registry. */
  keyHint?: string;
  /** Extra search terms a user might type. The matcher checks these
   *  in addition to `label` + `group`, so "OPDS" finds the API tokens
   *  page even though the label is "API tokens". */
  keywords?: readonly string[];
  /** Role gate. Filter at resolve time so non-admin users never see
   *  admin destinations. */
  role?: ActionRole;
}

/** Display order is "by relevance" within match-score; this array
 *  controls the deterministic tiebreaker so identical-score hits
 *  list in a stable order. Keep grouped by area (account → library →
 *  admin). */
export const SEARCH_ACTIONS: readonly SearchAction[] = [
  // ── Commands ──
  {
    // Resolved at run time from `/me/continue-reading`: the row shows the
    // issue it resumes and is hidden when nothing is in progress.
    id: "cmd-continue-reading",
    label: "Continue reading",
    group: "Actions",
    icon: BookOpenCheck,
    command: "continue-reading",
    keywords: ["resume", "last issue", "reader", "pick up"],
  },
  {
    id: "cmd-toggle-theme",
    label: "Toggle light / dark theme",
    group: "Actions",
    icon: SunMoon,
    command: "toggle-theme",
    keywords: ["theme", "dark mode", "light mode", "appearance"],
  },
  {
    id: "cmd-toggle-sidebar",
    label: "Toggle sidebar",
    group: "Actions",
    icon: PanelLeft,
    command: "toggle-sidebar",
    keybind: "toggleSidebar",
    keywords: ["navigation", "rail", "collapse", "expand"],
  },
  {
    id: "cmd-shortcuts",
    label: "Show keyboard shortcuts",
    group: "Actions",
    icon: Keyboard,
    command: "show-shortcuts",
    keyHint: "?",
    keywords: ["hotkeys", "cheat sheet", "keys", "help"],
  },
  {
    id: "cmd-tour",
    label: "Take the tour",
    group: "Actions",
    icon: MapIcon,
    href: "/?tour=1",
    keywords: ["onboarding", "help", "walkthrough", "guide", "intro"],
  },

  // ── Account / settings ──
  {
    id: "open-account",
    label: "Account settings",
    group: "Settings",
    icon: Users,
    href: "/settings/account",
    keywords: ["profile", "email", "password"],
  },
  {
    id: "open-reading-defaults",
    label: "Reading defaults",
    group: "Settings",
    icon: BookOpen,
    href: "/settings/reading",
    keywords: ["reader", "preferences", "page mode", "fit"],
  },
  {
    id: "open-keybinds",
    label: "Keyboard shortcuts",
    group: "Settings",
    icon: Keyboard,
    href: "/settings/keybinds",
    keywords: ["hotkeys", "bindings", "kbd"],
  },
  {
    id: "open-theme",
    label: "Theme & density",
    group: "Settings",
    icon: Palette,
    href: "/settings/theme",
    keywords: ["dark", "light", "accent", "compact", "comfortable"],
  },
  {
    id: "open-navigation",
    label: "Sidebar layout",
    group: "Settings",
    icon: LayoutDashboard,
    href: "/settings/sidebar",
    keywords: ["sidebar", "nav", "reorder"],
  },
  {
    id: "open-pages",
    label: "My pages",
    group: "Settings",
    icon: PlusSquare,
    href: "/settings/pages",
    keywords: ["dashboard", "custom page", "rails"],
  },
  {
    // The saved-views index lives under /settings, but it is a library
    // destination (every filter / CBL / collection view), so it is a
    // `go-*` entry in the Library group, not a Settings page.
    id: "go-views",
    label: "Saved views",
    group: "Library",
    icon: ListChecks,
    href: "/settings/views",
    keywords: [
      "views",
      "filters",
      "smart view",
      "cbl",
      "reading lists",
      "collections",
    ],
  },
  {
    id: "open-api-tokens",
    label: "App passwords & API tokens",
    group: "Settings",
    icon: Lock,
    href: "/settings/api-tokens",
    keywords: ["opds", "koreader", "tokens", "scoped"],
  },
  {
    id: "open-activity",
    label: "Reading stats",
    group: "Settings",
    icon: Activity,
    href: "/settings/activity",
    keywords: ["stats", "history", "minutes", "activity"],
  },
  {
    id: "open-downloads",
    label: "Downloads",
    group: "Settings",
    icon: Download,
    href: "/settings/downloads",
    keywords: ["offline", "saved issues", "storage"],
  },

  // ── Library / navigation ──
  {
    id: "go-home",
    label: "Home",
    group: "Library",
    icon: Sparkles,
    href: "/",
    keywords: ["dashboard", "rails"],
  },
  {
    id: "go-bookmarks",
    label: "Bookmarks",
    group: "Library",
    icon: BookmarkCheck,
    href: "/bookmarks",
    keywords: ["highlights", "notes", "favorites", "markers"],
  },
  {
    id: "go-collections",
    label: "Collections",
    group: "Library",
    icon: FolderClosed,
    href: "/collections",
    keywords: ["lists", "manual"],
  },
  {
    id: "go-want-to-read",
    label: "Want to Read",
    group: "Library",
    icon: BookOpen,
    href: "/views/want-to-read",
    keywords: ["queue", "wishlist", "wtr"],
  },
  {
    id: "go-browse",
    label: "Browse",
    group: "Library",
    icon: Compass,
    href: "/browse",
    keywords: [
      "publishers",
      "creators",
      "characters",
      "teams",
      "arcs",
      "locations",
    ],
  },
  {
    // `/log` is the reading log *and* its stats widgets (hero, heatmap,
    // top creators); `/settings/activity` is the per-user stats page.
    id: "go-log",
    label: "Reading log & stats",
    group: "Library",
    icon: ScrollText,
    href: "/log",
    keywords: ["history", "timeline", "stats", "activity", "log"],
  },
  {
    id: "go-search",
    label: "Open search page",
    group: "Library",
    icon: Search,
    href: "/search",
    keywords: ["browse", "results"],
  },

  // ── Admin (gated) ──
  {
    id: "admin-dashboard",
    label: "Admin overview",
    group: "Admin",
    icon: LayoutDashboard,
    href: "/admin",
    role: "admin",
    keywords: ["dashboard", "stats"],
  },
  {
    id: "admin-background-work",
    label: "Background work",
    group: "Admin",
    icon: ListChecks,
    href: "/admin/background-work",
    role: "admin",
    keywords: ["jobs", "queue", "scans", "workers", "thumbnails"],
  },
  {
    id: "admin-scan-dashboard",
    label: "Scan dashboard",
    group: "Admin",
    icon: Radar,
    href: "/admin/scan-dashboard",
    role: "admin",
    keywords: ["scans", "library scan", "progress"],
  },
  {
    id: "admin-findings",
    label: "Library events",
    group: "Admin",
    icon: ListOrdered,
    href: "/admin/findings",
    role: "admin",
    keywords: ["findings", "issues", "problems", "health"],
  },
  {
    id: "admin-metadata",
    label: "Metadata",
    group: "Admin",
    icon: Tags,
    href: "/admin/metadata",
    role: "admin",
    keywords: ["comicvine", "providers", "match", "tags"],
  },
  {
    id: "admin-duplicates",
    label: "Duplicates",
    group: "Admin",
    icon: Copy,
    href: "/admin/duplicates",
    role: "admin",
    keywords: ["dupes", "same issue", "merge"],
  },
  {
    id: "admin-starter-lists",
    label: "Starter lists",
    group: "Admin",
    icon: ListChecks,
    href: "/admin/starter-lists",
    role: "admin",
    keywords: ["reading order", "new users", "onboarding"],
  },
  {
    id: "admin-queue",
    label: "Queue",
    group: "Admin",
    icon: Gauge,
    href: "/admin/queue",
    role: "admin",
    keywords: ["redis", "workers", "jobs"],
  },
  {
    id: "admin-relationships",
    label: "Relationship suggestions",
    group: "Admin",
    icon: Network,
    href: "/admin/relationships",
    role: "admin",
    keywords: ["sequels", "series relationships", "review queue", "suggested"],
  },
  {
    id: "admin-libraries",
    label: "Manage libraries",
    group: "Admin",
    icon: Library,
    href: "/admin/libraries",
    role: "admin",
    // `library` (singular) is a deliberate keyword so the substring
    // match in `rankSearchActions` matches both `>library` and
    // `>libraries` — the bare `includes` check is "library"-shaped
    // and would otherwise miss the singular query against the plural
    // label.
    keywords: ["library", "scan", "roots", "paths"],
  },
  {
    id: "admin-users",
    label: "Users & access",
    group: "Admin",
    icon: Users,
    href: "/admin/users",
    role: "admin",
    keywords: ["accounts", "roles", "invite"],
  },
  {
    id: "admin-stats",
    label: "Site stats",
    group: "Admin",
    icon: BarChart3,
    href: "/admin/stats",
    role: "admin",
    keywords: ["analytics", "engagement"],
  },
  {
    id: "admin-audit",
    label: "Audit log",
    group: "Admin",
    icon: History,
    // The standalone /admin/audit page redirects into the unified
    // Activity feed; deep-link directly with the chip pre-applied so
    // the command-palette action lands on the same filtered view a
    // typed URL would.
    href: "/admin/activity?kinds=audit",
    role: "admin",
    keywords: ["actions", "trail"],
  },
  {
    id: "admin-logs",
    label: "Server logs",
    group: "Admin",
    icon: FileText,
    href: "/admin/logs",
    role: "admin",
    keywords: ["debug", "errors"],
  },
  {
    id: "admin-activity",
    label: "User activity",
    group: "Admin",
    icon: Activity,
    href: "/admin/activity",
    role: "admin",
    keywords: ["sessions", "reads"],
  },
  {
    id: "admin-server",
    label: "Server settings",
    group: "Admin",
    icon: Server,
    href: "/admin/server",
    role: "admin",
    keywords: ["config", "log level", "workers"],
  },
  {
    id: "admin-auth",
    label: "Authentication",
    group: "Admin",
    icon: ShieldCheck,
    href: "/admin/auth",
    role: "admin",
    keywords: ["oidc", "local", "sign-in"],
  },
  {
    id: "admin-email",
    label: "Email & SMTP",
    group: "Admin",
    icon: Mail,
    href: "/admin/email",
    role: "admin",
    keywords: ["smtp", "recovery"],
  },
  {
    id: "admin-ocr",
    label: "OCR queue",
    group: "Admin",
    icon: ImageIcon,
    href: "/admin/ocr",
    role: "admin",
    keywords: ["text detection", "scanner"],
  },
  {
    id: "admin-api-docs",
    label: "API docs",
    group: "Admin",
    icon: Wrench,
    href: "/admin/api-docs",
    role: "admin",
    keywords: ["openapi", "swagger", "reference"],
  },
] as const;

/** Pure ranking helper: given a needle, return the matching actions
 *  in best-first order. Scoring is intentionally simple — label
 *  match beats group match beats keyword match — but stable enough
 *  for a 30-entry registry. The `>` prefix on the input means "show
 *  only actions"; we accept the trimmed-of-`>` needle, NOT the raw
 *  string, so the caller controls when prefix mode is active.
 *
 *  Visible only if the role gate matches. Empty needle returns the
 *  registry in static order (filtered by role) so the action section
 *  in the modal isn't blank when the user opens `>` with no query. */
export function rankSearchActions(
  needle: string,
  role: string | undefined,
  opts: { exclude?: ReadonlySet<string> } = {},
): SearchAction[] {
  const lower = needle.trim().toLowerCase();
  const visible = visibleActions(role, opts.exclude);
  if (lower.length === 0) return [...visible];
  type Scored = { a: SearchAction; score: number; pos: number };
  const scored: Scored[] = [];
  for (const a of visible) {
    const labelLower = a.label.toLowerCase();
    const groupLower = a.group.toLowerCase();
    let score = -1;
    if (labelLower.startsWith(lower)) score = 100;
    else if (labelLower.includes(lower)) score = 70;
    else if (groupLower.startsWith(lower)) score = 50;
    else if (a.keywords?.some((k) => k.toLowerCase().includes(lower))) {
      score = 30;
    }
    if (score >= 0) {
      scored.push({ a, score, pos: scored.length });
    }
  }
  scored.sort((x, y) => y.score - x.score || x.pos - y.pos);
  return scored.map((s) => s.a);
}

/** Role- and context-filtered registry. `exclude` holds ids the current
 *  surface can't run right now (no sidebar in the reader, nothing in
 *  progress to continue). */
function visibleActions(
  role: string | undefined,
  exclude: ReadonlySet<string> | undefined,
): SearchAction[] {
  return SEARCH_ACTIONS.filter(
    (a) => (a.role !== "admin" || role === "admin") && !exclude?.has(a.id),
  );
}

/** What an empty input offers: the handful of commands people reach for
 *  most, so the palette is useful before the first keystroke. */
export const SUGGESTED_ACTION_IDS: readonly string[] = [
  "cmd-continue-reading",
  "go-log",
  "cmd-toggle-theme",
  "cmd-toggle-sidebar",
  "open-theme",
];

export function suggestedSearchActions(
  role: string | undefined,
  opts: { exclude?: ReadonlySet<string> } = {},
): SearchAction[] {
  const visible = new Map(
    visibleActions(role, opts.exclude).map((a) => [a.id, a]),
  );
  return SUGGESTED_ACTION_IDS.flatMap((id) => {
    const a = visible.get(id);
    return a ? [a] : [];
  });
}

/** Bucket ranked actions into display sections in `ACTION_GROUP_ORDER`,
 *  keeping the ranked order inside each section. Empty sections drop. */
export function groupSearchActions(
  actions: readonly SearchAction[],
): { group: ActionGroup; actions: SearchAction[] }[] {
  return ACTION_GROUP_ORDER.map((group) => ({
    group,
    actions: actions.filter((a) => a.group === group),
  })).filter((g) => g.actions.length > 0);
}

/** Detect the `>` prefix that flips the modal into pure command
 *  mode. Returns the input with the prefix + any leading whitespace
 *  stripped, plus a `commandMode` flag the modal uses to hide the
 *  content-search categories. */
export function parseCommandPrefix(raw: string): {
  needle: string;
  commandMode: boolean;
} {
  if (raw.startsWith(">")) {
    return { needle: raw.slice(1).trimStart(), commandMode: true };
  }
  return { needle: raw, commandMode: false };
}

const ACTION_CAP = 6;
export const SEARCH_ACTIONS_CAP = ACTION_CAP;
