"use client";

/**
 * Bulk-metadata **Review** queue (refine-bulk-metadata M3).
 *
 * Pick a batch (or arrive deep-linked from a series / saved-view bulk fetch),
 * watch live progress, then accept findings:
 *   - **Strong** (`single_good`) — one click "Accept all strong (N)".
 *   - **Needs review** — bulk "Fill missing" / "Replace all" that auto-applies
 *     the most-complete merge across providers (no per-item review), or open
 *     each in the Fetch-metadata dialog to compare candidates by hand.
 *   - **No match** — opening runs a fresh search.
 *
 * The bulk needs-review actions are the operator's consent to auto-resolve:
 * they leverage whatever providers matched to assemble the complete record and
 * apply directly, draining the queue without opening each item.
 */

import { AlertCircle, CheckCircle2, ChevronRight, Loader2 } from "lucide-react";
import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  MetadataMatchDialog,
  type MetadataMatchScope,
} from "@/components/library/MetadataMatchDialog";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Progress } from "@/components/ui/progress";
import { useBatchApply } from "@/lib/api/mutations";
import {
  queryKeys,
  useMetadataBatch,
  useMetadataBatches,
} from "@/lib/api/queries";
import {
  statusTone,
  statusToneText,
  statusToneDot,
} from "@/lib/ui/status-tone";
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
            <span className={statusToneText("warning")}>
              {a.awaiting_quota} awaiting quota
              {data.resume_eta &&
                ` · resumes ${new Date(data.resume_eta).toLocaleTimeString()}`}
            </span>
          )}
          {a.in_flight > 0 && <span>{a.in_flight} searching…</span>}
        </div>
        {data.exceeds_budget && (
          <p className={`text-xs ${statusToneText("warning")}`}>
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

      {/* Needs review → bulk auto-apply across providers, or open by hand */}
      {needsReview.length > 0 && (
        <NeedsReviewSection
          rows={needsReview}
          apply={apply}
          onOpen={setDialogScope}
        />
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

/**
 * Needs-review queue with bulk auto-apply. "Fill missing" / "Replace all"
 * apply the most-complete multi-provider merge to every unapplied item (or
 * the selected subset) without per-item review. Operators can still open a
 * single item to compare candidates by hand.
 */
function NeedsReviewSection({
  rows,
  apply,
  onOpen,
}: {
  rows: BatchChildRow[];
  apply: ReturnType<typeof useBatchApply>;
  onOpen: (scope: MetadataMatchScope) => void;
}) {
  const [scopeMode, setScopeMode] = React.useState<"all" | "selected">("all");
  const [selected, setSelected] = React.useState<Set<string>>(new Set());
  const [confirmReplace, setConfirmReplace] = React.useState(false);

  // Only unapplied rows are actionable; an already-applied child is skipped
  // server-side anyway.
  const unapplied = rows.filter((c) => !c.applied);
  const targetCount =
    scopeMode === "selected" ? selected.size : unapplied.length;
  const runIds = scopeMode === "selected" ? Array.from(selected) : undefined;
  const busy = apply.isPending;
  const disabled = busy || targetCount === 0;

  const toggle = (runId: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(runId)) next.delete(runId);
      else next.add(runId);
      return next;
    });

  const runApply = (mode: "fill_missing" | "replace_all") =>
    apply.mutate({ filter: "all_needs_review", mode, run_ids: runIds });

  return (
    <section className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h4 className="text-foreground text-sm font-semibold">
          Needs review ({rows.length})
        </h4>
        {/* Scope toggle: act on all unapplied items, or a hand-picked subset. */}
        <div className="bg-muted/40 inline-flex rounded-md p-0.5 text-xs">
          {(["all", "selected"] as const).map((m) => (
            <button
              key={m}
              type="button"
              onClick={() => setScopeMode(m)}
              className={
                scopeMode === m
                  ? "bg-background text-foreground rounded px-2 py-1 shadow-sm"
                  : "text-muted-foreground px-2 py-1"
              }
            >
              {m === "all" ? "All" : `Selected (${selected.size})`}
            </button>
          ))}
        </div>
      </div>

      <p className="text-muted-foreground text-xs">
        Auto-apply the most-complete metadata merged across every provider that
        matched — no per-item review. Covers prefer ComicVine. Your pinned
        fields are preserved. Or open a row to compare candidates by hand.
      </p>

      <div className="flex flex-wrap items-center gap-2">
        <Button
          size="sm"
          disabled={disabled}
          onClick={() => runApply("fill_missing")}
        >
          {busy && <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />}
          Fill missing ({targetCount})
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={disabled}
          onClick={() => setConfirmReplace(true)}
        >
          Replace all ({targetCount})
        </Button>
      </div>

      <ChildList
        rows={rows}
        tone="review"
        onOpen={onOpen}
        selectable={scopeMode === "selected"}
        selectedIds={selected}
        onToggleSelect={toggle}
      />

      <AlertDialog open={confirmReplace} onOpenChange={setConfirmReplace}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              Replace metadata for {targetCount} item
              {targetCount === 1 ? "" : "s"}?
            </AlertDialogTitle>
            <AlertDialogDescription>
              This overwrites existing non-pinned fields and the primary cover
              with the most-complete merge across providers. Fields you pinned
              are preserved. This can&rsquo;t be undone in bulk.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                setConfirmReplace(false);
                runApply("replace_all");
              }}
            >
              Replace all
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}

function ChildList({
  rows,
  tone,
  onOpen,
  selectable = false,
  selectedIds,
  onToggleSelect,
}: {
  rows: BatchChildRow[];
  tone: "strong" | "review" | "none";
  /** Open the Fetch-metadata dialog for a child (reuses its stored run). */
  onOpen: (scope: MetadataMatchScope) => void;
  /** Render a selection checkbox per unapplied row (Selected scope). */
  selectable?: boolean;
  selectedIds?: Set<string>;
  onToggleSelect?: (runId: string) => void;
}) {
  return (
    <ul className="divide-border/60 border-border/60 divide-y overflow-hidden rounded-md border">
      {rows.map((c) => {
        const scope = scopeFor(c);
        const label = (
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
        );
        const trailing = (
          <div className="flex items-center gap-2">
            <ToneDot tone={tone} />
            {scope && (
              <ChevronRight className="text-muted-foreground h-4 w-4" />
            )}
          </div>
        );

        // Selectable rows can't nest the open-button inside a checkbox row
        // (invalid HTML), so the checkbox toggles selection and a separate
        // open-button carries the label. Already-applied rows aren't
        // selectable (skipped server-side).
        if (selectable) {
          const checkable = !c.applied;
          return (
            <li key={c.run_id} className="flex items-center gap-3 px-3 py-2">
              <Checkbox
                checked={selectedIds?.has(c.run_id) ?? false}
                disabled={!checkable}
                onCheckedChange={() => onToggleSelect?.(c.run_id)}
                aria-label="Select for bulk apply"
              />
              {scope ? (
                <button
                  type="button"
                  onClick={() => onOpen(scope)}
                  className="hover:bg-muted/50 flex min-w-0 flex-1 items-center justify-between gap-3 rounded text-left transition-colors"
                >
                  {label}
                  {trailing}
                </button>
              ) : (
                <div className="flex min-w-0 flex-1 items-center justify-between gap-3">
                  {label}
                  {trailing}
                </div>
              )}
            </li>
          );
        }

        const inner = (
          <div className="flex items-center justify-between gap-3 px-3 py-2">
            {label}
            {trailing}
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
      ? statusToneDot("success")
      : tone === "review"
        ? statusToneDot("warning")
        : "bg-muted-foreground/40";
  return <span className={`inline-block h-2.5 w-2.5 rounded-full ${cls}`} />;
}

function StatusBadge({ status }: { status: string }) {
  if (status === "completed") {
    return (
      <Badge variant="outline" className={statusTone("success")}>
        <CheckCircle2 className="mr-1 h-3 w-3" /> Completed
      </Badge>
    );
  }
  if (status === "awaiting_quota") {
    return (
      <Badge variant="outline" className={statusTone("warning")}>
        <AlertCircle className="mr-1 h-3 w-3" /> Awaiting quota
      </Badge>
    );
  }
  if (status === "partial_failed") {
    return (
      <Badge variant="outline" className={statusTone("error")}>
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
