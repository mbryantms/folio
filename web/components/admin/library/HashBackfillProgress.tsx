"use client";

import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { useStartHashBackfill } from "@/lib/api/mutations";
import { useHashBackfill } from "@/lib/api/queries";
import type { HashBackfillView } from "@/lib/api/types";

/** Percent of live issues whose content hash has landed (0–100). An empty
 *  library reads as complete. */
export function hashBackfillPercent(
  v: Pick<HashBackfillView, "hashed" | "total">,
): number {
  if (v.total <= 0) return 100;
  return Math.min(100, Math.round((v.hashed / v.total) * 100));
}

/**
 * First-import lazy-hash progress (WP-3.2). Renders nothing while no
 * issue is pending, so libraries that never used the mode see no extra
 * chrome. Polls (via `useHashBackfill`) while hashes are outstanding.
 */
export function HashBackfillProgress({ librarySlug }: { librarySlug: string }) {
  const q = useHashBackfill(librarySlug);
  const start = useStartHashBackfill(librarySlug);
  const v = q.data;
  if (!v || v.pending === 0) return null;
  const pct = hashBackfillPercent(v);
  return (
    <div className="border-border bg-muted/40 space-y-2 rounded-md border px-3 py-3">
      <div className="flex items-center justify-between gap-3 text-sm">
        <span className="text-foreground font-medium">
          Content hashing in progress
        </span>
        <span className="text-muted-foreground tabular-nums">
          {v.hashed.toLocaleString()} / {v.total.toLocaleString()} ({pct}%)
        </span>
      </div>
      <Progress value={pct} aria-label="Content hashing progress" />
      <div className="flex items-center justify-between gap-3">
        <p className="text-muted-foreground text-xs">
          {v.pending.toLocaleString()} file{v.pending === 1 ? "" : "s"} still
          identified by path. Duplicate copies are flagged once their hashes
          land.
        </p>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={start.isPending}
          onClick={() => start.mutate()}
        >
          Resume
        </Button>
      </div>
    </div>
  );
}
