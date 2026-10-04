/**
 * Collection-tab helpers for the provider manifest (exact missing-issue
 * lists from the providers' issue lists; see
 * `crates/server/src/metadata/issue_manifest.rs`).
 */

import type { CollectionReportView } from "@/lib/api/types";

const PROVIDER_LABEL: Record<string, string> = {
  comicvine: "ComicVine",
  metron: "Metron",
  gcd: "GCD",
};

/** "ComicVine", "Metron", "GCD" (else the raw source). */
export function manifestProviderLabel(source: string): string {
  return PROVIDER_LABEL[source] ?? source;
}

/**
 * The main-run numbers the grid shows. With a provider manifest: what you
 * own plus what the providers list (no interpolated #71–499 for a folder
 * holding #1–70 and #500+). Otherwise the full `min..max` run.
 */
export function collectionRunChips(
  data: Pick<CollectionReportView, "expected_source" | "main_run">,
  ownedInts: Iterable<number>,
): number[] {
  const { min, max, missing, possibly_missing } = data.main_run;
  if (data.expected_source === "provider_manifest") {
    const set = new Set<number>(ownedInts);
    for (const n of missing) set.add(n);
    for (const n of possibly_missing ?? []) set.add(n);
    return Array.from(set).sort((a, b) => a - b);
  }
  const lo = min != null ? Math.round(min) : 0;
  const hi = max != null ? Math.round(max) : -1;
  const out: number[] = [];
  for (let n = lo; n <= hi; n++) out.push(n);
  return out;
}

/** A plain non-negative integer issue number ("12", not "605.1" / "½"). */
export function isIntegral(n: string): boolean {
  return /^\d+$/.test(n.trim());
}
