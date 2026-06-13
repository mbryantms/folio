"use client";

import * as React from "react";
import { Wrench, AlertTriangle } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useIssueHealth } from "@/lib/api/queries";
import { statusTone } from "@/lib/ui/status-tone";

/**
 * Tranche B of recovery-visibility: a compact badge that renders
 * inline with the other status chips on the issue detail page when
 * the file has open health-issues. The shape:
 *
 *   - Any warning-severity row (e.g. `SkippedArchiveEntries`)  →
 *     amber **Partial** chip with a tooltip explaining what's
 *     missing and how many of N entries were dropped.
 *   - Only info-severity rows (e.g. `RecoveredArchive`)        →
 *     muted **Recovered** chip with a tooltip naming the
 *     recovery technique.
 *   - No open rows                                              →
 *     renders nothing (the file is clean).
 *
 * The query is keyed by `(seriesSlug, issueSlug)` so React Query
 * caches it independently from the admin-level health list.
 * Failures are non-fatal — render-nothing on error keeps the
 * detail page from breaking when an admin endpoint is hiccupping.
 */
export function IssueHealthBadge({
  seriesSlug,
  issueSlug,
}: {
  seriesSlug: string;
  issueSlug: string;
}) {
  const { data } = useIssueHealth(seriesSlug, issueSlug);
  if (!data || data.length === 0) return null;

  const warnings = data.filter((d) => d.severity === "warning");
  const infos = data.filter((d) => d.severity === "info");

  if (warnings.length > 0) {
    return <PartialBadge rows={warnings} />;
  }
  if (infos.length > 0) {
    return <RecoveredBadge rows={infos} />;
  }
  return null;
}

function PartialBadge({ rows }: { rows: HealthRow[] }) {
  // Summarize across rows. Today there's at most one
  // SkippedArchiveEntries row per file (one row per reason); future
  // multi-defense scenarios would group naturally.
  const summary = rows.map(summarizeRow).join("; ");
  return (
    <TooltipProvider delayDuration={200}>
      <Tooltip>
        <TooltipTrigger asChild>
          <Badge variant="outline" className={statusTone("warning")}>
            <AlertTriangle aria-hidden="true" className="mr-1 h-3 w-3" />
            Partial
          </Badge>
        </TooltipTrigger>
        <TooltipContent className="max-w-xs text-xs leading-relaxed">
          {summary}
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}

function RecoveredBadge({ rows }: { rows: HealthRow[] }) {
  const summary = rows.map(summarizeRow).join("; ");
  return (
    <TooltipProvider delayDuration={200}>
      <Tooltip>
        <TooltipTrigger asChild>
          <Badge variant="secondary">
            <Wrench aria-hidden="true" className="mr-1 h-3 w-3" />
            Recovered
          </Badge>
        </TooltipTrigger>
        <TooltipContent className="max-w-xs text-xs leading-relaxed">
          {summary}
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}

type HealthRow = { kind: string; payload: unknown };

function summarizeRow(row: HealthRow): string {
  const data =
    row.payload && typeof row.payload === "object"
      ? ((row.payload as Record<string, unknown>).data as
          | Record<string, unknown>
          | undefined)
      : undefined;
  if (!data) return row.kind;
  if (row.kind === "SkippedArchiveEntries") {
    const dropped = typeof data.dropped === "number" ? data.dropped : "?";
    const total = typeof data.total === "number" ? data.total : "?";
    const reason =
      typeof data.reason === "string" ? data.reason : "soft defense";
    return `${dropped} of ${total} entries dropped (${reason})`;
  }
  if (row.kind === "RecoveredArchive") {
    const technique =
      typeof data.technique === "string" ? data.technique : "unknown";
    return `Repaired during scan via ${technique}`;
  }
  return row.kind;
}
