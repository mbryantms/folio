"use client";

import { Loader2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import type { CandidateView, MatchOutcomeView } from "@/lib/api/types";

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
export function MatchOutcomeBanner({
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
    <span className="text-muted-foreground ml-2 text-[10px] tracking-wide uppercase">
      via alternate cover
    </span>
  ) : null;

  if (outcome.kind === "single_good") {
    return (
      <div className="border-success/30 bg-success/5 my-2 rounded-md border p-3">
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
          <Button size="sm" onClick={onOneClickApply} disabled={isApplying}>
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
          Pick the right candidate from the list below — every option is a
          plausible match.
        </p>
      </div>
    );
  }

  if (outcome.kind === "single_bad_cover") {
    return (
      <div className="border-warning/30 bg-warning/5 my-2 rounded-md border p-3">
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
        Review the candidates below or re-search with different facts (try
        editing the series name or year).
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

export function CandidateRow({
  c,
  ordinal,
  disabled,
  isApplying,
  onApply,
  selectable,
  selected,
  onToggleSelect,
}: {
  c: CandidateView;
  ordinal: number;
  disabled: boolean;
  isApplying: boolean;
  onApply: () => void;
  selectable: boolean;
  selected: boolean;
  onToggleSelect: () => void;
}) {
  const parsed = parseCandidatePayload(c.candidate);
  const _ = ordinal;
  return (
    <li className="bg-muted/40 hover:bg-muted/60 flex gap-3 rounded-md p-3 transition-colors">
      {selectable && (
        <Checkbox
          checked={selected}
          onCheckedChange={onToggleSelect}
          aria-label="Include in comparison"
          className="mt-1 flex-none"
        />
      )}
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
