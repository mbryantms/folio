"use client";

import Link from "next/link";
import { HardDrive, Loader2 } from "lucide-react";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useLibraryStats } from "@/lib/api/queries";
import type { LibraryStatsView } from "@/lib/api/types";
import { formatBytes } from "@/lib/format";

function count(n: number): string {
  return n.toLocaleString();
}

/** One-line summary for the libraries list card:
 *  "1,204 series · 18,330 issues · 812 GB", plus the removed count when
 *  there is one. */
export function libraryStatsSummary(s: LibraryStatsView): string {
  const parts = [
    `${count(s.series_count)} series`,
    `${count(s.active_issue_count)} issues`,
    formatBytes(s.archive_bytes),
  ];
  if (s.removed_issue_count > 0) {
    parts.push(`${count(s.removed_issue_count)} removed`);
  }
  return parts.join(" · ");
}

/** The `.bak` figure: size and file count from the stored tally, or a
 *  note that the daily sweep has not counted this library yet. */
export function backupSummary(s: LibraryStatsView): string {
  if (s.backup_bytes == null) return "Not counted yet";
  const files = s.backup_file_count ?? 0;
  return `${formatBytes(s.backup_bytes)} · ${count(files)} ${files === 1 ? "file" : "files"}`;
}

/**
 * Series / issue counts and disk usage for one library (overview tab).
 * Every figure is read from the database — archive size is the scanned
 * file sizes of the active issues, backups are the tally the daily backup
 * sweep stores — so the card never waits on a walk of the library folder.
 */
export function LibraryStatsCard({ librarySlug }: { librarySlug: string }) {
  const q = useLibraryStats(librarySlug);
  const base = `/admin/libraries/${librarySlug}`;

  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="flex items-center gap-2 text-sm font-medium">
          <HardDrive className="size-4" aria-hidden="true" /> Contents and disk
          usage
        </CardTitle>
      </CardHeader>
      <CardContent>
        {q.isLoading ? (
          <div className="text-muted-foreground flex items-center gap-2 text-sm">
            <Loader2 className="size-4 animate-spin" aria-hidden="true" />{" "}
            Loading…
          </div>
        ) : !q.data ? (
          <p className="text-destructive text-sm">
            Failed to load library figures.
          </p>
        ) : (
          <>
            <dl className="grid grid-cols-1 gap-x-6 gap-y-3 text-sm sm:grid-cols-3 lg:grid-cols-6">
              <Stat label="Series" value={count(q.data.series_count)} />
              <Stat label="Issues" value={count(q.data.active_issue_count)} />
              <Stat
                label="Removed issues"
                value={count(q.data.removed_issue_count)}
                href={`${base}/removed`}
              />
              <Stat
                label="Archive size"
                value={formatBytes(q.data.archive_bytes)}
              />
              <Stat
                label="Backups (.bak)"
                value={backupSummary(q.data)}
                href={`${base}/health`}
              />
              <Stat
                label="Last full scan"
                value={
                  q.data.last_scan_at
                    ? new Date(q.data.last_scan_at).toLocaleString()
                    : "Never"
                }
              />
            </dl>
            <p className="text-muted-foreground mt-3 text-xs">
              Archive size adds up the scanned file sizes of the active issues.
              {q.data.backup_counted_at
                ? ` Backups counted ${new Date(q.data.backup_counted_at).toLocaleString()}; the daily backup sweep recounts.`
                : " The daily backup sweep counts backups."}
              {q.data.backup_stale
                ? " A scan has changed the library since then, so the backup figure may be behind until the next count."
                : null}
            </p>
          </>
        )}
      </CardContent>
    </Card>
  );
}

function Stat({
  label,
  value,
  href,
}: {
  label: string;
  value: string;
  href?: string;
}) {
  return (
    <div className="min-w-0">
      <dt className="text-muted-foreground text-xs">{label}</dt>
      <dd className="truncate font-medium tabular-nums">
        {href ? (
          <Link href={href} className="underline-offset-4 hover:underline">
            {value}
          </Link>
        ) : (
          value
        )}
      </dd>
    </div>
  );
}
