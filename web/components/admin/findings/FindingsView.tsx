"use client";

import * as React from "react";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import {
  AlertCircle,
  AlertTriangle,
  CheckCircle2,
  Info,
  Loader2,
  XCircle,
} from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/tabs";
import { useDismissHealthIssue } from "@/lib/api/mutations";
import {
  useAdminHealthIssues,
  useAdminScanRuns,
  useLibraryList,
} from "@/lib/api/queries";
import type { CrossLibHealthIssueView } from "@/lib/api/types";
import { cn } from "@/lib/utils";

type Tab = "health" | "scans";
type Severity = "all" | "error" | "warning" | "info";

/**
 * Cross-library findings page. Three rails in one shell:
 *
 *  1. Open health issues — joined `library_health_issue` x `library`,
 *     filter by severity / library, inline dismiss action.
 *  2. Recent scan runs — joined `scan_run` x `library`, color-coded
 *     state, jump back to the per-library history page.
 *
 * Filter state is reflected to URL query params so the dashboard
 * cards can deep-link into specific subsets ("show only failed scans
 * from last 7 days" etc.) and the page is browser-back-friendly.
 *
 * The "active scans" rail is intentionally omitted from this page —
 * it lives on the per-library `/admin/libraries/{slug}/scan` route
 * which already mounts the WS-driven `LiveScanProgress` view. The
 * dashboard's Scans card surfaces the count and links there.
 */
export function FindingsView() {
  const router = useRouter();
  const sp = useSearchParams();

  const tab = (sp.get("tab") as Tab) ?? "health";
  const severity = (sp.get("severity") as Severity) ?? "all";
  const libraryId = sp.get("library_id") ?? "all";
  const state = sp.get("state") ?? "all";

  const { data: libraries } = useLibraryList();

  function setParam(key: string, value: string | null) {
    const next = new URLSearchParams(sp);
    if (value === null || value === "" || value === "all") {
      next.delete(key);
    } else {
      next.set(key, value);
    }
    router.replace(`/admin/findings?${next.toString()}`, { scroll: false });
  }

  return (
    <Tabs value={tab} onValueChange={(v) => setParam("tab", v)}>
      <TabsList>
        <TabsTrigger value="health">Open health issues</TabsTrigger>
        <TabsTrigger value="scans">Scan runs</TabsTrigger>
      </TabsList>

      <TabsContent value="health" className="mt-4 space-y-3">
        <FilterRow>
          <SeverityChips value={severity} onChange={(v) => setParam("severity", v)} />
          <LibraryFilter
            libraries={libraries ?? []}
            value={libraryId}
            onChange={(v) => setParam("library_id", v)}
          />
        </FilterRow>
        <HealthIssuesRail
          severity={severity}
          libraryId={libraryId}
        />
      </TabsContent>

      <TabsContent value="scans" className="mt-4 space-y-3">
        <FilterRow>
          <StateChips value={state} onChange={(v) => setParam("state", v)} />
          <LibraryFilter
            libraries={libraries ?? []}
            value={libraryId}
            onChange={(v) => setParam("library_id", v)}
          />
        </FilterRow>
        <ScanRunsRail state={state} libraryId={libraryId} />
      </TabsContent>
    </Tabs>
  );
}

function FilterRow({ children }: { children: React.ReactNode }) {
  return (
    <div className="border-border/60 flex flex-wrap items-center gap-2 rounded-md border p-3">
      {children}
    </div>
  );
}

function SeverityChips({
  value,
  onChange,
}: {
  value: Severity;
  onChange: (v: Severity) => void;
}) {
  const options: { v: Severity; label: string }[] = [
    { v: "all", label: "All" },
    { v: "error", label: "Errors" },
    { v: "warning", label: "Warnings" },
    { v: "info", label: "Info" },
  ];
  return (
    <div className="flex items-center gap-1.5">
      <span className="text-muted-foreground mr-1 text-xs uppercase">Severity</span>
      {options.map((o) => (
        <button
          key={o.v}
          type="button"
          onClick={() => onChange(o.v)}
          className={cn(
            "rounded-full border px-2.5 py-0.5 text-xs transition-colors",
            value === o.v
              ? "border-primary bg-primary/10 text-primary"
              : "border-border text-muted-foreground hover:text-foreground",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

function StateChips({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  const options = [
    { v: "all", label: "All" },
    { v: "running", label: "Running" },
    { v: "complete", label: "Complete" },
    { v: "failed", label: "Failed" },
    { v: "cancelled", label: "Cancelled" },
  ];
  return (
    <div className="flex items-center gap-1.5">
      <span className="text-muted-foreground mr-1 text-xs uppercase">State</span>
      {options.map((o) => (
        <button
          key={o.v}
          type="button"
          onClick={() => onChange(o.v)}
          className={cn(
            "rounded-full border px-2.5 py-0.5 text-xs transition-colors",
            value === o.v
              ? "border-primary bg-primary/10 text-primary"
              : "border-border text-muted-foreground hover:text-foreground",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

function LibraryFilter({
  libraries,
  value,
  onChange,
}: {
  libraries: { id: string; name: string }[];
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <div className="flex items-center gap-1.5">
      <span className="text-muted-foreground mr-1 text-xs uppercase">Library</span>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="border-border bg-background rounded-md border px-2 py-1 text-xs"
      >
        <option value="all">All libraries</option>
        {libraries.map((l) => (
          <option key={l.id} value={l.id}>
            {l.name}
          </option>
        ))}
      </select>
    </div>
  );
}

function HealthIssuesRail({
  severity,
  libraryId,
}: {
  severity: Severity;
  libraryId: string;
}) {
  const filters = {
    severity: severity === "all" ? undefined : severity,
    library_id: libraryId === "all" ? undefined : libraryId,
    limit: 50,
  };
  const { data, isLoading, error } = useAdminHealthIssues(filters);

  if (isLoading) return <Skeleton className="h-64 w-full" />;
  if (error) return <p className="text-destructive text-sm">{error.message}</p>;
  const items = data?.items ?? [];
  if (items.length === 0) {
    return (
      <Card>
        <CardContent className="text-muted-foreground py-12 text-center text-sm">
          <CheckCircle2 className="text-primary mx-auto mb-2 h-6 w-6" />
          No matching health issues. Your libraries look clean.
        </CardContent>
      </Card>
    );
  }

  return (
    <div className="space-y-2">
      {items.map((row) => (
        <HealthIssueRow key={row.id} row={row} />
      ))}
      {data?.next_cursor ? (
        <p className="text-muted-foreground text-center text-xs">
          More rows available — refine the filters to drill in.
        </p>
      ) : null}
    </div>
  );
}

function HealthIssueRow({ row }: { row: CrossLibHealthIssueView }) {
  // The dismiss mutation is scoped to a library via the per-library
  // endpoint — pass the originating library_id from the row.
  const dismiss = useDismissHealthIssue(row.library_id);
  const isOpen = !row.resolved_at && !row.dismissed_at;
  const summary = formatPayload(row.kind, row.payload);

  return (
    <Card>
      <CardContent className="flex items-start gap-3 py-3">
        <SeverityIcon severity={row.severity} />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2 text-sm">
            <span className="font-mono text-xs">{row.kind}</span>
            <Badge variant="outline" className="text-[10px]">
              {row.library_slug ? (
                <Link
                  href={`/admin/libraries/${row.library_slug}/health`}
                  className="hover:underline"
                >
                  {row.library_name}
                </Link>
              ) : (
                row.library_name
              )}
            </Badge>
            <span className="text-muted-foreground ml-auto text-xs">
              {new Date(row.last_seen_at).toLocaleString()}
            </span>
          </div>
          {summary ? (
            <p className="text-muted-foreground mt-1 text-xs leading-relaxed wrap-anywhere">
              {summary}
            </p>
          ) : null}
        </div>
        {isOpen ? (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => dismiss.mutate({ issueId: row.id })}
            disabled={dismiss.isPending}
          >
            Dismiss
          </Button>
        ) : null}
      </CardContent>
    </Card>
  );
}

function SeverityIcon({ severity }: { severity: string }) {
  if (severity === "error")
    return <XCircle className="text-destructive mt-0.5 h-4 w-4 shrink-0" />;
  if (severity === "warning")
    return <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-amber-500" />;
  return <Info className="text-muted-foreground mt-0.5 h-4 w-4 shrink-0" />;
}

function ScanRunsRail({
  state,
  libraryId,
}: {
  state: string;
  libraryId: string;
}) {
  const filters = {
    state: state === "all" ? undefined : state,
    library_id: libraryId === "all" ? undefined : libraryId,
    limit: 50,
  };
  const { data, isLoading, error } = useAdminScanRuns(filters);

  if (isLoading) return <Skeleton className="h-64 w-full" />;
  if (error) return <p className="text-destructive text-sm">{error.message}</p>;
  const items = data?.items ?? [];
  if (items.length === 0) {
    return (
      <Card>
        <CardContent className="text-muted-foreground py-12 text-center text-sm">
          No scan runs match these filters.
        </CardContent>
      </Card>
    );
  }
  return (
    <Card>
      <CardContent className="p-0">
        <table className="w-full text-sm">
          <thead className="text-muted-foreground text-xs uppercase">
            <tr>
              <th className="border-border border-b p-3 text-left">State</th>
              <th className="border-border border-b p-3 text-left">Library</th>
              <th className="border-border border-b p-3 text-left">Kind</th>
              <th className="border-border border-b p-3 text-left">Started</th>
              <th className="border-border border-b p-3 text-left">Ended</th>
              <th className="border-border border-b p-3 text-left">Error</th>
            </tr>
          </thead>
          <tbody>
            {items.map((row) => (
              <tr key={row.id} className="hover:bg-muted/40">
                <td className="border-border border-b p-3">
                  <StateBadge state={row.state} />
                </td>
                <td className="border-border border-b p-3">
                  {row.library_slug ? (
                    <Link
                      href={`/admin/libraries/${row.library_slug}/history`}
                      className="hover:underline"
                    >
                      {row.library_name}
                    </Link>
                  ) : (
                    row.library_name
                  )}
                </td>
                <td className="border-border border-b p-3 font-mono text-xs">
                  {row.kind}
                  {row.series_name ? ` · ${row.series_name}` : ""}
                </td>
                <td className="border-border text-muted-foreground border-b p-3 text-xs">
                  {new Date(row.started_at).toLocaleString()}
                </td>
                <td className="border-border text-muted-foreground border-b p-3 text-xs">
                  {row.ended_at
                    ? new Date(row.ended_at).toLocaleString()
                    : "—"}
                </td>
                <td className="border-border border-b p-3 text-xs wrap-anywhere">
                  {row.error ?? ""}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </CardContent>
    </Card>
  );
}

function StateBadge({ state }: { state: string }) {
  if (state === "running") {
    return (
      <Badge variant="secondary" className="gap-1">
        <Loader2 className="h-3 w-3 animate-spin" />
        Running
      </Badge>
    );
  }
  if (state === "complete") {
    return (
      <Badge variant="outline" className="text-primary border-primary/40 gap-1">
        <CheckCircle2 className="h-3 w-3" />
        Complete
      </Badge>
    );
  }
  if (state === "failed") {
    return (
      <Badge variant="destructive" className="gap-1">
        <XCircle className="h-3 w-3" />
        Failed
      </Badge>
    );
  }
  if (state === "cancelled") {
    return (
      <Badge variant="secondary" className="gap-1">
        <AlertCircle className="h-3 w-3" />
        Cancelled
      </Badge>
    );
  }
  return <Badge variant="outline">{state}</Badge>;
}

/**
 * Best-effort summary from a health-issue payload — same heuristic the
 * per-library table uses ([HealthIssuesTable.tsx](../library/HealthIssuesTable.tsx)).
 * Kept inline here rather than imported to avoid coupling the
 * cross-library table to the per-library one; the payload schemas
 * are stable enough that a shared one-liner per kind is fine.
 */
function formatPayload(kind: string, p: unknown): string {
  if (!p || typeof p !== "object") return "";
  const obj = p as Record<string, unknown>;
  const path =
    typeof obj.path === "string"
      ? obj.path
      : typeof obj.file_path === "string"
        ? (obj.file_path as string)
        : "";
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
  for (const k of ["path", "file_path", "series_id", "issue_id", "reason", "details"]) {
    const v = obj[k];
    if (typeof v === "string" && v.length > 0) return v;
  }
  return JSON.stringify(p);
}
