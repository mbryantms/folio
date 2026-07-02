"use client";

import * as React from "react";
import Link from "next/link";
import type { ColumnDef } from "@tanstack/react-table";
import { ExternalLink, Loader2 } from "lucide-react";

import { BackupStorageCard } from "@/components/admin/library/BackupStorageCard";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable } from "@/components/ui/data-table";
import { FilterPill } from "@/components/ui/filter-pill";
import { Skeleton } from "@/components/ui/skeleton";
import { useHealthIssuesInfinite } from "@/lib/api/queries";
import {
  useDismissHealthIssue,
  useTriggerDeepValidate,
} from "@/lib/api/mutations";
import type { HealthIssueView } from "@/lib/api/types";
import {
  HEALTH_SEVERITIES,
  type HealthSeverityFilter,
} from "@/components/admin/severity";

type StatusFilter = "open" | "resolved" | "dismissed" | "all";

const STATUSES: StatusFilter[] = ["open", "resolved", "dismissed", "all"];

/** Synthetic rows (e.g. the metadata-drift summary) carry a `synth:` id and
 *  have no dismiss/resolve verbs — the server rejects mutations against them. */
function isSynthetic(id: string): boolean {
  return id.startsWith("synth:");
}

function severityVariant(s: string): "secondary" | "destructive" {
  return s === "error" ? "destructive" : "secondary";
}

/** Health payloads serialize as adjacently-tagged `{kind, data: {…}}`
 *  (see `library/health.rs::IssueKind`); the synthesized drift row is
 *  flat. Unwrap to the field object either way — reading top-level keys
 *  on nested payloads is why summaries used to render raw JSON
 *  (audit UX-3). */
function payloadFields(p: unknown): Record<string, unknown> {
  if (!p || typeof p !== "object") return {};
  const obj = p as Record<string, unknown>;
  if (obj.data && typeof obj.data === "object") {
    return obj.data as Record<string, unknown>;
  }
  return obj;
}

function payloadSummary(kind: string, p: unknown): string {
  const obj = payloadFields(p);
  if (Object.keys(obj).length === 0) return "";
  const path =
    typeof obj.path === "string"
      ? obj.path
      : typeof obj.file_path === "string"
        ? obj.file_path
        : "";

  // Synth drift row (writeback mode): counts, not a path — the raw JSON
  // rendered here before.
  if (kind === "MetadataDriftFromXml") {
    const issues =
      typeof obj.drifted_issue_count === "number"
        ? obj.drifted_issue_count
        : "?";
    const series =
      typeof obj.drifted_series_count === "number"
        ? obj.drifted_series_count
        : "?";
    return `${issues} issue${issues === 1 ? "" : "s"} across ${series} series ${
      issues === 1 ? "has" : "have"
    } user edits not yet written to XML`;
  }

  // Tranche A of recovery-visibility — render the structured fields
  // these new kinds carry so the operator sees the count + cause
  // inline rather than having to expand the payload.
  if (kind === "RecoveredArchive") {
    const technique =
      typeof obj.technique === "string" ? obj.technique : "unknown";
    return path
      ? `${path} — recovered (${technique})`
      : `recovered (${technique})`;
  }
  if (kind === "SkippedArchiveEntries") {
    const dropped = typeof obj.dropped === "number" ? obj.dropped : "?";
    const total = typeof obj.total === "number" ? obj.total : "?";
    const reason = typeof obj.reason === "string" ? obj.reason : "soft defense";
    const suffix = `${dropped} of ${total} entries dropped (${reason})`;
    return path ? `${path} — ${suffix}` : suffix;
  }

  const keys = ["path", "file_path", "folder", "reason", "error", "details"];
  const parts: string[] = [];
  for (const k of keys) {
    const v = obj[k];
    if (typeof v === "string" && v.length > 0) parts.push(v);
  }
  if (parts.length > 0) return parts.join(" — ");
  return JSON.stringify(obj);
}

/** Entity refs a health payload may carry (audit UX-3). `issue_id` rides
 *  the `/issues/{id}` permalink redirect; series ids resolve directly —
 *  `/series/{slug}` accepts a UUID. Today the id-bearing payloads are the
 *  synth drift row (`affected_series_ids`) and any future kind that adds
 *  `series_id`/`issue_id`; path-only kinds render no links. */
function payloadRefs(p: unknown): { seriesIds: string[]; issueId?: string } {
  const obj = payloadFields(p);
  const seriesIds: string[] = [];
  if (typeof obj.series_id === "string") seriesIds.push(obj.series_id);
  if (Array.isArray(obj.affected_series_ids)) {
    for (const v of obj.affected_series_ids) {
      if (typeof v === "string") seriesIds.push(v);
    }
  }
  return {
    seriesIds,
    issueId: typeof obj.issue_id === "string" ? obj.issue_id : undefined,
  };
}

/** How many series links a drift row renders before truncating with a
 *  "+N more" note — a library-wide drift can reference hundreds. */
const MAX_SERIES_LINKS = 5;

/** Jump-to-item links for a health row's payload refs — the fix action
 *  usually lives on the issue/series page, so hiding rows shouldn't be
 *  the path of least resistance (audit UX-3). */
function PayloadRefLinks({ payload }: { payload: unknown }) {
  const refs = payloadRefs(payload);
  if (refs.seriesIds.length === 0 && !refs.issueId) return null;
  const linkCls =
    "text-foreground/80 hover:text-foreground inline-flex items-center gap-1 text-xs underline-offset-2 hover:underline";
  const shown = refs.seriesIds.slice(0, MAX_SERIES_LINKS);
  const extra = refs.seriesIds.length - shown.length;
  return (
    <span className="mt-1 flex flex-wrap gap-3">
      {refs.issueId ? (
        <Link href={`/issues/${refs.issueId}`} className={linkCls}>
          <ExternalLink className="size-3" aria-hidden="true" />
          View issue
        </Link>
      ) : null}
      {shown.map((id, i) => (
        <Link key={id} href={`/series/${id}`} className={linkCls}>
          <ExternalLink className="size-3" aria-hidden="true" />
          {shown.length > 1 ? `View series ${i + 1}` : "View series"}
        </Link>
      ))}
      {extra > 0 ? (
        <span className="text-muted-foreground text-xs">+{extra} more</span>
      ) : null}
    </span>
  );
}

export function HealthIssuesTable({ libraryId }: { libraryId: string }) {
  const [status, setStatus] = React.useState<StatusFilter>("open");
  const [severity, setSeverity] = React.useState<HealthSeverityFilter>("all");
  const [kind, setKind] = React.useState<string | null>(null);
  const [confirmDeepValidate, setConfirmDeepValidate] = React.useState(false);

  const {
    data,
    isLoading,
    error,
    hasNextPage,
    isFetchingNextPage,
    fetchNextPage,
  } = useHealthIssuesInfinite(libraryId, { status, severity, kind });
  const dismiss = useDismissHealthIssue(libraryId);
  const deepValidate = useTriggerDeepValidate(libraryId);

  const items = React.useMemo(
    () => data?.pages.flatMap((p) => p.items) ?? [],
    [data],
  );
  // Counts come from the first page only (server-computed summary). Status
  // tallies are library-wide; kind facets are scoped to status + severity.
  const counts = data?.pages[0]?.counts;
  const statusCounts: Record<StatusFilter, number> = {
    open: counts?.open ?? 0,
    resolved: counts?.resolved ?? 0,
    dismissed: counts?.dismissed ?? 0,
    all: counts?.total ?? 0,
  };
  const kindFacets = counts?.kinds ?? [];

  const columns = React.useMemo<ColumnDef<HealthIssueView>[]>(
    () => [
      {
        accessorKey: "severity",
        header: "Severity",
        cell: ({ row }) => (
          <Badge
            variant={severityVariant(row.original.severity)}
            className="uppercase"
          >
            {row.original.severity}
          </Badge>
        ),
      },
      {
        accessorKey: "kind",
        header: "Kind",
        cell: ({ row }) => (
          <span className="font-mono text-xs">{row.original.kind}</span>
        ),
      },
      {
        id: "summary",
        header: "Summary",
        cell: ({ row }) => (
          <span className="text-muted-foreground block text-xs leading-relaxed wrap-anywhere whitespace-normal">
            {payloadSummary(row.original.kind, row.original.payload)}
            <PayloadRefLinks payload={row.original.payload} />
          </span>
        ),
      },
      {
        accessorKey: "first_seen_at",
        header: "First seen",
        cell: ({ row }) => (
          <span className="text-muted-foreground text-xs">
            {new Date(row.original.first_seen_at).toLocaleString()}
          </span>
        ),
      },
      {
        accessorKey: "last_seen_at",
        header: "Last seen",
        cell: ({ row }) => (
          <span className="text-muted-foreground text-xs">
            {new Date(row.original.last_seen_at).toLocaleString()}
          </span>
        ),
      },
      {
        id: "actions",
        header: "",
        cell: ({ row }) => {
          // Synthetic rows have no dismiss verb.
          if (isSynthetic(row.original.id)) return null;
          const isOpen =
            !row.original.resolved_at && !row.original.dismissed_at;
          if (!isOpen) {
            return (
              <span className="text-muted-foreground text-xs">
                {row.original.resolved_at ? "Resolved" : "Dismissed"}
              </span>
            );
          }
          return (
            <Button
              size="sm"
              variant="ghost"
              onClick={(e) => {
                e.stopPropagation();
                dismiss.mutate({ issueId: row.original.id });
              }}
              // Per-row pending (D7): only the row being dismissed is
              // disabled, not every Dismiss button.
              disabled={
                dismiss.isPending &&
                dismiss.variables?.issueId === row.original.id
              }
            >
              Dismiss
            </Button>
          );
        },
      },
    ],
    [dismiss],
  );

  if (isLoading && !data) return <Skeleton className="h-64 w-full" />;
  if (error) {
    return <p className="text-destructive text-sm">{error.message}</p>;
  }

  return (
    <div className="space-y-4">
      <BackupStorageCard libraryId={libraryId} />
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-muted-foreground max-w-prose text-xs">
          Page-decode failures don&apos;t surface during normal scans (they only
          read headers). Run a deep validate to walk every active page through
          the image decoder — slow, opt-in, and one library at a time. Findings
          appear as <span className="font-mono">UnreadablePage</span> rows below
          as the run progresses.
        </p>
        <Button
          size="sm"
          variant="outline"
          onClick={() => setConfirmDeepValidate(true)}
          disabled={deepValidate.isPending}
        >
          Validate page integrity
        </Button>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        {STATUSES.map((s) => (
          <FilterPill
            key={s}
            active={status === s}
            onClick={() => setStatus(s)}
            count={statusCounts[s]}
            className="capitalize"
          >
            {s}
          </FilterPill>
        ))}
        <span className="text-muted-foreground ml-2 text-xs">Severity:</span>
        {(["all", ...HEALTH_SEVERITIES] as HealthSeverityFilter[]).map((s) => (
          <FilterPill
            key={s}
            active={severity === s}
            onClick={() => setSeverity(s)}
            className="capitalize"
          >
            {s}
          </FilterPill>
        ))}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-muted-foreground text-xs">Kind:</span>
        <FilterPill active={kind === null} onClick={() => setKind(null)}>
          All kinds
        </FilterPill>
        {kindFacets.map((facet) => (
          <FilterPill
            key={facet.kind}
            active={kind === facet.kind}
            onClick={() =>
              setKind((cur) => (cur === facet.kind ? null : facet.kind))
            }
            count={facet.count}
            className="font-mono"
          >
            {facet.kind}
          </FilterPill>
        ))}
      </div>
      <DataTable
        columns={columns}
        data={items}
        emptyMessage="No matching issues."
      />
      {hasNextPage ? (
        <div className="flex justify-center">
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => void fetchNextPage()}
            disabled={isFetchingNextPage}
          >
            {isFetchingNextPage ? (
              <>
                <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />
                Loading more…
              </>
            ) : (
              "Load more"
            )}
          </Button>
        </div>
      ) : null}
      <AlertDialog
        open={confirmDeepValidate}
        onOpenChange={setConfirmDeepValidate}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Validate every page?</AlertDialogTitle>
            <AlertDialogDescription>
              Deep validation decodes every page in every active issue in this
              library. On very large libraries this can take one to two hours of
              CPU. Findings appear here as the run progresses.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                deepValidate.mutate();
                setConfirmDeepValidate(false);
              }}
              disabled={deepValidate.isPending}
            >
              Start validation
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
