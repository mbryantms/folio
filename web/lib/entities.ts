import type { EntityKindPath } from "@/lib/api/queries";
import type { EntitySlugs } from "@/lib/api/types";

/** Per-kind copy + capabilities for the WP-5.5 entity landing pages
 *  (`/characters`, `/teams`, `/arcs`, `/publishers`). One table so the
 *  index page, detail page and chip links can't disagree. */
export const ENTITY_KINDS: Record<
  EntityKindPath,
  {
    /** Page title for the browse index. */
    plural: string;
    /** Lowercase singular noun for empty states ("character"). */
    noun: string;
    /** Whether `/<kind>/{slug}/issues` exists (publishers are series-only). */
    hasIssues: boolean;
    /** Tab label for the issue grid. */
    issuesLabel: string;
  }
> = {
  characters: {
    plural: "Characters",
    noun: "character",
    hasIssues: true,
    issuesLabel: "Appearances",
  },
  teams: {
    plural: "Teams",
    noun: "team",
    hasIssues: true,
    issuesLabel: "Appearances",
  },
  arcs: {
    plural: "Story arcs",
    noun: "story arc",
    hasIssues: true,
    issuesLabel: "Reading order",
  },
  publishers: {
    plural: "Publishers",
    noun: "publisher",
    hasIssues: false,
    issuesLabel: "Issues",
  },
};

/** `/<kind>/<slug>` landing-page URL. */
export function entityUrl(kind: EntityKindPath, slug: string): string {
  return `/${kind}/${encodeURIComponent(slug)}`;
}

/** Chip field (as `ChipList` names it) → the entity kind + the slug map
 *  a detail endpoint returned for it. `null` for fields without a
 *  landing page (locations, genres, tags, …). */
export function entityChipTarget(
  field: string,
  slugs: EntitySlugs | null | undefined,
): { kind: EntityKindPath; map: Record<string, string> } | null {
  if (!slugs) return null;
  switch (field) {
    case "characters":
      return { kind: "characters", map: slugs.characters ?? {} };
    case "teams":
      return { kind: "teams", map: slugs.teams ?? {} };
    case "story_arc":
    case "arcs":
      return { kind: "arcs", map: slugs.arcs ?? {} };
    case "publisher":
    case "publishers":
      return { kind: "publishers", map: slugs.publishers ?? {} };
    default:
      return null;
  }
}

/** Landing-page href for a chip value, or `null` when the name has no
 *  entity row yet (the chip then keeps its library-grid fallback). */
export function entityHrefFor(
  field: string,
  name: string,
  slugs: EntitySlugs | null | undefined,
): string | null {
  const target = entityChipTarget(field, slugs);
  const slug = target?.map[name];
  return target && slug ? entityUrl(target.kind, slug) : null;
}
