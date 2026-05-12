"use client";

import * as React from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Eye, EyeOff } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable } from "@/components/ui/data-table";
import { Skeleton } from "@/components/ui/skeleton";
import { useHealthIssues } from "@/lib/api/queries";
import { useDismissHealthIssue } from "@/lib/api/mutations";
import type { HealthIssueView } from "@/lib/api/types";
import { cn } from "@/lib/utils";

type Filter = "open" | "resolved" | "dismissed" | "all";
type Severity = "all" | "info" | "warn" | "error";

function severityVariant(s: string): "secondary" | "destructive" {
  return s === "error" ? "destructive" : "secondary";
}

function payloadSummary(p: unknown): string {
  if (!p || typeof p !== "object") return "";
  const obj = p as Record<string, unknown>;
  const keys = [
    "path",
    "file_path",
    "series_id",
    "issue_id",
    "reason",
    "details",
  ];
  for (const k of keys) {
    const v = obj[k];
    if (typeof v === "string" && v.length > 0) return v;
  }
  return JSON.stringify(p);
}

export function HealthIssuesTable({ libraryId }: { libraryId: string }) {
  const { data, isLoading, error } = useHealthIssues(libraryId);
  const dismiss = useDismissHealthIssue(libraryId);
  const [filter, setFilter] = React.useState<Filter>("open");
  const [severity, setSeverity] = React.useState<Severity>("all");
  const [focusedKind, setFocusedKind] = React.useState<string | null>(null);
  const [hiddenKinds, setHiddenKinds] = React.useState<Set<string>>(
    () => new Set(),
  );

  const baseFiltered = React.useMemo(() => {
    if (!data) return [];
    return data.filter((row) => {
      const isResolved = !!row.resolved_at;
      const isDismissed = !!row.dismissed_at;
      const isOpen = !isResolved && !isDismissed;
      if (filter === "open" && !isOpen) return false;
      if (filter === "resolved" && !isResolved) return false;
      if (filter === "dismissed" && !isDismissed) return false;
      if (severity !== "all" && row.severity !== severity) return false;
      return true;
    });
  }, [data, filter, severity]);

  const kindCounts = React.useMemo(() => {
    const counts = new Map<string, number>();
    for (const row of baseFiltered) {
      counts.set(row.kind, (counts.get(row.kind) ?? 0) + 1);
    }
    return Array.from(counts.entries())
      .map(([kind, count]) => ({ kind, count }))
      .sort((a, b) => a.kind.localeCompare(b.kind));
  }, [baseFiltered]);

  const activeFocusedKind =
    focusedKind && kindCounts.some((k) => k.kind === focusedKind)
      ? focusedKind
      : null;

  const filtered = React.useMemo(
    () =>
      baseFiltered.filter((row) => {
        if (hiddenKinds.has(row.kind)) return false;
        if (activeFocusedKind && row.kind !== activeFocusedKind) return false;
        return true;
      }),
    [activeFocusedKind, baseFiltered, hiddenKinds],
  );

  const toggleKindHidden = React.useCallback((kind: string) => {
    setFocusedKind((current) => (current === kind ? null : current));
    setHiddenKinds((current) => {
      const next = new Set(current);
      if (next.has(kind)) {
        next.delete(kind);
      } else {
        next.add(kind);
      }
      return next;
    });
  }, []);

  const showAllKinds = React.useCallback(() => {
    setFocusedKind(null);
    setHiddenKinds(new Set());
  }, []);

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
          <span className="text-muted-foreground block text-xs leading-relaxed [overflow-wrap:anywhere] whitespace-normal">
            {payloadSummary(row.original.payload)}
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
              disabled={dismiss.isPending}
            >
              Dismiss
            </Button>
          );
        },
      },
    ],
    [dismiss],
  );

  if (isLoading) return <Skeleton className="h-64 w-full" />;
  if (error) {
    return <p className="text-destructive text-sm">{error.message}</p>;
  }

  const counts = {
    open: data?.filter((d) => !d.resolved_at && !d.dismissed_at).length ?? 0,
    resolved: data?.filter((d) => !!d.resolved_at).length ?? 0,
    dismissed: data?.filter((d) => !!d.dismissed_at).length ?? 0,
    all: data?.length ?? 0,
  };

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        {(["open", "resolved", "dismissed", "all"] as Filter[]).map((f) => (
          <button
            key={f}
            type="button"
            onClick={() => setFilter(f)}
            className={cn(
              "rounded-full border px-3 py-1 text-xs font-medium tracking-wider uppercase transition-colors",
              filter === f
                ? "border-primary bg-primary/10 text-primary"
                : "border-border text-muted-foreground hover:text-foreground",
            )}
          >
            {f} ({counts[f]})
          </button>
        ))}
        <span className="text-muted-foreground ml-2 text-xs">Severity:</span>
        {(["all", "info", "warn", "error"] as Severity[]).map((s) => (
          <button
            key={s}
            type="button"
            onClick={() => setSeverity(s)}
            className={cn(
              "rounded-full border px-2.5 py-0.5 text-[11px] tracking-wider uppercase transition-colors",
              severity === s
                ? "border-foreground/40 text-foreground"
                : "border-border text-muted-foreground hover:text-foreground",
            )}
          >
            {s}
          </button>
        ))}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-muted-foreground text-xs">Kind:</span>
        <button
          type="button"
          onClick={showAllKinds}
          className={cn(
            "rounded-full border px-2.5 py-0.5 text-[11px] tracking-wider uppercase transition-colors",
            !activeFocusedKind && hiddenKinds.size === 0
              ? "border-foreground/40 text-foreground"
              : "border-border text-muted-foreground hover:text-foreground",
          )}
        >
          All kinds
        </button>
        {kindCounts.map(({ kind, count }) => {
          const hidden = hiddenKinds.has(kind);
          const focused = activeFocusedKind === kind && !hidden;
          return (
            <span
              key={kind}
              className={cn(
                "inline-flex overflow-hidden rounded-full border text-[11px] transition-colors",
                focused
                  ? "border-primary bg-primary/10 text-primary"
                  : hidden
                    ? "border-border bg-muted/40 text-muted-foreground"
                    : "border-border text-muted-foreground",
              )}
            >
              <button
                type="button"
                disabled={hidden}
                onClick={() => setFocusedKind(focused ? null : kind)}
                className={cn(
                  "hover:text-foreground disabled:hover:text-muted-foreground px-2.5 py-0.5 font-mono transition-colors disabled:cursor-not-allowed disabled:line-through",
                )}
                title={hidden ? `${kind} is hidden` : `Show only ${kind}`}
              >
                {kind} ({count})
              </button>
              <button
                type="button"
                onClick={() => toggleKindHidden(kind)}
                className="border-border hover:bg-muted hover:text-foreground border-l px-1.5 py-0.5 transition-colors"
                aria-label={hidden ? `Show ${kind}` : `Hide ${kind}`}
                title={hidden ? `Show ${kind}` : `Hide ${kind}`}
              >
                {hidden ? (
                  <Eye className="h-3 w-3" />
                ) : (
                  <EyeOff className="h-3 w-3" />
                )}
              </button>
            </span>
          );
        })}
      </div>
      <DataTable
        columns={columns}
        data={filtered}
        emptyMessage="No matching issues."
      />
    </div>
  );
}
