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
import {
  useApplyMetadataForIssue,
  useApplyMetadataForSeries,
  useSearchMetadataForIssue,
  useSearchMetadataForSeries,
} from "@/lib/api/mutations";
import {
  useMe,
  useMetadataCandidatesIssue,
  useMetadataCandidatesSeries,
} from "@/lib/api/queries";
import type { ApplyMode, CandidateView } from "@/lib/api/types";

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
      <DialogContent className="sm:max-w-2xl">
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
  // Hooks must be unconditional. Call both sets — the empty-string
  // `enabled` guard inside each hook short-circuits the inactive
  // half so we don't fire two HTTP calls per render.
  const seriesSearch = useSearchMetadataForSeries(
    scope.kind === "series" ? scope.seriesSlug : "",
  );
  const issueSearch = useSearchMetadataForIssue(
    scope.kind === "issue" ? scope.seriesSlug : "",
    scope.kind === "issue" ? scope.issueSlug : "",
  );
  const seriesApply = useApplyMetadataForSeries(
    scope.kind === "series" ? scope.seriesSlug : "",
  );
  const issueApply = useApplyMetadataForIssue(
    scope.kind === "issue" ? scope.seriesSlug : "",
    scope.kind === "issue" ? scope.issueSlug : "",
  );
  const search = scope.kind === "series" ? seriesSearch : issueSearch;
  const apply = scope.kind === "series" ? seriesApply : issueApply;
  const [runId, setRunId] = React.useState<string | null>(null);
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

  // Auto-kick the search the first time the dialog opens. We track
  // "did we kick this dialog session?" in a ref so a re-render
  // (mode toggle, etc.) doesn't re-fire the search.
  const kickedRef = React.useRef(false);
  React.useEffect(() => {
    if (!open) {
      kickedRef.current = false;
      return;
    }
    if (kickedRef.current) return;
    kickedRef.current = true;
    search.mutate(undefined, {
      onSuccess: (data) => {
        if (data?.run_id) setRunId(data.run_id);
      },
    });
    // search is stable across renders; including it would loop.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const runStatus = candidates.data?.status ?? "queued";
  const isPolling = runStatus === "queued" || runStatus === "searching";
  const isFinalized =
    runStatus === "completed" ||
    runStatus === "failed" ||
    runStatus === "awaiting_quota";

  const onApply = (ordinal: number) => {
    if (!runId) return;
    setPickedOrdinal(ordinal);
    apply.mutate(
      {
        run_id: runId,
        ordinal,
        mode,
        apply_cover: applyCover,
        override_user_edits: overrideUserEdits,
      },
      { onSuccess: () => onClose() },
    );
  };

  const restart = () => {
    setRunId(null);
    setPickedOrdinal(null);
    kickedRef.current = false;
    // Re-trigger the kickoff effect.
    search.mutate(undefined, {
      onSuccess: (data) => {
        if (data?.run_id) setRunId(data.run_id);
      },
    });
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

      <ScrollArea className="max-h-[50vh] pr-3">
        {isPolling ? (
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
                onApply={() => onApply(i)}
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
    <li className="bg-card flex gap-3 rounded-md border p-3">
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
              "Apply"
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
