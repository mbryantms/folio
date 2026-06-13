"use client";

/**
 * Collection-completeness tab for the series detail page.
 *
 * Two things at a glance:
 *   1. **Ownership** — which main-run issues you have vs. which are missing.
 *      Interior gaps are *inferred* by interpolating the integer run between
 *      the lowest and highest owned issue (`series.json` carries only a count,
 *      never a per-issue manifest), so they're labelled as such.
 *   2. **Metadata** — each owned issue is colored by completeness
 *      (complete / partial / needs-metadata). Click any chip to see exactly
 *      what's missing and jump to the issue.
 *
 * The progress header stacks both signals: an overall bar (owned **and**
 * metadata-complete) plus two sub-bars for ownership and metadata on their own.
 */

import { AlertCircle, CheckCircle2, ChevronRight, Loader2 } from "lucide-react";
import Link from "next/link";
import * as React from "react";

import { useSeriesCollection } from "@/lib/api/queries";
import type { CollectionIssueEntry } from "@/lib/api/types";
import { ISSUE_GRID_CELL_RADIUS, ISSUE_GRID_COLS } from "@/lib/issue-grid";
import { metadataFieldLabels } from "@/lib/metadata-fields";
import { cn } from "@/lib/utils";
import { statusTone, statusToneDot } from "@/lib/ui/status-tone";
import { issueUrl } from "@/lib/urls";

type ChipStatus = "missing" | "needs_metadata" | "partial" | "complete";

/** Status → cell classes. The success/warning/error triad uses the
 *  conventional emerald → amber → red so the four states stay clearly
 *  distinguishable regardless of the active theme accent (a primary-tinted
 *  "complete" collided with amber "partial" on warm themes). `needs_metadata`
 *  rides the `--destructive` token and `missing` rides `--border`/muted so
 *  those two still track the theme. */
const STATUS_CLASSES: Record<ChipStatus, string> = {
  missing: "border border-dashed border-border text-muted-foreground/70",
  needs_metadata:
    "bg-destructive/15 text-destructive ring-1 ring-destructive/30",
  partial: "bg-warning/15 text-warning ring-1 ring-warning/30",
  complete: "bg-success/15 text-success ring-1 ring-success/30",
};

const STATUS_LABELS: Record<ChipStatus, string> = {
  missing: "Missing (inferred)",
  needs_metadata: "Needs metadata",
  partial: "Partial metadata",
  complete: "Complete",
};

/** A main-run issue is an owned, integer-numbered, non-special issue. */
function isMainRun(e: CollectionIssueEntry): boolean {
  return (
    e.special_type == null &&
    e.sort_number != null &&
    Number.isInteger(e.sort_number)
  );
}

type Selection =
  | { kind: "issue"; entry: CollectionIssueEntry }
  | { kind: "missing"; n: number }
  | null;

export function CollectionTab({ seriesSlug }: { seriesSlug: string }) {
  const { data, isLoading, isError } = useSeriesCollection(seriesSlug);
  const [selected, setSelected] = React.useState<Selection>(null);

  if (isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-8 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" />
        Loading collection…
      </div>
    );
  }
  if (isError || !data) {
    return (
      <p className="text-muted-foreground py-8 text-sm">
        Couldn&rsquo;t load collection details.
      </p>
    );
  }

  const { total_owned, total_expected, completeness_state } = data;
  const { missing, min, max, trailing_missing } = data.main_run;

  // Index owned main-run issues by their integer number for chip coloring.
  const mainByInt = new Map<number, CollectionIssueEntry>();
  for (const e of data.issues) {
    if (isMainRun(e)) mainByInt.set(Math.round(e.sort_number as number), e);
  }
  // Owned issues that aren't on the main run (annuals, one-shots, point issues).
  const specialEntries = data.issues.filter((e) => !isMainRun(e));

  const lo = min != null ? Math.round(min) : 0;
  const hi = max != null ? Math.round(max) : -1;
  const runChips: number[] = [];
  for (let n = lo; n <= hi; n++) runChips.push(n);

  const completeCount = data.issues.filter(
    (e) => e.metadata_tier === "complete",
  ).length;
  const ownedCount = total_owned;
  // Denominator for the bars: the publisher total when known, else what we own.
  const denom = total_expected ?? ownedCount;
  const overallPct = denom > 0 ? (completeCount / denom) * 100 : 0;

  function statusOf(n: number): ChipStatus {
    const entry = mainByInt.get(n);
    if (!entry) return "missing";
    return entry.metadata_tier as ChipStatus;
  }

  return (
    <div className="space-y-8">
      {/* ── Progress header: overall + two sub-bars ── */}
      <section className="space-y-3">
        <div className="flex items-end justify-between gap-4">
          <div className="flex items-baseline gap-3">
            <span className="text-foreground text-4xl font-semibold tabular-nums">
              {total_expected != null ? `${Math.round(overallPct)}%` : "—"}
            </span>
            <span className="text-muted-foreground text-sm">
              {total_expected != null
                ? `${completeCount} of ${total_expected} complete`
                : `${ownedCount} issue${ownedCount === 1 ? "" : "s"} owned · total unknown`}
            </span>
          </div>
          <StatePill state={completeness_state} />
        </div>

        {/* Overall stacked bar: emerald = owned & metadata-complete,
            amber = owned but incomplete, track = not owned. */}
        <StackedBar
          total={denom}
          segments={[
            { value: completeCount, className: statusToneDot("success") },
            {
              value: Math.max(ownedCount - completeCount, 0),
              className: statusToneDot("warning"),
            },
          ]}
        />

        <div className="grid gap-2 sm:grid-cols-2">
          {total_expected != null && (
            <SubBar
              label="Owned"
              value={ownedCount}
              total={total_expected}
              barClassName="bg-muted-foreground/50"
            />
          )}
          <SubBar
            label="Metadata complete"
            value={completeCount}
            total={ownedCount}
            barClassName={statusToneDot("success")}
          />
        </div>
      </section>

      {/* ── Issue grid (ownership + metadata status) ── */}
      {runChips.length > 0 ? (
        <section className="space-y-3">
          <div className="flex items-center justify-between">
            <h3 className="text-foreground text-sm font-semibold">Issues</h3>
            {missing.length > 0 && (
              <span className="text-muted-foreground text-xs">
                {missing.length} missing
                <span className="opacity-70"> (inferred)</span>
              </span>
            )}
          </div>
          <div className={ISSUE_GRID_COLS}>
            {runChips.map((n) => {
              const status = statusOf(n);
              const entry = mainByInt.get(n);
              const isSel =
                (selected?.kind === "missing" && selected.n === n) ||
                (selected?.kind === "issue" &&
                  entry != null &&
                  selected.entry.slug === entry.slug);
              return (
                <button
                  key={n}
                  type="button"
                  onClick={() =>
                    setSelected(
                      entry ? { kind: "issue", entry } : { kind: "missing", n },
                    )
                  }
                  title={`Issue #${n} — ${STATUS_LABELS[status]}`}
                  className={cn(
                    "flex aspect-square items-center justify-center text-[11px] font-medium tabular-nums transition-colors",
                    ISSUE_GRID_CELL_RADIUS,
                    STATUS_CLASSES[status],
                    isSel && "outline-ring outline-2 outline-offset-1",
                  )}
                >
                  {n}
                </button>
              );
            })}
            {trailing_missing > 0 && (
              <span
                title={`${trailing_missing} more issue${trailing_missing === 1 ? "" : "s"} expected beyond #${hi}`}
                className={cn(
                  "text-muted-foreground border-border/60 flex aspect-square items-center justify-center border border-dashed text-[10px]",
                  ISSUE_GRID_CELL_RADIUS,
                )}
              >
                +{trailing_missing}
              </span>
            )}
          </div>
          <Legend />
          {selected && (
            <SelectionDetail
              selection={selected}
              seriesSlug={seriesSlug}
              onClear={() => setSelected(null)}
            />
          )}
        </section>
      ) : (
        <p className="text-muted-foreground text-sm">
          No numbered issues to chart yet.
        </p>
      )}

      {/* ── Specials / extras ── */}
      {specialEntries.length > 0 && (
        <section className="space-y-3">
          <h3 className="text-foreground text-sm font-semibold">
            Specials &amp; extras
          </h3>
          <div className="flex flex-wrap gap-1.5">
            {specialEntries.map((e) => {
              const status = e.metadata_tier as ChipStatus;
              const isSel =
                selected?.kind === "issue" && selected.entry.slug === e.slug;
              return (
                <button
                  key={e.slug}
                  type="button"
                  onClick={() => setSelected({ kind: "issue", entry: e })}
                  title={`${e.special_type ?? "Special"} — ${STATUS_LABELS[status]}`}
                  className={cn(
                    "inline-flex items-center gap-1 rounded-md px-2 py-1 text-xs transition-colors",
                    STATUS_CLASSES[status],
                    isSel && "outline-ring outline-2 outline-offset-1",
                  )}
                >
                  {e.special_type && (
                    <span className="opacity-70">{e.special_type}</span>
                  )}
                  {e.number_raw ?? "—"}
                </button>
              );
            })}
          </div>
          <p className="text-muted-foreground text-xs">
            Annuals, one-shots, and point issues aren&rsquo;t counted toward the
            main-run gap math above.
          </p>
        </section>
      )}
    </div>
  );
}

/** Multi-segment progress bar (a single track with stacked colored fills). */
function StackedBar({
  total,
  segments,
}: {
  total: number;
  segments: { value: number; className: string }[];
}) {
  return (
    <div className="bg-secondary flex h-2 w-full overflow-hidden rounded-full">
      {segments.map((s, i) => {
        const pct = total > 0 ? Math.min((s.value / total) * 100, 100) : 0;
        if (pct <= 0) return null;
        return (
          <div
            key={i}
            className={cn("h-full transition-all", s.className)}
            style={{ width: `${pct}%` }}
          />
        );
      })}
    </div>
  );
}

/** A single labelled sub-bar with an `X / Y` caption. */
function SubBar({
  label,
  value,
  total,
  barClassName,
}: {
  label: string;
  value: number;
  total: number;
  barClassName: string;
}) {
  const pct = total > 0 ? Math.min((value / total) * 100, 100) : 0;
  return (
    <div className="space-y-1">
      <div className="text-muted-foreground flex items-center justify-between text-xs">
        <span>{label}</span>
        <span className="tabular-nums">
          {value} / {total}
        </span>
      </div>
      <div className="bg-secondary h-1.5 w-full overflow-hidden rounded-full">
        <div
          className={cn("h-full transition-all", barClassName)}
          style={{ width: `${pct}%` }}
        />
      </div>
    </div>
  );
}

function Legend() {
  const items: { status: ChipStatus; label: string }[] = [
    { status: "complete", label: "Complete" },
    { status: "partial", label: "Partial" },
    { status: "needs_metadata", label: "Needs metadata" },
    { status: "missing", label: "Missing" },
  ];
  return (
    <div className="text-muted-foreground flex flex-wrap gap-x-4 gap-y-1 text-xs">
      {items.map((it) => (
        <span key={it.status} className="inline-flex items-center gap-1.5">
          <span
            className={cn(
              "inline-block h-3 w-3 rounded-sm",
              STATUS_CLASSES[it.status],
            )}
          />
          {it.label}
        </span>
      ))}
    </div>
  );
}

/** Detail panel for the selected chip — what's missing + a link to the issue
 *  (owned) or a note that it's an inferred gap (missing). */
function SelectionDetail({
  selection,
  seriesSlug,
  onClear,
}: {
  selection: Exclude<Selection, null>;
  seriesSlug: string;
  onClear: () => void;
}) {
  if (selection.kind === "missing") {
    return (
      <div className="border-border/60 text-muted-foreground rounded-md border border-dashed p-3 text-sm">
        <div className="flex items-center justify-between">
          <span className="text-foreground font-medium">#{selection.n}</span>
          <ClearButton onClear={onClear} />
        </div>
        <p className="mt-1 text-xs">
          Not in your library — inferred missing from the surrounding run.
        </p>
      </div>
    );
  }

  const { entry } = selection;
  const status = entry.metadata_tier as ChipStatus;
  const heading = entry.number_raw
    ? `#${entry.number_raw}`
    : (entry.title ?? "Issue");
  return (
    <div className="border-border/60 rounded-md border p-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-foreground text-sm font-medium">
              {heading}
            </span>
            {entry.title && entry.number_raw && (
              <span className="text-muted-foreground truncate text-xs">
                {entry.title}
              </span>
            )}
            <span
              className={cn(
                "inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium",
                STATUS_CLASSES[status],
              )}
            >
              {STATUS_LABELS[status]}
            </span>
          </div>
          <p className="text-muted-foreground mt-1 text-xs">
            {entry.missing_core.length > 0
              ? `Missing: ${metadataFieldLabels(entry.missing_core)}`
              : "All core metadata present."}
          </p>
        </div>
        <ClearButton onClear={onClear} />
      </div>
      <Link
        href={issueUrl(seriesSlug, entry.slug)}
        className="text-primary mt-2 inline-flex items-center gap-1 text-xs font-medium hover:underline"
      >
        Open issue
        <ChevronRight className="h-3.5 w-3.5" />
      </Link>
    </div>
  );
}

function ClearButton({ onClear }: { onClear: () => void }) {
  return (
    <button
      type="button"
      onClick={onClear}
      className="text-muted-foreground hover:text-foreground text-xs"
      aria-label="Dismiss"
    >
      ✕
    </button>
  );
}

function StatePill({ state }: { state: string }) {
  if (state === "complete") {
    return (
      <span
        className={cn(
          "inline-flex items-center gap-1 rounded-full px-2.5 py-1 text-xs font-medium",
          statusTone("success"),
        )}
      >
        <CheckCircle2 className="h-3.5 w-3.5" />
        Complete
      </span>
    );
  }
  if (state === "incomplete") {
    return (
      <span
        className={cn(
          "inline-flex items-center gap-1 rounded-full px-2.5 py-1 text-xs font-medium",
          statusTone("warning"),
        )}
      >
        <AlertCircle className="h-3.5 w-3.5" />
        Incomplete
      </span>
    );
  }
  return (
    <span className="text-muted-foreground bg-muted inline-flex items-center rounded-full px-2.5 py-1 text-xs font-medium">
      Total unknown
    </span>
  );
}
