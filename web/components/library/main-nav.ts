import type { IconName, NavSection } from "@/components/admin/nav";
import type { SidebarLayoutView } from "@/lib/api/types";

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
  | "LayoutGrid"
  | string;

export type MainNavItem = {
  href: string;
  label: string;
  icon: MainNavIconKey;
  placeholder?: boolean;
  /** Multi-page rails M6: present for `kind="page"` entries; carries
   *  the `user_page.id` so the sidebar's DnD reorder can call
   *  `POST /me/pages/reorder` without re-resolving slugs. */
  pageId?: string;
};

/** A run of consecutive non-header / non-spacer entries grouped under
 *  a server-supplied section label. Empty section labels (custom rows
 *  without a header before them) collapse to `null` and the renderer
 *  shows the items without a heading. */
export type MainNavSection = {
  /** Section title displayed at the top of this run. `null` when the
   *  run has no preceding header (custom items inserted at the very
   *  top of the layout). */
  label: string | null;
  items: MainNavItem[];
  /** `true` for "spacer" boundaries — the renderer adds vertical
   *  padding above this section without a label row. Mutually
   *  exclusive with a meaningful `label`. */
  isSpacer?: boolean;
};

/**
 * Build the data-driven sidebar from a resolved [`SidebarLayoutView`].
 * The server emits a flat ordered list of entries — built-ins,
 * libraries, saved views, pages, plus explicit header/spacer rows.
 * This function splits that list into [`MainNavSection`]s using the
 * server-supplied `kind="header"` rows as section boundaries.
 *
 * Anything not preceded by a header lands in a `label: null` group so
 * the renderer can show those items without inventing a title.
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
  let current: MainNavSection = { label: null, items: [] };

  const flush = () => {
    if (current.items.length > 0 || current.isSpacer || current.label) {
      sections.push(current);
    }
  };

  for (const entry of layout.entries) {
    if (!entry.visible) continue;
    if (entry.kind === "header") {
      flush();
      current = { label: entry.label, items: [] };
      continue;
    }
    if (entry.kind === "spacer") {
      flush();
      sections.push({ label: null, items: [], isSpacer: true });
      current = { label: null, items: [] };
      continue;
    }
    current.items.push({
      href: `${localePrefix}${entry.href}`,
      label: entry.label,
      icon: entry.icon as MainNavIconKey,
      pageId: entry.kind === "page" ? entry.ref_id : undefined,
    });
  }
  flush();
  // Drop empty-and-labelless sections (e.g. trailing flush with nothing).
  return sections.filter(
    (s) => s.isSpacer || s.items.length > 0 || (s.label && s.label.length > 0),
  );
}

// Re-export for type compatibility with existing admin nav consumers.
export type { NavSection };
