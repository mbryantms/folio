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
  FileText,
  FolderClosed,
  History,
  Image as ImageIcon,
  Keyboard,
  LayoutDashboard,
  Library,
  ListChecks,
  Lock,
  Mail,
  Palette,
  PlusSquare,
  ScrollText,
  Search,
  Server,
  ShieldCheck,
  Sparkles,
  Users,
  Wrench,
} from "lucide-react";

/** Role gate. `undefined` means visible to every signed-in user. */
export type ActionRole = undefined | "admin";

export interface SearchAction {
  /** Stable id — used as the React key + the "recent commands" cache
   *  key when we add it. Don't reuse across registry entries. */
  id: string;
  /** Display label. Doubles as the primary fuzzy-match target. */
  label: string;
  /** Short hint shown under the label (e.g. "Settings", "Admin",
   *  "Library"). Helps disambiguate when the label is short. */
  group: string;
  /** Lucide icon to render in the thumb slot. */
  icon: LucideIcon;
  /** Destination route. Most actions are pure navigation. */
  href: string;
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
    href: "/settings/navigation",
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
    id: "open-views",
    label: "Saved views",
    group: "Settings",
    icon: ListChecks,
    href: "/settings/views",
    keywords: ["filters", "smart view", "cbl"],
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
    label: "Reading activity",
    group: "Settings",
    icon: Activity,
    href: "/settings/activity",
    keywords: ["stats", "history", "minutes"],
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
    id: "go-reading-log",
    label: "Reading log",
    group: "Library",
    icon: ScrollText,
    href: "/log",
    keywords: ["history", "timeline"],
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
): SearchAction[] {
  const lower = needle.trim().toLowerCase();
  const visible = SEARCH_ACTIONS.filter(
    (a) => a.role !== "admin" || role === "admin",
  );
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
