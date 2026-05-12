/**
 * Icon registry for saved-view rails. Drives the picker on the home page
 * (and the sidebar by extension — both surfaces resolve the same key
 * through `iconFor`).
 *
 * Keys are stored on `user_view_pins.icon` as free text. The server
 * accepts anything ≤64 chars; unknown values silently fall back to the
 * kind-based default below.
 *
 * To add icons: import the Lucide component, add an entry to `RAIL_ICONS`
 * with a stable kebab-case `key`, a short `label`, and a `category`.
 * The picker renders entries grouped by category so users can scan them
 * by theme.
 */

import type { LucideIcon } from "lucide-react";
import {
  Album,
  Archive,
  Award,
  Book,
  BookOpen,
  Bookmark,
  Box,
  Compass,
  Crown,
  Diamond,
  Drama,
  Eye,
  FileText,
  Filter,
  Flame,
  Folder,
  Ghost,
  Glasses,
  Hash,
  Heart,
  Hexagon,
  Layers,
  Library,
  ListOrdered,
  Newspaper,
  PenLine,
  Rocket,
  Scroll,
  Shield,
  Skull,
  Sparkles,
  Star,
  Sword,
  Swords,
  Tag,
  Tags,
  Trophy,
  Wand2,
  Zap,
} from "lucide-react";

import type { SavedViewKind, SavedViewView } from "@/lib/api/types";

export type RailIconCategory =
  | "reading"
  | "heroes"
  | "action"
  | "filter"
  | "decorative";

export type RailIconEntry = {
  key: string;
  label: string;
  category: RailIconCategory;
  Icon: LucideIcon;
};

export const RAIL_ICON_CATEGORY_LABELS: Record<RailIconCategory, string> = {
  reading: "Reading & books",
  heroes: "Heroes & villains",
  action: "Action & energy",
  filter: "Filter & organize",
  decorative: "Decorative",
};

/** ~38 curated icons that pair well with the dark/amber theme. The
 *  comic-themed ones (drama masks, skull, swords, shield, crown, ghost)
 *  sit in the "Heroes & villains" group — Lucide doesn't have actual
 *  superhero crests, but these read closest to comic tropes. */
export const RAIL_ICONS: ReadonlyArray<RailIconEntry> = [
  // Reading & books
  { key: "book-open", label: "Open book", category: "reading", Icon: BookOpen },
  { key: "book", label: "Book", category: "reading", Icon: Book },
  { key: "library", label: "Library", category: "reading", Icon: Library },
  { key: "bookmark", label: "Bookmark", category: "reading", Icon: Bookmark },
  {
    key: "newspaper",
    label: "Newspaper",
    category: "reading",
    Icon: Newspaper,
  },
  { key: "album", label: "Album", category: "reading", Icon: Album },
  { key: "scroll", label: "Scroll", category: "reading", Icon: Scroll },
  { key: "file-text", label: "Document", category: "reading", Icon: FileText },
  {
    key: "glasses",
    label: "Reading glasses",
    category: "reading",
    Icon: Glasses,
  },
  { key: "pen-line", label: "Pen", category: "reading", Icon: PenLine },

  // Heroes & villains (comic-themed)
  { key: "shield", label: "Shield", category: "heroes", Icon: Shield },
  { key: "sword", label: "Sword", category: "heroes", Icon: Sword },
  { key: "swords", label: "Crossed swords", category: "heroes", Icon: Swords },
  { key: "crown", label: "Crown", category: "heroes", Icon: Crown },
  { key: "trophy", label: "Trophy", category: "heroes", Icon: Trophy },
  { key: "skull", label: "Skull", category: "heroes", Icon: Skull },
  { key: "ghost", label: "Ghost", category: "heroes", Icon: Ghost },
  { key: "drama", label: "Drama masks", category: "heroes", Icon: Drama },
  { key: "eye", label: "Eye", category: "heroes", Icon: Eye },
  { key: "award", label: "Award", category: "heroes", Icon: Award },

  // Action & energy
  { key: "sparkles", label: "Sparkles", category: "action", Icon: Sparkles },
  { key: "zap", label: "Lightning bolt", category: "action", Icon: Zap },
  { key: "flame", label: "Flame", category: "action", Icon: Flame },
  { key: "rocket", label: "Rocket", category: "action", Icon: Rocket },
  { key: "star", label: "Star", category: "action", Icon: Star },
  { key: "heart", label: "Heart", category: "action", Icon: Heart },
  { key: "compass", label: "Compass", category: "action", Icon: Compass },
  { key: "wand", label: "Magic wand", category: "action", Icon: Wand2 },

  // Filter & organize
  { key: "filter", label: "Filter", category: "filter", Icon: Filter },
  {
    key: "list-ordered",
    label: "Numbered list",
    category: "filter",
    Icon: ListOrdered,
  },
  { key: "folder", label: "Folder", category: "filter", Icon: Folder },
  { key: "tag", label: "Tag", category: "filter", Icon: Tag },
  { key: "tags", label: "Tags", category: "filter", Icon: Tags },
  { key: "archive", label: "Archive", category: "filter", Icon: Archive },
  { key: "hash", label: "Hash", category: "filter", Icon: Hash },

  // Decorative
  { key: "layers", label: "Layers", category: "decorative", Icon: Layers },
  { key: "box", label: "Box", category: "decorative", Icon: Box },
  { key: "hexagon", label: "Hexagon", category: "decorative", Icon: Hexagon },
  { key: "diamond", label: "Diamond", category: "decorative", Icon: Diamond },
];

const RAIL_ICONS_BY_KEY: ReadonlyMap<string, RailIconEntry> = new Map(
  RAIL_ICONS.map((entry) => [entry.key, entry]),
);

/** Default icon key per kind — used when the user hasn't picked one and
 *  also as the value the picker highlights as "reset to default." */
export function defaultIconKeyForKind(kind: SavedViewKind): string {
  switch (kind) {
    case "system":
      return "sparkles";
    case "filter_series":
      return "filter";
    case "cbl":
      return "list-ordered";
    case "collection":
      return "folder";
  }
}

/** Resolve the Lucide component to render for a given view, honoring the
 *  per-user override and falling back to the kind default. Always returns
 *  a component (never null) so callers can render unconditionally.
 *
 *  Returns the entry (key + label + Icon) instead of just the component
 *  so the picker can flag the active one and so screen readers get a
 *  meaningful label via `aria-label`. */
export function railIconFor(view: SavedViewView): RailIconEntry {
  if (view.icon) {
    const hit = RAIL_ICONS_BY_KEY.get(view.icon);
    if (hit) return hit;
  }
  return (
    RAIL_ICONS_BY_KEY.get(defaultIconKeyForKind(view.kind)) ??
    // The kind defaults are always in the registry; the fallback is
    // here for type safety only.
    RAIL_ICONS[0]
  );
}

export function railIconByKey(
  key: string | null | undefined,
): RailIconEntry | null {
  if (!key) return null;
  return RAIL_ICONS_BY_KEY.get(key) ?? null;
}
