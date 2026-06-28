"use client";

/**
 * Reading-list refresh dashboard — the bulk "check / review / update" bar
 * above the reading-lists grid.
 *
 * Workflow: "Check for updates" dry-runs every list (no changes applied) and
 * lists which ones have upstream changes with their diff counts. The user can
 * then update each one individually (review-before-apply) or hit "Update all"
 * to apply everything at once. Per-list updates reuse the existing
 * single-list refresh mutation, so they carry its detailed toast + cache
 * invalidation.
 */

import * as React from "react";
import { AlertTriangle, CheckCircle2, Loader2, RefreshCw } from "lucide-react";

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
import { Button } from "@/components/ui/button";
import {
  useCheckAllCblLists,
  useRefreshAllCblLists,
  useRefreshCblList,
} from "@/lib/api/mutations";
import type { CblBulkItemView } from "@/lib/api/types";
import { statusToneText } from "@/lib/ui/status-tone";
import { cn } from "@/lib/utils";

export function CblRefreshDashboard({ count }: { count: number }) {
  const checkAll = useCheckAllCblLists();
  const refreshAll = useRefreshAllCblLists();
  // `null` until the user runs a check; then the per-list outcomes.
  const [results, setResults] = React.useState<CblBulkItemView[] | null>(null);
  // Lists the user has updated individually from the review panel — dropped
  // from the pending set so the panel reflects what's left to do.
  const [appliedIds, setAppliedIds] = React.useState<ReadonlySet<string>>(
    new Set(),
  );

  if (count === 0) return null;

  const busy = checkAll.isPending || refreshAll.isPending;
  const pending = (results ?? []).filter(
    (r) => r.ok && r.changed && !appliedIds.has(r.id),
  );
  const failed = (results ?? []).filter((r) => !r.ok);

  function onCheckAll() {
    setAppliedIds(new Set());
    checkAll.mutate(undefined, {
      onSuccess: (d) => setResults(d?.items ?? []),
    });
  }
  function onUpdateAll() {
    refreshAll.mutate(undefined, {
      onSuccess: () => {
        setResults(null);
        setAppliedIds(new Set());
      },
    });
  }

  return (
    <div className="border-border bg-card mb-4 space-y-3 rounded-lg border p-3 sm:p-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="space-y-0.5">
          <h3 className="text-foreground text-sm font-semibold">
            Reading list updates
          </h3>
          <p className="text-muted-foreground text-xs">
            Check {count === 1 ? "your reading list" : `all ${count} lists`} for
            upstream changes, review, then update.
          </p>
        </div>
        <div className="flex shrink-0 gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={busy}
            onClick={onCheckAll}
          >
            {checkAll.isPending ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <RefreshCw className="h-4 w-4" />
            )}
            Check for updates
          </Button>
          <AlertDialog>
            <AlertDialogTrigger asChild>
              <Button type="button" size="sm" disabled={busy}>
                {refreshAll.isPending ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : null}
                Update all
              </Button>
            </AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Update all reading lists?</AlertDialogTitle>
                <AlertDialogDescription>
                  This re-fetches every reading list from its source and applies
                  any changes — new, removed, or reordered issues — across{" "}
                  {count} {count === 1 ? "list" : "lists"}. Your manual matches
                  are preserved. Prefer to look first? Use{" "}
                  <span className="font-medium">Check for updates</span> and
                  update lists one at a time.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Cancel</AlertDialogCancel>
                <AlertDialogAction onClick={onUpdateAll}>
                  Update all
                </AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        </div>
      </div>

      {results !== null && (
        <div className="border-border/60 border-t pt-3">
          {pending.length === 0 && failed.length === 0 ? (
            <p className="text-muted-foreground flex items-center gap-2 text-sm">
              <CheckCircle2
                className={cn("h-4 w-4", statusToneText("success"))}
              />
              Everything is up to date.
            </p>
          ) : (
            <ul className="space-y-1.5">
              {pending.map((item) => (
                <ReviewRow
                  key={item.id}
                  item={item}
                  onApplied={() =>
                    setAppliedIds((s) => new Set(s).add(item.id))
                  }
                />
              ))}
              {failed.map((item) => (
                <li
                  key={item.id}
                  className="text-muted-foreground flex items-center gap-2 text-sm"
                >
                  <AlertTriangle
                    className={cn(
                      "h-4 w-4 shrink-0",
                      statusToneText("warning"),
                    )}
                  />
                  <span className="text-foreground truncate">{item.name}</span>
                  <span className="text-xs">
                    couldn&rsquo;t be checked
                    {item.error ? ` — ${item.error}` : ""}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}

/** One updatable list in the review panel: name + diff summary + Update. */
function ReviewRow({
  item,
  onApplied,
}: {
  item: CblBulkItemView;
  onApplied: () => void;
}) {
  const refresh = useRefreshCblList(item.id);
  return (
    <li className="flex flex-wrap items-center gap-2 text-sm">
      <span className="text-foreground truncate font-medium">{item.name}</span>
      <span className="text-muted-foreground text-xs">
        {summarizeCounts(item)}
      </span>
      <Button
        type="button"
        size="sm"
        variant="outline"
        className="ml-auto"
        disabled={refresh.isPending}
        onClick={() => refresh.mutate({}, { onSuccess: onApplied })}
      >
        {refresh.isPending ? (
          <Loader2 className="h-3.5 w-3.5 animate-spin" />
        ) : null}
        Update
      </Button>
    </li>
  );
}

function summarizeCounts(i: CblBulkItemView): string {
  const parts: string[] = [];
  if (i.added) parts.push(`+${i.added} added`);
  if (i.removed) parts.push(`−${i.removed} removed`);
  if (i.reordered) parts.push(`${i.reordered} reordered`);
  if (i.rematched) parts.push(`${i.rematched} newly matched`);
  return parts.length ? parts.join(" · ") : "changes available";
}
