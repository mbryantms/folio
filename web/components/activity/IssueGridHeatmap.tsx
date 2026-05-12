"use client";

import Link from "next/link";

import { formatDurationMs } from "@/lib/activity";
import type { IssueSummaryView, RereadIssueEntry } from "@/lib/api/types";

/**
 * Square grid: one cell per issue in publication order, color = read count.
 * 0 = unread (muted), 1 = read once (light), 2-3 (medium), 4+ (saturated).
 * Cells link to the issue's reader; hovering shows title + read count.
 */
export function IssueGridHeatmap({
  issues,
  rereads,
  seriesSlug,
}: {
  issues: ReadonlyArray<IssueSummaryView>;
  rereads: ReadonlyArray<RereadIssueEntry>;
  seriesSlug: string;
}) {
  const readsById = new Map<string, RereadIssueEntry>();
  for (const r of rereads) readsById.set(r.issue_id, r);

  if (issues.length === 0) {
    return (
      <p className="text-muted-foreground text-sm">
        No issues in this series yet.
      </p>
    );
  }

  return (
    <figure className="space-y-2">
      <ul className="text-primary grid [grid-template-columns:repeat(auto-fill,minmax(28px,1fr))] gap-1.5">
        {issues.map((issue) => {
          const entry = readsById.get(issue.id);
          const reads = entry?.reads ?? 0;
          const intensity = bucket(reads);
          const opacity = INTENSITY_OPACITY[intensity];
          const fill = intensity === 0 ? "var(--color-muted)" : "currentColor";
          return (
            <li key={issue.id} className="aspect-square">
              <Link
                href={`/series/${seriesSlug}/issues/${issue.slug}`}
                title={`#${issue.number ?? "?"} ${issue.title ?? ""}\n${reads > 0 ? `Read ×${reads}` : "Unread"}${entry ? ` · ${formatDurationMs(entry.active_ms)}` : ""}`}
                className="focus-visible:ring-ring block size-full rounded focus-visible:ring-2 focus-visible:outline-none"
                style={{ backgroundColor: fill, opacity }}
                aria-label={`Issue ${issue.number ?? issue.title ?? "?"}, ${reads > 0 ? `read ${reads} time${reads === 1 ? "" : "s"}` : "unread"}`}
              />
            </li>
          );
        })}
      </ul>
      <Legend />
    </figure>
  );
}

const INTENSITY_OPACITY: Record<0 | 1 | 2 | 3 | 4, number> = {
  0: 1,
  1: 0.35,
  2: 0.6,
  3: 0.8,
  4: 1,
};

function bucket(reads: number): 0 | 1 | 2 | 3 | 4 {
  if (reads <= 0) return 0;
  if (reads === 1) return 1;
  if (reads <= 3) return 2;
  if (reads <= 5) return 3;
  return 4;
}

function Legend() {
  const labels = ["Unread", "Read", "2–3×", "4–5×", "6+×"];
  return (
    <figcaption className="text-muted-foreground flex flex-wrap items-center gap-3 text-xs">
      {([0, 1, 2, 3, 4] as const).map((level) => (
        <span key={level} className="flex items-center gap-1.5">
          <span
            aria-hidden="true"
            className="text-primary inline-block size-2.5 rounded-sm"
            style={{
              backgroundColor:
                level === 0 ? "var(--color-muted)" : "currentColor",
              opacity: INTENSITY_OPACITY[level],
            }}
          />
          {labels[level]}
        </span>
      ))}
    </figcaption>
  );
}
