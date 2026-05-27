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
} from "@/lib/api/mutations";
import {
  useMe,
  useMetadataCandidatesIssue,
  useMetadataCandidatesSeries,
  useMetadataProposedDiffIssue,
  useMetadataProposedDiffSeries,
} from "@/lib/api/queries";
import type {
  ApplyMode,
  CandidateView,
  SearchStartedResp,
} from "@/lib/api/types";

import {
  MetadataPreviewPane,
  defaultSelectedFields,
} from "@/components/library/MetadataPreviewPane";

export type MetadataMatchScope =
  | { kind: "series"; seriesSlug: string }
  | { kind: "issue"; seriesSlug: string; issueSlug: string };

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

  // Close the dialog when the apply mutation resolves successfully.
  React.useEffect(() => {
    if (apply.isSuccess) onClose();
    // onClose is stable enough (parent owns it via useState setter
    // for the open boolean) that adding it to deps would loop only
    // if the parent re-creates the setter unnecessarily.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [apply.isSuccess]);

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
          {isPolling
            ? "Searching providers…"
            : runStatus === "awaiting_quota"
              ? "Providers are out of quota — try again shortly."
              : runStatus === "failed"
                ? "Search failed — see Error below."
                : `${candidates.data?.candidates.length ?? 0} match${
                    (candidates.data?.candidates.length ?? 0) === 1 ? "" : "es"
                  } from ${candidates.data?.providers.join(", ") ?? "providers"}.`}
        </DialogDescription>
      </DialogHeader>

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
