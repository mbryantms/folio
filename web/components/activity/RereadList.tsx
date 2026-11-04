"use client";

import Link from "next/link";
import { useState } from "react";

import { SegmentedControl } from "@/components/settings/SegmentedControl";
import { formatDurationMs } from "@/lib/activity";
import type { RereadIssueEntry, RereadSeriesEntry } from "@/lib/api/types";

type Kind = "issues" | "series";

type Row = {
  key: string;
  href: string;
  /** Muted lead-in before the title (the series for an issue row). */
  prefix?: string;
  title: string;
  reads: number;
  detail: string;
};

/**
 * Most-reread issues and series, one list at a time behind an Issues /
 * Series toggle (the toggle hides when only one kind has rereads). Each
 * row carries an inline bar scaled to the most-reread entry. Empty when
 * nothing has been read more than once.
 */
export function RereadList({
  issues,
  series,
}: {
  issues: ReadonlyArray<RereadIssueEntry>;
  series: ReadonlyArray<RereadSeriesEntry>;
}) {
  const issueRows: Row[] = issues
    .filter((i) => i.reads > 1)
    .slice(0, 8)
    .map((e) => ({
      key: e.issue_id,
      href: `/series/${e.series_id}`,
      prefix: `${e.series_name}${e.number_raw ? ` #${e.number_raw}` : ""}`,
      title: e.title ?? "",
      reads: e.reads,
      detail: formatDurationMs(e.active_ms),
    }));
  const seriesRows: Row[] = series
    .filter((s) => s.reads > 1)
    .slice(0, 8)
    .map((e) => ({
      key: e.series_id,
      href: `/series/${e.series_id}`,
      title: e.name,
      reads: e.reads,
      detail: `${e.distinct_issues} issue${e.distinct_issues === 1 ? "" : "s"}`,
    }));

  const [picked, setPicked] = useState<Kind>("issues");

  if (issueRows.length === 0 && seriesRows.length === 0) {
    return (
      <p className="text-muted-foreground text-sm">
        Rereads show up once you&apos;ve revisited an issue or series.
      </p>
    );
  }
  const both = issueRows.length > 0 && seriesRows.length > 0;
  const kind: Kind = both ? picked : issueRows.length > 0 ? "issues" : "series";
  const rows = kind === "issues" ? issueRows : seriesRows;
  const max = Math.max(...rows.map((r) => r.reads), 1);

  return (
    <div className="space-y-3">
      {both ? (
        <SegmentedControl
          value={kind}
          onChange={setPicked}
          options={[
            { value: "issues", label: "Issues" },
            { value: "series", label: "Series" },
          ]}
          ariaLabel="Reread list"
        />
      ) : (
        <p className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
          {kind === "issues" ? "Issues" : "Series"}
        </p>
      )}
      <ol className="space-y-1">
        {rows.map((row, i) => (
          <li
            key={row.key}
            className="hover:bg-muted/50 -mx-2 flex items-start gap-3 rounded-md px-2 py-1.5"
          >
            <span className="text-muted-foreground w-5 shrink-0 pt-0.5 text-right text-xs tabular-nums">
              {i + 1}
            </span>
            <span className="min-w-0 flex-1 space-y-1.5">
              <span className="flex items-baseline justify-between gap-3 text-sm">
                <Link
                  href={row.href}
                  className="text-foreground min-w-0 truncate font-medium hover:underline"
                >
                  {row.prefix ? (
                    <span className="text-muted-foreground font-normal">
                      {row.prefix}
                      {row.title ? " " : ""}
                    </span>
                  ) : null}
                  {row.title}
                </Link>
                <span className="text-muted-foreground shrink-0 text-xs whitespace-nowrap tabular-nums">
                  <span className="text-foreground font-semibold">
                    ×{row.reads}
                  </span>
                  {" · "}
                  {row.detail}
                </span>
              </span>
              <span
                aria-hidden="true"
                className="bg-muted block h-1.5 w-full overflow-hidden rounded-full"
              >
                <span
                  className="block h-full rounded-full"
                  style={{
                    width: `${Math.max(2, (row.reads / max) * 100)}%`,
                    backgroundColor:
                      i === 0
                        ? "var(--primary)"
                        : "color-mix(in oklab, var(--primary) 62%, var(--muted))",
                  }}
                />
              </span>
            </span>
          </li>
        ))}
      </ol>
    </div>
  );
}
