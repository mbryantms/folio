/**
 * Coverage text helpers (range hygiene): what an accept removed, and
 * whether the last analysis still needs the admin's attention. Pure —
 * shared by the accept toast (`acceptSummary`) and the coverage card.
 */

import type {
  AcceptOutcome,
  CoverageAnalysisResp,
  CoverageRangeRef,
} from "@/lib/api/types";

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

/**
 * Whether the last coverage analysis still has anything for the admin to
 * act on, plus a one-line summary for the Details tab's collapsed
 * "Coverage analysis" disclosure. Pure.
 *
 * The analysis stays on screen after everything is accepted, so the card
 * collapses it by default once it is purely explanatory (all providers
 * up to date) and opens it by itself while a decision is pending: a
 * proposal to accept, a conflict, a stale mapping, a provider that errored
 * or was rate-limited or only partially listed, a run in progress, or a
 * failed run. `not_configured` / `no_candidates` are informational and do
 * not count.
 */
export function analysisAttention(data: CoverageAnalysisResp): {
  needsAttention: boolean;
  summary: string;
} {
  if (data.state === "queued" || data.state === "running") {
    return { needsAttention: true, summary: "Running in the background…" };
  }
  if (data.state === "failed") {
    return { needsAttention: true, summary: "Analysis failed" };
  }
  const finished = data.finished_at
    ? `finished ${new Date(data.finished_at).toLocaleString()}`
    : null;
  const review = data.providers.filter(
    (p) =>
      p.status === "partial" ||
      p.status === "error" ||
      p.status === "rate_limited" ||
      p.status === "not_listable" ||
      (p.status === "analyzed" &&
        (p.has_changes ||
          p.conflicts.length > 0 ||
          p.stale_ranges.length > 0 ||
          p.proposed_ranges.some((r) => r.status === "conflict"))),
  );
  const analysed = data.providers.filter(
    (p) => p.status === "analyzed" || p.status === "partial",
  ).length;
  const head =
    review.length > 0
      ? `${review.length} provider${review.length === 1 ? "" : "s"} need${review.length === 1 ? "s" : ""} review`
      : analysed > 0
        ? "all providers up to date"
        : "no provider could be analysed";
  return {
    needsAttention: review.length > 0,
    summary: finished ? `${head} · ${finished}` : head,
  };
}
