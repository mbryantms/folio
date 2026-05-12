"use client";

import * as React from "react";

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
import { useRemovedItems } from "@/lib/api/queries";
import { useConfirmIssueRemoval, useRestoreIssue } from "@/lib/api/mutations";
import type { RemovedIssueView } from "@/lib/api/types";

type LocalAction = { issueId: string; kind: "restore" | "confirm" };

export function RemovedItemsTable({ libraryId }: { libraryId: string }) {
  const { data, isLoading, error } = useRemovedItems(libraryId);
  const restore = useRestoreIssue(libraryId);
  const confirmRemoval = useConfirmIssueRemoval(libraryId);

  // Optimistic hide list — items the user just acted on. They get hidden
  // immediately; the query invalidation removes them from `data` shortly.
  const [optimistic, applyOptimistic] = React.useReducer(
    (acc: LocalAction[], action: LocalAction): LocalAction[] => [
      ...acc,
      action,
    ],
    [],
  );
  const hidden = React.useMemo(
    () => new Set(optimistic.map((o) => o.issueId)),
    [optimistic],
  );

  const visibleIssues = (data?.issues ?? []).filter((i) => !hidden.has(i.id));
  const visibleSeries = data?.series ?? [];

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
            Issues
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
                      applyOptimistic({ issueId: issue.id, kind: "restore" });
                      restore.mutate({ issueId: issue.id });
                    }}
                    onConfirm={() => {
                      applyOptimistic({ issueId: issue.id, kind: "confirm" });
                      confirmRemoval.mutate({ issueId: issue.id });
                    }}
                  />
                ))}
              </TableBody>
            </Table>
          </div>
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
                </TableRow>
              </TableHeader>
              <TableBody>
                {visibleSeries.map((s) => (
                  <TableRow key={s.id}>
                    <TableCell className="font-medium">{s.name}</TableCell>
                    <TableCell className="text-muted-foreground font-mono text-xs [overflow-wrap:anywhere]">
                      {s.folder_path ?? "—"}
                    </TableCell>
                    <TableCell className="text-muted-foreground text-xs">
                      {new Date(s.removed_at).toLocaleString()}
                    </TableCell>
                    <TableCell>
                      <Badge
                        variant={
                          s.removal_confirmed_at ? "destructive" : "secondary"
                        }
                      >
                        {s.removal_confirmed_at ? "Confirmed" : "Soft-deleted"}
                      </Badge>
                    </TableCell>
                  </TableRow>
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
        <span className="block max-w-xl [overflow-wrap:anywhere]">
          {issue.file_path}
        </span>
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
                  <span className="font-mono [overflow-wrap:anywhere]">
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
