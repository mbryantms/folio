/**
 * Series-relationship helpers for the web (WP-7.1 / WP-7.3 / WP-7.5).
 *
 * The kind list is **not** hard-coded here: it comes from the server's
 * catalogue (`GET /relationship-kinds`, `useRelationshipKinds`), which
 * mirrors `RelationshipKind` in `crates/server/src/relationships/mod.rs`
 * (labels, inverses, UI groups, allowed qualifiers / coverage). These
 * helpers only slice that catalogue.
 */
import type {
  RelationshipCatalogue,
  RelationshipGroup,
  RelationshipKind,
  RelationshipKindInfo,
} from "@/lib/api/types";

/** Kinds grouped for a picker, in the catalogue's group + kind order. */
export function groupedKinds(
  catalogue: RelationshipCatalogue | undefined,
): Array<{
  group: RelationshipGroup;
  label: string;
  kinds: RelationshipKindInfo[];
}> {
  if (!catalogue) return [];
  return catalogue.groups
    .map((g) => ({
      group: g.group,
      label: g.label,
      kinds: catalogue.kinds.filter((k) => k.group === g.group),
    }))
    .filter((g) => g.kinds.length > 0);
}

/** The catalogue entry for `kind` (undefined until the catalogue loads). */
export function kindInfo(
  catalogue: RelationshipCatalogue | undefined,
  kind: RelationshipKind,
): RelationshipKindInfo | undefined {
  return catalogue?.kinds.find((k) => k.kind === kind);
}

/** Display label for a kind ("Sequel to"); falls back to the raw kind. */
export function kindLabel(
  catalogue: RelationshipCatalogue | undefined,
  kind: RelationshipKind,
): string {
  return kindInfo(catalogue, kind)?.label ?? kind.replace(/_/g, " ");
}

/** Catalogue position of a kind (for stable ordering of grouped lists). */
export function kindOrder(
  catalogue: RelationshipCatalogue | undefined,
  kind: RelationshipKind,
): number {
  const i = catalogue?.kinds.findIndex((k) => k.kind === kind) ?? -1;
  return i < 0 ? Number.MAX_SAFE_INTEGER : i;
}

/** Secondary caption for a scoped relationship: ranges, coverage,
 *  qualifier and note joined with " · " (empty string when unscoped). */
export function scopeCaption(r: {
  from_range?: string | null;
  to_range?: string | null;
  coverage?: string | null;
  qualifier_label?: string | null;
  note?: string | null;
  kind?: string;
}): string {
  const parts: string[] = [];
  // A tie-in role is already folded into the kind label.
  if (r.qualifier_label && r.kind !== "tie_in_to" && r.kind !== "has_tie_in") {
    parts.push(r.qualifier_label);
  }
  if (r.from_range && r.to_range)
    parts.push(`#${r.from_range} ↔ #${r.to_range}`);
  else if (r.from_range) parts.push(`this: #${r.from_range}`);
  else if (r.to_range) parts.push(`#${r.to_range}`);
  if (r.coverage) parts.push(`${r.coverage} coverage`);
  if (r.note) parts.push(r.note);
  return parts.join(" · ");
}
