"use client";

import { Cover } from "@/components/Cover";
import { Badge } from "@/components/ui/badge";
import { Progress } from "@/components/ui/progress";
import { formatIssueHeading, formatPublicationStatus } from "@/lib/format";
import type { IssueSummaryView, SeriesView } from "@/lib/api/types";

/**
 * Rich card-hover previews (audit 3.7 discovery). Rendered inside a
 * `HoverCardContent` so they only mount on a desktop hover — a low-cost
 * "peek" at a series/issue without leaving the grid. Touch + keyboard
 * flows never trigger Radix HoverCard, so the card's tap / long-press /
 * cover-menu behavior is unchanged.
 */

/** "2012", "2012–2018", or null. Em-dash for the range. */
function yearRange(start: number | null, end: number | null): string | null {
  if (start == null && end == null) return null;
  const lo = start ?? end!;
  const hi = end ?? start!;
  return lo === hi ? String(lo) : `${lo}–${hi}`;
}

export function SeriesHoverPreview({ series }: { series: SeriesView }) {
  const status = formatPublicationStatus(series.status);
  const years = yearRange(
    series.earliest_year ?? series.year ?? null,
    series.latest_year ?? null,
  );
  const issueCount = series.issue_count ?? series.total_issues ?? null;
  const meta = [
    series.publisher,
    years,
    issueCount != null
      ? `${issueCount} issue${issueCount === 1 ? "" : "s"}`
      : null,
  ]
    .filter(Boolean)
    .join(" · ");
  const finished = series.progress_summary?.finished ?? 0;
  const total = series.progress_summary?.total ?? issueCount ?? 0;
  const pct =
    total > 0 ? Math.round((Math.min(finished, total) / total) * 100) : 0;
  const genres = (series.genres ?? []).slice(0, 4);

  return (
    <div className="flex gap-3">
      <div className="w-16 shrink-0">
        <Cover
          src={series.cover_url}
          alt=""
          fallback={series.publisher ?? series.name}
        />
      </div>
      <div className="min-w-0 flex-1 space-y-1.5">
        <p className="text-sm leading-snug font-semibold">{series.name}</p>
        <div className="flex flex-wrap items-center gap-1.5">
          {status ? (
            <Badge variant="outline" className="text-[10px]">
              {status}
            </Badge>
          ) : null}
          {meta ? (
            <span className="text-muted-foreground text-xs">{meta}</span>
          ) : null}
        </div>
        {total > 0 ? (
          <div className="space-y-1">
            <Progress
              value={pct}
              aria-label={`Read ${finished} of ${total} issues`}
            />
            <p className="text-muted-foreground text-[11px]">
              {finished === total ? "Caught up" : `${finished} / ${total} read`}
            </p>
          </div>
        ) : null}
        {series.summary ? (
          <p className="text-muted-foreground line-clamp-3 text-xs leading-snug">
            {series.summary}
          </p>
        ) : null}
        {genres.length > 0 ? (
          <div className="flex flex-wrap gap-1">
            {genres.map((g) => (
              <Badge
                key={g}
                variant="secondary"
                className="text-[10px] font-normal"
              >
                {g}
              </Badge>
            ))}
          </div>
        ) : null}
      </div>
    </div>
  );
}

export function IssueHoverPreview({ issue }: { issue: IssueSummaryView }) {
  const heading = formatIssueHeading(issue);
  const meta = [
    issue.series_name,
    issue.page_count != null
      ? `${issue.page_count} page${issue.page_count === 1 ? "" : "s"}`
      : null,
    issue.year != null ? String(issue.year) : null,
  ]
    .filter(Boolean)
    .join(" · ");
  const stateLabel =
    issue.state === "active"
      ? null
      : issue.state === "missing"
        ? "Missing file"
        : issue.state;

  return (
    <div className="flex gap-3">
      <div className="w-16 shrink-0">
        <Cover src={issue.cover_url} alt="" fallback={heading} />
      </div>
      <div className="min-w-0 flex-1 space-y-1.5">
        <p className="text-sm leading-snug font-semibold">{heading}</p>
        {meta ? <p className="text-muted-foreground text-xs">{meta}</p> : null}
        {stateLabel ? (
          <Badge variant="outline" className="text-[10px] capitalize">
            {stateLabel}
          </Badge>
        ) : null}
      </div>
    </div>
  );
}
