"use client";

/**
 * Guided "Refresh this series…" (coverage tie-ins PR 3).
 *
 * One stepper over the three things you'd otherwise have to know to run
 * in order, plus the review:
 *
 *   1. **Series match** — the embedded "Match this series…" form (coverage
 *      hints included), or *Keep current match* when the series already
 *      has provider ids.
 *   2. **Coverage** — the analysis a series apply queues on its own
 *      (seeded with the match), or *Analyze coverage*; accept or skip per
 *      provider.
 *   3. **Per-issue fetch** — an *All issues* / *Only missing or partial*
 *      batch, which looks covered issues up directly instead of searching.
 *   4. **Review** — the batch's strong / needs-review counts, direct vs
 *      searched lookups, *Accept all strong* and *Fill missing*, and a link
 *      to the full Review page.
 *
 * Every step reads server state, so closing the dialog mid-flow and
 * reopening it resumes where it got to: `GET …/metadata/refresh-status`
 * names the step (latest series apply, coverage job, batch — all kept for
 * 24 h) and the steps below poll the existing endpoints (coverage
 * analysis, batch status) every 2 s while something runs. Nothing writes
 * without a click; the individual menu actions stay available.
 */

import {
  AlertCircle,
  Check,
  CheckCircle2,
  Circle,
  CircleDashed,
  Loader2,
  SkipForward,
} from "lucide-react";
import Link from "next/link";
import * as React from "react";
import { toast } from "sonner";
import { useQueryClient } from "@tanstack/react-query";

import { BatchLookupSummary } from "@/components/admin/metadata/BatchLookupSummary";
import { promptHeadline } from "@/components/library/CoverageAfterMatchPrompt";
import { MetadataMatchForm } from "@/components/library/MetadataMatchDialog";
import {
  GroupedList,
  candidateLabel,
} from "@/components/library/ProviderCoverageAnalysis";
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  useAcceptProviderCoverage,
  useAnalyzeProviderCoverage,
  useBatchApply,
  useCreateSeriesBatch,
} from "@/lib/api/mutations";
import {
  queryKeys,
  useMetadataBatch,
  useProviderCoverageAnalysis,
  useSeriesRefreshStatus,
} from "@/lib/api/queries";
import type {
  BatchStatusResp,
  CoverageAnalysisResp,
  CoverageLocalIssue,
  FetchScopeEstimate,
  ProposedRange,
  ProviderCoverageView,
  RefreshStep,
  SeriesBatchScope,
  SeriesRefreshStatusResp,
} from "@/lib/api/types";
import { providerLabel } from "@/lib/metadata/quota";
import { statusToneText } from "@/lib/ui/status-tone";
import { cn } from "@/lib/utils";

export const REFRESH_STEPS: ReadonlyArray<{ id: RefreshStep; label: string }> =
  [
    { id: "match", label: "Series match" },
    { id: "coverage", label: "Coverage" },
    { id: "fetch", label: "Per-issue fetch" },
    { id: "review", label: "Review" },
  ];

const stepIndex = (s: RefreshStep) =>
  REFRESH_STEPS.findIndex((x) => x.id === s);

/** Coverage request bound per analysis (`coverage::request_budget`). */
const COVERAGE_BUDGET = "40 ComicVine, 30 Metron and 30 GCD";

type StepOutcome = "done" | "skipped";

/** `"ComicVine: 170 direct · 3 searched (≈ 173–176 requests)"`. A direct
 *  lookup is one detail request; a search is 1–2 (plus the detail the
 *  apply fetches later). Issue lists come once per provider series. */
export function estimateLine(p: {
  source: string;
  direct: number;
  search: number;
}): string {
  const low = p.direct + p.search;
  const high = p.direct + p.search * 2;
  const cost = low === high ? `${low}` : `${low}–${high}`;
  return `${providerLabel(p.source)}: ${p.direct} direct · ${p.search} searched (≈ ${cost} request${high === 1 ? "" : "s"})`;
}

function rangeLabel(r: ProposedRange): string {
  const bounds = r.low === r.high ? `#${r.low}` : `#${r.low}–${r.high}`;
  const name = r.provider_series_name
    ? `${r.provider_series_name}${r.declared_year != null ? ` (${r.declared_year})` : ""}`
    : `series #${r.provider_series_id}`;
  return `${bounds} → ${name}`;
}

const batchRunning = (b: BatchStatusResp | undefined) =>
  b?.status === "running" || b?.status === "awaiting_quota";

export function SeriesRefreshDialog({
  open,
  onOpenChange,
  seriesSlug,
  seriesName,
  libraryId,
  onCloseAutoFocus,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  seriesSlug: string;
  seriesName: string;
  libraryId: string;
  /** Return focus to the opener (the dialog has no trigger of its own). */
  onCloseAutoFocus?: (e: Event) => void;
}) {
  // The match step's compare view is a wide table; widen while it shows.
  const [wide, setWide] = React.useState(false);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        onCloseAutoFocus={onCloseAutoFocus}
        className={cn(
          "flex flex-col sm:max-h-[90vh]",
          wide ? "sm:max-w-5xl" : "sm:max-w-2xl",
        )}
      >
        <DialogHeader className="pr-8">
          <DialogTitle>Refresh this series</DialogTitle>
          <DialogDescription>
            Match {seriesName}, check which provider series hold its issues,
            fetch every issue, then review — in one place. Each step picks up
            where you left it.
          </DialogDescription>
        </DialogHeader>
        {open && (
          <SeriesRefreshFlow
            seriesSlug={seriesSlug}
            libraryId={libraryId}
            onClose={() => onOpenChange(false)}
            onWideChange={setWide}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

/**
 * The stepper itself — split from the dialog shell so vitest can render it
 * without Radix portals (same split as `MetadataMatchForm`). Remounts on
 * every open, so a reopen starts from the server's `resume_step`.
 */
export function SeriesRefreshFlow({
  seriesSlug,
  libraryId,
  onClose,
  onWideChange,
}: {
  seriesSlug: string;
  libraryId: string;
  onClose: () => void;
  onWideChange?: (wide: boolean) => void;
}) {
  const qc = useQueryClient();
  // Polls only while the coverage step waits for the job a series apply
  // queues server-side (it appears a moment after the apply lands).
  const [waitForCoverageJob, setWaitForCoverageJob] = React.useState(false);
  const status = useSeriesRefreshStatus(seriesSlug, {
    poll: waitForCoverageJob,
  });
  const data = status.data;

  // The step the flow resumed at is pinned on first load: later status
  // refetches (polling, or the page re-hydrating after an apply) must not
  // move the user, only their own clicks do.
  const [resumeStep, setResumeStep] = React.useState<RefreshStep | null>(null);
  const [step, setStep] = React.useState<RefreshStep | null>(null);
  if (resumeStep === null && data) {
    setResumeStep(data.resume_step);
    setStep(data.resume_step);
  }
  const [outcomes, setOutcomes] = React.useState<
    Partial<Record<RefreshStep, StepOutcome>>
  >({});
  const [matchedHere, setMatchedHere] = React.useState(false);
  // `undefined` = not decided yet (follow the server's batch on resume),
  // `null` = no batch for this run of the flow, else the batch id.
  const [batchChoice, setBatchChoice] = React.useState<
    string | null | undefined
  >(undefined);
  // Auto-advance fetch → review when the batch finishes, unless the user
  // navigated back to the fetch step on purpose.
  const [autoAdvance, setAutoAdvance] = React.useState(true);

  const batchId =
    batchChoice !== undefined
      ? batchChoice
      : resumeStep === "fetch" || resumeStep === "review"
        ? (data?.batch?.batch_id ?? null)
        : null;
  const batch = useMetadataBatch(batchId);
  const batchDone = !!batch.data && !batchRunning(batch.data);

  const current: RefreshStep | null =
    step === "fetch" && batchDone && autoAdvance ? "review" : step;

  // Furthest step reached: steps up to it are clickable in the stepper.
  const [maxIdx, setMaxIdx] = React.useState(0);
  const currentIdx = current ? stepIndex(current) : 0;
  const reachedIdx = Math.max(maxIdx, currentIdx);

  const go = (next: RefreshStep, mark?: StepOutcome) => {
    if (current && mark) setOutcomes((o) => ({ ...o, [current]: mark }));
    setMaxIdx((m) => Math.max(m, currentIdx, stepIndex(next)));
    setStep(next);
  };

  const outcomeOf = (id: RefreshStep): StepOutcome | "current" | "pending" => {
    if (id === current) return "current";
    if (outcomes[id]) return outcomes[id]!;
    return stepIndex(id) < reachedIdx ? "done" : "pending";
  };

  // Focus management: each step change moves focus to the new step's
  // heading so keyboard / screen-reader users land on the new content.
  const headingRef = React.useRef<HTMLHeadingElement>(null);
  const lastFocused = React.useRef<RefreshStep | null>(null);
  React.useEffect(() => {
    if (!current) return;
    if (lastFocused.current !== null && lastFocused.current !== current) {
      headingRef.current?.focus();
    }
    lastFocused.current = current;
  }, [current]);

  const refetchStatus = () =>
    qc.invalidateQueries({
      queryKey: queryKeys.seriesRefreshStatus(seriesSlug),
    });

  if (status.isLoading || !data || !current) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-8 text-sm">
        {status.isError ? (
          <>
            <AlertCircle className="h-4 w-4" /> Couldn&rsquo;t load this
            series&rsquo; refresh state.{" "}
            <button
              type="button"
              className="focus-visible:ring-ring rounded-sm underline focus-visible:ring-2 focus-visible:outline-none"
              onClick={() => void status.refetch()}
            >
              Retry
            </button>
          </>
        ) : (
          <>
            <Loader2 className="h-4 w-4 animate-spin" /> Loading…
          </>
        )}
      </div>
    );
  }

  const stepLabel = REFRESH_STEPS[currentIdx]?.label ?? "";

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-4">
      <ol
        aria-label="Refresh steps"
        className="grid grid-cols-2 gap-1.5 sm:grid-cols-4"
      >
        {REFRESH_STEPS.map((s, i) => {
          const o = outcomeOf(s.id);
          const reachable = i <= reachedIdx && o !== "current";
          const Icon =
            o === "done"
              ? CheckCircle2
              : o === "skipped"
                ? SkipForward
                : o === "current"
                  ? Circle
                  : CircleDashed;
          const stateText =
            o === "done"
              ? "done"
              : o === "skipped"
                ? "skipped"
                : o === "current"
                  ? "current step"
                  : "not started";
          return (
            <li key={s.id}>
              <button
                type="button"
                disabled={!reachable}
                aria-current={o === "current" ? "step" : undefined}
                onClick={() => {
                  if (s.id === "fetch") setAutoAdvance(false);
                  go(s.id);
                }}
                className={cn(
                  "border-border/60 flex w-full items-center gap-1.5 rounded-md border px-2 py-1.5 text-left text-xs transition-colors",
                  "focus-visible:ring-ring focus-visible:ring-2 focus-visible:outline-none",
                  o === "current"
                    ? "bg-muted text-foreground font-medium"
                    : "text-muted-foreground",
                  reachable && "hover:bg-muted/50 hover:text-foreground",
                  "disabled:cursor-default",
                )}
              >
                <Icon
                  aria-hidden
                  className={cn(
                    "h-3.5 w-3.5 shrink-0",
                    o === "done" && statusToneText("success"),
                  )}
                />
                <span className="min-w-0 truncate">
                  {i + 1}. {s.label}
                </span>
                <span className="sr-only"> ({stateText})</span>
              </button>
            </li>
          );
        })}
      </ol>

      {/* The one scroll region (≥ sm; the phone sheet scrolls as a whole).
          Padding keeps focus rings inside it from being clipped. */}
      <section
        aria-labelledby="series-refresh-step-heading"
        className="-mx-1 min-h-0 flex-1 space-y-3 px-1 pb-1 sm:overflow-y-auto"
      >
        <h3
          id="series-refresh-step-heading"
          ref={headingRef}
          tabIndex={-1}
          className="focus-visible:ring-ring rounded-sm text-sm font-semibold focus-visible:ring-2 focus-visible:outline-none"
        >
          Step {currentIdx + 1} of {REFRESH_STEPS.length}: {stepLabel}
        </h3>

        {current === "match" && (
          <MatchStep
            seriesSlug={seriesSlug}
            libraryId={libraryId}
            status={data}
            onWideChange={onWideChange}
            onKeep={() => go("coverage", "skipped")}
            onApplied={() => {
              setMatchedHere(true);
              setWaitForCoverageJob(true);
              void refetchStatus();
              onWideChange?.(false);
              go("coverage", "done");
            }}
          />
        )}
        {current === "coverage" && (
          <CoverageStep
            seriesSlug={seriesSlug}
            status={data}
            matchedHere={matchedHere}
            onWaitingChange={setWaitForCoverageJob}
            onSkip={() => go("fetch", "skipped")}
            onContinue={() => go("fetch", "done")}
          />
        )}
        {current === "fetch" && (
          <FetchStep
            seriesSlug={seriesSlug}
            status={data}
            batchId={batchId}
            batch={batch.data}
            onStarted={(id) => {
              setBatchChoice(id);
              setAutoAdvance(true);
              void refetchStatus();
            }}
            onStartOver={() => setBatchChoice(null)}
            onReview={() => go("review", "done")}
          />
        )}
        {current === "review" && batchId && (
          <ReviewStep batchId={batchId} batch={batch.data} onDone={onClose} />
        )}
        {current === "review" && !batchId && (
          <p className="text-muted-foreground text-sm">
            No per-issue fetch ran yet.{" "}
            <Button
              variant="link"
              className="h-auto p-0"
              onClick={() => go("fetch")}
            >
              Start one
            </Button>
          </p>
        )}
      </section>
    </div>
  );
}

// ───────── 1. series match ─────────

function MatchStep({
  seriesSlug,
  libraryId,
  status,
  onWideChange,
  onKeep,
  onApplied,
}: {
  seriesSlug: string;
  libraryId: string;
  status: SeriesRefreshStatusResp;
  onWideChange?: (wide: boolean) => void;
  onKeep: () => void;
  onApplied: () => void;
}) {
  const links = status.series_match.links;
  const [searching, setSearching] = React.useState(links.length === 0);
  const scope = React.useMemo(
    () => ({ kind: "series" as const, seriesSlug, libraryId }),
    [seriesSlug, libraryId],
  );

  if (!searching) {
    return (
      <div className="space-y-3 text-sm">
        <p>This series is already matched:</p>
        <ul className="border-border/60 divide-border/60 divide-y rounded-md border">
          {links.map((l) => (
            <li
              key={`${l.source}-${l.external_id}`}
              className="flex flex-wrap items-center justify-between gap-2 px-3 py-2"
            >
              <span>
                {providerLabel(l.source)}{" "}
                <code className="text-muted-foreground text-xs">
                  #{l.external_id}
                </code>
              </span>
            </li>
          ))}
        </ul>
        <p className="text-muted-foreground text-xs">
          Keep it to go straight to coverage, or search again to pick a
          different series (asks each enabled provider once; a recent search
          with results is reused).
        </p>
        <div className="flex flex-wrap gap-2">
          <Button onClick={onKeep}>Keep current match</Button>
          <Button variant="outline" onClick={() => setSearching(true)}>
            Search for a match
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-2">
      {links.length > 0 && (
        <Button
          variant="link"
          size="sm"
          className="h-auto p-0 text-xs"
          onClick={() => {
            onWideChange?.(false);
            setSearching(false);
          }}
        >
          ← Keep the current match instead
        </Button>
      )}
      <MetadataMatchForm
        embedded
        open
        scope={scope}
        onClose={() => {}}
        onApplied={onApplied}
        onCompareModeChange={onWideChange}
      />
    </div>
  );
}

// ───────── 2. coverage ─────────

function CoverageStep({
  seriesSlug,
  status,
  matchedHere,
  onWaitingChange,
  onSkip,
  onContinue,
}: {
  seriesSlug: string;
  status: SeriesRefreshStatusResp;
  /** A series match was applied in this run of the flow. */
  matchedHere: boolean;
  onWaitingChange: (waiting: boolean) => void;
  onSkip: () => void;
  onContinue: () => void;
}) {
  const analysis = useProviderCoverageAnalysis(seriesSlug);
  const analyze = useAnalyzeProviderCoverage(seriesSlug);
  const accept = useAcceptProviderCoverage(seriesSlug);
  const [accepting, setAccepting] = React.useState<string | null>(null);
  const [skipped, setSkipped] = React.useState<Set<string>>(new Set());

  const job = status.coverage;
  const appliedAt = status.series_match.applied_at;
  // A match applied here queues its own (seeded) analysis unless the
  // setting is off; until that job shows up — newer than the apply — wait
  // for it rather than starting a second, unseeded one.
  const jobAfterMatch =
    !!job &&
    (!appliedAt || Date.parse(job.requested_at) >= Date.parse(appliedAt));
  const waiting =
    matchedHere &&
    status.coverage_after_series_apply !== "off" &&
    !jobAfterMatch &&
    !analyze.isPending &&
    !analyze.isSuccess;
  React.useEffect(() => {
    onWaitingChange(waiting);
    return () => onWaitingChange(false);
  }, [waiting, onWaitingChange]);

  const view: CoverageAnalysisResp | null | undefined = analysis.data;
  // The status names a newer job than the cached analysis (the seeded one
  // just appeared): fetch it now instead of waiting out the stale time.
  const staleView =
    !!job &&
    analysis.isFetched &&
    (!view || Date.parse(job.requested_at) > Date.parse(view.requested_at));
  const { refetch } = analysis;
  React.useEffect(() => {
    if (staleView) void refetch();
  }, [staleView, refetch]);
  const running =
    analyze.isPending ||
    staleView ||
    view?.state === "queued" ||
    view?.state === "running";

  const runAnalysis = () => analyze.mutate({ auto_accept: false });
  // `mainSeriesId` is the admin's pick from "Choose series"; `null` follows
  // the proposal's main — the same contract as the Details tab card.
  const onAccept = (source: string, mainSeriesId: string | null) => {
    setAccepting(source);
    accept.mutate(
      { source, main_series_id: mainSeriesId },
      { onSettled: () => setAccepting(null) },
    );
  };

  const budgetNote = (
    <p className="text-muted-foreground text-xs">
      Spends at most {COVERAGE_BUDGET} requests; issue lists are cached for 24
      hours, so a repeat usually costs one search per provider. A check after a
      match only lists the matched series (about 1–3 requests).
    </p>
  );

  if (waiting) {
    return (
      <div className="space-y-3 text-sm">
        <p className="text-muted-foreground flex items-center gap-2">
          <Loader2 className="h-4 w-4 animate-spin" />
          Waiting for the coverage check your match queued…
        </p>
        {budgetNote}
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" onClick={runAnalysis}>
            Run it now
          </Button>
          <Button variant="ghost" onClick={onSkip}>
            Skip coverage
          </Button>
        </div>
      </div>
    );
  }

  if (running) {
    return (
      <div className="space-y-3 text-sm">
        <p className="text-muted-foreground flex items-center gap-2">
          <Loader2 className="h-4 w-4 animate-spin" />
          {(view?.trigger ?? job?.trigger ?? "analyze") !== "analyze"
            ? "Checking which issues the series you matched holds, and where the rest live…"
            : "Listing candidate series on ComicVine, Metron and GCD. This can take a minute (provider rate limits)."}
        </p>
        {budgetNote}
        <Button variant="ghost" onClick={onSkip}>
          Skip coverage
        </Button>
      </div>
    );
  }

  if (!view) {
    return (
      <div className="space-y-3 text-sm">
        <p>
          Coverage works out which ComicVine, Metron and GCD series hold which
          of this series&rsquo; issues (by number and cover date), so the
          per-issue fetch can look issues up directly instead of searching.
        </p>
        {budgetNote}
        <div className="flex flex-wrap gap-2">
          <Button onClick={runAnalysis} disabled={analyze.isPending}>
            {analyze.isPending && (
              <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
            )}
            Analyze coverage
          </Button>
          <Button variant="ghost" onClick={onSkip}>
            Skip coverage
          </Button>
        </div>
      </div>
    );
  }

  if (view.state === "failed") {
    return (
      <div className="space-y-3 text-sm">
        <p className="text-destructive">
          The coverage analysis failed: {view.error ?? "unknown error"}
        </p>
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" onClick={runAnalysis}>
            Run again
          </Button>
          <Button variant="ghost" onClick={onSkip}>
            Skip coverage
          </Button>
        </div>
      </div>
    );
  }

  const localTotal = view.local_issues.length;
  const autoSources = new Set(view.auto_accepted.map((o) => o.source));
  return (
    <div className="space-y-3 text-sm">
      <p className="text-muted-foreground text-xs">
        {view.finished_at
          ? `Analysed ${new Date(view.finished_at).toLocaleString()}. `
          : ""}
        Accept a provider to save its series and range mappings, or skip it.
        Nothing is written until you accept.
      </p>
      <ul className="space-y-2" aria-label="Coverage by provider">
        {view.providers.map((p) => (
          <CoverageProviderRow
            key={p.source}
            p={p}
            local={view.local_issues}
            localTotal={localTotal}
            auto={autoSources.has(p.source)}
            skipped={skipped.has(p.source)}
            accepting={accepting}
            onAccept={onAccept}
            onSkip={() => setSkipped((prev) => new Set(prev).add(p.source))}
          />
        ))}
      </ul>
      <div className="flex flex-wrap gap-2">
        <Button onClick={onContinue}>Continue to per-issue fetch</Button>
        <Button
          variant="outline"
          onClick={runAnalysis}
          disabled={analyze.isPending}
        >
          Analyze again
        </Button>
      </div>
    </div>
  );
}

// ───────── 3. per-issue fetch ─────────

/**
 * One provider in the coverage step: the headline (what would change),
 * then the same facts the Details tab's analysis card shows — which
 * provider series is proposed as the main and which one you're linked
 * to now, how confident the analysis is and why, the series → issues
 * breakdown, every conflict and stale mapping — plus "Choose series"
 * when more than one candidate lists your issues. Enough to accept
 * here without opening the Details tab first.
 */
function CoverageProviderRow({
  p,
  local,
  localTotal,
  auto,
  skipped,
  accepting,
  onAccept,
  onSkip,
}: {
  p: ProviderCoverageView;
  local: CoverageLocalIssue[];
  localTotal: number;
  auto: boolean;
  skipped: boolean;
  accepting: string | null;
  onAccept: (source: string, mainSeriesId: string | null) => void;
  onSkip: () => void;
}) {
  const [picked, setPicked] = React.useState<string | null>(null);
  const chosen = picked ?? p.main_series_id ?? "";
  const chosenOther = !!chosen && chosen !== p.main_series_id;
  const assigned = p.status === "analyzed" || p.status === "partial";
  const newRanges = p.proposed_ranges.filter((r) => r.status === "new");
  const conflictRanges = p.proposed_ranges.filter(
    (r) => r.status === "conflict",
  );
  // A conflict (your own link or range disagrees) still accepts here: the
  // server refuses only when a user-set link differs, and says so.
  const canAccept =
    !auto && assigned && !!p.main_series_id && (p.has_changes || chosenOther);
  const main = p.candidates.find(
    (c) => c.provider_series_id === p.main_series_id,
  );
  const current = p.current_series_id
    ? p.candidates.find((c) => c.provider_series_id === p.current_series_id)
    : undefined;
  const busy = accepting !== null;

  return (
    <li
      className="border-border/60 space-y-2 rounded-md border p-3"
      data-testid={`refresh-coverage-${p.source}`}
    >
      <div className="flex flex-wrap items-start justify-between gap-2">
        <p className="min-w-0 flex-1 break-words">
          {promptHeadline(p, localTotal, auto)}
        </p>
        {!(canAccept && !skipped) && (
          <Badge variant="outline" className="shrink-0 font-normal">
            {auto
              ? "Accepted"
              : skipped
                ? "Skipped"
                : p.has_changes
                  ? "Needs review"
                  : "Done"}
          </Badge>
        )}
      </div>

      {assigned && p.main_series_id && (
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-xs">
          <dt className="text-muted-foreground">Proposed main</dt>
          <dd className="min-w-0 break-words">
            {main?.url ? (
              <a
                href={main.url}
                target="_blank"
                rel="noreferrer"
                className="hover:underline"
              >
                {candidateLabel(main, p.main_series_id)}
              </a>
            ) : (
              candidateLabel(main, p.main_series_id)
            )}
            {main && (
              <span className="text-muted-foreground">
                {" "}
                · lists {main.listed_count} issues, {main.local_matches} of
                yours
              </span>
            )}
          </dd>
          <dt className="text-muted-foreground">Linked now</dt>
          <dd className="min-w-0 break-words">
            {p.current_series_id
              ? p.current_series_id === p.main_series_id
                ? "the same series"
                : candidateLabel(current, p.current_series_id)
              : "nothing yet"}
          </dd>
        </dl>
      )}

      {assigned && p.confidence_reasons.length > 0 && (
        <p className="text-muted-foreground text-xs">
          {p.confidence_reasons.join(" · ")}
        </p>
      )}
      {assigned && <GroupedList p={p} local={local} />}
      {newRanges.length > 0 && !auto && (
        <ul className="text-muted-foreground text-xs">
          {newRanges.map((r) => (
            <li
              key={`${r.provider_series_id}-${r.low}`}
              className="break-words"
            >
              {rangeLabel(r)}
            </li>
          ))}
        </ul>
      )}
      {p.uncovered.length > 0 && (
        <p className="text-xs">
          <span className="text-muted-foreground">
            No {p.source_label} series has{" "}
          </span>
          {p.uncovered.map((n) => `#${n}`).join(", ")}
        </p>
      )}
      {!auto && (p.conflicts.length > 0 || conflictRanges.length > 0) && (
        <ul className={cn("space-y-0.5 text-xs", statusToneText("warning"))}>
          {p.conflicts.map((c) => (
            <li key={c} className="break-words">
              {c}
            </li>
          ))}
          {conflictRanges
            .filter((r) => r.note)
            .map((r) => (
              <li key={`${r.low}-${r.high}`} className="break-words">
                #{r.low}–{r.high} not mapped: {r.note}
              </li>
            ))}
        </ul>
      )}
      {!auto && p.stale_ranges.length > 0 && (
        <ul className="text-muted-foreground space-y-0.5 text-xs">
          {p.stale_ranges.map((r) => (
            <li key={r.id} className="break-words">
              Stale mapping{" "}
              {r.range_low === r.range_high
                ? `#${r.range_low}`
                : `#${r.range_low ?? ""}–${r.range_high ?? ""}`}{" "}
              → {r.provider_series_name ?? `#${r.provider_series_id}`} — removed
              on accept ({r.reason})
            </li>
          ))}
        </ul>
      )}
      <p className="text-muted-foreground text-xs tabular-nums">
        {p.requests} of {p.request_budget} requests · {p.confidence} confidence
      </p>

      {canAccept && !skipped && (
        <div className="flex flex-wrap items-center gap-2">
          {p.candidates.length > 1 && (
            <Select value={chosen} onValueChange={setPicked}>
              <SelectTrigger
                className="h-8 w-full text-xs sm:w-64"
                aria-label={`${p.source_label} main series`}
              >
                <SelectValue placeholder="Main series" />
              </SelectTrigger>
              <SelectContent>
                {p.candidates.map((c) => (
                  <SelectItem
                    key={c.provider_series_id}
                    value={c.provider_series_id}
                  >
                    {candidateLabel(c, c.provider_series_id)} ·{" "}
                    {c.local_matches} of yours
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
          <Button
            size="sm"
            onClick={() => onAccept(p.source, chosenOther ? chosen : null)}
            disabled={busy}
            aria-label={`Accept ${p.source_label}`}
          >
            {accepting === p.source ? (
              <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
            ) : (
              <Check className="mr-1 h-3.5 w-3.5" />
            )}
            Accept
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={onSkip}
            aria-label={`Skip ${p.source_label}`}
          >
            Skip
          </Button>
        </div>
      )}
    </li>
  );
}

function FetchStep({
  seriesSlug,
  status,
  batchId,
  batch,
  onStarted,
  onStartOver,
  onReview,
}: {
  seriesSlug: string;
  status: SeriesRefreshStatusResp;
  batchId: string | null;
  batch: BatchStatusResp | undefined;
  onStarted: (batchId: string) => void;
  onStartOver: () => void;
  onReview: () => void;
}) {
  const createBatch = useCreateSeriesBatch(seriesSlug);
  const [scope, setScope] = React.useState<SeriesBatchScope>("incomplete");
  const estimates = status.fetch_estimate;
  const est = (s: SeriesBatchScope): FetchScopeEstimate | undefined =>
    estimates.find((e) => e.scope === s);
  const chosen = est(scope);

  if (batchId) {
    const searched = batch?.aggregate.searched ?? 0;
    const total = batch?.items_total ?? 0;
    const pct = total > 0 ? Math.min((searched / total) * 100, 100) : 0;
    const running = batchRunning(batch);
    return (
      <div className="space-y-3 text-sm">
        {!batch ? (
          <p className="text-muted-foreground flex items-center gap-2">
            <Loader2 className="h-4 w-4 animate-spin" /> Loading the batch…
          </p>
        ) : (
          <>
            <div className="flex items-center justify-between gap-2">
              <span className="flex items-center gap-2">
                {running && <Loader2 className="h-4 w-4 animate-spin" />}
                {running
                  ? batch.status === "awaiting_quota"
                    ? "Waiting for provider quota — the rest resumes on its own."
                    : "Searching issues…"
                  : "Finished."}
              </span>
              <span className="text-muted-foreground text-xs tabular-nums">
                {searched} / {total} searched
              </span>
            </div>
            <Progress value={pct} aria-label="Issues searched" />
            <BatchLookupSummary lookups={batch.aggregate.lookups} />
            {batch.aggregate.partial > 0 && (
              <p className={`text-xs ${statusToneText("warning")}`}>
                {batch.aggregate.partial} issue
                {batch.aggregate.partial === 1 ? "" : "s"} matched fewer than
                all providers (a provider failed or was never asked) — flagged
                in Review.
              </p>
            )}
          </>
        )}
        <div className="flex flex-wrap gap-2">
          <Button onClick={onReview} disabled={!batch}>
            {running ? "Review results so far" : "Review results"}
          </Button>
          {!running && (
            <Button variant="outline" onClick={onStartOver}>
              Fetch again
            </Button>
          )}
        </div>
      </div>
    );
  }

  const start = () =>
    createBatch.mutate(
      { scope },
      {
        onSuccess: (resp) => {
          if (!resp) return;
          if (resp.items_total === 0) {
            toast.info(
              scope === "incomplete"
                ? "Every issue already has complete metadata"
                : "No issues to search",
            );
            return;
          }
          onStarted(resp.batch_id);
        },
      },
    );

  return (
    <div className="space-y-3 text-sm">
      <RadioGroup
        value={scope}
        onValueChange={(v) => setScope(v as SeriesBatchScope)}
        aria-label="Which issues to fetch"
        className="gap-2"
      >
        {(
          [
            ["incomplete", "Only missing or partial"],
            ["all", "All issues"],
          ] as const
        ).map(([value, label]) => (
          <Label
            key={value}
            className="border-border/60 has-[[data-state=checked]]:bg-muted/50 flex cursor-pointer items-center gap-2 rounded-md border px-3 py-2 font-normal"
          >
            <RadioGroupItem value={value} />
            <span>
              {label}{" "}
              <span className="text-muted-foreground tabular-nums">
                ({est(value)?.issues ?? 0} issue
                {(est(value)?.issues ?? 0) === 1 ? "" : "s"})
              </span>
            </span>
          </Label>
        ))}
      </RadioGroup>
      {chosen && chosen.providers.length > 0 && (
        <div className="text-muted-foreground space-y-0.5 text-xs">
          <p>
            Expected provider calls (direct = looked up through the
            series&rsquo; coverage, no search):
          </p>
          <ul aria-label="Expected provider calls" className="space-y-0.5">
            {chosen.providers.map((p) => (
              <li key={p.source} className="tabular-nums">
                {estimateLine(p)}
              </li>
            ))}
          </ul>
          <p>
            Plus each provider series&rsquo; issue list once (cached 24 hours).
            Results wait for your review — nothing is applied automatically.
          </p>
        </div>
      )}
      <Button
        onClick={start}
        disabled={createBatch.isPending || (chosen?.issues ?? 0) === 0}
      >
        {createBatch.isPending && (
          <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
        )}
        Fetch {chosen?.issues ?? 0} issue
        {(chosen?.issues ?? 0) === 1 ? "" : "s"}
      </Button>
      {(chosen?.issues ?? 0) === 0 && scope === "incomplete" && (
        <p className="text-muted-foreground text-xs">
          Every issue already has complete metadata.
        </p>
      )}
    </div>
  );
}

// ───────── 4. review ─────────

function ReviewStep({
  batchId,
  batch,
  onDone,
}: {
  batchId: string;
  batch: BatchStatusResp | undefined;
  onDone: () => void;
}) {
  const apply = useBatchApply(batchId);
  const [confirmReplace, setConfirmReplace] = React.useState(false);
  if (!batch) {
    return (
      <p className="text-muted-foreground flex items-center gap-2 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading the batch…
      </p>
    );
  }
  const a = batch.aggregate;
  const strong = batch.children.filter(
    (c) => c.outcome_kind === "single_good" && !c.applied,
  ).length;
  const needsReview = batch.children.filter(
    (c) =>
      ["multi_good", "single_bad_cover", "multi_bad_cover"].includes(
        c.outcome_kind ?? "",
      ) && !c.applied,
  ).length;
  const reviewHref = `/admin/metadata?tab=review&batch=${encodeURIComponent(batchId)}`;
  const replaceable = strong + needsReview;
  // "Replace all" covers every unapplied match: strong ones with their
  // single candidate, needs-review ones with the most-complete merge
  // across providers — both in replace_all mode, so existing non-pinned
  // fields and the primary cover are overwritten.
  const replaceAll = async () => {
    setConfirmReplace(false);
    if (needsReview > 0) {
      await apply.mutateAsync({
        filter: "all_needs_review",
        mode: "replace_all",
      });
    }
    if (strong > 0) {
      await apply.mutateAsync({ filter: "all_strong", mode: "replace_all" });
    }
  };
  return (
    <div className="space-y-3 text-sm">
      {batchRunning(batch) && (
        <p className="text-muted-foreground flex items-center gap-2 text-xs">
          <Loader2 className="h-3.5 w-3.5 animate-spin" />
          Still searching ({a.searched} of {batch.items_total}) — the counts
          update as issues finish.
        </p>
      )}
      <dl className="grid grid-cols-2 gap-2 sm:grid-cols-4">
        {(
          [
            ["Strong", a.strong],
            ["Need review", a.needs_review],
            ["No match", a.no_match],
            ["Applied", a.applied],
          ] as const
        ).map(([label, n]) => (
          <div
            key={label}
            className="border-border/60 rounded-md border px-3 py-2"
          >
            <dt className="text-muted-foreground text-xs">{label}</dt>
            <dd className="text-base font-semibold tabular-nums">{n}</dd>
          </div>
        ))}
      </dl>
      <BatchLookupSummary lookups={a.lookups} />
      <div className="flex flex-wrap gap-2">
        <Button
          disabled={strong === 0 || apply.isPending}
          onClick={() => apply.mutate({ filter: "all_strong" })}
        >
          {apply.isPending && (
            <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
          )}
          Accept all strong ({strong})
        </Button>
        <Button
          variant="outline"
          disabled={needsReview === 0 || apply.isPending}
          onClick={() =>
            apply.mutate({
              filter: "all_needs_review",
              mode: "fill_missing",
            })
          }
        >
          Fill missing ({needsReview})
        </Button>
        <Button
          variant="outline"
          disabled={replaceable === 0 || apply.isPending}
          onClick={() => setConfirmReplace(true)}
        >
          Replace all ({replaceable})
        </Button>
      </div>
      <p className="text-muted-foreground text-xs">
        <em>Fill missing</em> applies the most complete merge across providers
        to the needs-review issues without replacing what&rsquo;s there.{" "}
        <em>Replace all</em> overwrites existing non-pinned fields on every
        unapplied issue — strong matches with their match, needs-review issues
        with the merge. Fields you set by hand are always kept. Open the Review
        page to go through issues one by one or see no-match issues.
      </p>
      <AlertDialog open={confirmReplace} onOpenChange={setConfirmReplace}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              Replace metadata for {replaceable} issue
              {replaceable === 1 ? "" : "s"}?
            </AlertDialogTitle>
            <AlertDialogDescription>
              This overwrites existing non-pinned fields and the primary cover:
              strong matches take their single match, needs-review issues take
              the most-complete merge across every provider that matched (the
              richest credits, characters and teams; other fields from the
              preferred provider). Fields you pinned are preserved. This
              can&rsquo;t be undone in bulk.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={() => void replaceAll()}>
              Replace all
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <div className="flex flex-wrap gap-2">
        <Button asChild variant="outline">
          <Link href={reviewHref}>Open in Review</Link>
        </Button>
        <Button variant="ghost" onClick={onDone}>
          Done
        </Button>
      </div>
    </div>
  );
}
