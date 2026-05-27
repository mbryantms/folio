"use client";

/**
 * "Fetch metadata…" dialog (metadata-providers-1.0 M5).
 *
 * The first user-visible piece of the metadata-providers integration.
 * Flow:
 *   1. user opens via Series Settings menu → POST /metadata/search fires
 *   2. dialog polls /metadata/candidates until the run finalizes
 *   3. ranked candidate cards render with confidence badges
 *   4. user picks one → POST /metadata/apply with mode toggle
 *
 * The dialog never auto-writes — every apply is an explicit click.
 * Mode defaults to `fill_missing` (the safer choice); `replace_all`
 * is a deliberate opt-in. `override_user_edits` is admin-only and
 * surfaced as a subtle secondary toggle to keep accidental clobbers
 * rare.
 */

import { Loader2, RefreshCw } from "lucide-react";
import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Switch } from "@/components/ui/switch";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { apiMutate } from "@/lib/api/mutations";
import {
  useApplyMetadataForIssue,
  useApplyMetadataForSeries,
  useClearIssueFieldPin,
} from "@/lib/api/mutations";
import {
  useLibrary,
  useMe,
  useMetadataCandidatesIssue,
  useMetadataCandidatesSeries,
  useMetadataProposedDiffIssue,
  useMetadataProposedDiffSeries,
} from "@/lib/api/queries";
import { useScanEvents } from "@/lib/api/scan-events";
import type {
  ApplyMode,
  CandidateView,
  MatchOutcomeView,
  SearchStartedResp,
} from "@/lib/api/types";

import {
  MetadataPreviewPane,
  defaultSelectedFields,
} from "@/components/library/MetadataPreviewPane";

export type MetadataMatchScope =
  | { kind: "series"; seriesSlug: string; libraryId: string }
  | { kind: "issue"; seriesSlug: string; issueSlug: string; libraryId: string };

export function MetadataMatchDialog({
  open,
  onOpenChange,
  scope,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  scope: MetadataMatchScope;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[90vh] flex-col sm:max-w-2xl">
        <MetadataMatchForm
          scope={scope}
          onClose={() => onOpenChange(false)}
          open={open}
        />
      </DialogContent>
    </Dialog>
  );
}

/**
 * Inner form — extracted so vitest can render without Radix portals
 * (matches the EditMetadataDialog split pattern).
 */
export function MetadataMatchForm({
  scope,
  onClose,
  open,
}: {
  scope: MetadataMatchScope;
  onClose: () => void;
  /** Used to gate the auto-kickoff effect so the search only fires
   *  once when the dialog opens. */
  open: boolean;
}) {
  const me = useMe();
  const isAdmin = me.data?.role === "admin";
  // Apply mutations stay on the useApiMutation pattern — they're
  // fired by an explicit user click (the Apply button) post-mount,
  // so the React 19 StrictMode dev mount→unmount→remount cycle has
  // long settled by the time the user clicks. The auto-kick search
  // below uses apiMutate directly instead, see below.
  const seriesApply = useApplyMetadataForSeries(
    scope.kind === "series" ? scope.seriesSlug : "",
  );
  const issueApply = useApplyMetadataForIssue(
    scope.kind === "issue" ? scope.seriesSlug : "",
    scope.kind === "issue" ? scope.issueSlug : "",
  );
  const apply = scope.kind === "series" ? seriesApply : issueApply;
  // M5.3 — issue-scope Revert-pin support. Series-scope diff lives on
  // the series row, not the issue row; surfacing series-scope pin
  // revert is a follow-up.
  const clearIssuePin = useClearIssueFieldPin(
    scope.kind === "issue" ? scope.seriesSlug : "",
    scope.kind === "issue" ? scope.issueSlug : "",
  );
  const onRevertPin =
    scope.kind === "issue"
      ? async (field: string) => {
          await clearIssuePin.mutateAsync({ field });
        }
      : undefined;
  const qc = useQueryClient();
  const [runId, setRunId] = React.useState<string | null>(null);
  const [searchPending, setSearchPending] = React.useState(false);
  const [searchError, setSearchError] = React.useState<string | null>(null);
  const seriesCandidates = useMetadataCandidatesSeries(
    scope.kind === "series" ? scope.seriesSlug : "",
    scope.kind === "series" ? runId : null,
  );
  const issueCandidates = useMetadataCandidatesIssue(
    scope.kind === "issue" ? scope.seriesSlug : "",
    scope.kind === "issue" ? scope.issueSlug : "",
    scope.kind === "issue" ? runId : null,
  );
  const candidates = scope.kind === "series" ? seriesCandidates : issueCandidates;
  const [mode, setMode] = React.useState<ApplyMode>("fill_missing");
  const [applyCover, setApplyCover] = React.useState(true);
  const [overrideUserEdits, setOverrideUserEdits] = React.useState(false);
  const [pickedOrdinal, setPickedOrdinal] = React.useState<number | null>(null);
  // M5 preview-pane state: when set, the dialog switches from the
  // candidate-list view to the diff preview for the chosen ordinal.
  const [previewOrdinal, setPreviewOrdinal] = React.useState<number | null>(
    null,
  );
  const [selectedFields, setSelectedFields] = React.useState<Set<string>>(
    new Set(),
  );
  const [overrideExternalIdSources, setOverrideExternalIdSources] =
    React.useState<Set<string>>(new Set());
  const seriesDiff = useMetadataProposedDiffSeries(
    scope.kind === "series" ? scope.seriesSlug : "",
    scope.kind === "series" ? runId : null,
    scope.kind === "series" ? previewOrdinal : null,
    mode,
    overrideUserEdits,
  );
  const issueDiff = useMetadataProposedDiffIssue(
    scope.kind === "issue" ? scope.seriesSlug : "",
    scope.kind === "issue" ? scope.issueSlug : "",
    scope.kind === "issue" ? runId : null,
    scope.kind === "issue" ? previewOrdinal : null,
    mode,
    overrideUserEdits,
  );
  const diffQuery = scope.kind === "series" ? seriesDiff : issueDiff;
  // Seed the default-checked set the first time a diff resolves for a
  // given ordinal. Tracked by a ref so re-renders for the same diff
  // don't re-stomp user toggle state.
  const lastSeededOrdinal = React.useRef<number | null>(null);
  React.useEffect(() => {
    if (
      previewOrdinal != null &&
      diffQuery.data &&
      lastSeededOrdinal.current !== previewOrdinal
    ) {
      setSelectedFields(defaultSelectedFields(diffQuery.data));
      setOverrideExternalIdSources(new Set());
      lastSeededOrdinal.current = previewOrdinal;
    }
  }, [previewOrdinal, diffQuery.data]);

  // Auto-kick the search via raw apiMutate — NOT useApiMutation.
  // Backstory: TanStack Query v5's useMutation observer ends up
  // disconnected from its state machine when fired from an effect
  // that gets cleaned up by React 19 StrictMode's intentional
  // mount → unmount → remount dev cycle. The mutationFn promise
  // resolves (the network roundtrip completes), but the observer's
  // `data`/`status`/per-call onSuccess all silently drop on the
  // floor — the kick effect would set `search.mutate()` running,
  // then the cleanup tears the observer down, then the resolution
  // arrives nowhere. Bypassing the observer entirely with a direct
  // apiMutate call sidesteps the issue: the response lands in plain
  // React state via the local `searchPending`/`runId` slots, which
  // survive the strict-mode cycle just like any other useState.
  const searchPath = React.useMemo(
    () =>
      scope.kind === "series"
        ? `/series/${encodeURIComponent(scope.seriesSlug)}/metadata/search`
        : `/series/${encodeURIComponent(scope.seriesSlug)}/issues/${encodeURIComponent(scope.issueSlug)}/metadata/search`,
    [scope],
  );
  const candidatesInvalidateKey = React.useMemo(
    () =>
      scope.kind === "series"
        ? ["series", scope.seriesSlug, "metadata", "candidates"]
        : [
            "series",
            scope.seriesSlug,
            "issues",
            scope.issueSlug,
            "metadata",
            "candidates",
          ],
    [scope],
  );

  const runSearch = React.useCallback(() => {
    setSearchPending(true);
    setSearchError(null);
    let cancelled = false;
    void (async () => {
      try {
        const result = await apiMutate<SearchStartedResp>({
          path: searchPath,
          method: "POST",
        });
        if (cancelled) return;
        if (result?.run_id) {
          setRunId(result.run_id);
          qc.invalidateQueries({ queryKey: candidatesInvalidateKey });
        } else {
          setSearchError("Empty response from search endpoint.");
        }
      } catch (e) {
        if (cancelled) return;
        const msg = e instanceof Error ? e.message : "Search failed.";
        setSearchError(msg);
        toast.error(msg);
      } finally {
        if (!cancelled) setSearchPending(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [searchPath, candidatesInvalidateKey, qc]);

  const kickedRef = React.useRef(false);
  React.useEffect(() => {
    if (!open) {
      kickedRef.current = false;
      return;
    }
    if (kickedRef.current) return;
    kickedRef.current = true;
    runSearch();
    // runSearch is stable per (searchPath, key) — those don't change
    // mid-dialog.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const runStatus = candidates.data?.status ?? "queued";
  // "Searching providers..." renders when EITHER the POST is in
  // flight (`searchPending`) OR the run row has been created but the
  // worker hasn't finalized it yet (`runStatus` queued/searching).
  // The search-creation step that races with StrictMode lands the
  // run row in DB — surfacing searchPending lets the dialog show
  // progress even before the run row is queryable.
  const isPolling =
    searchPending || runStatus === "queued" || runStatus === "searching";
  const isFinalized =
    runStatus === "completed" ||
    runStatus === "failed" ||
    runStatus === "awaiting_quota";

  // M5: clicking a candidate's "Preview" now stages the diff view
  // instead of immediately writing. The actual apply fires from the
  // preview pane's "Apply N changes" button via `onConfirmApply`.
  const onEnterPreview = (ordinal: number) => {
    setPreviewOrdinal(ordinal);
    setPickedOrdinal(ordinal);
    // Reset per-ordinal selection state; seeded once diff resolves.
    lastSeededOrdinal.current = null;
    setSelectedFields(new Set());
    setOverrideExternalIdSources(new Set());
  };
  const onExitPreview = () => {
    setPreviewOrdinal(null);
  };
  const onConfirmApply = () => {
    if (!runId || previewOrdinal == null) return;
    apply.mutate({
      run_id: runId,
      ordinal: previewOrdinal,
      mode,
      apply_cover: applyCover,
      override_user_edits: overrideUserEdits,
      selected_fields: Array.from(selectedFields),
      override_external_id_sources: Array.from(overrideExternalIdSources),
    });
    // The auto-close happens via the apply.isSuccess watcher below
    // rather than a per-call onSuccess — same StrictMode-remount race
    // that strands the search kick's onSuccess.
  };

  // M8: one-click apply for `single_good_match`. Skips the preview
  // pane — empty `selected_fields` is the apply-all signal the
  // backend already understands. Operator can still cancel via Esc
  // since the mutation finalizes asynchronously.
  const onOneClickApply = () => {
    if (!runId) return;
    setPickedOrdinal(0);
    apply.mutate({
      run_id: runId,
      ordinal: 0,
      mode,
      apply_cover: applyCover,
      override_user_edits: overrideUserEdits,
      selected_fields: [],
      override_external_id_sources: [],
    });
  };

  // M5.2 — When the target library has `metadata_writeback_enabled=true`,
  // an apply enqueues a downstream `RewriteIssueSidecarsJob` which then
  // triggers a scoped scanner rescan. The DB cache reflects the new
  // metadata only after that rescan finishes. Auto-close-on-apply would
  // hand the user back to an issue page that still shows the old data
  // for 1-3 seconds. Instead we transition into a `waiting_for_rescan`
  // state, watch the scan WebSocket for a `scan.completed` event
  // matching this library, and close then.
  const libraryQ = useLibrary(scope.libraryId);
  const writebackEnabled = Boolean(
    libraryQ.data?.metadata_writeback_enabled &&
      libraryQ.data?.allow_archive_writeback,
  );
  // Watershed timestamp set at apply-time. We ignore any `scan.completed`
  // event whose payload `at` predates it (the user might have triggered
  // a scan elsewhere; we want our scan, not the earlier one).
  const [applyAt, setApplyAt] = React.useState<number | null>(null);
  // `apply.isSuccess` resolves when the apply-API POST returned 202,
  // not when the rewrite + rescan actually finished. We use it as the
  // trigger to enter the waiting state.
  React.useEffect(() => {
    if (!apply.isSuccess) return;
    if (!writebackEnabled) {
      onClose();
      return;
    }
    // eslint-disable-next-line react-hooks/set-state-in-effect -- legitimate transition: apply success triggers wait-for-rescan.
    setApplyAt(Date.now());
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [apply.isSuccess, writebackEnabled]);

  // Subscribe to the library's scan events only when waiting. The
  // existing `useScanEvents` hook auto-reconnects + filters by
  // libraryId server-side, and tolerates re-subscribes (module-level
  // ticket dedupe).
  const waitingForRescan = applyAt !== null;
  const scanEvents = useScanEvents({
    libraryId: waitingForRescan ? scope.libraryId : undefined,
    // The dialog already owns "Apply succeeded" feedback; don't toast.
    toastCompletions: false,
    toastErrors: false,
  });
  // Watch the events buffer for a `scan.completed` for this library.
  // The subscription only starts when `applyAt` is set, so any
  // completed event in the buffer is by definition post-apply — no
  // need to filter by timestamp (the `scan.completed` payload doesn't
  // carry one anyway; `scan.started` does, but we don't need it).
  React.useEffect(() => {
    if (!waitingForRescan) return;
    const completed = scanEvents.events.find((e) => e.type === "scan.completed");
    if (completed) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- WS event arrival is the trigger; resetting `applyAt` to null also tears down the subscription on the next render.
      setApplyAt(null);
      onClose();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scanEvents.events, waitingForRescan]);

  // 30s timeout fallback — close anyway with an info toast so the user
  // isn't stuck if the WS missed the event (rare; broadcast lag,
  // disconnect-reconnect, etc.). The rewrite has already landed by this
  // point; the data will refresh on the next page navigation.
  React.useEffect(() => {
    if (!waitingForRescan) return;
    const t = setTimeout(() => {
      setApplyAt(null);
      toast.info("Refresh may take a moment — close and reopen the page to see the latest data.");
      onClose();
    }, 30_000);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [waitingForRescan]);

  // Series-scope progress chip — derived from `scan.progress` events.
  // The scanner emits these continuously while it walks the series;
  // the latest `series_scanned/series_total` gives a usable progress
  // hint for the user. Issue-scope rescans are too short to be worth
  // a progress bar — the spinner alone is fine.
  //
  // Computed inline (no useMemo) so the react-compiler's preservation
  // analysis doesn't have to bend around a `for`-loop with a return
  // inside. The compiler handles surrounding memoization automatically.
  let seriesProgress: { done: number; total: number } | null = null;
  if (waitingForRescan && scope.kind === "series") {
    for (let i = scanEvents.events.length - 1; i >= 0; i--) {
      const e = scanEvents.events[i];
      if (e.type === "scan.progress" && e.series_total > 0) {
        seriesProgress = { done: e.series_scanned, total: e.series_total };
        break;
      }
    }
  }

  const restart = () => {
    setRunId(null);
    setPickedOrdinal(null);
    kickedRef.current = false;
    runSearch();
  };

  return (
    <>
      <DialogHeader>
        <DialogTitle>Fetch metadata</DialogTitle>
        <DialogDescription>
          {waitingForRescan
            ? scope.kind === "series"
              ? seriesProgress
                ? `Writing sidecars + scanning ${seriesProgress.done}/${seriesProgress.total}…`
                : "Writing sidecars + scanning series…"
              : "Writing sidecar + refreshing…"
            : isPolling
              ? "Searching providers…"
              : runStatus === "awaiting_quota"
                ? "Providers are out of quota — try again shortly."
                : runStatus === "failed"
                  ? "Search failed — see Error below."
                  : `${candidates.data?.candidates.length ?? 0} match${
                      (candidates.data?.candidates.length ?? 0) === 1
                        ? ""
                        : "es"
                    } from ${
                      candidates.data?.providers.join(", ") ?? "providers"
                    }.`}
        </DialogDescription>
      </DialogHeader>

      {waitingForRescan && (
        <div className="text-muted-foreground flex items-center gap-2 py-2 text-sm">
          <Loader2 className="h-4 w-4 animate-spin" />
          {scope.kind === "series" && seriesProgress ? (
            <span>
              Rescanning series ({seriesProgress.done} of {seriesProgress.total})
            </span>
          ) : (
            <span>
              Sidecar written — waiting for the rescan to ingest the new XML…
            </span>
          )}
        </div>
      )}

      <div className="flex items-center justify-between gap-3 pb-2">
        <div className="flex items-center gap-2">
          <Label htmlFor="mmd-mode" className="text-sm font-medium">
            Mode
          </Label>
          <RadioGroup
            id="mmd-mode"
            value={mode}
            onValueChange={(v) => setMode(v as ApplyMode)}
            className="flex gap-3"
          >
            <Label className="flex cursor-pointer items-center gap-1.5 text-sm">
              <RadioGroupItem value="fill_missing" /> Fill missing
            </Label>
            <Label className="flex cursor-pointer items-center gap-1.5 text-sm">
              <RadioGroupItem value="replace_all" /> Replace all
            </Label>
          </RadioGroup>
        </div>
        <div className="flex items-center gap-2 text-sm">
          <Label htmlFor="mmd-cover" className="cursor-pointer">
            Apply cover
          </Label>
          <Switch
            id="mmd-cover"
            checked={applyCover}
            onCheckedChange={setApplyCover}
          />
        </div>
      </div>

      {isAdmin && (
        <div className="text-muted-foreground flex items-center gap-2 pb-2 text-xs">
          <Switch
            id="mmd-override"
            checked={overrideUserEdits}
            onCheckedChange={setOverrideUserEdits}
          />
          <Label htmlFor="mmd-override" className="cursor-pointer">
            Override user-edited fields (audited as{" "}
            <code>metadata_apply_force</code>)
          </Label>
        </div>
      )}

      {previewOrdinal != null ? (
        <MetadataPreviewPane
          data={diffQuery.data}
          isLoading={diffQuery.isLoading || diffQuery.isFetching}
          errorMessage={
            diffQuery.error
              ? diffQuery.error instanceof Error
                ? diffQuery.error.message
                : "Failed to compute preview."
              : null
          }
          selectedFields={selectedFields}
          overrideExternalIdSources={overrideExternalIdSources}
          onChangeSelected={setSelectedFields}
          onChangeOverrideSources={setOverrideExternalIdSources}
          onBack={onExitPreview}
          onApply={onConfirmApply}
          isApplying={apply.isPending}
          canOverride={isAdmin}
          onRevertPin={onRevertPin}
        />
      ) : (
        <ScrollArea className="max-h-[50vh] pr-3 [&>div>div]:block!">
          {searchError ? (
            <div className="text-destructive py-6 text-sm">
              {searchError}{" "}
              <button onClick={restart} className="underline">
                Retry
              </button>
            </div>
          ) : isPolling ? (
            <div className="text-muted-foreground flex items-center justify-center gap-2 py-12 text-sm">
              <Loader2 className="h-4 w-4 animate-spin" /> Searching providers…
            </div>
          ) : runStatus === "awaiting_quota" ? (
            <div className="text-muted-foreground py-6 text-sm">
              Every configured provider is out of quota right now.{" "}
              <button onClick={restart} className="underline">
                Retry
              </button>{" "}
              once the budget refills.
            </div>
          ) : runStatus === "failed" ? (
            <div className="text-destructive py-6 text-sm">
              Error: {candidates.data?.error_summary ?? "unknown failure"}.{" "}
              <button onClick={restart} className="underline">
                Retry
              </button>
            </div>
          ) : (
            <>
              {candidates.data?.match_outcome && (
                <MatchOutcomeBanner
                  outcome={candidates.data.match_outcome}
                  topCandidate={candidates.data.candidates[0]}
                  isApplying={apply.isPending && pickedOrdinal === 0}
                  onOneClickApply={onOneClickApply}
                  onShowDetails={() => onEnterPreview(0)}
                />
              )}
              <ul className="space-y-2 py-2">
                {(candidates.data?.candidates ?? []).map((c, i) => (
                  <CandidateRow
                    key={`${c.source}-${c.external_id}-${i}`}
                    c={c}
                    ordinal={i}
                    disabled={apply.isPending}
                    isApplying={apply.isPending && pickedOrdinal === i}
                    onApply={() => onEnterPreview(i)}
                  />
                ))}
                {isFinalized && (candidates.data?.candidates.length ?? 0) === 0 && (
                  <li className="text-muted-foreground py-8 text-center text-sm">
                    No matches. Try editing the series name or year.
                  </li>
                )}
              </ul>
            </>
          )}
        </ScrollArea>
      )}

      <DialogFooter className="flex items-center justify-between gap-2 sm:justify-between">
        <Button
          variant="ghost"
          size="sm"
          onClick={restart}
          disabled={isPolling || apply.isPending}
        >
          <RefreshCw className="mr-1.5 h-3.5 w-3.5" /> Re-search
        </Button>
        <Button variant="outline" onClick={onClose}>
          Close
        </Button>
      </DialogFooter>
    </>
  );
}

/**
 * Match-outcome banner (matching-accuracy-1.0 M8).
 *
 * Renders header copy + a primary-action button per outcome variant —
 * the dialog state machine ComicTagger uses for the same UX cue:
 *   - `single_good`         → "Strong match: …" + one-click Apply
 *   - `multi_good`          → "Multiple strong matches" + pick from list
 *   - `single_bad_cover`    → "One plausible match — cover doesn't match"
 *   - `multi_bad_cover`     → "No strong match. Review or re-search"
 *   - `no_match`            → no banner (the empty-state row handles it)
 */
function MatchOutcomeBanner({
  outcome,
  topCandidate,
  isApplying,
  onOneClickApply,
  onShowDetails,
}: {
  outcome: MatchOutcomeView;
  topCandidate: CandidateView | undefined;
  isApplying: boolean;
  onOneClickApply: () => void;
  onShowDetails: () => void;
}) {
  if (outcome.kind === "no_match") {
    return null;
  }
  const topName = topCandidate
    ? extractCandidateTitle(topCandidate)
    : "Top candidate";
  const altBadge = outcome.matched_via_alternate ? (
    <span className="text-muted-foreground ml-2 text-[10px] uppercase tracking-wide">
      via alternate cover
    </span>
  ) : null;

  if (outcome.kind === "single_good") {
    return (
      <div className="bg-emerald-500/5 border-emerald-500/30 my-2 rounded-md border p-3">
        <div className="flex items-start justify-between gap-3">
          <div className="space-y-1">
            <p className="text-sm font-medium">
              Strong match: {topName}
              {altBadge}
            </p>
            <p className="text-muted-foreground text-xs">
              Apply now to write every field. Use{" "}
              <button
                type="button"
                onClick={onShowDetails}
                className="underline underline-offset-2"
              >
                Show details
              </button>{" "}
              to pick fields first.
            </p>
          </div>
          <Button
            size="sm"
            onClick={onOneClickApply}
            disabled={isApplying}
          >
            {isApplying ? (
              <>
                <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />
                Applying
              </>
            ) : (
              "Apply"
            )}
          </Button>
        </div>
      </div>
    );
  }

  if (outcome.kind === "multi_good") {
    return (
      <div className="bg-muted/40 border-border my-2 rounded-md border p-3">
        <p className="text-sm font-medium">Multiple strong matches</p>
        <p className="text-muted-foreground text-xs">
          Pick the right candidate from the list below — every option is
          a plausible match.
        </p>
      </div>
    );
  }

  if (outcome.kind === "single_bad_cover") {
    return (
      <div className="bg-amber-500/5 border-amber-500/30 my-2 rounded-md border p-3">
        <p className="text-sm font-medium">
          One plausible match — cover doesn&rsquo;t match{altBadge}
        </p>
        <p className="text-muted-foreground text-xs">
          {outcome.top_hamming != null
            ? `Cover Hamming distance ${outcome.top_hamming} bits — verify before applying.`
            : "No cover comparison available. Verify the match by reviewing the details before applying."}
        </p>
      </div>
    );
  }

  // multi_bad_cover
  return (
    <div className="bg-muted/40 border-border my-2 rounded-md border p-3">
      <p className="text-sm font-medium">No strong match</p>
      <p className="text-muted-foreground text-xs">
        Review the candidates below or re-search with different facts
        (try editing the series name or year).
      </p>
    </div>
  );
}

/**
 * Pull a readable title out of the candidate payload for the banner.
 * `CandidateView.candidate` is the raw provider payload — series
 * candidates carry `name + year`; issue candidates carry
 * `series_name + issue_number`. Best-effort; the dialog will still
 * render even if neither field is present.
 */
function extractCandidateTitle(c: CandidateView): string {
  const payload =
    c.candidate && typeof c.candidate === "object"
      ? (c.candidate as Record<string, unknown>)
      : {};
  const seriesName =
    typeof payload.name === "string"
      ? payload.name
      : typeof payload.series_name === "string"
        ? payload.series_name
        : null;
  const issueNumber =
    typeof payload.issue_number === "string" ? payload.issue_number : null;
  const year = typeof payload.year === "number" ? payload.year : null;
  if (seriesName && issueNumber) {
    return `${seriesName} #${issueNumber}`;
  }
  if (seriesName && year) {
    return `${seriesName} (${year})`;
  }
  return seriesName ?? c.external_id;
}

function CandidateRow({
  c,
  ordinal,
  disabled,
  isApplying,
  onApply,
}: {
  c: CandidateView;
  ordinal: number;
  disabled: boolean;
  isApplying: boolean;
  onApply: () => void;
}) {
  const parsed = parseCandidatePayload(c.candidate);
  const _ = ordinal;
  return (
    <li className="bg-muted/40 hover:bg-muted/60 flex gap-3 rounded-md p-3 transition-colors">
      {parsed.cover_image_url ? (
        // eslint-disable-next-line @next/next/no-img-element
        <img
          src={parsed.cover_image_url}
          alt={parsed.name ?? c.external_id}
          loading="lazy"
          className="h-20 w-14 shrink-0 rounded object-cover"
        />
      ) : (
        <div className="bg-muted h-20 w-14 shrink-0 rounded" aria-hidden />
      )}
      <div className="min-w-0 flex-1">
        <div className="flex items-center justify-between gap-2">
          <div className="flex min-w-0 items-center gap-2">
            <span className="truncate text-sm font-medium">
              {parsed.name ?? c.external_id}
              {parsed.year ? ` (${parsed.year})` : ""}
            </span>
            <ConfidenceBadge bucket={c.bucket} score={c.score} />
            <SourceBadge source={c.source} url={parsed.external_url} />
          </div>
          <Button
            size="sm"
            onClick={onApply}
            disabled={disabled}
            aria-busy={isApplying}
          >
            {isApplying ? (
              <>
                <Loader2 className="mr-1 h-3 w-3 animate-spin" /> Applying
              </>
            ) : (
              "Preview"
            )}
          </Button>
        </div>
        <div className="text-muted-foreground mt-1 text-xs">
          {parsed.publisher ?? "—"}
          {parsed.issue_count != null
            ? ` · ${parsed.issue_count} issue${parsed.issue_count === 1 ? "" : "s"}`
            : ""}
        </div>
      </div>
    </li>
  );
}

function ConfidenceBadge({ bucket, score }: { bucket: string; score: number }) {
  const label = bucket.toUpperCase();
  const variant =
    bucket === "high"
      ? "default"
      : bucket === "medium"
        ? "secondary"
        : "outline";
  return (
    <Badge
      variant={variant as "default" | "secondary" | "outline"}
      title={`Score: ${score.toFixed(1)}`}
    >
      {label}
    </Badge>
  );
}

function SourceBadge({
  source,
  url,
}: {
  source: string;
  url: string | null | undefined;
}) {
  if (url) {
    return (
      <a
        href={url}
        target="_blank"
        rel="noreferrer"
        className="text-muted-foreground text-xs underline-offset-2 hover:underline"
      >
        {labelForSource(source)} ↗
      </a>
    );
  }
  return (
    <span className="text-muted-foreground text-xs">
      {labelForSource(source)}
    </span>
  );
}

function labelForSource(s: string): string {
  switch (s) {
    case "comicvine":
      return "ComicVine";
    case "metron":
      return "Metron";
    case "gcd":
      return "GCD";
    case "marvel":
      return "Marvel";
    case "locg":
      return "LoCG";
    default:
      return s;
  }
}

/** Best-effort parse of the JSONB-stored SeriesCandidate. The shape
 *  matches the Rust `SeriesCandidate` struct but we don't fail loudly
 *  on schema drift — surface what we can. */
function parseCandidatePayload(payload: unknown): {
  name: string | null;
  year: number | null;
  publisher: string | null;
  issue_count: number | null;
  cover_image_url: string | null;
  external_url: string | null;
} {
  if (!payload || typeof payload !== "object") {
    return {
      name: null,
      year: null,
      publisher: null,
      issue_count: null,
      cover_image_url: null,
      external_url: null,
    };
  }
  const obj = payload as Record<string, unknown>;
  return {
    name: typeof obj.name === "string" ? obj.name : null,
    year: typeof obj.year === "number" ? obj.year : null,
    publisher: typeof obj.publisher === "string" ? obj.publisher : null,
    issue_count: typeof obj.issue_count === "number" ? obj.issue_count : null,
    cover_image_url:
      typeof obj.cover_image_url === "string" ? obj.cover_image_url : null,
    external_url:
      typeof obj.external_url === "string" ? obj.external_url : null,
  };
}
