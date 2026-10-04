import type { HintSkipReason, SeriesCoverageHint } from "@/lib/api/types";

/** Candidates the "Match this series…" dialog asks hints for up front. */
export const AUTO_HINT_COUNT = 3;

/** Sources whose series issue lists coverage can read. */
export const HINT_SOURCES = new Set(["comicvine", "metron", "gcd"]);

const SKIP_LABEL: Record<HintSkipReason, string> = {
  not_configured: "provider not configured",
  not_listable: "this provider can't list a series' issues",
  budget: "this series' coverage budget for the provider is spent for the hour",
  rate_limited: "the provider's rate limit was reached",
  time_budget: "the provider was too slow — try again",
  error: "the provider's issue list couldn't be read",
  no_local_issues: "no numbered local issues to compare",
};

/** "issue" / "issues". */
function issues(n: number): string {
  return n === 1 ? "issue" : "issues";
}

/**
 * One-line coverage summary for a series candidate, e.g.
 * "Covers 160 of your 173 issues · #600–611 aren't in this series".
 * Display only: it never says anything about the match score.
 */
export function formatCoverageHint(h: SeriesCoverageHint): string {
  if (h.status === "not_computed") {
    return `Coverage not computed — ${h.reason ? SKIP_LABEL[h.reason] : "skipped"}.`;
  }
  const parts: string[] = [];
  if (h.local_total > 0 && h.covered === h.local_total) {
    parts.push(
      h.local_total === 1
        ? "Covers your 1 issue"
        : `Covers all ${h.local_total} of your issues`,
    );
  } else {
    parts.push(
      `Covers ${h.covered} of your ${h.local_total} ${issues(h.local_total)}`,
    );
  }
  if (h.missing_count > 0 && h.missing_runs.length > 0) {
    const shown = h.missing_runs.join(", ");
    const more = h.missing_runs.length >= 6 ? ", …" : "";
    const verb =
      h.missing_count === 1 && h.missing_runs.length === 1 ? "isn't" : "aren't";
    parts.push(`${shown}${more} ${verb} in this series`);
  }
  if (h.date_conflicts > 0) {
    parts.push(
      `${h.date_conflicts} cover date${h.date_conflicts === 1 ? "" : "s"} disagree`,
    );
  }
  let line = parts.join(" · ");
  if (h.partial) line += " (issue list only partly read)";
  return line;
}

/** Tone of a computed hint: full / most / little coverage. */
export function coverageHintTone(
  h: SeriesCoverageHint,
): "full" | "partial" | "low" | "none" {
  if (h.status !== "computed" || h.local_total === 0) return "none";
  if (h.covered === h.local_total) return "full";
  return h.covered * 2 >= h.local_total ? "partial" : "low";
}
