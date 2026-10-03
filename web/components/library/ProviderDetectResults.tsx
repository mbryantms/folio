"use client";

/**
 * `<ProviderDetectResults>` — the per-provider outcome of "Detect from
 * providers" (`POST /series/{slug}/provider-ranges/detect`).
 *
 * One block per provider: how its series was found (linked, applied
 * match, cross-reference, strict search), what it lists, each uncovered
 * run and what happened to it, ranges that look stale, and — when the
 * search found only possible matches — the candidates to confirm. The
 * card owns the mutations; this component only renders and calls back.
 */

import { Check, ExternalLink, Loader2, Trash2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type {
  DetectGap,
  DetectResp,
  DetectSourceResult,
  LinkCandidate,
  LinkMethod,
  ProviderRangeRow,
  SourceStatus,
} from "@/lib/api/types";
import { cn } from "@/lib/utils";

const STATUS_LABEL: Record<SourceStatus, string> = {
  scanned: "Scanned",
  not_enumerable: "Can't list issues",
  not_configured: "Not configured",
  no_series: "No series found",
  needs_confirmation: "Needs confirmation",
  rate_limited: "Rate limited",
  skipped: "Skipped",
  error: "Error",
};

const METHOD_LABEL: Record<LinkMethod, string> = {
  linked: "linked series",
  applied: "applied match",
  bridge: "cross-reference",
  search: "series search",
};

/** "#600–611" / "#600". */
export function formatGap(low: string, high: string): string {
  return low === high ? `#${low}` : `#${low}–${high}`;
}

function statusTone(status: SourceStatus): string {
  switch (status) {
    case "scanned":
      return "border-primary/40 text-foreground";
    case "needs_confirmation":
    case "rate_limited":
      return "border-amber-500/50 text-foreground";
    case "error":
      return "border-destructive/60 text-foreground";
    default:
      return "border-border text-muted-foreground";
  }
}

function seriesLabel(
  name: string | null | undefined,
  year: number | null | undefined,
): string | null {
  if (!name) return null;
  return year != null ? `${name} (${year})` : name;
}

function gapLine(g: DetectGap): string {
  const range = formatGap(g.low, g.high);
  const n = `${g.issue_count} issue${g.issue_count === 1 ? "" : "s"}`;
  const target = g.provider_series_name
    ? `${g.provider_series_name} #${g.provider_series_id}`
    : g.provider_series_id
      ? `series #${g.provider_series_id}`
      : null;
  switch (g.status) {
    case "mapped":
      return `${range} (${n}) → mapped to ${target}`;
    case "already_mapped":
      return `${range} (${n}) → already mapped${target ? ` to ${target}` : ""}`;
    case "unresolved":
      return `${range} (${n}) → no other series lists these issues`;
    case "skipped":
      return `${range} (${n}) → not checked this time (request budget)`;
    case "error":
      return `${range} (${n}) → error: ${g.error ?? "provider call failed"}`;
  }
}

/** One-line summary of a provider's result, for the block header. */
function summary(r: DetectSourceResult): string {
  switch (r.status) {
    case "scanned":
      return r.gap_details.length === 0
        ? `Lists ${r.matched_local} of your numbered issues; nothing outside it.`
        : `Lists ${r.matched_local} of your numbered issues.`;
    case "not_enumerable":
      return `${r.source_label} can't list a series' issues, so it can't show a split.`;
    case "not_configured":
      return `Set up ${r.source_label} under Admin → Metadata to include it.`;
    case "no_series":
      return r.error ?? `No ${r.source_label} series matches this one closely.`;
    case "needs_confirmation":
      return "Possible matches — confirm one to use it.";
    case "rate_limited":
      return (
        r.error ?? "The provider's rate limit was reached; try again later."
      );
    case "skipped":
      return "Not checked this time (time budget); run detection again.";
    case "error":
      return r.error ?? "The provider call failed.";
  }
}

export function ProviderDetectResults({
  result,
  confirmingId,
  onConfirm,
  onRemoveStale,
}: {
  result: DetectResp;
  /** `source:external_id` of the candidate being confirmed, if any. */
  confirmingId: string | null;
  onConfirm: (source: string, candidate: LinkCandidate) => void;
  onRemoveStale: (sourceLabel: string, row: ProviderRangeRow) => void;
}) {
  return (
    <div
      className="border-border/60 space-y-3 rounded-md border p-3 text-xs"
      aria-live="polite"
    >
      {result.results.length === 0 && (
        <p className="text-muted-foreground">
          No metadata providers are configured.
        </p>
      )}
      {result.results.map((r) => {
        const label = seriesLabel(
          r.provider_series_name,
          r.provider_series_year,
        );
        return (
          <section key={r.source} className="min-w-0 space-y-1.5">
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
              <span className="text-foreground text-sm font-medium">
                {r.source_label}
              </span>
              <Badge
                variant="outline"
                className={cn("font-normal", statusTone(r.status))}
              >
                {STATUS_LABEL[r.status]}
              </Badge>
            </div>
            {r.provider_series_id && (
              <p className="text-muted-foreground break-words">
                Series:{" "}
                {r.provider_series_url ? (
                  <a
                    href={r.provider_series_url}
                    target="_blank"
                    rel="noreferrer"
                    className="text-foreground hover:underline"
                  >
                    {label ?? `#${r.provider_series_id}`}
                    <ExternalLink
                      aria-hidden
                      className="ml-0.5 inline h-3 w-3 align-[-1px]"
                    />
                  </a>
                ) : (
                  <span className="text-foreground">
                    {label ?? `#${r.provider_series_id}`}
                  </span>
                )}
                {label && <span> · #{r.provider_series_id}</span>}
                {r.resolved_via && (
                  <span> · via {METHOD_LABEL[r.resolved_via]}</span>
                )}
                {r.id_recorded && <span> · saved to external IDs</span>}
              </p>
            )}
            <p className="text-muted-foreground">{summary(r)}</p>
            {r.gap_details.length > 0 && (
              <ul className="space-y-0.5">
                {r.gap_details.map((g) => (
                  <li
                    key={`${g.low}-${g.high}`}
                    className={cn(
                      "break-words",
                      g.status === "mapped"
                        ? "text-foreground"
                        : "text-muted-foreground",
                    )}
                  >
                    {gapLine(g)}
                  </li>
                ))}
              </ul>
            )}
            {r.uncovered_specials > 0 && (
              <p className="text-muted-foreground">
                {r.uncovered_specials} annual/special issue
                {r.uncovered_specials === 1 ? " isn't" : "s aren't"} in this
                series; specials aren&apos;t range-mapped.
              </p>
            )}
            {r.stale_ranges.map((row) => (
              <div
                key={row.id}
                className="flex items-center justify-between gap-2"
              >
                <p className="text-muted-foreground min-w-0 break-words">
                  Mapping {formatGap(row.range_low ?? "", row.range_high ?? "")}{" "}
                  → {row.provider_series_name ?? `#${row.provider_series_id}`}{" "}
                  looks stale: the matched series now lists those issues.
                </p>
                <Button
                  variant="ghost"
                  size="icon"
                  className="text-muted-foreground hover:text-foreground h-7 w-7 shrink-0"
                  onClick={() => onRemoveStale(r.source_label, row)}
                  aria-label={`Remove stale ${r.source_label} mapping`}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
            ))}
            {r.candidates.length > 0 && (
              <ul className="space-y-1.5">
                {r.candidates.map((c) => {
                  const key = `${r.source}:${c.external_id}`;
                  const busy = confirmingId === key;
                  return (
                    <li
                      key={key}
                      className="border-border/60 flex flex-wrap items-center justify-between gap-2 rounded-md border px-2 py-1.5"
                    >
                      <div className="min-w-0 space-y-0.5">
                        <p className="text-foreground break-words">
                          {c.url ? (
                            <a
                              href={c.url}
                              target="_blank"
                              rel="noreferrer"
                              className="hover:underline"
                            >
                              {seriesLabel(c.name, c.year)}
                            </a>
                          ) : (
                            seriesLabel(c.name, c.year)
                          )}{" "}
                          <span className="text-muted-foreground">
                            #{c.external_id}
                          </span>
                        </p>
                        <p className="text-muted-foreground">
                          {c.publisher ? `${c.publisher} · ` : ""}score{" "}
                          {Math.round(c.score)}
                          {c.issue_overlap != null &&
                            ` · lists ${Math.round(c.issue_overlap * 100)}% of your issues`}{" "}
                          · {c.reason}
                        </p>
                      </div>
                      <Button
                        variant="outline"
                        size="sm"
                        className="h-7 shrink-0"
                        disabled={confirmingId !== null}
                        onClick={() => onConfirm(r.source, c)}
                      >
                        {busy ? (
                          <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
                        ) : (
                          <Check className="mr-1 h-3.5 w-3.5" />
                        )}
                        Use this series
                      </Button>
                    </li>
                  );
                })}
              </ul>
            )}
          </section>
        );
      })}
      {result.agreement && (
        <p className="text-muted-foreground border-border/60 border-t pt-2">
          {result.agreement.summary}
        </p>
      )}
    </div>
  );
}
