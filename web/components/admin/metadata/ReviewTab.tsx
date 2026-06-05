"use client";

/**
 * Bulk-metadata **Review** queue (refine-bulk-metadata M3).
 *
 * Pick a batch (or arrive deep-linked from a series / saved-view bulk fetch),
 * watch live progress, then accept findings:
 *   - **Strong** (`single_good`) — one click "Accept all strong (N)".
 *   - **Needs review** — open each in the Fetch-metadata dialog (which reuses
 *     the candidates already pulled by the batch — no re-search) and apply.
 *   - **No match** — opening runs a fresh search.
 *
 * Nothing here auto-applies: batch children run as `manual`, so this queue is
 * the accept surface.
 */

import { AlertCircle, CheckCircle2, ChevronRight, Loader2 } from "lucide-react";
import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  MetadataMatchDialog,
  type MetadataMatchScope,
} from "@/components/library/MetadataMatchDialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { useBatchApply } from "@/lib/api/mutations";
import {
  queryKeys,
  useMetadataBatch,
  useMetadataBatches,
} from "@/lib/api/queries";
import type { BatchChildRow } from "@/lib/api/types";

/** Resolve a child row to the Fetch-metadata dialog scope. The dialog probes +
 *  adopts the child's already-completed run (no re-search), so the operator
 *  reviews/applies the stored candidates in place. */
function scopeFor(c: BatchChildRow): MetadataMatchScope | null {
  if (!c.series_slug || !c.library_id) return null;
  return c.issue_slug
    ? {
        kind: "issue",
        seriesSlug: c.series_slug,
        issueSlug: c.issue_slug,
        libraryId: c.library_id,
      }
    : { kind: "series", seriesSlug: c.series_slug, libraryId: c.library_id };
}

export function ReviewTab({
  initialBatchId,
}: {
  initialBatchId?: string | null;
}) {
  const [batchId, setBatchId] = React.useState<string | null>(
    initialBatchId ?? null,
  );

  if (!batchId) {
    return <BatchPicker onPick={setBatchId} />;
  }
  return <BatchReview batchId={batchId} onBack={() => setBatchId(null)} />;
}

function BatchPicker({ onPick }: { onPick: (id: string) => void }) {
  const { data, isLoading } = useMetadataBatches();
  if (isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-8 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading batches…
      </div>
    );
  }
  const batches = data?.batches ?? [];
  if (batches.length === 0) {
    return (
      <p className="text-muted-foreground py-8 text-sm">
        No bulk-metadata batches yet. Start one from a series&rsquo; “Fetch all
        issues” action or a saved view.
      </p>
    );
  }
  return (
    <ul className="divide-border/60 border-border/60 divide-y overflow-hidden rounded-md border">
      {batches.map((b) => (
        <li key={b.batch_id}>
          <button
            type="button"
            onClick={() => onPick(b.batch_id)}
            className="hover:bg-muted/50 flex w-full items-center justify-between gap-3 px-3 py-2 text-left transition-colors"
          >
            <div>
              <div className="text-foreground text-sm font-medium">
                {scopeLabel(b.scope)} · {b.items_total} item
                {b.items_total === 1 ? "" : "s"}
              </div>
              <div className="text-muted-foreground text-xs">
                {new Date(b.created_at).toLocaleString()}
              </div>
            </div>
            <div className="flex items-center gap-2">
              <StatusBadge status={b.status} />
              <ChevronRight className="text-muted-foreground h-4 w-4" />
            </div>
          </button>
        </li>
      ))}
    </ul>
  );
}

function BatchReview({
  batchId,
  onBack,
}: {
  batchId: string;
  onBack: () => void;
}) {
  const { data, isLoading, isError } = useMetadataBatch(batchId);
  const apply = useBatchApply(batchId);
  const qc = useQueryClient();
  const [dialogScope, setDialogScope] =
    React.useState<MetadataMatchScope | null>(null);

  if (isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-8 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading batch…
      </div>
    );
  }
  if (isError || !data) {
    return (
      <p className="text-muted-foreground py-8 text-sm">
        Couldn&rsquo;t load this batch.
      </p>
    );
  }

  const a = data.aggregate;
  const searchedPct =
    data.items_total > 0 ? (a.searched / data.items_total) * 100 : 0;
  const strongRows = data.children.filter(
    (c) => c.outcome_kind === "single_good" && !c.applied,
  );
  const needsReview = data.children.filter((c) =>
    ["multi_good", "single_bad_cover", "multi_bad_cover"].includes(
      c.outcome_kind ?? "",
    ),
  );
  const noMatch = data.children.filter((c) => c.outcome_kind === "no_match");

  return (
    <div className="space-y-6">
      <button
        type="button"
        onClick={onBack}
        className="text-muted-foreground hover:text-foreground text-xs"
      >
        ← All batches
      </button>

      {/* Progress header */}
      <section className="space-y-2">
        <div className="flex items-center justify-between gap-4">
          <div className="flex items-center gap-2">
            <h3 className="text-foreground text-sm font-semibold">
              {scopeLabel(data.scope)}
            </h3>
            <StatusBadge status={data.status} />
          </div>
          <span className="text-muted-foreground text-xs tabular-nums">
            {a.searched} / {data.items_total} searched
          </span>
        </div>
        <Progress value={Math.min(searchedPct, 100)} />
        <div className="text-muted-foreground flex flex-wrap gap-x-4 gap-y-1 text-xs">
          <span>{a.strong} strong</span>
          <span>{a.needs_review} need review</span>
          <span>{a.no_match} no match</span>
          <span>{a.applied} applied</span>
          {a.awaiting_quota > 0 && (
            <span className="text-amber-600 dark:text-amber-400">
              {a.awaiting_quota} awaiting quota
              {data.resume_eta &&
                ` · resumes ${new Date(data.resume_eta).toLocaleTimeString()}`}
            </span>
          )}
          {a.in_flight > 0 && <span>{a.in_flight} searching…</span>}
        </div>
        {data.exceeds_budget && (
          <p className="text-xs text-amber-600 dark:text-amber-400">
            This batch exceeds the provider&rsquo;s daily budget — items beyond
            it park and auto-resume when the window frees.
          </p>
        )}
      </section>

      {/* Strong → accept all */}
      <section className="space-y-2">
        <div className="flex items-center justify-between">
          <h4 className="text-foreground text-sm font-semibold">
            Strong matches ({strongRows.length})
          </h4>
          {strongRows.length > 0 && (
            <Button
              size="sm"
              disabled={apply.isPending}
              onClick={() => apply.mutate({ filter: "all_strong" })}
            >
              {apply.isPending && (
                <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
              )}
              Accept all strong ({strongRows.length})
            </Button>
          )}
        </div>
        {strongRows.length === 0 ? (
          <p className="text-muted-foreground text-xs">
            No strong matches awaiting acceptance.
          </p>
        ) : (
          <ChildList rows={strongRows} tone="strong" onOpen={setDialogScope} />
        )}
      </section>

      {/* Needs review → open the dialog in place (reuses the stored run) */}
      {needsReview.length > 0 && (
        <section className="space-y-2">
          <h4 className="text-foreground text-sm font-semibold">
            Needs review ({needsReview.length})
          </h4>
          <p className="text-muted-foreground text-xs">
            Open each to compare the candidates already pulled and apply.
          </p>
          <ChildList rows={needsReview} tone="review" onOpen={setDialogScope} />
        </section>
      )}

      {/* No match → opening re-runs a fresh search */}
      {noMatch.length > 0 && (
        <section className="space-y-2">
          <h4 className="text-foreground text-sm font-semibold">
            No match ({noMatch.length})
          </h4>
          <ChildList rows={noMatch} tone="none" onOpen={setDialogScope} />
        </section>
      )}

      {dialogScope && (
        <MetadataMatchDialog
          open
          scope={dialogScope}
          onOpenChange={(next) => {
            if (!next) {
              setDialogScope(null);
              // An apply may have flipped a child; refresh the queue.
              qc.invalidateQueries({
                queryKey: queryKeys.metadataBatch(batchId),
              });
            }
          }}
        />
      )}
    </div>
  );
}

function ChildList({
  rows,
  tone,
  onOpen,
}: {
  rows: BatchChildRow[];
  tone: "strong" | "review" | "none";
  /** Open the Fetch-metadata dialog for a child (reuses its stored run). */
  onOpen: (scope: MetadataMatchScope) => void;
}) {
  return (
    <ul className="divide-border/60 border-border/60 divide-y overflow-hidden rounded-md border">
      {rows.map((c) => {
        const scope = scopeFor(c);
        const inner = (
          <div className="flex items-center justify-between gap-3 px-3 py-2">
            <div className="min-w-0">
              <span className="text-foreground text-sm">
                {c.label ?? c.scope_entity_id ?? c.run_id}
              </span>
              {c.applied && (
                <Badge variant="outline" className="ml-2 text-[10px]">
                  Applied
                </Badge>
              )}
            </div>
            <div className="flex items-center gap-2">
              <ToneDot tone={tone} />
              {scope && (
                <ChevronRight className="text-muted-foreground h-4 w-4" />
              )}
            </div>
          </div>
        );
        return (
          <li key={c.run_id}>
            {scope ? (
              <button
                type="button"
                onClick={() => onOpen(scope)}
                className="hover:bg-muted/50 block w-full text-left transition-colors"
              >
                {inner}
              </button>
            ) : (
              inner
            )}
          </li>
        );
      })}
    </ul>
  );
}

function ToneDot({ tone }: { tone: "strong" | "review" | "none" }) {
  const cls =
    tone === "strong"
      ? "bg-emerald-500"
      : tone === "review"
        ? "bg-amber-500"
        : "bg-muted-foreground/40";
  return <span className={`inline-block h-2.5 w-2.5 rounded-full ${cls}`} />;
}

function StatusBadge({ status }: { status: string }) {
  if (status === "completed") {
    return (
      <Badge
        variant="outline"
        className="border-emerald-500/60 text-emerald-600 dark:text-emerald-400"
      >
        <CheckCircle2 className="mr-1 h-3 w-3" /> Completed
      </Badge>
    );
  }
  if (status === "awaiting_quota") {
    return (
      <Badge
        variant="outline"
        className="border-amber-500/60 text-amber-600 dark:text-amber-400"
      >
        <AlertCircle className="mr-1 h-3 w-3" /> Awaiting quota
      </Badge>
    );
  }
  if (status === "partial_failed") {
    return (
      <Badge
        variant="outline"
        className="border-red-500/60 text-red-600 dark:text-red-400"
      >
        Partial
      </Badge>
    );
  }
  return (
    <Badge variant="secondary">
      <Loader2 className="mr-1 h-3 w-3 animate-spin" /> Running
    </Badge>
  );
}

function scopeLabel(scope: string): string {
  switch (scope) {
    case "series_issues":
      return "Series — all issues";
    case "saved_view":
      return "Saved view";
    case "library_refresh":
      return "Library refresh";
    default:
      return scope;
  }
}
