"use client";

/**
 * `<CoverageAfterMatchPrompt>` — the result of the coverage analysis a
 * series match queued (`trigger` `series_match` / `bulk_series_match`),
 * shown on the series' Details tab above the coverage grid.
 *
 * Per analysed provider, one line in plain words: "This folder spans 2
 * Metron series — accept the ranges?" with the usual Accept, or "Your
 * match covers all 173 issues" when there's nothing to do. The analysis
 * is seeded with the matched series, so Accept keeps it as the main.
 * Nothing is written until the admin accepts, unless the server's
 * `metadata.coverage_auto_accept` setting let the job accept a
 * high-confidence result itself (reported here).
 */

import { Check, Loader2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type {
  CoverageAnalysisResp,
  ProposedRange,
  ProviderCoverageView,
} from "@/lib/api/types";

function rangeLabel(r: ProposedRange): string {
  const bounds = r.low === r.high ? `#${r.low}` : `#${r.low}–${r.high}`;
  const name = r.provider_series_name
    ? `${r.provider_series_name}${r.declared_year != null ? ` (${r.declared_year})` : ""}`
    : `series #${r.provider_series_id}`;
  return `${bounds} → ${name}`;
}

/** Distinct provider series the proposal assigns local issues to. */
export function seriesSpanned(p: ProviderCoverageView): number {
  return new Set(
    p.cells.map((c) => c.provider_series_id).filter((id): id is string => !!id),
  ).size;
}

/** The headline for one provider's post-match result. */
export function promptHeadline(
  p: ProviderCoverageView,
  localTotal: number,
  autoAccepted: boolean,
): string {
  const newRanges = p.proposed_ranges.filter((r) => r.status === "new");
  const spans = seriesSpanned(p);
  if (autoAccepted) {
    return `${p.source_label}: accepted automatically (high confidence).`;
  }
  if (p.status !== "analyzed" && p.status !== "partial") {
    return `${p.source_label}: coverage couldn't be checked${p.error ? ` — ${p.error}` : ""}.`;
  }
  if (p.has_changes && newRanges.length > 0) {
    return `This folder spans ${spans} ${p.source_label} series — accept the range${newRanges.length === 1 ? "" : "s"}?`;
  }
  if (p.has_changes) {
    return `${p.source_label} proposes a different main series — accept it?`;
  }
  if (p.uncovered.length > 0) {
    return `${p.source_label}: ${p.uncovered.length} of your ${localTotal} issues aren't in any ${p.source_label} series found.`;
  }
  return `${p.source_label}: your match covers ${localTotal === 1 ? "your issue" : `all ${localTotal} issues`} — nothing left to accept.`;
}

export function CoverageAfterMatchPrompt({
  data,
  acceptingSource,
  onAccept,
}: {
  data: CoverageAnalysisResp;
  acceptingSource: string | null;
  onAccept: (source: string, mainSeriesId: string | null) => void;
}) {
  if (data.trigger === "analyze" || data.state !== "done") return null;
  const localTotal = data.local_issues.length;
  const autoSources = new Set(data.auto_accepted.map((o) => o.source));
  return (
    <div
      className="border-border/60 bg-muted/30 space-y-2 rounded-md border p-3"
      data-testid="coverage-after-match"
    >
      <p className="text-sm font-medium">
        {data.trigger === "series_match"
          ? "After your series match"
          : "After the bulk series match"}
      </p>
      <ul className="space-y-2">
        {data.providers.map((p) => {
          const newRanges = p.proposed_ranges.filter((r) => r.status === "new");
          const auto = autoSources.has(p.source);
          const canAccept = !auto && p.has_changes && p.conflicts.length === 0;
          const accepting = acceptingSource === p.source;
          return (
            <li
              key={p.source}
              className="flex flex-wrap items-start justify-between gap-2"
            >
              <div className="min-w-0 flex-1 space-y-0.5">
                <p className="text-sm break-words">
                  {promptHeadline(p, localTotal, auto)}
                </p>
                {newRanges.length > 0 && !auto && (
                  <ul className="text-muted-foreground text-xs">
                    {newRanges.map((r) => (
                      <li
                        key={`${r.provider_series_id}-${r.low}`}
                        className="break-words"
                      >
                        {rangeLabel(r)}
                      </li>
                    ))}
                  </ul>
                )}
                {p.conflicts.length > 0 && !auto && (
                  <p className="text-warning text-xs break-words">
                    {p.conflicts[0]} — review it in the grid below.
                  </p>
                )}
              </div>
              {canAccept ? (
                <Button
                  size="sm"
                  onClick={() => onAccept(p.source, null)}
                  disabled={acceptingSource !== null}
                >
                  {accepting ? (
                    <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
                  ) : (
                    <Check className="mr-1 h-3.5 w-3.5" />
                  )}
                  Accept
                </Button>
              ) : (
                <Badge variant="outline" className="font-normal">
                  {auto ? "Accepted" : p.has_changes ? "Needs review" : "Done"}
                </Badge>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}
