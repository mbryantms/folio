import type { IconName, NavSection } from "@/components/admin/nav";
import type {
  LibraryView,
  SavedViewKind,
  SavedViewView,
} from "@/lib/api/types";

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

function railIconDefaultKey(kind: SavedViewKind): MainNavIconKey {
  switch (kind) {
    case "system":
      return "sparkles";
    case "filter_series":
      return "filter";
    case "cbl":
      return "list-ordered";
    case "collection":
      return "Folder";
  }
}

/** URL builder for a saved view sidebar entry. System rails get the
 *  kebab-case alias (matches the home rail link); user-authored views
 *  use their UUID. */
function viewHref(localePrefix: string, v: SavedViewView): string {
  if (v.kind === "system" && v.system_key) {
    return `${localePrefix}/views/${v.system_key.replace(/_/g, "-")}`;
  }
  return `${localePrefix}/views/${v.id}`;
}

export type MainNavSection = {
  label: string;
  items: MainNavItem[];
};

/**
 * Sidebar nav for the main reader app (library / series / issue routes).
 * Mirrors the admin sidebar's data-driven shape so we can reuse the
 * `IconName` + serialization conventions across both shells.
 *
 * Per-library entries are rendered as a single section so the user can
 * jump straight to one library without bouncing back to the home page.
 * Saved views the user has flagged `show_in_sidebar` get their own
 * "Saved views" section.
 */
export function mainNav(
  localePrefix: string,
  libraries: LibraryView[],
  sidebarViews: SavedViewView[] = [],
): MainNavSection[] {
  const sections: MainNavSection[] = [
    {
      label: "Browse",
      items: [
        { href: `${localePrefix}/`, label: "Home", icon: "Home" },
        // Favorites was dropped — favoriting only exists as a marker
        // sub-type (page/panel level) per the markers + collections
        // plan. The bookmarks index aggregates every marker kind, not
        // just `kind='bookmark'`.
        {
          href: `${localePrefix}/bookmarks`,
          label: "Bookmarks",
          icon: "Bookmark",
        },
        // Markers + Collections M3: the manual list surface lives at
        // `/collections`; the per-user system Want to Read collection
        // resolves through the kebab-case alias on the views detail
        // route (`/views/want-to-read` → system_key='want_to_read').
        {
          href: `${localePrefix}/collections`,
          label: "Collections",
          icon: "Folder",
        },
        {
          href: `${localePrefix}/views/want-to-read`,
          label: "Want to Read",
          icon: "ListPlus",
        },
      ],
    },
    {
      label: "Libraries",
      items: [
        {
          // `?library=all` lands on the metadata grid view (alphabetical
          // by default, full filter chrome). Bare `/` is reserved for
          // the saved-views Home page (M7).
          href: `${localePrefix}/?library=all`,
          label: "All Libraries",
          icon: "Library",
        },
        ...libraries.map<MainNavItem>((lib) => ({
          href: `${localePrefix}/?library=${lib.id}`,
          label: lib.name,
          icon: "Library",
        })),
      ],
    },
  ];
  if (sidebarViews.length > 0) {
    sections.push({
      label: "Saved views",
      items: sidebarViews.map<MainNavItem>((v) => ({
        href: viewHref(localePrefix, v),
        label: v.name,
        // Prefer the user's per-view icon override (stored on
        // `user_view_pins.icon`); the sidebar resolver falls back to the
        // kind-based default via the rail-icon registry if the key is
        // unknown.
        icon: v.icon ?? railIconDefaultKey(v.kind),
      })),
    });
  }
  return sections;
}

// Re-export for type compatibility with existing admin nav consumers.
export type { NavSection };
