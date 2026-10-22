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
import { Checkbox } from "@/components/ui/checkbox";
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
  useConfirmRemovals,
  useConfirmSeriesRemoval,
  useRestoreIssue,
  useRestoreSeries,
} from "@/lib/api/mutations";
import type { RemovedIssueView, RemovedSeriesView } from "@/lib/api/types";

type HideAction = { type: "hide" | "rollback"; id: string };

/**
 * Admin "Removed" tab. Two groups, both from `GET /libraries/{id}/removed`:
 *
 * - **Series** — one row per folder the scanner found missing, carrying its
 *   removed-issue tally. Restore / Confirm act on the series *and* its issues
 *   as a unit, so a deleted run is one click, not one per file.
 * - **Issues** — *loose* files that vanished while their series folder is
 *   still on disk. Issues of a removed series are not repeated here.
 *
 * Pending rows (soft-deleted, not yet confirmed) carry a checkbox; the
 * header offers "Confirm selected" and "Confirm all pending". Confirming is
 * the operator's early "yes, it's really gone": the nightly sweep does the
 * same after `soft_delete_days`, so a NAS that is merely unmounted keeps its
 * rows restorable until someone says otherwise. Nothing here touches files.
 */
export function RemovedItemsTable({ libraryId }: { libraryId: string }) {
  // Cursor-paginated (audit UX-11): a bulk removal can strand thousands of
  // issue rows; the old one-shot query loaded them all at once. Removed
  // series + the totals ride on the first page.
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
  const confirmIssue = useConfirmIssueRemoval(libraryId);
  const confirmSeries = useConfirmSeriesRemoval(libraryId);
  const confirmBulk = useConfirmRemovals(libraryId);

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

  // Hand-picked bulk scope. Series ids cascade to their issues server-side.
  const [selectedSeries, setSelectedSeries] = React.useState<Set<string>>(
    () => new Set(),
  );
  const [selectedIssues, setSelectedIssues] = React.useState<Set<string>>(
    () => new Set(),
  );
  const toggleIn =
    (set: React.Dispatch<React.SetStateAction<Set<string>>>) =>
    (id: string, on: boolean) =>
      set((prev) => {
        const next = new Set(prev);
        if (on) next.add(id);
        else next.delete(id);
        return next;
      });
  const toggleSeries = toggleIn(setSelectedSeries);
  const toggleIssue = toggleIn(setSelectedIssues);
  const clearSelection = () => {
    setSelectedSeries(new Set());
    setSelectedIssues(new Set());
  };

  const firstPage = data?.pages[0];
  const allIssues = data?.pages.flatMap((p) => p.issues) ?? [];
  const totalIssues = firstPage?.total_issues ?? allIssues.length;
  const pendingIssues = firstPage?.pending_issues ?? 0;
  const pendingSeries = firstPage?.pending_series ?? 0;
  const visibleIssues = allIssues.filter((i) => !hidden.has(i.id));
  const visibleSeries = (firstPage?.series ?? []).filter(
    (s) => !hidden.has(s.id),
  );
  const pendingSeriesRows = visibleSeries.filter(
    (s) => !s.removal_confirmed_at,
  );
  const pendingIssueRows = visibleIssues.filter((i) => !i.removal_confirmed_at);
  // Selection can hold ids that have since left the list (confirmed via
  // a row button, dropped by a refetch) — count only what is still shown.
  const selectedSeriesCount = pendingSeriesRows.filter((s) =>
    selectedSeries.has(s.id),
  ).length;
  const selectedIssueCount = pendingIssueRows.filter((i) =>
    selectedIssues.has(i.id),
  ).length;
  const selectedCount = selectedSeriesCount + selectedIssueCount;
  const anyPending = pendingIssues > 0 || pendingSeries > 0;
  const busy = confirmBulk.isPending;

  if (isLoading) return <Skeleton className="h-64 w-full" />;
  if (error) return <p className="text-destructive text-sm">{error.message}</p>;

  if (visibleIssues.length === 0 && visibleSeries.length === 0) {
    return (
      <p className="border-border bg-card/40 text-muted-foreground rounded-md border border-dashed px-4 py-12 text-center text-sm">
        No pending removals.
      </p>
    );
  }

  const runSelected = () => {
    confirmBulk.mutate(
      {
        all: false,
        series_ids: pendingSeriesRows
          .filter((s) => selectedSeries.has(s.id))
          .map((s) => s.id),
        issue_ids: pendingIssueRows
          .filter((i) => selectedIssues.has(i.id))
          .map((i) => i.id),
      },
      { onSuccess: clearSelection },
    );
  };
  const runAll = () => {
    confirmBulk.mutate(
      { all: true, series_ids: [], issue_ids: [] },
      { onSuccess: clearSelection },
    );
  };

  return (
    <div className="space-y-6">
      {/* Bulk bar: what is waiting, and the two ways to clear it. */}
      <div className="border-border bg-card flex flex-wrap items-center justify-between gap-3 rounded-md border px-4 py-3">
        <div className="text-sm">
          {anyPending ? (
            <>
              <span className="text-foreground font-medium">
                {describePending(pendingIssues, pendingSeries)}
              </span>{" "}
              <span className="text-muted-foreground">
                awaiting confirmation. Files on disk are never touched;
                unconfirmed rows auto-confirm after the library&apos;s
                soft-delete window.
              </span>
            </>
          ) : (
            <span className="text-muted-foreground">
              Everything here is confirmed; the nightly purge removes the
              records after the retention window.
            </span>
          )}
        </div>
        {anyPending ? (
          <div className="flex flex-wrap items-center gap-2">
            {selectedCount > 0 ? (
              <Button
                type="button"
                size="sm"
                variant="ghost"
                onClick={clearSelection}
                disabled={busy}
              >
                Clear selection
              </Button>
            ) : null}
            <ConfirmDialog
              title={`Confirm ${selectedCount.toLocaleString()} selected removal${selectedCount === 1 ? "" : "s"}?`}
              description={`Marks ${describeSelection(selectedSeriesCount, selectedIssueCount)} permanently removed. Selected series take all of their removed issues with them. The original files are unaffected; only Folio's records are confirmed.`}
              onConfirm={runSelected}
              trigger={
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={busy || selectedCount === 0}
                >
                  Confirm selected
                  {selectedCount > 0 ? ` (${selectedCount})` : ""}
                </Button>
              }
            />
            <ConfirmDialog
              title="Confirm every pending removal?"
              description={`Marks ${describePending(pendingIssues, pendingSeries)} permanently removed in this library. Do this only once you know the files are really gone — a storage mount that is just offline should be restored, not confirmed. The original files are unaffected.`}
              onConfirm={runAll}
              trigger={
                <Button type="button" size="sm" disabled={busy}>
                  {busy ? (
                    <Loader2 className="mr-1.5 size-3.5 animate-spin" />
                  ) : null}
                  Confirm all pending
                </Button>
              }
            />
          </div>
        ) : null}
      </div>

      {visibleSeries.length > 0 ? (
        <section className="space-y-2">
          <h3 className="text-muted-foreground text-xs font-semibold tracking-widest uppercase">
            Series{" "}
            <span className="font-normal tracking-normal">
              ({visibleSeries.length.toLocaleString()})
            </span>
          </h3>
          <div className="border-border bg-card rounded-md border">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="w-10">
                    <span className="sr-only">Select</span>
                  </TableHead>
                  <TableHead>Name</TableHead>
                  <TableHead>Folder</TableHead>
                  <TableHead>Issues</TableHead>
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
                    selected={selectedSeries.has(s.id)}
                    onSelect={(on) => toggleSeries(s.id, on)}
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
                    onConfirm={() => {
                      // Confirmed rows stay listed (state flips), so no
                      // optimistic hide — the refetch updates the badge.
                      confirmSeries.mutate({ seriesSlug: s.slug });
                    }}
                  />
                ))}
              </TableBody>
            </Table>
          </div>
        </section>
      ) : null}

      {visibleIssues.length > 0 ? (
        <section className="space-y-2">
          <h3 className="text-muted-foreground text-xs font-semibold tracking-widest uppercase">
            Issues{" "}
            <span className="font-normal tracking-normal">
              ({totalIssues.toLocaleString()})
            </span>
          </h3>
          <p className="text-muted-foreground text-xs">
            Files that went missing while their series folder is still on disk.
            Issues of a removed series are listed with the series above.
          </p>
          <div className="border-border bg-card rounded-md border">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="w-10">
                    <span className="sr-only">Select</span>
                  </TableHead>
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
                    selected={selectedIssues.has(issue.id)}
                    onSelect={(on) => toggleIssue(issue.id, on)}
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
                      confirmIssue.mutate({
                        seriesSlug: issue.series_slug,
                        issueSlug: issue.slug,
                      });
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
    </div>
  );
}

function plural(n: number, one: string, many: string): string {
  return `${n.toLocaleString()} ${n === 1 ? one : many}`;
}

/** "9 issues across 1 series" / "2 issues" / "1 series". */
function describePending(issues: number, series: number): string {
  const parts: string[] = [];
  if (issues > 0) parts.push(plural(issues, "issue", "issues"));
  if (series > 0) {
    parts.push(
      issues > 0
        ? `across ${plural(series, "series", "series")}`
        : plural(series, "series", "series"),
    );
  }
  return parts.join(" ") || "nothing";
}

function describeSelection(series: number, issues: number): string {
  const parts: string[] = [];
  if (series > 0) parts.push(plural(series, "series", "series"));
  if (issues > 0) parts.push(plural(issues, "loose issue", "loose issues"));
  return parts.join(" and ") || "nothing";
}

function ConfirmDialog({
  trigger,
  title,
  description,
  onConfirm,
}: {
  trigger: React.ReactNode;
  title: string;
  description: string;
  onConfirm: () => void;
}) {
  return (
    <AlertDialog>
      <AlertDialogTrigger asChild>{trigger}</AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={onConfirm}>Confirm</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

function StateBadge({ confirmed }: { confirmed: boolean }) {
  return (
    <Badge variant={confirmed ? "destructive" : "secondary"}>
      {confirmed ? "Confirmed" : "Pending"}
    </Badge>
  );
}

function SeriesRow({
  series,
  selected,
  onSelect,
  onRestore,
  onConfirm,
}: {
  series: RemovedSeriesView;
  selected: boolean;
  onSelect: (on: boolean) => void;
  onRestore: () => void;
  onConfirm: () => void;
}) {
  const pending = !series.removal_confirmed_at;
  return (
    <TableRow data-state={selected ? "selected" : undefined}>
      <TableCell>
        {pending ? (
          <Checkbox
            checked={selected}
            onCheckedChange={(v) => onSelect(v === true)}
            aria-label={`Select ${series.name}`}
          />
        ) : null}
      </TableCell>
      <TableCell className="font-medium">{series.name}</TableCell>
      <TableCell className="text-muted-foreground font-mono text-xs wrap-anywhere">
        {series.folder_path ?? "—"}
      </TableCell>
      <TableCell className="text-xs tabular-nums">
        {series.issue_count.toLocaleString()}
        {series.pending_issue_count > 0 &&
        series.pending_issue_count !== series.issue_count ? (
          <span className="text-muted-foreground">
            {" "}
            ({series.pending_issue_count.toLocaleString()} pending)
          </span>
        ) : null}
      </TableCell>
      <TableCell className="text-muted-foreground text-xs">
        {new Date(series.removed_at).toLocaleString()}
      </TableCell>
      <TableCell>
        <StateBadge confirmed={!pending} />
      </TableCell>
      <TableCell className="space-x-2 text-right">
        {/* Restores the series row plus any of its issues whose files are
            back on disk; the server 409s while the folder is still missing
            (audit UX-11). */}
        <Button size="sm" variant="ghost" onClick={onRestore}>
          Restore
        </Button>
        {pending ? (
          <ConfirmDialog
            title="Confirm permanent removal?"
            description={`This marks ${series.name} and its ${plural(series.issue_count, "removed issue", "removed issues")} permanently removed. The original files are unaffected; only Folio's records are confirmed.`}
            onConfirm={onConfirm}
            trigger={
              <Button size="sm" variant="outline">
                Confirm removal
              </Button>
            }
          />
        ) : null}
      </TableCell>
    </TableRow>
  );
}

function IssueRow({
  issue,
  selected,
  onSelect,
  onRestore,
  onConfirm,
}: {
  issue: RemovedIssueView;
  selected: boolean;
  onSelect: (on: boolean) => void;
  onRestore: () => void;
  onConfirm: () => void;
}) {
  const pending = !issue.removal_confirmed_at;
  return (
    <TableRow data-state={selected ? "selected" : undefined}>
      <TableCell>
        {pending ? (
          <Checkbox
            checked={selected}
            onCheckedChange={(v) => onSelect(v === true)}
            aria-label={`Select ${issue.file_path}`}
          />
        ) : null}
      </TableCell>
      <TableCell className="font-mono text-xs">
        <span className="block max-w-xl wrap-anywhere">{issue.file_path}</span>
      </TableCell>
      <TableCell className="text-muted-foreground text-xs">
        {new Date(issue.removed_at).toLocaleString()}
      </TableCell>
      <TableCell>
        <StateBadge confirmed={!pending} />
      </TableCell>
      <TableCell className="space-x-2 text-right">
        <Button size="sm" variant="ghost" onClick={onRestore}>
          Restore
        </Button>
        {pending ? (
          <ConfirmDialog
            title="Confirm permanent removal?"
            description={`This marks ${issue.file_path} permanently removed. The original file is unaffected; only Folio's record is confirmed.`}
            onConfirm={onConfirm}
            trigger={
              <Button size="sm" variant="outline">
                Confirm removal
              </Button>
            }
          />
        ) : null}
      </TableCell>
    </TableRow>
  );
}
