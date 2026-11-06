"use client";

import * as React from "react";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import { AlertTriangle, CheckCircle2, Info, XCircle } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { FilterPill } from "@/components/folio/filter-pill";
import { HealthIssueCardsSkeleton } from "@/components/skeletons/AdminShapes";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/folio/tabs";
import { FilterRow, LibraryFilter } from "@/components/admin/AdminFilters";
import {
  HealthPayloadExtras,
  PayloadRefLinks,
} from "@/components/admin/library/HealthIssueExtras";
import { LibraryEventsList } from "@/components/admin/library/LibraryEventsList";
import { useDismissHealthIssue } from "@/lib/api/mutations";
import { InfiniteSentinel } from "@/components/folio/infinite-sentinel";
import { useAdminHealthIssues, useLibraryList } from "@/lib/api/queries";
import type { CrossLibHealthIssueView } from "@/lib/api/types";
import {
  healthKindLabel,
  healthPayloadSummary,
} from "@/lib/library/health-issues";
import { statusToneText } from "@/lib/ui/status-tone";

import type { HealthSeverityFilter } from "@/components/admin/severity";

type Tab = "activity" | "health";
type Severity = HealthSeverityFilter;

/**
 * Library events (`/admin/findings`) — the Library stream minus scans:
 *
 *  1. Activity (observability-split M11): the durable, itemized
 *     `library_events` manifest — every issue/series add·update·remove,
 *     thumbnail/metadata/archive op, scoped by library. Distinct from
 *     `/admin/activity`, the *Server* stream (audit + user activity +
 *     the runtime log); the two streams are deliberately non-overlapping.
 *  2. Open health issues — joined `library_health_issue` x `library`,
 *     filter by severity / library, inline dismiss action.
 *
 * Scan runs live on the Scans page (`/admin/scans?history=runs`).
 *
 * Filter state is reflected to URL query params so the dashboard
 * cards can deep-link into specific subsets and the page is
 * browser-back-friendly.
 */
export function FindingsView() {
  const router = useRouter();
  const sp = useSearchParams();

  const tab = (sp.get("tab") as Tab) ?? "activity";
  const severity = (sp.get("severity") as Severity) ?? "all";
  const libraryId = sp.get("library_id") ?? "all";

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
        <TabsTrigger value="activity">Activity</TabsTrigger>
        <TabsTrigger value="health">Open health issues</TabsTrigger>
      </TabsList>

      <TabsContent value="activity" className="mt-4 space-y-3">
        <FilterRow>
          <LibraryFilter
            libraries={libraries ?? []}
            value={libraryId}
            onChange={(v) => setParam("library_id", v)}
          />
        </FilterRow>
        <LibraryEventsList
          libraryId={libraryId === "all" ? undefined : libraryId}
          showLibrary
        />
      </TabsContent>

      <TabsContent value="health" className="mt-4 space-y-3">
        <FilterRow>
          <SeverityChips
            value={severity}
            onChange={(v) => setParam("severity", v)}
          />
          <LibraryFilter
            libraries={libraries ?? []}
            value={libraryId}
            onChange={(v) => setParam("library_id", v)}
          />
        </FilterRow>
        <HealthIssuesRail severity={severity} libraryId={libraryId} />
      </TabsContent>
    </Tabs>
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
      <span className="text-muted-foreground mr-1 text-xs uppercase">
        Severity
      </span>
      {options.map((o) => (
        <FilterPill
          key={o.v}
          active={value === o.v}
          onClick={() => onChange(o.v)}
        >
          {o.label}
        </FilterPill>
      ))}
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
  const query = useAdminHealthIssues(filters);
  const { data, isLoading, error } = query;

  if (isLoading) return <HealthIssueCardsSkeleton />;
  if (error) return <p className="text-destructive text-sm">{error.message}</p>;
  const items = data?.pages.flatMap((p) => p.items) ?? [];
  if (items.length === 0) {
    return (
      <Card>
        <CardContent className="text-muted-foreground py-12 text-center text-sm">
          <CheckCircle2 className="text-primary mx-auto mb-2 size-6" />
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
      <InfiniteSentinel query={query} />
    </div>
  );
}

function HealthIssueRow({ row }: { row: CrossLibHealthIssueView }) {
  // The dismiss mutation is scoped to a library via the per-library
  // endpoint — pass the originating library_id from the row.
  const dismiss = useDismissHealthIssue(row.library_id);
  const isOpen = !row.resolved_at && !row.dismissed_at;
  const summary = healthPayloadSummary(row.kind, row.payload);

  return (
    <Card>
      <CardContent className="flex flex-row items-start gap-3 py-3">
        <SeverityIcon severity={row.severity} />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2 text-sm">
            <span className="text-sm font-medium">
              {healthKindLabel(row.kind)}
            </span>
            <span className="text-muted-foreground font-mono text-xs">
              {row.kind}
            </span>
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
          <HealthPayloadExtras kind={row.kind} payload={row.payload} />
          <PayloadRefLinks payload={row.payload} />
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
    return <XCircle className="text-destructive mt-0.5 size-4 shrink-0" />;
  if (severity === "warning")
    // Shares the warning tone the dashboard's `SeverityRow` already
    // uses ([DashboardClient.tsx]) via `statusToneText`. Single
    // palette across the admin shell, no per-page drift.
    return (
      <AlertTriangle
        className={`mt-0.5 size-4 shrink-0 ${statusToneText("warning")}`}
      />
    );
  return <Info className="text-muted-foreground mt-0.5 size-4 shrink-0" />;
}
