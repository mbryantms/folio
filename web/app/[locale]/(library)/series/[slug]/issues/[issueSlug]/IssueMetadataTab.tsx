"use client";

/**
 * Issue **Metadata** tab — a total overview of where this issue's metadata
 * came from and how fresh it is. Deliberately does NOT re-list field values
 * (those live on Details / Credits / Cast / Genres / Covers); it surfaces the
 * things shown nowhere else:
 *
 *   - completeness breakdown (tier + missing core fields)
 *   - which source files Folio has (ComicInfo / MetronInfo / series.json)
 *   - freshness (last synced, last rewritten)
 *   - per-field provenance (field → source → when) — the differentiator
 *   - external IDs (the folded-in External IDs tab, add/unlink intact)
 *   - user-pinned fields (survive rescans)
 */

import {
  AlertCircle,
  CheckCircle2,
  HelpCircle,
  Loader2,
  MinusCircle,
} from "lucide-react";
import * as React from "react";

import { ExternalIdsCard } from "@/components/library/ExternalIdsCard";
import { useIssueMetadataOverview } from "@/lib/api/queries";
import { formatRelativeDate } from "@/lib/format";
import { metadataFieldLabel, metadataFieldLabels } from "@/lib/metadata-fields";
import { cn } from "@/lib/utils";

export function IssueMetadataTab({
  seriesSlug,
  issueSlug,
}: {
  seriesSlug: string;
  issueSlug: string;
}) {
  const { data, isLoading, isError } = useIssueMetadataOverview(
    seriesSlug,
    issueSlug,
  );

  if (isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-8 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" />
        Loading metadata…
      </div>
    );
  }
  if (isError || !data) {
    return (
      <p className="text-muted-foreground py-8 text-sm">
        Couldn&rsquo;t load metadata overview.
      </p>
    );
  }

  const c = data.completeness;

  return (
    <div className="space-y-8">
      {/* ── Completeness ── */}
      {c && (
        <section className="space-y-2">
          <div className="flex items-center gap-2">
            <h3 className="text-foreground text-sm font-semibold">
              Completeness
            </h3>
            <TierPill tier={c.tier} />
          </div>
          {c.missing_core.length > 0 ? (
            <p className="text-muted-foreground text-sm">
              Missing core: {metadataFieldLabels(c.missing_core)}
            </p>
          ) : (
            <p className="text-muted-foreground text-sm">
              All core metadata present.
            </p>
          )}
          {c.missing_recommended.length > 0 && (
            <p className="text-muted-foreground text-xs">
              Also nice to have: {metadataFieldLabels(c.missing_recommended)}
            </p>
          )}
        </section>
      )}

      {/* ── Source files ── */}
      <section className="space-y-2">
        <h3 className="text-foreground text-sm font-semibold">Source files</h3>
        <ul className="space-y-1.5 text-sm">
          <SourceFileRow
            label="ComicInfo.xml"
            state={data.source_files.comicinfo}
          />
          <SourceFileRow
            label="MetronInfo.xml"
            state={data.source_files.metroninfo}
          />
          <SourceFileRow
            label="series.json"
            state={data.source_files.series_json}
          />
        </ul>
      </section>

      {/* ── Freshness ── */}
      <section className="space-y-2">
        <h3 className="text-foreground text-sm font-semibold">Freshness</h3>
        <dl className="grid gap-x-6 gap-y-1.5 text-sm sm:grid-cols-2">
          <FreshnessRow
            label="Last metadata sync"
            value={data.last_metadata_sync_at}
          />
          <FreshnessRow
            label="Last file rewrite"
            value={data.last_rewrite_at}
            suffix={
              data.last_rewrite_kind
                ? ` (${data.last_rewrite_kind})`
                : undefined
            }
          />
        </dl>
      </section>

      {/* ── External IDs (folded-in tab) ── */}
      <section className="space-y-2">
        <h3 className="text-foreground text-sm font-semibold">External IDs</h3>
        <ExternalIdsCard
          entityType="issue"
          seriesSlug={seriesSlug}
          issueSlug={issueSlug}
          chrome="bare"
        />
      </section>

      {/* ── Provenance ── */}
      <section className="space-y-2">
        <h3 className="text-foreground text-sm font-semibold">
          Field provenance
        </h3>
        {data.provenance.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            No provenance recorded yet.
          </p>
        ) : (
          <div className="border-border/60 overflow-hidden rounded-md border">
            <table className="w-full text-sm">
              <thead className="bg-muted/50 text-muted-foreground text-xs">
                <tr>
                  <th className="px-3 py-2 text-left font-medium">Field</th>
                  <th className="px-3 py-2 text-left font-medium">Source</th>
                  <th className="px-3 py-2 text-left font-medium">When</th>
                </tr>
              </thead>
              <tbody className="divide-border/60 divide-y">
                {data.provenance.map((p) => (
                  <tr key={p.field}>
                    <td className="text-foreground px-3 py-2">
                      {metadataFieldLabel(p.field)}
                    </td>
                    <td className="text-muted-foreground px-3 py-2">
                      {p.source_label}
                    </td>
                    <td className="text-muted-foreground px-3 py-2">
                      {formatRelativeDate(p.set_at)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>

      {/* ── Pinned edits ── */}
      {data.user_edited.length > 0 && (
        <section className="space-y-2">
          <h3 className="text-foreground text-sm font-semibold">
            Your pinned fields
          </h3>
          <p className="text-muted-foreground text-xs">
            These were edited by you and are preserved across rescans and
            provider syncs.
          </p>
          <div className="flex flex-wrap gap-1.5">
            {data.user_edited.map((f) => (
              <span
                key={f}
                className="bg-secondary text-secondary-foreground inline-flex items-center rounded-md px-2 py-0.5 text-xs"
              >
                {metadataFieldLabel(f)}
              </span>
            ))}
          </div>
        </section>
      )}
    </div>
  );
}

function TierPill({ tier }: { tier: string }) {
  if (tier === "complete") {
    return (
      <span className="inline-flex items-center gap-1 rounded-full bg-emerald-500/15 px-2.5 py-1 text-xs font-medium text-emerald-600 dark:text-emerald-400">
        <CheckCircle2 className="h-3.5 w-3.5" />
        Complete
      </span>
    );
  }
  if (tier === "partial") {
    return (
      <span className="inline-flex items-center gap-1 rounded-full bg-amber-500/15 px-2.5 py-1 text-xs font-medium text-amber-600 dark:text-amber-400">
        <AlertCircle className="h-3.5 w-3.5" />
        Partial
      </span>
    );
  }
  return (
    <span className="inline-flex items-center gap-1 rounded-full bg-red-500/15 px-2.5 py-1 text-xs font-medium text-red-600 dark:text-red-400">
      <AlertCircle className="h-3.5 w-3.5" />
      Needs metadata
    </span>
  );
}

/** `state` is the API value: `"present"` | `"absent"` | `"unknown"`
 *  (`"unknown"` = scanned before tracking existed → rescan to detect). */
function SourceFileRow({ label, state }: { label: string; state: string }) {
  const icon =
    state === "present" ? (
      <CheckCircle2 className="h-4 w-4 text-emerald-500" />
    ) : state === "unknown" ? (
      <HelpCircle className="text-muted-foreground/50 h-4 w-4" />
    ) : (
      <MinusCircle className="text-muted-foreground/50 h-4 w-4" />
    );
  return (
    <li className="flex items-center gap-2">
      {icon}
      <span className={cn(state !== "present" && "text-muted-foreground")}>
        {label}
      </span>
      {state === "unknown" && (
        <span className="text-muted-foreground text-xs">
          · unknown — rescan to detect
        </span>
      )}
    </li>
  );
}

function FreshnessRow({
  label,
  value,
  suffix,
}: {
  label: string;
  value?: string | null;
  suffix?: string;
}) {
  return (
    <div className="flex items-center justify-between gap-3 sm:block">
      <dt className="text-muted-foreground text-xs">{label}</dt>
      <dd className="text-foreground">
        {value ? `${formatRelativeDate(value)}${suffix ?? ""}` : "Never"}
      </dd>
    </div>
  );
}
