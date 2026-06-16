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
import { Switch } from "@/components/ui/switch";
import { useQueryClient } from "@tanstack/react-query";
import { useRouter } from "next/navigation";
import { toast } from "sonner";

import { apiMutate } from "@/lib/api/mutations";
import {
  useApplyCompositeMetadataForIssue,
  useApplyCompositeMetadataForSeries,
  useApplyMetadataForIssue,
  useApplyMetadataForSeries,
  useClearIssueFieldPin,
} from "@/lib/api/mutations";
import {
  jsonFetch,
  useMe,
  useMetadataCandidatesIssue,
  useMetadataCandidatesSeries,
  useMetadataCompositeDiffIssue,
  useMetadataCompositeDiffSeries,
  useMetadataProposedDiffIssue,
  useMetadataProposedDiffSeries,
} from "@/lib/api/queries";
import type {
  ApplyMode,
  CandidatesResp,
  SearchStartedResp,
} from "@/lib/api/types";

import {
  MetadataCompareView,
  defaultFieldSources,
} from "@/components/library/MetadataCompareView";
import {
  MetadataPreviewPane,
  defaultSelectedFields,
} from "@/components/library/MetadataPreviewPane";
import {
  CandidateRow,
  MatchOutcomeBanner,
} from "@/components/library/MetadataMatchCandidates";
import { useMetadataApplyWait } from "@/components/library/useMetadataApplyWait";

export type MetadataMatchScope =
  | { kind: "series"; seriesSlug: string; libraryId: string }
  | { kind: "issue"; seriesSlug: string; issueSlug: string; libraryId: string };

export function MetadataMatchDialog({
  open,
  onOpenChange,
  scope,
  onApplied,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  scope: MetadataMatchScope;
  /** Fired once an apply lands (rescan completed / timeout fallback),
   *  after re-hydration. When provided, it replaces the default
   *  close-on-apply — the worklist controller uses it to advance to the
   *  next series instead of dismissing. Omit for the standalone dialog,
   *  which just closes. */
  onApplied?: () => void;
}) {
  // The compare view renders a wide per-candidate table; widen the
  // dialog while it's active so columns aren't crushed. The inner form
  // remounts on each open and reports its compare state on mount, so
  // this resets to narrow on reopen without an extra effect.
  const [wide, setWide] = React.useState(false);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className={`flex max-h-[90vh] flex-col ${
          wide ? "sm:max-w-5xl" : "sm:max-w-2xl"
        }`}
      >
        <MetadataMatchForm
          scope={scope}
          onClose={() => onOpenChange(false)}
          onApplied={onApplied}
          open={open}
          onCompareModeChange={setWide}
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
  onApplied,
  open,
  onCompareModeChange,
}: {
  scope: MetadataMatchScope;
  onClose: () => void;
  /** Fired after a successful apply re-hydrates. When set, the dialog
   *  defers to it instead of self-closing (worklist auto-advance). */
  onApplied?: () => void;
  /** Used to gate the auto-kickoff effect so the search only fires
   *  once when the dialog opens. */
  open: boolean;
  /** Lets the wrapping dialog widen itself while the compare table is
   *  shown. */
  onCompareModeChange?: (compare: boolean) => void;
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
  const seriesComposite = useApplyCompositeMetadataForSeries(
    scope.kind === "series" ? scope.seriesSlug : "",
  );
  const issueComposite = useApplyCompositeMetadataForIssue(
    scope.kind === "issue" ? scope.seriesSlug : "",
    scope.kind === "issue" ? scope.issueSlug : "",
  );
  const compositeApply =
    scope.kind === "series" ? seriesComposite : issueComposite;
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
  const router = useRouter();
  // Re-hydrate the issue/series page after a metadata apply lands. The
  // apply path is async (sidecar rewrite → scoped rescan → DB cache), so
  // this runs once the rescan completes: `router.refresh()` re-runs the
  // RSC (details / credits / external IDs / cover index come from the
  // server `IssueDetailView`), and the query invalidations refetch the
  // client-side tabs (the Covers gallery, issue health, etc.).
  const rehydrate = React.useCallback(() => {
    router.refresh();
    qc.invalidateQueries({ queryKey: ["issues"] });
    qc.invalidateQueries({ queryKey: ["series"] });
  }, [router, qc]);
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
  const candidates =
    scope.kind === "series" ? seriesCandidates : issueCandidates;
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

  // True when the dialog adopted a prior completed run instead of
  // firing a fresh provider search (quota saver).
  const [reused, setReused] = React.useState(false);

  // ── Composite (multi-provider) compare mode ──────────────────────
  const [compareMode, setCompareMode] = React.useState(false);
  React.useEffect(() => {
    onCompareModeChange?.(compareMode);
  }, [compareMode, onCompareModeChange]);
  // Candidate ordinals the user picked to feed the comparison (columns).
  const [selectedOrdinals, setSelectedOrdinals] = React.useState<Set<number>>(
    new Set(),
  );
  // field key → chosen candidate ordinal (absent = keep mine).
  const [fieldSources, setFieldSources] = React.useState<
    Record<string, number>
  >({});
  const compareRunId = compareMode ? runId : null;
  const includeList = React.useMemo(
    () => Array.from(selectedOrdinals).sort((a, b) => a - b),
    [selectedOrdinals],
  );
  const seriesCompositeDiff = useMetadataCompositeDiffSeries(
    scope.kind === "series" ? scope.seriesSlug : "",
    scope.kind === "series" ? compareRunId : null,
    mode,
    overrideUserEdits,
    includeList,
  );
  const issueCompositeDiff = useMetadataCompositeDiffIssue(
    scope.kind === "issue" ? scope.seriesSlug : "",
    scope.kind === "issue" ? scope.issueSlug : "",
    scope.kind === "issue" ? compareRunId : null,
    mode,
    overrideUserEdits,
    includeList,
  );
  const compositeDiff =
    scope.kind === "series" ? seriesCompositeDiff : issueCompositeDiff;
  // Seed the per-field candidate picks from the server's merge-policy
  // defaults whenever the composite diff resolves for a new
  // (mode/override/include) combination. Tracked by a ref so user
  // toggles aren't re-stomped on re-render.
  const lastSeededComposite = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (!compareMode || !compositeDiff.data) return;
    const key = `${mode}:${overrideUserEdits}:${includeList.join(",")}`;
    if (lastSeededComposite.current === key) return;
    setFieldSources(defaultFieldSources(compositeDiff.data));
    lastSeededComposite.current = key;
  }, [compareMode, compositeDiff.data, mode, overrideUserEdits, includeList]);

  // Seed the compare-column selection from the best (first-ranked)
  // candidate per provider the first time a finalized candidate list
  // arrives. Tracked by run id so a fresh search re-seeds.
  const lastSeededSelection = React.useRef<string | null>(null);
  React.useEffect(() => {
    const list = candidates.data?.candidates;
    if (!runId || !list || list.length === 0) return;
    if (lastSeededSelection.current === runId) return;
    const seen = new Set<string>();
    const picks = new Set<number>();
    list.forEach((c, i) => {
      if (!seen.has(c.source)) {
        seen.add(c.source);
        picks.add(i);
      }
    });
    setSelectedOrdinals(picks);
    lastSeededSelection.current = runId;
  }, [runId, candidates.data]);

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

  const candidatesProbePath = React.useMemo(
    () =>
      scope.kind === "series"
        ? `/series/${encodeURIComponent(scope.seriesSlug)}/metadata/candidates`
        : `/series/${encodeURIComponent(scope.seriesSlug)}/issues/${encodeURIComponent(scope.issueSlug)}/metadata/candidates`,
    [scope],
  );

  const runSearch = React.useCallback(() => {
    setSearchPending(true);
    setSearchError(null);
    setReused(false);
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

  // Quota saver: on open, probe the latest completed run for this scope
  // (a cheap GET, no provider quota). If one exists with candidates,
  // adopt it instead of firing a fresh fan-out. The explicit "Re-search"
  // button always forces a fresh search via `runSearch`.
  const kickOrReuse = React.useCallback(() => {
    setSearchPending(true);
    setSearchError(null);
    setReused(false);
    let cancelled = false;
    void (async () => {
      try {
        const latest = await jsonFetch<CandidatesResp>(
          candidatesProbePath,
        ).catch(() => null);
        if (cancelled) return;
        if (
          latest &&
          latest.status === "completed" &&
          (latest.candidates?.length ?? 0) > 0 &&
          latest.run_id
        ) {
          setRunId(latest.run_id);
          setReused(true);
          setSearchPending(false);
          return;
        }
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
  }, [searchPath, candidatesProbePath, candidatesInvalidateKey, qc]);

  const kickedRef = React.useRef(false);
  React.useEffect(() => {
    if (!open) {
      kickedRef.current = false;
      return;
    }
    if (kickedRef.current) return;
    kickedRef.current = true;
    kickOrReuse();
    // kickOrReuse is stable per (searchPath, probePath, key) — those
    // don't change mid-dialog.
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

  const onChangeFieldSource = (field: string, ordinal: number | null) => {
    setFieldSources((prev) => {
      const next = { ...prev };
      if (ordinal == null) delete next[field];
      else next[field] = ordinal;
      return next;
    });
  };
  const onToggleCompareSelect = (ordinal: number) => {
    setSelectedOrdinals((prev) => {
      const next = new Set(prev);
      if (next.has(ordinal)) next.delete(ordinal);
      else next.add(ordinal);
      return next;
    });
  };
  const onRemoveColumn = (ordinal: number) => {
    setSelectedOrdinals((prev) => {
      const next = new Set(prev);
      next.delete(ordinal);
      return next;
    });
  };
  const onConfirmCompositeApply = () => {
    if (!runId || !compositeDiff.data) return;
    const included = includeList;
    const field_sources = Object.entries(fieldSources).map(
      ([field, ordinal]) => ({ field, ordinal }),
    );
    compositeApply.mutate({
      run_id: runId,
      field_sources,
      included,
      mode,
      apply_cover: applyCover,
      override_user_edits: overrideUserEdits,
      override_external_id_sources: [],
    });
  };

  // Either apply path (single-candidate or composite) drives the same
  // post-apply waiting / close machinery below.
  const applyDidSucceed = apply.isSuccess || compositeApply.isSuccess;
  const applyIsPending = apply.isPending || compositeApply.isPending;

  // Post-apply waiting (202 → rescan/metadata.applied → rehydrate → resolve)
  // lives in its own hook; see useMetadataApplyWait for the full rationale.
  const { waitingForRescan, seriesProgress } = useMetadataApplyWait({
    applyDidSucceed,
    libraryId: scope.libraryId,
    isSeriesScope: scope.kind === "series",
    rehydrate,
    onApplied,
    onClose,
  });

  const restart = () => {
    setRunId(null);
    setPickedOrdinal(null);
    setPreviewOrdinal(null);
    setCompareMode(false);
    setSelectedOrdinals(new Set());
    setFieldSources({});
    lastSeededComposite.current = null;
    lastSeededSelection.current = null;
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
              Rescanning series ({seriesProgress.done} of {seriesProgress.total}
              )
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
      ) : compareMode ? (
        <MetadataCompareView
          data={compositeDiff.data}
          isLoading={compositeDiff.isLoading || compositeDiff.isFetching}
          errorMessage={
            compositeDiff.error
              ? compositeDiff.error instanceof Error
                ? compositeDiff.error.message
                : "Failed to compare candidates."
              : null
          }
          fieldSources={fieldSources}
          onRemoveColumn={onRemoveColumn}
          onChangeFieldSource={onChangeFieldSource}
          onBack={() => setCompareMode(false)}
          onApply={onConfirmCompositeApply}
          isApplying={compositeApply.isPending}
        />
      ) : (
        <div className="max-h-[50vh] overflow-y-auto pr-1">
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
              {reused && isFinalized && (
                <div className="text-muted-foreground pb-1 text-xs">
                  Showing your last search — providers weren&apos;t re-queried.
                  Use Re-search for fresh results.
                </div>
              )}
              {(candidates.data?.candidates.length ?? 0) >= 2 && (
                <div className="text-muted-foreground flex items-center justify-end gap-2 pb-1 text-xs">
                  <span>{selectedOrdinals.size} selected to compare</span>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={selectedOrdinals.size < 2}
                    onClick={() => setCompareMode(true)}
                  >
                    Compare ({selectedOrdinals.size})
                  </Button>
                </div>
              )}
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
                    selectable={(candidates.data?.candidates.length ?? 0) >= 2}
                    selected={selectedOrdinals.has(i)}
                    onToggleSelect={() => onToggleCompareSelect(i)}
                  />
                ))}
                {isFinalized &&
                  (candidates.data?.candidates.length ?? 0) === 0 && (
                    <li className="text-muted-foreground py-8 text-center text-sm">
                      No matches. Try editing the series name or year.
                    </li>
                  )}
              </ul>
            </>
          )}
        </div>
      )}

      <DialogFooter className="flex items-center justify-between gap-2 sm:justify-between">
        <Button
          variant="ghost"
          size="sm"
          onClick={restart}
          disabled={isPolling || applyIsPending}
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
