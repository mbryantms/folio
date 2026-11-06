"use client";

import * as React from "react";
import Link from "next/link";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import {
  AlertCircle,
  CheckCircle2,
  Loader2,
  ScrollText,
  XCircle,
} from "lucide-react";

import { FilterRow, LibraryFilter } from "@/components/admin/AdminFilters";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import { FilterPill } from "@/components/folio/filter-pill";
import { InfiniteSentinel } from "@/components/folio/infinite-sentinel";
import { ScanRunsCardSkeleton } from "@/components/skeletons/AdminShapes";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { libraryScanHref } from "@/lib/admin/routes";
import { useAdminScanRunsInfinite, useLibraryList } from "@/lib/api/queries";

/**
 * Every scan run across libraries (Scans → History → Every scan). State +
 * library filters live in the URL (`?state=` / `?library_id=`) so the admin
 * dashboard can deep-link ("failed scans").
 */
export function ScanRunsHistory() {
  const router = useRouter();
  const pathname = usePathname();
  const sp = useSearchParams();
  const state = sp.get("state") ?? "all";
  const libraryId = sp.get("library_id") ?? "all";
  const { data: libraries } = useLibraryList();

  function setParam(key: string, value: string) {
    const next = new URLSearchParams(sp);
    if (value === "all") next.delete(key);
    else next.set(key, value);
    const qs = next.toString();
    router.replace(`${pathname}${qs ? `?${qs}` : ""}`, { scroll: false });
  }

  return (
    <div className="space-y-3">
      <FilterRow>
        <StateChips value={state} onChange={(v) => setParam("state", v)} />
        <LibraryFilter
          libraries={libraries ?? []}
          value={libraryId}
          onChange={(v) => setParam("library_id", v)}
        />
      </FilterRow>
      <ScanRunsRail state={state} libraryId={libraryId} />
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
      <span className="text-muted-foreground mr-1 text-xs uppercase">
        State
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
  const query = useAdminScanRunsInfinite(filters);
  const { data, isLoading, error } = query;

  if (isLoading) return <ScanRunsCardSkeleton />;
  if (error) return <p className="text-destructive text-sm">{error.message}</p>;
  const items = data?.pages.flatMap((p) => p.items) ?? [];
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
    <div className="space-y-3">
      <Card>
        <CardContent className="p-0">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>State</TableHead>
                <TableHead>Library</TableHead>
                <TableHead>Kind</TableHead>
                <TableHead>Started</TableHead>
                <TableHead>Ended</TableHead>
                <TableHead>Error</TableHead>
                <TableHead>
                  <span className="sr-only">Logs</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {items.map((row) => (
                <TableRow key={row.id}>
                  <TableCell>
                    <StateBadge state={row.state} />
                  </TableCell>
                  <TableCell>
                    {row.library_slug ? (
                      <Link
                        href={libraryScanHref(row.library_slug)}
                        className="hover:underline"
                      >
                        {row.library_name}
                      </Link>
                    ) : (
                      row.library_name
                    )}
                  </TableCell>
                  <TableCell className="font-mono text-xs">
                    {row.kind}
                    {row.series_name ? ` · ${row.series_name}` : ""}
                  </TableCell>
                  <TableCell className="text-muted-foreground text-xs">
                    {new Date(row.started_at).toLocaleString()}
                  </TableCell>
                  <TableCell className="text-muted-foreground text-xs">
                    {row.ended_at
                      ? new Date(row.ended_at).toLocaleString()
                      : "—"}
                  </TableCell>
                  <TableCell className="text-xs wrap-anywhere">
                    {row.error ?? ""}
                  </TableCell>
                  <TableCell className="text-xs">
                    {/* Cross-link into the runtime log tail scoped to
                      this scan's library (Server activity → Log). */}
                    <Link
                      href={`/admin/activity?view=log&library_id=${encodeURIComponent(row.library_id)}`}
                      className="text-muted-foreground hover:text-foreground inline-flex items-center gap-1 underline-offset-2 hover:underline"
                    >
                      <ScrollText className="size-3.5" aria-hidden="true" />
                      Logs
                    </Link>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
      <InfiniteSentinel query={query} />
    </div>
  );
}

function StateBadge({ state }: { state: string }) {
  if (state === "running") {
    return (
      <Badge variant="secondary" className="gap-1">
        <Loader2 className="size-3 animate-spin" />
        Running
      </Badge>
    );
  }
  if (state === "complete") {
    return (
      <Badge variant="outline" className="text-primary border-primary/40 gap-1">
        <CheckCircle2 className="size-3" />
        Complete
      </Badge>
    );
  }
  if (state === "failed") {
    return (
      <Badge variant="destructive" className="gap-1">
        <XCircle className="size-3" />
        Failed
      </Badge>
    );
  }
  if (state === "cancelled") {
    return (
      <Badge variant="secondary" className="gap-1">
        <AlertCircle className="size-3" />
        Cancelled
      </Badge>
    );
  }
  return <Badge variant="outline">{state}</Badge>;
}
