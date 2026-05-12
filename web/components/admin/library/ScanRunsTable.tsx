"use client";

import * as React from "react";
import type { ColumnDef } from "@tanstack/react-table";
import Link from "next/link";

import { Badge } from "@/components/ui/badge";
import { DataTable } from "@/components/ui/data-table";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useScanRuns } from "@/lib/api/queries";
import { cn } from "@/lib/utils";
import type { ScanRunKind, ScanRunView } from "@/lib/api/types";

type Filter = "all" | ScanRunKind;

const FILTERS: { value: Filter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "library", label: "Library" },
  { value: "series", label: "Series" },
  { value: "issue", label: "Issue" },
];

function stateVariant(s: string): "default" | "secondary" | "destructive" {
  if (s === "failed") return "destructive";
  if (s === "complete" || s === "completed") return "default";
  return "secondary";
}

function kindVariant(k: ScanRunKind): "default" | "secondary" | "outline" {
  if (k === "library") return "default";
  if (k === "series") return "secondary";
  return "outline";
}

function durationFromStats(stats: unknown): string {
  if (!stats || typeof stats !== "object") return "";
  const ms =
    (stats as Record<string, unknown>).elapsed_ms ??
    (stats as Record<string, unknown>).duration_ms;
  if (typeof ms !== "number") return "";
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  return `${(ms / 60_000).toFixed(1)}m`;
}

/** Pull the user-visible result counts out of the stats blob. The scanner
 *  emits both the M3-era `files_*` keys and the legacy short keys; we
 *  prefer the long ones (more accurate) and fall back gracefully. */
function resultsFromStats(stats: unknown): {
  added: number;
  updated: number;
  removed: number;
  restored: number;
  duplicate: number;
} {
  if (!stats || typeof stats !== "object") {
    return { added: 0, updated: 0, removed: 0, restored: 0, duplicate: 0 };
  }
  const s = stats as Record<string, unknown>;
  const num = (k: string): number =>
    typeof s[k] === "number" ? (s[k] as number) : 0;
  return {
    added: num("files_added") || num("added"),
    updated: num("files_updated") || num("updated"),
    removed: num("issues_removed") || num("removed"),
    restored: num("issues_restored"),
    duplicate: num("files_duplicate"),
  };
}

export function ScanRunsTable({ libraryId }: { libraryId: string }) {
  const [filter, setFilter] = React.useState<Filter>("all");
  const { data, isLoading, error } = useScanRuns(libraryId, { kind: filter });

  const columns = React.useMemo<ColumnDef<ScanRunView>[]>(
    () => [
      {
        accessorKey: "started_at",
        header: "Started",
        cell: ({ row }) => (
          <span className="text-xs">
            {new Date(row.original.started_at).toLocaleString()}
          </span>
        ),
      },
      {
        accessorKey: "kind",
        header: "Type",
        cell: ({ row }) => <KindCell run={row.original} />,
      },
      {
        accessorKey: "state",
        header: "State",
        cell: ({ row }) => (
          <Badge
            variant={stateVariant(row.original.state)}
            className="uppercase"
          >
            {row.original.state}
          </Badge>
        ),
      },
      {
        id: "duration",
        header: "Duration",
        cell: ({ row }) => (
          <span className="text-muted-foreground text-xs">
            {durationFromStats(row.original.stats) || "—"}
          </span>
        ),
      },
      {
        id: "results",
        header: "Results",
        cell: ({ row }) => <ResultsCell run={row.original} />,
      },
    ],
    [],
  );

  if (isLoading && !data) return <Skeleton className="h-64 w-full" />;
  if (error) return <p className="text-destructive text-sm">{error.message}</p>;

  return (
    <div className="space-y-4">
      <Tabs
        value={filter}
        onValueChange={(v) => setFilter(v as Filter)}
        className="w-fit"
      >
        <TabsList>
          {FILTERS.map((f) => (
            <TabsTrigger key={f.value} value={f.value}>
              {f.label}
            </TabsTrigger>
          ))}
        </TabsList>
      </Tabs>
      <DataTable
        columns={columns}
        data={data ?? []}
        emptyMessage={
          filter === "all" ? "No scans yet." : `No ${filter} scans yet.`
        }
        renderExpanded={(row) => <ScanRunDetails run={row.original} />}
      />
    </div>
  );
}

function ScanRunDetails({ run }: { run: ScanRunView }) {
  const stats = statsRecord(run.stats);
  const phases = phaseEntries(stats);
  return (
    <div className="bg-background/60 space-y-4 rounded-md p-3">
      <div className="grid gap-3 text-xs sm:grid-cols-2 lg:grid-cols-4">
        <DetailMetric
          label="Duration"
          value={durationFromStats(run.stats) || "—"}
        />
        <DetailMetric
          label="Files/sec"
          value={formatRate(numberStat(stats, "files_per_sec"))}
        />
        <DetailMetric
          label="Bytes/sec"
          value={formatBytesRate(numberStat(stats, "bytes_per_sec"))}
        />
        <DetailMetric
          label="Skipped folders"
          value={String(numberStat(stats, "series_skipped_unchanged") ?? 0)}
        />
      </div>
      <div className="space-y-2">
        <h4 className="text-muted-foreground text-xs font-semibold tracking-widest uppercase">
          Phase breakdown
        </h4>
        {phases.length === 0 ? (
          <p className="text-muted-foreground text-xs">
            No phase timing data recorded for this run.
          </p>
        ) : (
          <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
            {phases.map(([phase, ms]) => (
              <DetailMetric
                key={phase}
                label={phase.replaceAll("_", " ")}
                value={formatMs(ms)}
              />
            ))}
          </div>
        )}
      </div>
      <details className="group">
        <summary className="text-muted-foreground cursor-pointer text-xs font-semibold tracking-widest uppercase">
          Developer details
        </summary>
        <pre className="bg-background/80 mt-2 overflow-x-auto rounded p-3 font-mono text-[11px] leading-relaxed">
          {JSON.stringify(run.stats ?? {}, null, 2)}
        </pre>
      </details>
    </div>
  );
}

function DetailMetric({ label, value }: { label: string; value: string }) {
  return (
    <div className="border-border rounded-md border p-3">
      <dt className="text-muted-foreground truncate text-[10px] font-medium tracking-widest uppercase">
        {label}
      </dt>
      <dd className="text-foreground mt-1 truncate font-medium tabular-nums">
        {value}
      </dd>
    </div>
  );
}

function statsRecord(stats: unknown): Record<string, unknown> {
  return stats && typeof stats === "object"
    ? (stats as Record<string, unknown>)
    : {};
}

function numberStat(
  stats: Record<string, unknown>,
  key: string,
): number | null {
  const value = stats[key];
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function phaseEntries(stats: Record<string, unknown>): [string, number][] {
  const raw = stats.phase_timings_ms;
  if (!raw || typeof raw !== "object") return [];
  return Object.entries(raw as Record<string, unknown>)
    .filter((entry): entry is [string, number] => typeof entry[1] === "number")
    .sort((a, b) => b[1] - a[1]);
}

function formatMs(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  return `${(ms / 60_000).toFixed(1)}m`;
}

function formatRate(value: number | null): string {
  if (typeof value !== "number") return "—";
  return value >= 100 ? value.toFixed(0) : value.toFixed(1);
}

function formatBytesRate(value: number | null): string {
  if (typeof value !== "number") return "—";
  if (value < 1024) return `${value.toFixed(0)} B/s`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB/s`;
  if (value < 1024 * 1024 * 1024) {
    return `${(value / 1024 / 1024).toFixed(1)} MiB/s`;
  }
  return `${(value / 1024 / 1024 / 1024).toFixed(1)} GiB/s`;
}

function KindCell({ run }: { run: ScanRunView }) {
  const label =
    run.kind === "library"
      ? "Library"
      : run.kind === "series"
        ? "Series"
        : "Issue";
  return (
    <div className="flex flex-col gap-1">
      <Badge
        variant={kindVariant(run.kind)}
        className="w-fit text-[10px] uppercase"
      >
        {label}
      </Badge>
      <KindTarget run={run} />
    </div>
  );
}

function KindTarget({ run }: { run: ScanRunView }) {
  // Library scans have no narrower target — leave the cell label-only.
  if (run.kind === "library") return null;

  // Issue scans surface the originating issue (admin-only context — the
  // issue page is reachable by id even without library access here).
  if (run.kind === "issue" && run.issue_id) {
    return (
      <Link
        href={`/issues/${run.issue_id}`}
        className={cn(
          "text-muted-foreground hover:text-foreground text-[11px] underline-offset-2 hover:underline",
        )}
        title={run.series_name ?? undefined}
      >
        Issue {run.issue_id.slice(0, 8)}…
      </Link>
    );
  }

  // Series scans (and issue scans without a remembered issue_id) link to
  // the series. Fall back to the raw id when the joined name is missing.
  if (run.series_id) {
    return (
      <Link
        href={`/series/${run.series_id}`}
        className="text-muted-foreground hover:text-foreground text-[11px] underline-offset-2 hover:underline"
        title={run.series_name ?? undefined}
      >
        {run.series_name ?? "(deleted series)"}
      </Link>
    );
  }
  return null;
}

function ResultsCell({ run }: { run: ScanRunView }) {
  const r = resultsFromStats(run.stats);
  const parts: string[] = [];
  if (r.added > 0) parts.push(`+${r.added} added`);
  if (r.updated > 0) parts.push(`~${r.updated} updated`);
  if (r.removed > 0) parts.push(`−${r.removed} removed`);
  if (r.restored > 0) parts.push(`↻${r.restored} restored`);
  if (r.duplicate > 0) parts.push(`⊘${r.duplicate} duplicate`);
  if (run.error) {
    return (
      <span className="text-destructive text-xs">
        {run.error.length > 80 ? `${run.error.slice(0, 80)}…` : run.error}
      </span>
    );
  }
  if (parts.length === 0) {
    return <span className="text-muted-foreground text-xs">no changes</span>;
  }
  return (
    <span className="text-muted-foreground text-xs">{parts.join("  ·  ")}</span>
  );
}
