"use client";

import * as React from "react";
import { Loader2 } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Skeleton } from "@/components/ui/skeleton";
import { useRemovedItemsInfinite } from "@/lib/api/queries";
import {
  useConfirmIssueRemoval,
  useRestoreIssue,
  useRestoreSeries,
} from "@/lib/api/mutations";
import type { RemovedIssueView, RemovedSeriesView } from "@/lib/api/types";

type HideAction = { type: "hide" | "rollback"; id: string };

export function RemovedItemsTable({ libraryId }: { libraryId: string }) {
  // Cursor-paginated (audit UX-11): a bulk removal can strand thousands of
  // issue rows; the old one-shot query loaded them all at once. Removed
  // series + the total ride on the first page.
  const {
    data,
    isLoading,
    error,
    hasNextPage,
    isFetchingNextPage,
    fetchNextPage,
  } = useRemovedItemsInfinite(libraryId);
  const restore = useRestoreIssue(libraryId);
  const restoreSeries = useRestoreSeries(libraryId);
  const confirmRemoval = useConfirmIssueRemoval(libraryId);

  // Optimistic hide set — items the user just acted on are hidden
  // immediately; the query invalidation drops them from `data` shortly.
  // On mutation failure we roll the item back into view (D7) so a failed
  // restore/confirm doesn't make the row silently vanish. Issue ids
  // (content hashes) and series ids (UUIDs) can share the set safely.
  const [hidden, dispatch] = React.useReducer(
    (acc: Set<string>, action: HideAction): Set<string> => {
      const next = new Set(acc);
      if (action.type === "hide") next.add(action.id);
      else next.delete(action.id);
      return next;
    },
    new Set<string>(),
  );

  const allIssues = data?.pages.flatMap((p) => p.issues) ?? [];
  const totalIssues = data?.pages[0]?.total_issues ?? allIssues.length;
  const visibleIssues = allIssues.filter((i) => !hidden.has(i.id));
  const visibleSeries = (data?.pages[0]?.series ?? []).filter(
    (s) => !hidden.has(s.id),
  );

  if (isLoading) return <Skeleton className="h-64 w-full" />;
  if (error) return <p className="text-destructive text-sm">{error.message}</p>;

  if (visibleIssues.length === 0 && visibleSeries.length === 0) {
    return (
      <p className="border-border bg-card/40 text-muted-foreground rounded-md border border-dashed px-4 py-12 text-center text-sm">
        No pending removals.
      </p>
    );
  }

  return (
    <div className="space-y-6">
      {visibleIssues.length > 0 ? (
        <section className="space-y-2">
          <h3 className="text-muted-foreground text-xs font-semibold tracking-widest uppercase">
            Issues{" "}
            <span className="font-normal tracking-normal">
              ({totalIssues.toLocaleString()})
            </span>
          </h3>
          <div className="border-border bg-card rounded-md border">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>File</TableHead>
                  <TableHead>Removed at</TableHead>
                  <TableHead>State</TableHead>
                  <TableHead className="text-right">Actions</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {visibleIssues.map((issue) => (
                  <IssueRow
                    key={issue.id}
                    issue={issue}
                    onRestore={() => {
                      dispatch({ type: "hide", id: issue.id });
                      restore.mutate(
                        {
                          seriesSlug: issue.series_slug,
                          issueSlug: issue.slug,
                        },
                        {
                          onError: () =>
                            dispatch({ type: "rollback", id: issue.id }),
                        },
                      );
                    }}
                    onConfirm={() => {
                      dispatch({ type: "hide", id: issue.id });
                      confirmRemoval.mutate(
                        {
                          seriesSlug: issue.series_slug,
                          issueSlug: issue.slug,
                        },
                        {
                          onError: () =>
                            dispatch({ type: "rollback", id: issue.id }),
                        },
                      );
                    }}
                  />
                ))}
              </TableBody>
            </Table>
          </div>
          {hasNextPage ? (
            <div className="flex justify-center pt-1">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => void fetchNextPage()}
                disabled={isFetchingNextPage}
              >
                {isFetchingNextPage ? (
                  <>
                    <Loader2 className="mr-1.5 size-3.5 animate-spin" />
                    Loading more…
                  </>
                ) : (
                  "Load more"
                )}
              </Button>
            </div>
          ) : null}
        </section>
      ) : null}
      {visibleSeries.length > 0 ? (
        <section className="space-y-2">
          <h3 className="text-muted-foreground text-xs font-semibold tracking-widest uppercase">
            Series
          </h3>
          <div className="border-border bg-card rounded-md border">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Name</TableHead>
                  <TableHead>Folder</TableHead>
                  <TableHead>Removed at</TableHead>
                  <TableHead>State</TableHead>
                  <TableHead className="text-right">Actions</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {visibleSeries.map((s) => (
                  <SeriesRow
                    key={s.id}
                    series={s}
                    onRestore={() => {
                      dispatch({ type: "hide", id: s.id });
                      restoreSeries.mutate(
                        { seriesSlug: s.slug },
                        {
                          onError: () =>
                            dispatch({ type: "rollback", id: s.id }),
                        },
                      );
                    }}
                  />
                ))}
              </TableBody>
            </Table>
          </div>
        </section>
      ) : null}
    </div>
  );
}

function IssueRow({
  issue,
  onRestore,
  onConfirm,
}: {
  issue: RemovedIssueView;
  onRestore: () => void;
  onConfirm: () => void;
}) {
  return (
    <TableRow>
      <TableCell className="font-mono text-xs">
        <span className="block max-w-xl wrap-anywhere">{issue.file_path}</span>
      </TableCell>
      <TableCell className="text-muted-foreground text-xs">
        {new Date(issue.removed_at).toLocaleString()}
      </TableCell>
      <TableCell>
        <Badge
          variant={issue.removal_confirmed_at ? "destructive" : "secondary"}
        >
          {issue.removal_confirmed_at ? "Confirmed" : "Soft-deleted"}
        </Badge>
      </TableCell>
      <TableCell className="space-x-2 text-right">
        <Button size="sm" variant="ghost" onClick={onRestore}>
          Restore
        </Button>
        {!issue.removal_confirmed_at ? (
          <AlertDialog>
            <AlertDialogTrigger asChild>
              <Button size="sm" variant="outline">
                Confirm removal
              </Button>
            </AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Confirm permanent removal?</AlertDialogTitle>
                <AlertDialogDescription>
                  This marks{" "}
                  <span className="font-mono wrap-anywhere">
                    {issue.file_path}
                  </span>{" "}
                  permanently removed. The original file is unaffected; only
                  Folio&apos;s record is removed.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Cancel</AlertDialogCancel>
                <AlertDialogAction onClick={onConfirm}>
                  Confirm
                </AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        ) : null}
      </TableCell>
    </TableRow>
  );
}

function SeriesRow({
  series,
  onRestore,
}: {
  series: RemovedSeriesView;
  onRestore: () => void;
}) {
  return (
    <TableRow>
      <TableCell className="font-medium">{series.name}</TableCell>
      <TableCell className="text-muted-foreground font-mono text-xs wrap-anywhere">
        {series.folder_path ?? "—"}
      </TableCell>
      <TableCell className="text-muted-foreground text-xs">
        {new Date(series.removed_at).toLocaleString()}
      </TableCell>
      <TableCell>
        <Badge
          variant={series.removal_confirmed_at ? "destructive" : "secondary"}
        >
          {series.removal_confirmed_at ? "Confirmed" : "Soft-deleted"}
        </Badge>
      </TableCell>
      <TableCell className="text-right">
        {/* Restores the series row plus any of its issues whose files are
            back on disk; the server 409s while the folder is still missing
            (audit UX-11). */}
        <Button size="sm" variant="ghost" onClick={onRestore}>
          Restore
        </Button>
      </TableCell>
    </TableRow>
  );
}
