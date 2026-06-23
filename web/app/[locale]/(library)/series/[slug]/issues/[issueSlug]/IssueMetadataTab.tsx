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
  Check,
  CheckCircle2,
  HelpCircle,
  Loader2,
  MinusCircle,
  RotateCcw,
  Sparkles,
} from "lucide-react";
import dynamic from "next/dynamic";
import * as React from "react";

import { ExternalIdsCard } from "@/components/library/ExternalIdsCard";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useIssueMetadataOverview } from "@/lib/api/queries";

// Heavy match dialog — lazy so the Metadata tab stays light; the chunk
// loads only when the user opens the per-issue match flow from here.
const MetadataMatchDialog = dynamic(
  () =>
    import("@/components/library/MetadataMatchDialog").then(
      (m) => m.MetadataMatchDialog,
    ),
  { ssr: false },
);
import { useSetIssueMetadataAccepted } from "@/lib/api/mutations";
import { formatRelativeDate } from "@/lib/format";
import { metadataFieldLabel, metadataFieldLabels } from "@/lib/metadata-fields";
import { cn } from "@/lib/utils";
import { statusTone, statusToneText } from "@/lib/ui/status-tone";

export function IssueMetadataTab({
  seriesSlug,
  issueSlug,
  libraryId,
}: {
  seriesSlug: string;
  issueSlug: string;
  libraryId: string;
}) {
  const { data, isLoading, isError } = useIssueMetadataOverview(
    seriesSlug,
    issueSlug,
  );
  const setAccepted = useSetIssueMetadataAccepted(seriesSlug, issueSlug);
  // Own per-issue match dialog (self-contained — no coordination with the
  // sibling settings menu). Mounted on first open and kept mounted so the
  // close animation still runs (G6 idiom).
  const [matchOpen, setMatchOpen] = React.useState(false);
  const [matchMounted, setMatchMounted] = React.useState(false);
  if (matchOpen && !matchMounted) setMatchMounted(true);

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
            {/* Escape hatch (B4): a thin/unmatched issue can be marked complete
                so it leaves the unmatched worklist. `accepted` is reversible
                and never hides the gaps above. */}
            {c.tier === "accepted" ? (
              <div className="space-y-1.5 pt-1">
                <p className="text-muted-foreground text-xs">
                  Marked complete despite the gaps above — hidden from the
                  unmatched worklist.
                </p>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={setAccepted.isPending}
                  onClick={() => setAccepted.mutate(false)}
                >
                  <RotateCcw className="h-3.5 w-3.5" />
                  Reopen for metadata
                </Button>
              </div>
            ) : c.tier !== "complete" ? (
              <div className="space-y-1.5 pt-1">
                <p className="text-muted-foreground text-xs">
                  No usable provider match? Mark it complete to clear it from
                  the unmatched worklist (reversible).
                </p>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={setAccepted.isPending}
                  onClick={() => setAccepted.mutate(true)}
                >
                  <Check className="h-3.5 w-3.5" />
                  Mark metadata complete
                </Button>
              </div>
            ) : null}
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

      {/* ── Alternate provider series (provider series-boundary divergence) ──
           Renders only when providers disagree on which series this issue
           belongs to; then lists where it maps in EACH provider, flags the
           split ones, and shows whether it's matched. Invisible otherwise. */}
      {data.alternate_provider_series.length > 0 && (
        <section className="space-y-2">
          <h3 className="text-foreground text-sm font-semibold">
            Alternate provider series
          </h3>
          <div className="border-border bg-card space-y-3 rounded-lg border p-4">
            <p className="text-muted-foreground text-sm">
              Providers don&rsquo;t all file this issue under the same series
              (e.g. a legacy renumbering). Here&rsquo;s where it maps in each
              and whether it&rsquo;s matched — its metadata is pulled from the
              series flagged <span className="font-medium">separate</span>.
            </p>
            <ul className="space-y-2">
              {data.alternate_provider_series.map((a) => {
                const name =
                  a.provider_series_name ?? `#${a.provider_series_id}`;
                const label =
                  a.declared_year != null
                    ? `${name} (${a.declared_year})`
                    : name;
                const rangeLabel =
                  a.range_low && a.range_high
                    ? a.range_low === a.range_high
                      ? `#${a.range_low}`
                      : `#${a.range_low}–${a.range_high}`
                    : null;
                return (
                  <li
                    key={`${a.source}:${a.provider_series_id}`}
                    className="flex flex-wrap items-center gap-x-2 gap-y-1"
                  >
                    <Badge variant="outline" className="font-normal">
                      {a.source_label}
                    </Badge>
                    {a.provider_series_url ? (
                      <a
                        href={a.provider_series_url}
                        target="_blank"
                        rel="noreferrer"
                        className="hover:underline"
                      >
                        {label} ↗
                      </a>
                    ) : (
                      <span>{label}</span>
                    )}
                    {a.diverges ? (
                      <Badge variant="secondary" className="font-normal">
                        separate{rangeLabel ? ` · ${rangeLabel}` : ""}
                      </Badge>
                    ) : (
                      <span className="text-muted-foreground text-[10px] font-medium tracking-wider uppercase">
                        main run
                      </span>
                    )}
                    {a.matched_issue_id ? (
                      a.matched_issue_url ? (
                        <a
                          href={a.matched_issue_url}
                          target="_blank"
                          rel="noreferrer"
                          className={cn(
                            "inline-flex items-center gap-1 text-xs hover:underline",
                            statusToneText("success"),
                          )}
                        >
                          <Check className="h-3 w-3" /> matched ↗
                        </a>
                      ) : (
                        <span
                          className={cn(
                            "inline-flex items-center gap-1 text-xs",
                            statusToneText("success"),
                          )}
                        >
                          <Check className="h-3 w-3" /> matched
                        </span>
                      )
                    ) : (
                      <span className="text-muted-foreground text-xs">
                        not matched
                      </span>
                    )}
                  </li>
                );
              })}
            </ul>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setMatchOpen(true)}
            >
              <Sparkles className="h-3.5 w-3.5" />
              Find &amp; match metadata
            </Button>
          </div>
        </section>
      )}

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

      {matchMounted && (
        <MetadataMatchDialog
          open={matchOpen}
          onOpenChange={setMatchOpen}
          scope={{
            kind: "issue",
            seriesSlug,
            issueSlug,
            libraryId,
          }}
        />
      )}
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
  if (tier === "accepted") {
    // Operator-acknowledged (B4) — distinct from genuine completeness; the
    // info tone signals "manually resolved, gaps remain" without the alarm of
    // the error pill.
    return (
      <span
        className={cn(
          "inline-flex items-center gap-1 rounded-full px-2.5 py-1 text-xs font-medium",
          statusTone("info"),
        )}
      >
        <Check className="h-3.5 w-3.5" />
        Accepted
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
