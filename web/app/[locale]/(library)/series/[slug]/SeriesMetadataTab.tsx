"use client";

/**
 * Series page › **Metadata** tab — everything administrative about a
 * series' metadata, so the header, Issues and Collection stay reading
 * surfaces:
 *
 *   1. a summary strip (series ids, issue completeness, issue ids,
 *      provider sync and sidecar state);
 *   2. the fetch actions that used to live under the ··· menu's "Fetch
 *      metadata" — Refresh this series, Match this series, the per-issue
 *      batch (only missing or partial / all) — plus the latest run;
 *   3. Identifiers and Provider series mapping (moved from Details);
 *   4. an audit table: every issue with its tier, missing core fields,
 *      the three provider ids, the tagging tool that wrote its sidecar and
 *      its latest run — server-side filters, row selection, and the two
 *      existing bulk actions (fetch the selection, mark it complete);
 *   5. what the folder holds, and field provenance.
 *
 * Covers stay on the Issues tab; this is the list you scan for "what is
 * still missing".
 */

import { ChevronRight, ListChecks, Loader2, Sparkles } from "lucide-react";
import dynamic from "next/dynamic";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import * as React from "react";
import { toast } from "sonner";

import { ExternalIdsCard } from "@/components/library/ExternalIdsCard";
import { SeriesProviderRangesCard } from "@/components/library/SeriesProviderRangesCard";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  useAcceptSeriesIssues,
  useCreateSeriesBatch,
  useCreateSeriesSelectionBatch,
} from "@/lib/api/mutations";
import {
  useSeriesMetadataIssues,
  useSeriesMetadataOverview,
} from "@/lib/api/queries";
import type { SeriesMetadataIssueFilters } from "@/lib/api/query-keys";
import type {
  ExternalIdRow,
  SeriesMetadataIssueRow,
  SeriesMetadataOverview,
} from "@/lib/api/types";
import { formatRelativeDate } from "@/lib/format";
import { metadataFieldLabels } from "@/lib/metadata-fields";
import { useSelection } from "@/lib/selection/use-selection";
import { issueUrl } from "@/lib/urls";
import { cn } from "@/lib/utils";

const MetadataMatchDialog = dynamic(
  () =>
    import("@/components/library/MetadataMatchDialog").then(
      (m) => m.MetadataMatchDialog,
    ),
  { ssr: false },
);
const SeriesRefreshDialog = dynamic(
  () =>
    import("@/components/library/SeriesRefreshDialog").then(
      (m) => m.SeriesRefreshDialog,
    ),
  { ssr: false },
);

const SOURCES = [
  { key: "comicvine", label: "ComicVine" },
  { key: "metron", label: "Metron" },
  { key: "gcd", label: "GCD" },
] as const;
type SourceKey = (typeof SOURCES)[number]["key"];

const TIER_LABELS: Record<string, string> = {
  complete: "Complete",
  partial: "Partial",
  needs_metadata: "Needs metadata",
  accepted: "Accepted",
};
const TIER_CLASSES: Record<string, string> = {
  complete: "bg-success/15 text-success ring-1 ring-success/30",
  partial: "bg-warning/15 text-warning ring-1 ring-warning/30",
  needs_metadata:
    "bg-destructive/15 text-destructive ring-1 ring-destructive/30",
  accepted: "bg-secondary text-muted-foreground ring-1 ring-border",
};

const TOOL_LABELS: Record<string, string> = {
  folio: "Folio",
  comictagger_comicvine: "ComicTagger · ComicVine",
  comictagger_metron: "ComicTagger · Metron",
  comictagger_gcd: "ComicTagger · GCD",
  comictagger_other: "ComicTagger",
  mylar: "Mylar",
  scraper: "Other scraper",
  untagged: "No tags",
};

export function SeriesMetadataTab({
  seriesSlug,
  seriesName,
  libraryId,
}: {
  seriesSlug: string;
  seriesName: string;
  libraryId: string;
}) {
  const overview = useSeriesMetadataOverview(seriesSlug);
  return (
    <div className="space-y-8">
      <SummaryStrip overview={overview.data} loading={overview.isLoading} />
      <Actions
        seriesSlug={seriesSlug}
        seriesName={seriesName}
        libraryId={libraryId}
        overview={overview.data}
      />
      <div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
        <Section
          title="Identifiers"
          description="Provider identifiers linked to this series."
        >
          <ExternalIdsCard
            entityType="series"
            seriesSlug={seriesSlug}
            chrome="bare"
          />
        </Section>
        <Section
          title="Provider series mapping"
          description="How this run maps onto each provider's series. Analyze coverage finds the main series and files annuals and specials under their own."
        >
          <SeriesProviderRangesCard seriesSlug={seriesSlug} />
        </Section>
      </div>
      <IssuesTable seriesSlug={seriesSlug} overview={overview.data} />
      <div className="grid grid-cols-1 gap-6 lg:grid-cols-2">
        <FilesCard overview={overview.data} />
        <ProvenanceCard overview={overview.data} />
      </div>
    </div>
  );
}

// ───────── summary strip ─────────

function SummaryStrip({
  overview,
  loading,
}: {
  overview: SeriesMetadataOverview | undefined;
  loading: boolean;
}) {
  if (loading && !overview) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-2 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading metadata…
      </div>
    );
  }
  if (!overview) return null;
  const t = overview.tiers;
  const total = Math.max(overview.issue_ids.total, 1);
  const pct = (n: number) => Math.min(100, (n / total) * 100);
  const byMs = new Map(overview.series_ids.map((r) => [r.source, r]));
  return (
    <section
      className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4"
      data-testid="series-metadata-summary"
    >
      <Tile label="Series identifiers">
        <div className="flex flex-wrap gap-1.5">
          {SOURCES.map((s) => {
            const row = byMs.get(s.key);
            return <IdChip key={s.key} label={s.label} row={row} />;
          })}
        </div>
      </Tile>
      <Tile label="Issue completeness">
        <div
          className="bg-secondary flex h-2 w-full overflow-hidden rounded-full"
          role="img"
          aria-label={`${t.complete} complete, ${t.partial} partial, ${t.needs_metadata} need metadata, ${t.accepted} accepted`}
        >
          <div
            className="bg-success"
            style={{ width: `${pct(t.complete)}%` }}
          />
          <div className="bg-warning" style={{ width: `${pct(t.partial)}%` }} />
          <div
            className="bg-destructive"
            style={{ width: `${pct(t.needs_metadata)}%` }}
          />
          <div
            className="bg-muted-foreground/50"
            style={{ width: `${pct(t.accepted)}%` }}
          />
        </div>
        <div className="text-muted-foreground text-xs tabular-nums">
          {t.complete} complete · {t.partial} partial · {t.needs_metadata} need
          metadata
          {t.accepted > 0 ? ` · ${t.accepted} accepted` : ""}
        </div>
      </Tile>
      <Tile label="Issue identifiers">
        <div className="text-muted-foreground text-xs tabular-nums">
          ComicVine {overview.issue_ids.comicvine} · Metron{" "}
          {overview.issue_ids.metron} · GCD {overview.issue_ids.gcd} of{" "}
          {overview.issue_ids.total}
        </div>
        <div className="text-muted-foreground text-xs tabular-nums">
          All three: {overview.issue_ids.all_sources}
        </div>
      </Tile>
      <Tile label="Provider sync">
        <div className="text-muted-foreground text-xs">
          Series:{" "}
          {overview.sync.series_last_sync_at
            ? (formatRelativeDate(overview.sync.series_last_sync_at) ??
              overview.sync.series_last_sync_at)
            : "never"}
          {overview.sync.metadata_sync_paused ? " · paused" : ""}
        </div>
        <div className="text-muted-foreground text-xs tabular-nums">
          Issues ever fetched: {overview.sync.issues_synced} · sidecars
          rewritten: {overview.sync.sidecars_rewritten}
          {overview.sync.last_rewrite_at
            ? ` (${formatRelativeDate(overview.sync.last_rewrite_at) ?? ""})`
            : ""}
        </div>
      </Tile>
    </section>
  );
}

function Tile({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="border-border/60 flex flex-col gap-1.5 rounded-md border p-3">
      <div className="text-muted-foreground text-[11px] font-medium tracking-wide uppercase">
        {label}
      </div>
      {children}
    </div>
  );
}

function IdChip({
  label,
  row,
}: {
  label: string;
  row: ExternalIdRow | undefined;
}) {
  const inner = (
    <>
      <span
        className={cn(
          "inline-block h-2 w-2 rounded-full",
          row ? "bg-success" : "bg-muted-foreground/30",
        )}
      />
      {label}{" "}
      {row ? (
        <span className="tabular-nums">{row.external_id}</span>
      ) : (
        <span className="text-muted-foreground">none</span>
      )}
    </>
  );
  const cls =
    "border-border/60 inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs";
  return row?.external_url ? (
    <a
      href={row.external_url}
      target="_blank"
      rel="noreferrer"
      className={cn(cls, "hover:underline")}
    >
      {inner}
    </a>
  ) : (
    <span className={cls}>{inner}</span>
  );
}

// ───────── actions ─────────

function Actions({
  seriesSlug,
  seriesName,
  libraryId,
  overview,
}: {
  seriesSlug: string;
  seriesName: string;
  libraryId: string;
  overview: SeriesMetadataOverview | undefined;
}) {
  const router = useRouter();
  const searchParams = useSearchParams();
  const [refreshOpen, setRefreshOpen] = React.useState(false);
  const [refreshMounted, setRefreshMounted] = React.useState(false);
  // `?match=1` (the needs-metadata worklist's "Find metadata") opens the
  // match dialog on arrival.
  const [matchOpen, setMatchOpen] = React.useState(
    () => searchParams.get("match") === "1",
  );
  const [matchMounted, setMatchMounted] = React.useState(matchOpen);
  const handleMatchOpenChange = (next: boolean) => {
    setMatchOpen(next);
    if (!next && typeof window !== "undefined") {
      const url = new URL(window.location.href);
      if (url.searchParams.has("match")) {
        url.searchParams.delete("match");
        window.history.replaceState({}, "", url.toString());
      }
    }
  };
  const createBatch = useCreateSeriesBatch(seriesSlug);
  const fetchMetadata = (scope: "all" | "incomplete") => {
    createBatch.mutate(
      { scope },
      {
        onSuccess: (resp) => {
          if (!resp) return;
          if (resp.items_total === 0) {
            toast.info(
              scope === "incomplete"
                ? "Every issue already has complete metadata"
                : resp.recently_fetched > 0
                  ? "Every issue was searched in the last 24 hours — results are in Review"
                  : "No issues to search",
            );
            return;
          }
          const summary =
            resp.remainder > 0
              ? `Searching ${resp.items_total} of ${resp.eligible} issues for metadata — ${resp.remainder} more on the next run`
              : `Searching ${resp.items_total} issues for metadata`;
          toast.success(summary, {
            action: {
              label: "Review",
              onClick: () =>
                router.push(
                  `/admin/metadata?tab=review&batch=${resp.batch_id}`,
                ),
            },
          });
        },
      },
    );
  };
  const incomplete = overview
    ? overview.tiers.partial + overview.tiers.needs_metadata
    : null;
  const last = overview?.last_run ?? null;
  return (
    <section
      className="flex flex-wrap items-center gap-2"
      data-testid="series-metadata-actions"
    >
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => {
          setRefreshMounted(true);
          setRefreshOpen(true);
        }}
      >
        <ListChecks className="mr-1.5 h-4 w-4" /> Refresh this series…
      </Button>
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => {
          setMatchMounted(true);
          setMatchOpen(true);
        }}
      >
        <Sparkles className="mr-1.5 h-4 w-4" /> Match this series…
      </Button>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={createBatch.isPending}
        onClick={() => fetchMetadata("incomplete")}
      >
        <Sparkles className="mr-1.5 h-4 w-4" /> Fetch metadata · only missing or
        partial{incomplete != null ? ` (${incomplete})` : ""}
      </Button>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={createBatch.isPending}
        onClick={() => fetchMetadata("all")}
      >
        <Sparkles className="mr-1.5 h-4 w-4" /> Fetch metadata · all issues
      </Button>
      {overview ? (
        <span className="text-muted-foreground text-xs">
          {last ? (
            <>
              Last run: {last.scope} · {last.status} ·{" "}
              {formatRelativeDate(last.started_at) ?? last.started_at}
              {" · "}
            </>
          ) : (
            "No metadata runs yet · "
          )}
          <Link href="/admin/metadata?tab=runs" className="hover:underline">
            {overview.runs_total} run{overview.runs_total === 1 ? "" : "s"} on
            this series →
          </Link>
        </span>
      ) : null}
      {refreshMounted && (
        <SeriesRefreshDialog
          open={refreshOpen}
          onOpenChange={setRefreshOpen}
          seriesSlug={seriesSlug}
          seriesName={seriesName}
          libraryId={libraryId}
        />
      )}
      {matchMounted && (
        <MetadataMatchDialog
          open={matchOpen}
          onOpenChange={handleMatchOpenChange}
          scope={{ kind: "series", seriesSlug, libraryId }}
        />
      )}
    </section>
  );
}

// ───────── sections ─────────

function Section({
  title,
  description,
  children,
}: {
  title: string;
  description?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="space-y-3">
      <div>
        <h3 className="text-foreground text-sm font-semibold">{title}</h3>
        {description ? (
          <p className="text-muted-foreground text-xs">{description}</p>
        ) : null}
      </div>
      {children}
    </section>
  );
}

// ───────── issues table ─────────

type TierFilter = NonNullable<SeriesMetadataIssueFilters["tier"]>;

function IssuesTable({
  seriesSlug,
  overview,
}: {
  seriesSlug: string;
  overview: SeriesMetadataOverview | undefined;
}) {
  const [tier, setTier] = React.useState<TierFilter | undefined>(undefined);
  const [without, setWithout] = React.useState<SourceKey | undefined>(
    undefined,
  );
  const [specials, setSpecials] = React.useState(false);
  const filters: SeriesMetadataIssueFilters = React.useMemo(
    () => ({
      tier,
      without,
      kind: specials ? "specials" : undefined,
    }),
    [tier, without, specials],
  );
  const q = useSeriesMetadataIssues(seriesSlug, filters);
  const items = React.useMemo(() => q.data?.items ?? [], [q.data]);
  const selection = useSelection(items);
  const selectedRows = items.filter((r) => selection.isSelected(r.id));
  const selectionBatch = useCreateSeriesSelectionBatch(seriesSlug);
  const accept = useAcceptSeriesIssues(seriesSlug);
  const router = useRouter();

  const fetchSelected = () => {
    const issue_ids = selectedRows.map((r) => r.id);
    if (issue_ids.length === 0) return;
    selectionBatch.mutate(
      { issue_ids },
      {
        onSuccess: (resp) => {
          if (!resp) return;
          toast.success(
            `Searching ${resp.items_total} issue${resp.items_total === 1 ? "" : "s"} for metadata`,
            {
              action: {
                label: "Review",
                onClick: () =>
                  router.push(
                    `/admin/metadata?tab=review&batch=${resp.batch_id}`,
                  ),
              },
            },
          );
          selection.exit();
        },
      },
    );
  };
  const markSelected = (accepted: boolean) => {
    const issue_ids = selectedRows
      .filter((r) => r.accepted !== accepted)
      .map((r) => r.id);
    if (issue_ids.length === 0) return;
    accept.mutate(
      { issue_ids, accepted },
      { onSuccess: () => selection.exit() },
    );
  };
  const t = overview?.tiers;
  const ids = overview?.issue_ids;
  const chips: {
    key: string;
    label: string;
    count?: number;
    active: boolean;
    onClick: () => void;
    tone?: "destructive";
  }[] = [
    {
      key: "all",
      label: "All",
      count: ids?.total,
      active: tier == null && without == null && !specials,
      onClick: () => {
        setTier(undefined);
        setWithout(undefined);
        setSpecials(false);
      },
    },
    {
      key: "needs",
      label: "Needs metadata",
      count: t?.needs_metadata,
      active: tier === "needs_metadata",
      tone: "destructive",
      onClick: () =>
        setTier(tier === "needs_metadata" ? undefined : "needs_metadata"),
    },
    {
      key: "partial",
      label: "Partial",
      count: t?.partial,
      active: tier === "partial",
      onClick: () => setTier(tier === "partial" ? undefined : "partial"),
    },
    {
      key: "complete",
      label: "Complete",
      count: t?.complete,
      active: tier === "complete",
      onClick: () => setTier(tier === "complete" ? undefined : "complete"),
    },
    ...(t && t.accepted > 0
      ? [
          {
            key: "accepted",
            label: "Accepted",
            count: t.accepted,
            active: tier === "accepted",
            onClick: () =>
              setTier(tier === "accepted" ? undefined : "accepted"),
          },
        ]
      : []),
    ...SOURCES.map((s) => ({
      key: `no-${s.key}`,
      label: `No ${s.label} id`,
      count: ids ? ids.total - ids[s.key] : undefined,
      active: without === s.key,
      onClick: () => setWithout(without === s.key ? undefined : s.key),
    })),
    {
      key: "specials",
      label: "Annuals & specials",
      active: specials,
      onClick: () => setSpecials(!specials),
    },
  ];
  const allSelected = items.length > 0 && selectedRows.length === items.length;
  return (
    <Section
      title="Issues"
      description="Every issue, with what is still missing. Covers live on the Issues tab; this is the audit list."
    >
      <div
        className="flex flex-wrap items-center gap-1.5"
        role="group"
        aria-label="Filters"
      >
        {chips.map((c) => (
          <button
            key={c.key}
            type="button"
            aria-pressed={c.active}
            onClick={c.onClick}
            className={cn(
              "border-border/60 inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs transition-colors",
              c.active
                ? "bg-foreground text-background border-foreground"
                : "hover:bg-secondary",
              !c.active && c.tone === "destructive" && "text-destructive",
            )}
          >
            {c.label}
            {c.count != null ? (
              <span className="tabular-nums opacity-80">{c.count}</span>
            ) : null}
          </button>
        ))}
      </div>

      {q.isLoading && !q.data ? (
        <div className="text-muted-foreground flex items-center gap-2 py-4 text-sm">
          <Loader2 className="h-4 w-4 animate-spin" /> Loading issues…
        </div>
      ) : items.length === 0 ? (
        <p className="text-muted-foreground py-4 text-sm">
          No issues match these filters.
        </p>
      ) : (
        <div className="overflow-x-auto">
          <table
            className="w-full min-w-[880px] text-sm"
            data-testid="series-metadata-issues"
          >
            <thead className="text-muted-foreground text-xs tracking-wide uppercase">
              <tr>
                <th className="py-1.5 pr-2 text-left font-medium">
                  <input
                    type="checkbox"
                    aria-label="Select all shown"
                    checked={allSelected}
                    onChange={() =>
                      allSelected ? selection.clear() : selection.selectAll()
                    }
                  />
                </th>
                <th className="py-1.5 pr-3 text-left font-medium">Issue</th>
                <th className="py-1.5 pr-3 text-left font-medium">
                  Cover date
                </th>
                <th className="py-1.5 pr-3 text-left font-medium">Tier</th>
                <th className="py-1.5 pr-3 text-left font-medium">
                  Missing core
                </th>
                <th className="py-1.5 pr-3 text-left font-medium">
                  CV · Metron · GCD
                </th>
                <th className="py-1.5 pr-3 text-left font-medium">Tagged by</th>
                <th className="py-1.5 pr-3 text-left font-medium">
                  Last fetched
                </th>
                <th className="py-1.5 text-left font-medium"></th>
              </tr>
            </thead>
            <tbody className="divide-border/60 divide-y">
              {items.map((r) => (
                <IssueRow
                  key={r.id}
                  row={r}
                  seriesSlug={seriesSlug}
                  selected={selection.isSelected(r.id)}
                  onToggle={(ev) => selection.toggle(r.id, ev)}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}

      <div className="flex flex-wrap items-center gap-2 text-xs">
        <span className="text-muted-foreground tabular-nums">
          {selectedRows.length} selected
          {q.data ? ` · ${q.data.total} of ${q.data.series_total} shown` : ""}
        </span>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={selectedRows.length === 0 || selectionBatch.isPending}
          onClick={fetchSelected}
        >
          Fetch metadata for selected
        </Button>
        {selectedRows.some((r) => !r.accepted) ? (
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={accept.isPending}
            onClick={() => markSelected(true)}
          >
            Mark selected complete
          </Button>
        ) : null}
        {selectedRows.length > 0 && selectedRows.every((r) => r.accepted) ? (
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={accept.isPending}
            onClick={() => markSelected(false)}
          >
            Revert &ldquo;complete&rdquo;
          </Button>
        ) : null}
        <span className="text-muted-foreground">
          The existing per-issue batch (reviewed before apply) and the existing
          acknowledgement.
        </span>
      </div>
    </Section>
  );
}

function IssueRow({
  row,
  seriesSlug,
  selected,
  onToggle,
}: {
  row: SeriesMetadataIssueRow;
  seriesSlug: string;
  selected: boolean;
  onToggle: (ev: { shiftKey?: boolean }) => void;
}) {
  const tierKey =
    row.accepted && row.tier !== "complete" ? "accepted" : row.tier;
  return (
    <tr
      data-testid={`issue-row-${row.slug}`}
      className={cn(selected && "bg-secondary/40")}
    >
      <td className="py-1.5 pr-2">
        <input
          type="checkbox"
          aria-label={`Select ${row.label}`}
          checked={selected}
          onChange={() => onToggle({})}
          onClick={(e) => {
            if (e.shiftKey) {
              e.preventDefault();
              onToggle({ shiftKey: true });
            }
          }}
        />
      </td>
      <td className="py-1.5 pr-3">
        <Link href={issueUrl(seriesSlug, row.slug)} className="hover:underline">
          {row.label}
        </Link>
        {row.title ? (
          <span className="text-muted-foreground ml-2 text-xs">
            {row.title}
          </span>
        ) : null}
      </td>
      <td className="py-1.5 pr-3 tabular-nums">
        {row.cover_date ?? <span className="text-muted-foreground">none</span>}
      </td>
      <td className="py-1.5 pr-3">
        <span
          className={cn(
            "inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium",
            TIER_CLASSES[tierKey] ?? TIER_CLASSES.partial,
          )}
        >
          {TIER_LABELS[tierKey] ?? tierKey}
        </span>
      </td>
      <td className="py-1.5 pr-3 text-xs">
        {row.missing_core.length > 0 ? (
          metadataFieldLabels(row.missing_core)
        ) : (
          <span className="text-muted-foreground">—</span>
        )}
      </td>
      <td className="py-1.5 pr-3">
        <span className="inline-flex items-center gap-1.5">
          {SOURCES.map((s) => {
            const id = row.ids[s.key];
            return (
              <span
                key={s.key}
                title={id ? `${s.label} ${id}` : `No ${s.label} id`}
                className={cn(
                  "inline-block h-2.5 w-2.5 rounded-full",
                  id ? "bg-success" : "bg-muted-foreground/30",
                )}
              />
            );
          })}
        </span>
      </td>
      <td className="text-muted-foreground py-1.5 pr-3 text-xs">
        {TOOL_LABELS[row.tagged_by] ?? row.tagged_by}
      </td>
      <td className="text-muted-foreground py-1.5 pr-3 text-xs">
        {row.last_run ? (
          <>
            {formatRelativeDate(row.last_run.started_at) ??
              row.last_run.started_at}
            {row.last_run.applied > 0
              ? " · applied"
              : row.last_run.matched_high > 0
                ? ` · ${row.last_run.matched_high} strong, not applied`
                : ` · ${row.last_run.status}`}
          </>
        ) : (
          "never"
        )}
      </td>
      <td className="py-1.5">
        <Link
          href={`${issueUrl(seriesSlug, row.slug)}?tab=metadata`}
          className="text-primary inline-flex items-center gap-0.5 text-xs hover:underline"
        >
          Open
          <ChevronRight className="h-3 w-3" />
        </Link>
      </td>
    </tr>
  );
}

// ───────── files + provenance ─────────

function FilesCard({
  overview,
}: {
  overview: SeriesMetadataOverview | undefined;
}) {
  if (!overview) return null;
  const f = overview.files;
  const ext = f.by_extension
    .map((e) => `${e.extension.toUpperCase()} ${e.count}`)
    .join(" · ");
  return (
    <Section title="Files and sidecars">
      <dl className="grid grid-cols-[140px_1fr] gap-x-4 gap-y-1.5 text-sm">
        <dt className="text-muted-foreground">Archives</dt>
        <dd className="tabular-nums">
          {f.archives}
          {ext ? <span className="text-muted-foreground"> · {ext}</span> : null}
        </dd>
        <dt className="text-muted-foreground">ComicInfo.xml</dt>
        <dd className="tabular-nums">
          {f.with_comicinfo} of {f.archives}
        </dd>
        <dt className="text-muted-foreground">MetronInfo.xml</dt>
        <dd className="tabular-nums">
          {f.with_metroninfo} of {f.archives}
        </dd>
        <dt className="text-muted-foreground">series.json / cvinfo</dt>
        <dd>
          {f.series_json_present ? "series.json" : "no series.json"}
          {" · "}
          {f.cvinfo_present ? "cvinfo" : "no cvinfo"}
        </dd>
        <dt className="text-muted-foreground">Backups</dt>
        <dd className="tabular-nums">
          {f.backups == null ? (
            <span className="text-muted-foreground">folder not readable</span>
          ) : (
            <>
              {f.backups}{" "}
              <span className="text-muted-foreground">.bak files</span>
            </>
          )}
        </dd>
        {f.folder_path ? (
          <>
            <dt className="text-muted-foreground">Folder</dt>
            <dd className="truncate font-mono text-xs" title={f.folder_path}>
              {f.folder_path}
            </dd>
          </>
        ) : null}
      </dl>
    </Section>
  );
}

function ProvenanceCard({
  overview,
}: {
  overview: SeriesMetadataOverview | undefined;
}) {
  if (!overview) return null;
  const rows = overview.series_provenance;
  const p = overview.issue_provenance;
  return (
    <Section
      title="Field provenance"
      description="Who set each series field; the issue page's Metadata tab has the same table per issue."
    >
      {rows.length === 0 ? (
        <p className="text-muted-foreground text-sm">
          No provenance recorded on the series yet.
        </p>
      ) : (
        <table className="w-full text-sm">
          <thead className="text-muted-foreground text-xs tracking-wide uppercase">
            <tr>
              <th className="py-1 text-left font-medium">Field</th>
              <th className="py-1 text-left font-medium">Source</th>
              <th className="py-1 text-left font-medium">When</th>
            </tr>
          </thead>
          <tbody className="divide-border/60 divide-y">
            {rows.map((r) => (
              <tr key={r.field}>
                <td className="py-1">{metadataFieldLabels([r.field])}</td>
                <td className="py-1">
                  <Badge variant="outline" className="text-[10px]">
                    {r.set_by}
                  </Badge>
                </td>
                <td className="text-muted-foreground py-1 text-xs">
                  {formatRelativeDate(r.set_at) ?? r.set_at}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <p className="text-muted-foreground text-xs tabular-nums">
        Issue fields: {p.file} issues carry file-tier values · {p.provider}{" "}
        provider-set · {p.user} pinned by you.
      </p>
    </Section>
  );
}
