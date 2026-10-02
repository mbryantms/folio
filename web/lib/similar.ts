import type { SimilarReason } from "@/lib/api/types";

/** Human label for one "because" entry of a similar-series match
 *  (WP-7.4): `writer Ed Brubaker`, `character Bucky Barnes`,
 *  `arc Winter Soldier`, `sequel of Daredevil`. Creators lead with their
 *  credit role; relationships with their kind; everything else with the
 *  entity kind. */
export function reasonLabel(r: SimilarReason): string {
  if (r.kind === "creator") {
    return `${humanize(r.role ?? "creator")} ${r.name}`;
  }
  if (r.kind === "relationship") {
    return `${humanize(r.role ?? "related to")} ${r.name}`;
  }
  return `${r.kind} ${r.name}`;
}

/** "writer Ed Brubaker, character Bucky Barnes, arc Winter Soldier" —
 *  the first `max` reasons, strongest first (server order). */
export function formatBecause(reasons: SimilarReason[], max = 3): string {
  return reasons.slice(0, max).map(reasonLabel).join(", ");
}

function humanize(s: string): string {
  return s.replace(/_/g, " ");
}
