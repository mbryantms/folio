/**
 * Coverage-accept text helpers (range hygiene): what an accept removed.
 * Pure — shared by the accept toast (`acceptSummary`) and the coverage
 * card's persistent note.
 */

import type { AcceptOutcome, CoverageRangeRef } from "@/lib/api/types";

const COVERAGE_SOURCE_LABEL: Record<string, string> = {
  comicvine: "ComicVine",
  metron: "Metron",
  gcd: "GCD",
};

/** "#42–70 → Fantastic Four (1961)" for one range row. */
export function staleRangeLabel(r: CoverageRangeRef): string {
  const lo = r.range_low;
  const hi = r.range_high;
  const nums =
    lo && hi
      ? lo === hi
        ? `#${lo}`
        : `#${lo}–${hi}`
      : lo
        ? `#${lo}+`
        : hi
          ? `up to #${hi}`
          : "all issues";
  const name = r.provider_series_name?.trim();
  const target = name
    ? r.declared_year != null && !/\(\d{4}\)$/.test(name)
      ? `${name} (${r.declared_year})`
      : name
    : `#${r.provider_series_id}`;
  return `${nums} → ${target}`;
}

/**
 * "Removed 1 stale range: GCD #42–70 → Fantastic Four (1961)" — the stale
 * automated ranges an accept deleted, or null when it deleted none.
 */
export function staleRemovedSummary(data: AcceptOutcome | null): string | null {
  const removed = data?.stale_ranges_removed ?? [];
  if (!data || removed.length === 0) return null;
  const source = COVERAGE_SOURCE_LABEL[data.source] ?? data.source;
  const n = removed.length;
  return `Removed ${n} stale range${n === 1 ? "" : "s"}: ${removed
    .map((r) => `${source} ${staleRangeLabel(r)}`)
    .join("; ")}`;
}
