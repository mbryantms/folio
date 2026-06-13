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
import { statusTone, statusToneText } from "@/lib/ui/status-tone";

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
    <div className="space-y-6">
      {/* ── At-a-glance status: the three short sections sit side-by-side as
           cards so they use the full width instead of each stranding its
           right half on desktop. The detail tables stay full-width below. ── */}
      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        {c && (
          <MetaCard
            title="Completeness"
            headerExtra={<TierPill tier={c.tier} />}
          >
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
          </MetaCard>
        )}

        <MetaCard title="Source files">
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
        </MetaCard>

        <MetaCard title="Freshness">
          <dl className="space-y-2 text-sm">
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
        </MetaCard>

        {data.user_edited.length > 0 && (
          <MetaCard title="Your pinned fields">
            <p className="text-muted-foreground mb-2 text-xs">
              Edited by you — preserved across rescans and provider syncs.
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
          </MetaCard>
        )}
      </div>

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
                  <tr key={p.field} className="hover:bg-muted/30">
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
    </div>
  );
}

/** Card wrapper for the at-a-glance status row. Uses theme card tokens so it
 *  groups a short status block without the heavy full-width section feel. */
function MetaCard({
  title,
  headerExtra,
  children,
}: {
  title: string;
  headerExtra?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="border-border bg-card space-y-3 rounded-lg border p-4">
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-foreground text-sm font-semibold">{title}</h3>
        {headerExtra}
      </div>
      {children}
    </section>
  );
}

function TierPill({ tier }: { tier: string }) {
  if (tier === "complete") {
    return (
      <span
        className={cn(
          "inline-flex items-center gap-1 rounded-full px-2.5 py-1 text-xs font-medium",
          statusTone("success"),
        )}
      >
        <CheckCircle2 className="h-3.5 w-3.5" />
        Complete
      </span>
    );
  }
  if (tier === "partial") {
    return (
      <span
        className={cn(
          "inline-flex items-center gap-1 rounded-full px-2.5 py-1 text-xs font-medium",
          statusTone("warning"),
        )}
      >
        <AlertCircle className="h-3.5 w-3.5" />
        Partial
      </span>
    );
  }
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 rounded-full px-2.5 py-1 text-xs font-medium",
        statusTone("error"),
      )}
    >
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
      <CheckCircle2 className={cn("h-4 w-4", statusToneText("success"))} />
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
    <div className="flex items-baseline justify-between gap-3">
      <dt className="text-muted-foreground text-xs">{label}</dt>
      <dd className="text-foreground text-right">
        {value ? `${formatRelativeDate(value)}${suffix ?? ""}` : "Never"}
      </dd>
    </div>
  );
}
