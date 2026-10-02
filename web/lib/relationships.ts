/**
 * Series-relationship kinds for the web (WP-7.1 / WP-7.3). Mirrors
 * `RelationshipKind` in `crates/server/src/relationships/mod.rs`.
 */
import type { RelationshipKind } from "@/lib/api/types";

/** Kinds in the order the add-form offers them, with the sentence the
 *  admin is completing ("This series is a sequel of …"). */
export const RELATIONSHIP_KINDS: Array<{
  value: RelationshipKind;
  label: string;
}> = [
  { value: "sequel_of", label: "Sequel of" },
  { value: "prequel_of", label: "Prequel of" },
  { value: "spin_off_of", label: "Spin-off of" },
  { value: "has_spin_off", label: "Has spin-off" },
  { value: "crossover_with", label: "Crossover with" },
  { value: "collects", label: "Collects" },
  { value: "collected_in", label: "Collected in" },
  { value: "same_universe", label: "Same universe as" },
  { value: "see_also", label: "See also" },
];

/** The kind of the reverse edge (`to → from`). */
export const INVERSE_KIND: Record<RelationshipKind, RelationshipKind> = {
  sequel_of: "prequel_of",
  prequel_of: "sequel_of",
  spin_off_of: "has_spin_off",
  has_spin_off: "spin_off_of",
  collects: "collected_in",
  collected_in: "collects",
  crossover_with: "crossover_with",
  same_universe: "same_universe",
  see_also: "see_also",
};

/** Display label for a kind ("Sequel of"). */
export function kindLabel(kind: RelationshipKind): string {
  return RELATIONSHIP_KINDS.find((k) => k.value === kind)?.label ?? kind;
}
