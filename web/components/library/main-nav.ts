import type { IconName, NavSection } from "@/components/admin/nav";
import type { SidebarEntryView, SidebarLayoutView } from "@/lib/api/types";

/** Icon key — either a fixed nav-bar icon (`IconName` and the legacy
 *  aliases listed here) or a rail-icon registry key like `"sparkles"`,
 *  `"shield"`, `"list-ordered"`. The sidebar resolver consults both
 *  registries; unknown keys fall back to the default. */
export type MainNavIconKey =
  | IconName
  | "Bookmark"
  | "Heart"
  | "ListPlus"
  | "Folder"
  | "Sparkles"
  | "Calendar"
  | "Home"
  | string;

export type MainNavItem = {
  href: string;
  label: string;
  icon: MainNavIconKey;
  placeholder?: boolean;
};

export type MainNavSection = {
  label: string;
  items: MainNavItem[];
};

/** Section label for a run of entries that share the same `kind`. With
 *  navigation customization M1 the order is user-driven, so a user who
 *  drops a saved view between two built-ins will see three short
 *  sections instead of one — the alternative ("Browse" wrapping
 *  interleaved kinds) would be misleading. */
function sectionLabelForKind(kind: SidebarEntryView["kind"]): string {
  switch (kind) {
    case "builtin":
      return "Browse";
    case "library":
      return "Libraries";
    case "view":
      return "Saved views";
  }
}

/**
 * Build the data-driven sidebar from a resolved [`SidebarLayoutView`].
 * The layout is computed server-side (see `server::api::sidebar_layout`)
 * and already encodes order, visibility, label, icon, and href for every
 * entry — built-ins, libraries the user can see, and saved views. This
 * function just filters out hidden entries and groups consecutive
 * same-kind entries into sections so the existing
 * [`MainSidebar`](./MainSidebar.tsx) renderer can keep its
 * `MainNavSection[]` interface.
 *
 * `localePrefix` is prepended to every href — empty string for the
 * locale-neutral routing folio uses today; left in for symmetry with
 * the admin nav.
 */
export function mainNav(
  localePrefix: string,
  layout: SidebarLayoutView,
): MainNavSection[] {
  const sections: MainNavSection[] = [];
  let current: MainNavSection | null = null;
  let currentKind: SidebarEntryView["kind"] | null = null;

  for (const entry of layout.entries) {
    if (!entry.visible) continue;
    if (currentKind !== entry.kind) {
      current = { label: sectionLabelForKind(entry.kind), items: [] };
      sections.push(current);
      currentKind = entry.kind;
    }
    current!.items.push({
      href: `${localePrefix}${entry.href}`,
      label: entry.label,
      icon: entry.icon as MainNavIconKey,
    });
  }
  return sections;
}

// Re-export for type compatibility with existing admin nav consumers.
export type { NavSection };
