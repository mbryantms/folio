"use client";

/**
 * `<ProviderCoverageAnalysis>` — the result of a provider-coverage
 * analysis (`GET /series/{slug}/provider-coverage/analysis`).
 *
 * Shows which provider series holds which local issue, for ComicVine,
 * Metron and GCD at once:
 *  - wide screens: a compact grid, local issues × provider, with
 *    consecutive issues that every provider files the same way collapsed
 *    into one row ("#1–5"), in its own scroll region;
 *  - every width: per provider, a grouped list (series → its issues). On
 *    narrow screens (< 640 px) the grid is dropped and the grouped lists
 *    carry the whole picture, so nothing scrolls sideways.
 *
 * Below it, one block per provider: confidence and why, uncovered issues,
 * conflicts with user-set data, stale automated ranges (accepting the
 * provider deletes them; remove one sooner via the card's AlertDialog),
 * and "Accept" / "Choose series". Stale ranges an accept removed are
 * listed above the providers ("Removed 1 stale range: GCD #42–70 → …").
 * The card owns the mutations; this component renders and calls back.
 */

import {
  AlertTriangle,
  Check,
  ExternalLink,
  Loader2,
  Trash2,
} from "lucide-react";
import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type {
  CoverageAnalysisResp,
  CoverageCandidateView,
  CoverageConfidence,
  CoverageLocalIssue,
  CoverageRangeRef,
  CoverageStatus,
  ProviderCoverageView,
} from "@/lib/api/types";
import { staleRemovedSummary } from "@/lib/coverage-ranges";
import { cn } from "@/lib/utils";

/**
 * Categorical hues (HSL triplets), one per distinct provider series within
 * a provider. First slot is the theme amber; the rest hold up on the dark
 * and light themes. Shared with the coverage bar in the card.
 */
export const SERIES_COLORS = [
  "38 92% 55%", // amber (theme primary)
  "199 89% 48%", // sky
  "262 83% 58%", // violet
  "160 84% 39%", // emerald
  "350 89% 60%", // rose
  "173 80% 40%", // teal
  "25 95% 53%", // orange
  "292 84% 61%", // fuchsia
];

/** Below this width the grid collapses into per-provider lists. */
export const WIDE_QUERY = "(min-width: 640px)";

const STATUS_LABEL: Record<CoverageStatus, string> = {
  analyzed: "Analyzed",
  not_configured: "Not configured",
  not_listable: "Can't list issues",
  no_candidates: "No series found",
  partial: "Partial",
  rate_limited: "Rate limited",
  error: "Error",
};

const CONFIDENCE_LABEL: Record<CoverageConfidence, string> = {
  high: "High confidence",
  medium: "Medium confidence",
  low: "Low confidence",
  none: "No match",
};

function confidenceVariant(
  c: CoverageConfidence,
): "default" | "secondary" | "outline" {
  if (c === "high") return "default";
  if (c === "medium") return "secondary";
  return "outline";
}

/** True when the viewport is at least 640 px wide (SSR: false). */
export function useWideLayout(): boolean {
  return React.useSyncExternalStore(
    (onChange) => {
      if (typeof window === "undefined" || !window.matchMedia) return () => {};
      const mql = window.matchMedia(WIDE_QUERY);
      mql.addEventListener?.("change", onChange);
      return () => mql.removeEventListener?.("change", onChange);
    },
    () =>
      typeof window !== "undefined" && !!window.matchMedia
        ? window.matchMedia(WIDE_QUERY).matches
        : false,
    () => false,
  );
}

/** "#1–5, #500–502": runs of consecutive local issues (in local order). */
export function runsLabel(
  indices: number[],
  local: CoverageLocalIssue[],
): string {
  const sorted = [...indices].sort((a, b) => a - b);
  const runs: Array<[number, number]> = [];
  for (const i of sorted) {
    const last = runs[runs.length - 1];
    if (last && last[1] === i - 1) last[1] = i;
    else runs.push([i, i]);
  }
  return runs
    .map(([a, b]) => {
      const lo = local[a]?.number ?? "?";
      const hi = local[b]?.number ?? "?";
      return a === b ? `#${lo}` : `#${lo}–${hi}`;
    })
    .join(", ");
}

export type GridRow = {
  /** First and last local index of the row (inclusive). */
  first: number;
  last: number;
  /** Per provider (aligned with the input), the series id or null. */
  series: Array<string | null>;
};

/**
 * Rows of the wide grid: consecutive local issues that every provider
 * files under the same series collapse into one row.
 */
export function gridRows(
  local: CoverageLocalIssue[],
  providers: ProviderCoverageView[],
): GridRow[] {
  const rows: GridRow[] = [];
  local.forEach((_, i) => {
    const series = providers.map((p) => p.cells[i]?.provider_series_id ?? null);
    const prev = rows[rows.length - 1];
    if (
      prev &&
      prev.last === i - 1 &&
      prev.series.every((s, k) => s === series[k])
    ) {
      prev.last = i;
    } else {
      rows.push({ first: i, last: i, series });
    }
  });
  return rows;
}

export type SeriesGroup = {
  seriesId: string | null;
  role: "main" | "range" | "uncovered";
  indices: number[];
};

/** Narrow layout: a provider's issues grouped by series, main first. */
export function seriesGroups(p: ProviderCoverageView): SeriesGroup[] {
  const order: string[] = [];
  const byId = new Map<string, number[]>();
  const uncovered: number[] = [];
  p.cells.forEach((c, i) => {
    const id = c.provider_series_id ?? null;
    if (!id) {
      uncovered.push(i);
      return;
    }
    if (!byId.has(id)) {
      byId.set(id, []);
      order.push(id);
    }
    byId.get(id)!.push(i);
  });
  const main = p.main_series_id ?? null;
  order.sort((a, b) => (a === main ? -1 : b === main ? 1 : 0));
  const groups: SeriesGroup[] = order.map((id) => ({
    seriesId: id,
    role: id === main ? "main" : "range",
    indices: byId.get(id)!,
  }));
  if (uncovered.length > 0)
    groups.push({ seriesId: null, role: "uncovered", indices: uncovered });
  return groups;
}

function candidateLabel(c: CoverageCandidateView | undefined, id: string) {
  if (!c?.name) return `#${id}`;
  return c.year != null ? `${c.name} (${c.year})` : c.name;
}

function colorFor(p: ProviderCoverageView, id: string | null): string {
  if (!id) return "transparent";
  const ids = p.candidates.map((c) => c.provider_series_id);
  // Main is always the first colour; the rest follow candidate order.
  const ordered = [
    ...(p.main_series_id ? [p.main_series_id] : []),
    ...ids.filter((x) => x !== p.main_series_id),
  ];
  const i = Math.max(0, ordered.indexOf(id));
  return `hsl(${SERIES_COLORS[i % SERIES_COLORS.length]})`;
}

function Dot({ color }: { color: string }) {
  return (
    <span
      aria-hidden
      className="h-2.5 w-2.5 shrink-0 rounded-full"
      style={{ backgroundColor: color }}
    />
  );
}

/** Providers whose cells carry an assignment (analysed / partial). */
function gridProviders(data: CoverageAnalysisResp): ProviderCoverageView[] {
  return data.providers.filter(
    (p) => p.status === "analyzed" || p.status === "partial",
  );
}

export function ProviderCoverageAnalysis({
  data,
  acceptingSource,
  onAccept,
  onRemoveStale,
  removedNotes = [],
}: {
  data: CoverageAnalysisResp;
  /** Source whose accept is in flight. */
  acceptingSource: string | null;
  onAccept: (source: string, mainSeriesId: string | null) => void;
  onRemoveStale: (sourceLabel: string, row: CoverageRangeRef) => void;
  /** "Removed 1 stale range: …" from accepts made in this view. */
  removedNotes?: string[];
}) {
  const wide = useWideLayout();
  const local = data.local_issues;
  const shown = gridProviders(data);
  const removed = [
    ...data.auto_accepted.flatMap((o) => staleRemovedSummary(o) ?? []),
    ...removedNotes,
  ];

  return (
    <div className="space-y-4" data-testid="coverage-analysis">
      {wide && shown.length > 0 && local.length > 0 && (
        <CoverageGrid local={local} providers={shown} />
      )}
      {data.auto_accepted.length > 0 && (
        <p className="text-muted-foreground text-xs">
          Accepted automatically:{" "}
          {data.auto_accepted
            .map((o) => data.providers.find((p) => p.source === o.source))
            .filter(Boolean)
            .map((p) => p!.source_label)
            .join(", ")}
          .
        </p>
      )}
      {removed.length > 0 && (
        <ul
          className="space-y-0.5 text-xs"
          role="status"
          data-testid="coverage-stale-removed"
        >
          {removed.map((note) => (
            <li key={note} className="flex gap-1.5">
              <Check className="text-primary mt-0.5 h-3.5 w-3.5 shrink-0" />
              <span>{note}</span>
            </li>
          ))}
        </ul>
      )}
      {data.providers.map((p) => (
        <ProviderBlock
          key={p.source}
          p={p}
          local={local}
          accepting={acceptingSource === p.source}
          busy={acceptingSource !== null}
          onAccept={onAccept}
          onRemoveStale={onRemoveStale}
        />
      ))}
    </div>
  );
}

function CoverageGrid({
  local,
  providers,
}: {
  local: CoverageLocalIssue[];
  providers: ProviderCoverageView[];
}) {
  const rows = gridRows(local, providers);
  return (
    <ScrollArea
      className="border-border/40 rounded-md border"
      viewportClassName="max-h-80 focus-visible:ring-ring focus-visible:ring-2 focus-visible:outline-none focus-visible:ring-inset"
      viewportProps={{ tabIndex: 0, "aria-label": "Coverage grid" }}
    >
      <table className="w-full table-fixed text-xs" data-testid="coverage-grid">
        <thead className="bg-muted/40 sticky top-0">
          <tr>
            <th className="w-24 px-2 py-1.5 text-left font-medium">Issues</th>
            {providers.map((p) => (
              <th key={p.source} className="px-2 py-1.5 text-left font-medium">
                {p.source_label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.first} className="border-border/30 border-t">
              <td className="px-2 py-1 tabular-nums">
                {runsLabel(
                  Array.from(
                    { length: r.last - r.first + 1 },
                    (_, k) => r.first + k,
                  ),
                  local,
                )}
              </td>
              {providers.map((p, k) => {
                const id = r.series[k];
                const cand = p.candidates.find(
                  (c) => c.provider_series_id === id,
                );
                return (
                  <td key={p.source} className="px-2 py-1">
                    {id ? (
                      <span className="flex min-w-0 items-center gap-1.5">
                        <Dot color={colorFor(p, id)} />
                        <span className="truncate">
                          {candidateLabel(cand, id)}
                        </span>
                        {id === p.main_series_id && (
                          <span className="text-muted-foreground shrink-0 text-[10px] tracking-wider uppercase">
                            main
                          </span>
                        )}
                      </span>
                    ) : (
                      <span className="text-muted-foreground">—</span>
                    )}
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </ScrollArea>
  );
}

function ProviderBlock({
  p,
  local,
  accepting,
  busy,
  onAccept,
  onRemoveStale,
}: {
  p: ProviderCoverageView;
  local: CoverageLocalIssue[];
  accepting: boolean;
  busy: boolean;
  onAccept: (source: string, mainSeriesId: string | null) => void;
  onRemoveStale: (sourceLabel: string, row: CoverageRangeRef) => void;
}) {
  const [choosing, setChoosing] = React.useState(false);
  // The admin's pick; `null` follows the proposal's main.
  const [picked, setPicked] = React.useState<string | null>(null);
  const chosen = picked ?? p.main_series_id ?? "";
  const assigned = p.status === "analyzed" || p.status === "partial";
  const chosenOther = !!chosen && chosen !== p.main_series_id;
  const canAccept =
    assigned && !!p.main_series_id && (p.has_changes || chosenOther);
  const conflictRanges = p.proposed_ranges.filter(
    (r) => r.status === "conflict",
  );

  return (
    <section
      className="border-border/40 space-y-2 rounded-md border p-3"
      data-testid={`coverage-provider-${p.source}`}
    >
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <span className="font-medium">{p.source_label}</span>
        {assigned ? (
          <Badge
            variant={confidenceVariant(p.confidence)}
            className="font-normal"
          >
            {CONFIDENCE_LABEL[p.confidence]}
          </Badge>
        ) : (
          <Badge
            variant="outline"
            className="text-muted-foreground font-normal"
          >
            {STATUS_LABEL[p.status]}
          </Badge>
        )}
        {p.status === "partial" && (
          <Badge variant="outline" className="font-normal">
            Partial
          </Badge>
        )}
        <span className="text-muted-foreground ml-auto text-xs tabular-nums">
          {p.requests} request{p.requests === 1 ? "" : "s"}
        </span>
      </div>

      {!assigned && (
        <p className="text-muted-foreground text-xs">
          {p.status === "not_configured"
            ? `Set up ${p.source_label} under Admin → Metadata to include it.`
            : p.status === "no_candidates"
              ? `No ${p.source_label} series lists these issues.`
              : (p.error ?? STATUS_LABEL[p.status])}
        </p>
      )}

      {assigned && (
        <>
          {p.confidence_reasons.length > 0 && (
            <p className="text-muted-foreground text-xs">
              {p.confidence_reasons.join(" · ")}
            </p>
          )}
          <GroupedList p={p} local={local} />
          {p.uncovered.length > 0 && (
            <p
              className="text-xs"
              data-testid={`coverage-uncovered-${p.source}`}
            >
              <span className="text-muted-foreground">
                No {p.source_label} series has{" "}
              </span>
              {p.uncovered.map((n) => `#${n}`).join(", ")}
            </p>
          )}
          {p.unranged_specials.length > 0 && (
            <p className="text-muted-foreground text-xs">
              {p.unranged_specials.map((n) => `#${n}`).join(", ")} sit in
              another series but can&rsquo;t be mapped by range (not a plain
              number).
            </p>
          )}
          {(p.conflicts.length > 0 || conflictRanges.length > 0) && (
            <ul className="space-y-0.5 text-xs">
              {p.conflicts.map((c) => (
                <li key={c} className="flex gap-1.5">
                  <AlertTriangle className="text-primary mt-0.5 h-3.5 w-3.5 shrink-0" />
                  <span>{c}</span>
                </li>
              ))}
              {conflictRanges
                .filter((r) => r.note)
                .map((r) => (
                  <li key={`${r.low}-${r.high}`} className="flex gap-1.5">
                    <AlertTriangle className="text-primary mt-0.5 h-3.5 w-3.5 shrink-0" />
                    <span>
                      #{r.low}–{r.high} not mapped: {r.note}
                    </span>
                  </li>
                ))}
            </ul>
          )}
          {p.stale_ranges.length > 0 && (
            <ul className="space-y-1 text-xs">
              {p.stale_ranges.map((r) => (
                <li
                  key={r.id}
                  className="flex items-center justify-between gap-2"
                >
                  <span className="min-w-0">
                    Stale mapping{" "}
                    {r.range_low === r.range_high
                      ? `#${r.range_low}`
                      : `#${r.range_low ?? ""}–${r.range_high ?? ""}`}{" "}
                    → {r.provider_series_name ?? `#${r.provider_series_id}`}:{" "}
                    <span className="text-muted-foreground">{r.reason}</span>
                  </span>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="text-muted-foreground/60 hover:text-foreground h-6 w-6 shrink-0"
                    onClick={() => onRemoveStale(p.source_label, r)}
                    aria-label={`Remove stale ${p.source_label} mapping`}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                </li>
              ))}
            </ul>
          )}

          <div className="flex flex-wrap items-center gap-2 pt-1">
            {choosing && p.candidates.length > 1 && (
              <Select value={chosen} onValueChange={setPicked}>
                <SelectTrigger
                  className="h-8 w-full text-xs sm:w-64"
                  aria-label={`${p.source_label} main series`}
                >
                  <SelectValue placeholder="Main series" />
                </SelectTrigger>
                <SelectContent>
                  {p.candidates.map((c) => (
                    <SelectItem
                      key={c.provider_series_id}
                      value={c.provider_series_id}
                    >
                      {candidateLabel(c, c.provider_series_id)} ·{" "}
                      {c.local_matches} of yours
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
            <Button
              size="sm"
              variant={canAccept ? "default" : "outline"}
              disabled={!canAccept || busy}
              onClick={() => onAccept(p.source, chosenOther ? chosen : null)}
            >
              {accepting ? (
                <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
              ) : (
                <Check className="mr-1 h-3.5 w-3.5" />
              )}
              {canAccept ? "Accept" : "Up to date"}
            </Button>
            {!choosing && p.candidates.length > 1 && (
              <Button
                size="sm"
                variant="ghost"
                disabled={busy}
                onClick={() => setChoosing(true)}
              >
                Choose series
              </Button>
            )}
          </div>
        </>
      )}
    </section>
  );
}

function SeriesLine({
  p,
  g,
  local,
}: {
  p: ProviderCoverageView;
  g: SeriesGroup;
  local: CoverageLocalIssue[];
}) {
  const id = g.seriesId!;
  const cand = p.candidates.find((c) => c.provider_series_id === id);
  const label = candidateLabel(cand, id);
  return (
    <li className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5">
      <Dot color={colorFor(p, id)} />
      {cand?.url ? (
        <a
          href={cand.url}
          target="_blank"
          rel="noreferrer"
          className="inline-flex min-w-0 items-center gap-1 hover:underline"
        >
          <span className="truncate">{label}</span>
          <ExternalLink className="h-3 w-3 shrink-0" />
        </a>
      ) : (
        <span className="truncate">{label}</span>
      )}
      <span
        className={cn(
          "text-[10px] font-medium tracking-wider uppercase",
          g.role === "main" ? "text-primary" : "text-muted-foreground",
        )}
      >
        {g.role === "main" ? "main" : "range"}
      </span>
      <span className="text-muted-foreground tabular-nums">
        {runsLabel(g.indices, local)}
      </span>
    </li>
  );
}

function GroupedList({
  p,
  local,
}: {
  p: ProviderCoverageView;
  local: CoverageLocalIssue[];
}) {
  const groups = seriesGroups(p).filter((g) => g.seriesId);
  return (
    <ul className="space-y-1 text-xs" data-testid={`coverage-list-${p.source}`}>
      {groups.map((g) => (
        <SeriesLine key={g.seriesId} p={p} g={g} local={local} />
      ))}
    </ul>
  );
}
