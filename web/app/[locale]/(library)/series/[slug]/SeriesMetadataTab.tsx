"use client";

/**
 * Series page › **Metadata** tab — everything administrative about a
 * series' metadata, so the header, Issues and Collection stay reading
 * surfaces. Top to bottom:
 *
 *   1. a summary strip of four tiles, each a ratio or a state that no
 *      card below repeats: issue completeness, per-provider issue-id
 *      coverage, provider mapping, provider sync;
 *   2. the fetch actions — one primary split "Fetch metadata" (only
 *      missing or partial / all issues), Match this series, Refresh this
 *      series — plus the latest run;
 *   3. Provider series mapping, with each provider's series-level id in
 *      its header (`<SeriesProviderRangesCard>` — the old Identifiers card
 *      is folded into it: the id *is* the default mapping);
 *   4. what the folder holds, and field provenance, side by side;
 *   5. the audit table, last and bounded: every issue with its tier,
 *      missing core fields, the three provider ids and its latest run —
 *      server-side filters, row selection, the two bulk actions. It opens
 *      on the issues that need attention and scrolls inside the card, so
 *      a 600-issue run does not push the page to twenty screens.
 *
 * Covers stay on the Issues tab; this is the list you scan for "what is
 * still missing".
 */

import {
  ChevronDown,
  ChevronRight,
  ListChecks,
  Loader2,
  Pin,
  Search,
  Sparkles,
} from "lucide-react";
import dynamic from "next/dynamic";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import * as React from "react";
import { toast } from "sonner";

import { SeriesProviderRangesCard } from "@/components/library/SeriesProviderRangesCard";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { CopyButton } from "@/components/ui/copy-button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { EmptyState } from "@/components/ui/empty-state";
import { FilterPill } from "@/components/ui/filter-pill";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  useAcceptSeriesIssues,
  useCreateSeriesBatch,
  useCreateSeriesSelectionBatch,
} from "@/lib/api/mutations";
import {
  useMe,
  useProviderCoverageAnalysis,
  useProviderCoverageSeries,
  useSeriesMetadataIssues,
  useSeriesMetadataOverview,
} from "@/lib/api/queries";
import type { SeriesMetadataIssueFilters } from "@/lib/api/query-keys";
import type {
  CoverageAnalysisResp,
  ProviderCoverageResp,
  SeriesMetadataIssueRow,
  SeriesMetadataOverview,
} from "@/lib/api/types";
import { analysisAttention } from "@/lib/coverage-ranges";
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
  { key: "comicvine", label: "ComicVine", short: "CV" },
  { key: "metron", label: "Metron", short: "M" },
  { key: "gcd", label: "GCD", short: "GCD" },
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

/** Who set a field, grouped for the provenance bar and badges. */
type ProvenanceClass = "user" | "provider" | "file";
const SET_BY_LABELS: Record<string, string> = {
  user: "You",
  comicvine: "ComicVine",
  metron: "Metron",
  gcd: "GCD",
  cross_reference: "Cross-reference",
  comicinfo: "ComicInfo",
  metroninfo: "MetronInfo",
  series_json: "series.json",
  scanner_inference: "Scanner",
};
function provenanceClass(setBy: string): ProvenanceClass {
  if (setBy === "user") return "user";
  if (
    setBy === "comicvine" ||
    setBy === "metron" ||
    setBy === "gcd" ||
    setBy === "cross_reference"
  ) {
    return "provider";
  }
  return "file";
}
const PROVENANCE_SWATCH: Record<ProvenanceClass, string> = {
  user: "bg-primary",
  provider: "bg-sky-500",
  file: "bg-muted-foreground/50",
};
const PROVENANCE_BADGE: Record<ProvenanceClass, string> = {
  user: "border-primary bg-primary/10 text-primary",
  provider: "border-transparent bg-secondary text-secondary-foreground",
  file: "border-border text-muted-foreground",
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
    <div className="space-y-6">
      <SummaryStrip
        seriesSlug={seriesSlug}
        overview={overview.data}
        loading={overview.isLoading}
      />
      <Actions
        seriesSlug={seriesSlug}
        seriesName={seriesName}
        libraryId={libraryId}
        overview={overview.data}
      />
      <SeriesProviderRangesCard seriesSlug={seriesSlug} />
      <div className="grid grid-cols-1 items-stretch gap-4 lg:grid-cols-2">
        <FilesCard overview={overview.data} />
        <ProvenanceCard overview={overview.data} />
      </div>
      <IssuesTable seriesSlug={seriesSlug} overview={overview.data} />
    </div>
  );
}

// ───────── summary strip ─────────

function SummaryStrip({
  seriesSlug,
  overview,
  loading,
}: {
  seriesSlug: string;
  overview: SeriesMetadataOverview | undefined;
  loading: boolean;
}) {
  const me = useMe();
  const isAdmin = me.data?.role === "admin";
  // Same query keys as the mapping card, so these are free once it loads.
  const coverage = useProviderCoverageSeries(seriesSlug);
  const analysis = useProviderCoverageAnalysis(seriesSlug, {
    enabled: isAdmin,
  });
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
  const pct = (n: number) => `${Math.min(100, (n / total) * 100)}%`;
  return (
    <section
      className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4"
      data-testid="series-metadata-summary"
    >
      <Tile label="Issue completeness">
        <div className="text-xl font-semibold tabular-nums">
          {t.complete}{" "}
          <span className="text-muted-foreground text-sm font-normal">
            of {overview.issue_ids.total} complete
          </span>
        </div>
        <div
          className="bg-secondary flex h-1.5 w-full overflow-hidden rounded-full"
          role="img"
          aria-label={`${t.complete} complete, ${t.partial} partial, ${t.needs_metadata} need metadata, ${t.accepted} accepted`}
        >
          <div className="bg-success" style={{ width: pct(t.complete) }} />
          <div className="bg-warning" style={{ width: pct(t.partial) }} />
          <div
            className="bg-destructive"
            style={{ width: pct(t.needs_metadata) }}
          />
          <div
            className="bg-muted-foreground/50"
            style={{ width: pct(t.accepted) }}
          />
        </div>
        <Legend
          items={[
            { swatch: "bg-success", label: "Complete", count: t.complete },
            { swatch: "bg-warning", label: "Partial", count: t.partial },
            {
              swatch: "bg-destructive",
              label: "Needs metadata",
              count: t.needs_metadata,
            },
            ...(t.accepted > 0
              ? [
                  {
                    swatch: "bg-muted-foreground/50",
                    label: "Accepted",
                    count: t.accepted,
                  },
                ]
              : []),
          ]}
        />
      </Tile>
      <Tile label="Issue identifiers">
        {SOURCES.map((s) => (
          <MiniBar
            key={s.key}
            label={s.label}
            value={overview.issue_ids[s.key]}
            total={overview.issue_ids.total}
          />
        ))}
        <div className="text-muted-foreground text-xs tabular-nums">
          All three: {overview.issue_ids.all_sources}
        </div>
      </Tile>
      <MappingTile
        overview={overview}
        coverage={coverage.data}
        analysis={analysis.data ?? null}
      />
      <Tile label="Provider sync">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-base font-semibold">
            {overview.sync.series_last_sync_at
              ? (formatRelativeDate(overview.sync.series_last_sync_at) ??
                overview.sync.series_last_sync_at)
              : "Never synced"}
          </span>
          {overview.sync.metadata_sync_paused ? (
            <Badge
              variant="outline"
              className="border-warning/40 bg-warning/10 text-warning font-medium"
            >
              paused
            </Badge>
          ) : null}
        </div>
        <div className="text-muted-foreground text-xs tabular-nums">
          {overview.sync.issues_synced} issue
          {overview.sync.issues_synced === 1 ? "" : "s"} ever fetched
        </div>
        <div className="text-muted-foreground text-xs tabular-nums">
          {overview.sync.sidecars_rewritten} sidecar
          {overview.sync.sidecars_rewritten === 1 ? "" : "s"} rewritten
          {overview.sync.last_rewrite_at
            ? ` · ${formatRelativeDate(overview.sync.last_rewrite_at) ?? ""}`
            : ""}
        </div>
      </Tile>
    </section>
  );
}

function MappingTile({
  overview,
  coverage,
  analysis,
}: {
  overview: SeriesMetadataOverview;
  coverage: ProviderCoverageResp | undefined;
  analysis: CoverageAnalysisResp | null;
}) {
  const linked = new Set(overview.series_ids.map((r) => r.source));
  const mapped = SOURCES.filter((s) => linked.has(s.key)).length;
  const overrideSources = (coverage?.providers ?? [])
    .filter((p) => p.segments.some((seg) => seg.via_range))
    .map((p) => p.source_label);
  const overrides = (coverage?.providers ?? []).reduce(
    (n, p) =>
      n +
      new Set(p.segments.filter((s) => s.via_range).map((s) => s.range_id))
        .size,
    0,
  );
  const attention = analysis ? analysisAttention(analysis) : null;
  const analysed =
    analysis && analysis.finished_at
      ? (formatRelativeDate(analysis.finished_at) ?? analysis.finished_at)
      : null;
  return (
    <Tile label="Provider mapping">
      <div className="text-xl font-semibold tabular-nums">
        {mapped}{" "}
        <span className="text-muted-foreground text-sm font-normal">
          of {SOURCES.length} providers mapped
        </span>
      </div>
      <div className="text-muted-foreground text-xs tabular-nums">
        {overrides === 0
          ? "No range overrides"
          : `${overrides} range override${overrides === 1 ? "" : "s"} · ${overrideSources.join(", ")}`}
      </div>
      <div className="text-muted-foreground text-xs">
        {attention && analysis
          ? analysis.state === "queued" || analysis.state === "running"
            ? "Coverage analysis running…"
            : analysis.state === "failed"
              ? "Last coverage analysis failed"
              : `Coverage analyzed ${analysed ?? ""} · ${attention.needsAttention ? "review pending" : "nothing pending"}`
          : "Coverage not analyzed yet"}
      </div>
    </Tile>
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
    <Card className="flex min-w-0 flex-col gap-2 p-4">
      <div className="text-muted-foreground text-[11px] font-semibold tracking-wide uppercase">
        {label}
      </div>
      {children}
    </Card>
  );
}

function Legend({
  items,
}: {
  items: { swatch: string; label: string; count: number }[];
}) {
  return (
    <div className="text-muted-foreground flex flex-wrap gap-x-3 gap-y-1 text-xs tabular-nums">
      {items.map((it) => (
        <span key={it.label} className="inline-flex items-center gap-1.5">
          <span
            aria-hidden
            className={cn("inline-block h-2 w-2 rounded-sm", it.swatch)}
          />
          {it.label} {it.count}
        </span>
      ))}
    </div>
  );
}

/** `label ▬▬▬▬▬▬ n / total` — one row of a per-provider coverage tile. */
function MiniBar({
  label,
  value,
  total,
}: {
  label: string;
  value: number;
  total: number;
}) {
  const pct = total > 0 ? Math.min(100, (value / total) * 100) : 0;
  return (
    <div
      className="grid grid-cols-[76px_1fr_auto] items-center gap-2 text-xs"
      role="img"
      aria-label={`${label}: ${value} of ${total}`}
    >
      <span>{label}</span>
      <div className="bg-secondary h-1.5 overflow-hidden rounded-full">
        <div className="bg-success h-full" style={{ width: `${pct}%` }} />
      </div>
      <span className="text-muted-foreground tabular-nums">
        {value} / {total}
      </span>
    </div>
  );
}

/** `label ▬▬▬▬ n of m` — a ratio row in the Files card. */
function RatioRow({ value, total }: { value: number; total: number }) {
  const pct = total > 0 ? Math.min(100, (value / total) * 100) : 0;
  return (
    <span className="flex items-center gap-2">
      <span
        className="bg-secondary h-1.5 w-full max-w-50 overflow-hidden rounded-full"
        role="img"
        aria-label={`${value} of ${total}`}
      >
        <span
          className={cn(
            "block h-full",
            value === total ? "bg-success" : "bg-warning",
          )}
          style={{ width: `${pct}%` }}
        />
      </span>
      <span className="text-muted-foreground text-xs tabular-nums">
        {value} of {total}
      </span>
    </span>
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
      {/* Split button: the primary face runs the common scope (only what
          is missing or partial); the chevron offers both scopes. */}
      <div className="inline-flex" role="group" aria-label="Fetch metadata">
        <Button
          type="button"
          size="sm"
          className="rounded-r-none"
          disabled={createBatch.isPending || incomplete === 0}
          onClick={() => fetchMetadata("incomplete")}
        >
          <Sparkles className="mr-1.5 h-4 w-4" /> Fetch metadata
          {incomplete != null && incomplete > 0 ? (
            <span className="ml-1 tabular-nums opacity-80">({incomplete})</span>
          ) : null}
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              size="sm"
              className="border-primary-foreground/20 rounded-l-none border-l px-2"
              disabled={createBatch.isPending}
              aria-label="Fetch metadata options"
            >
              <ChevronDown className="h-4 w-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            <DropdownMenuItem
              disabled={incomplete === 0}
              onSelect={() => fetchMetadata("incomplete")}
            >
              Only missing or partial
              {incomplete != null ? ` (${incomplete})` : ""}
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => fetchMetadata("all")}>
              All issues
              {overview ? ` (${overview.issue_ids.total})` : ""}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => {
          setMatchMounted(true);
          setMatchOpen(true);
        }}
      >
        <Search className="mr-1.5 h-4 w-4" /> Match this series…
      </Button>
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
      {overview ? (
        <span className="text-muted-foreground text-xs sm:ml-auto">
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

// ───────── card chrome ─────────

function SectionCard({
  title,
  description,
  action,
  children,
  className,
  testId,
}: {
  title: string;
  description?: string;
  action?: React.ReactNode;
  children: React.ReactNode;
  className?: string;
  testId?: string;
}) {
  return (
    <Card className={cn("flex flex-col", className)} data-testid={testId}>
      <CardHeader className="flex flex-col gap-3 space-y-0 pb-3 sm:flex-row sm:items-start sm:justify-between">
        <div className="space-y-1">
          <CardTitle className="text-sm font-semibold">{title}</CardTitle>
          {description ? (
            <CardDescription className="text-xs">{description}</CardDescription>
          ) : null}
        </div>
        {action ? <div className="flex shrink-0 gap-2">{action}</div> : null}
      </CardHeader>
      <CardContent className="flex flex-1 flex-col gap-3 text-sm">
        {children}
      </CardContent>
    </Card>
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
  return (
    <SectionCard
      title="Files and sidecars"
      description="What the folder holds, and which archives carry each sidecar."
      testId="series-metadata-files"
    >
      <dl className="grid grid-cols-[110px_1fr] items-center gap-x-4 gap-y-2 text-sm sm:grid-cols-[132px_1fr]">
        <dt className="text-muted-foreground">Archives</dt>
        <dd className="flex flex-wrap items-center gap-1.5 tabular-nums">
          {f.archives}
          {f.by_extension.map((e) => (
            <Badge
              key={e.extension}
              variant="secondary"
              className="font-normal tabular-nums"
            >
              {e.extension.toUpperCase()} {e.count}
            </Badge>
          ))}
        </dd>
        <dt className="text-muted-foreground">ComicInfo.xml</dt>
        <dd>
          <RatioRow value={f.with_comicinfo} total={f.archives} />
        </dd>
        <dt className="text-muted-foreground">MetronInfo.xml</dt>
        <dd>
          <RatioRow value={f.with_metroninfo} total={f.archives} />
        </dd>
        <dt className="text-muted-foreground">Tagged by</dt>
        <dd className="flex flex-wrap items-center gap-1.5">
          {f.tagged_by.length === 0 ? (
            <span className="text-muted-foreground">—</span>
          ) : (
            f.tagged_by.map((t) => (
              <Badge
                key={t.tool}
                variant="secondary"
                className="font-normal tabular-nums"
              >
                {t.label} {t.count}
              </Badge>
            ))
          )}
        </dd>
        <dt className="text-muted-foreground">Folder files</dt>
        <dd className="flex flex-wrap items-center gap-1.5">
          <FileBadge present={f.series_json_present} name="series.json" />
          <FileBadge present={f.cvinfo_present} name="cvinfo" />
          {f.backups == null ? (
            <Badge
              variant="outline"
              className="text-muted-foreground font-normal"
            >
              folder not readable
            </Badge>
          ) : (
            <Badge
              variant="outline"
              className="text-muted-foreground font-normal tabular-nums"
            >
              {f.backups} .bak
            </Badge>
          )}
        </dd>
        <dt className="text-muted-foreground">Last scan</dt>
        <dd className="text-muted-foreground text-xs">
          {f.last_scanned_at
            ? (formatRelativeDate(f.last_scanned_at) ?? f.last_scanned_at)
            : "never"}
        </dd>
        {f.folder_path ? (
          <>
            <dt className="text-muted-foreground self-start pt-0.5">Folder</dt>
            <dd className="flex items-start gap-1">
              <span className="min-w-0 font-mono text-xs leading-relaxed break-all">
                {f.folder_path}
              </span>
              <CopyButton
                value={f.folder_path}
                label=""
                variant="ghost"
                size="icon"
                className="text-muted-foreground/60 hover:text-foreground h-6 w-6 shrink-0"
                aria-label="Copy folder path"
              />
            </dd>
          </>
        ) : null}
      </dl>
    </SectionCard>
  );
}

function FileBadge({ present, name }: { present: boolean; name: string }) {
  return (
    <Badge
      variant="outline"
      className={cn(
        "font-normal",
        present
          ? "border-success/30 bg-success/10 text-success"
          : "text-muted-foreground",
      )}
    >
      {present ? name : `no ${name}`}
    </Badge>
  );
}

function ProvenanceCard({
  overview,
}: {
  overview: SeriesMetadataOverview | undefined;
}) {
  const [pinnedOnly, setPinnedOnly] = React.useState(false);
  if (!overview) return null;
  const rows = overview.series_provenance;
  const p = overview.issue_provenance;
  // Count by set_by for the bar + legend (user first, then providers,
  // then file tiers), each group keeping the API's ordering of sources.
  const counts = new Map<string, number>();
  for (const r of rows) counts.set(r.set_by, (counts.get(r.set_by) ?? 0) + 1);
  const order = (k: string) =>
    ({ user: 0, provider: 1, file: 2 })[provenanceClass(k)];
  const groups = Array.from(counts.entries()).sort(
    (a, b) => order(a[0]) - order(b[0]) || b[1] - a[1],
  );
  const shown = pinnedOnly ? rows.filter((r) => r.set_by === "user") : rows;
  const pinned = counts.get("user") ?? 0;
  return (
    <SectionCard
      title="Field provenance"
      description="Who set each series field. The issue page's Metadata tab has the same table per issue."
      testId="series-metadata-provenance"
      action={
        pinned > 0 ? (
          <FilterPill
            active={pinnedOnly}
            count={pinned}
            onClick={() => setPinnedOnly((v) => !v)}
          >
            <Pin className="h-3 w-3" aria-hidden /> Pinned by you
          </FilterPill>
        ) : null
      }
    >
      {rows.length === 0 ? (
        <EmptyState
          size="sm"
          description="No provenance recorded on the series yet. Its values were written by the scanner from the folder."
        />
      ) : (
        <>
          <div>
            <div
              className="bg-secondary flex h-1.5 w-full overflow-hidden rounded-full"
              role="img"
              aria-label={groups
                .map(([k, n]) => `${n} ${SET_BY_LABELS[k] ?? k}`)
                .join(", ")}
            >
              {groups.map(([k, n]) => (
                <div
                  key={k}
                  className={cn(
                    "border-background border-l first:border-l-0",
                    PROVENANCE_SWATCH[provenanceClass(k)],
                  )}
                  style={{ width: `${(n / rows.length) * 100}%` }}
                />
              ))}
            </div>
            <div className="mt-2">
              <Legend
                items={groups.map(([k, n]) => ({
                  swatch: PROVENANCE_SWATCH[provenanceClass(k)],
                  label: SET_BY_LABELS[k] ?? k,
                  count: n,
                }))}
              />
            </div>
          </div>
          <ScrollArea
            className="border-border/60 rounded-md border"
            viewportClassName="max-h-72"
            viewportProps={{ tabIndex: 0 }}
          >
            {/* `<Table>` wraps itself in an `overflow-auto` div, which would
                become the sticky header's scroll container instead of the
                viewport above; let it overflow so the viewport scrolls. */}
            <div className="[&>div]:overflow-visible">
              <Table className="text-xs">
                <TableHeader className="bg-card sticky top-0 z-10">
                  <TableRow>
                    <TableHead className="h-8 text-[11px]">Field</TableHead>
                    <TableHead className="h-8 text-[11px]">Set by</TableHead>
                    <TableHead className="h-8 text-[11px]">When</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {shown.map((r) => (
                    <TableRow key={r.field}>
                      <TableCell className="py-1.5">
                        {metadataFieldLabels([r.field])}
                      </TableCell>
                      <TableCell className="py-1.5">
                        <Badge
                          variant="outline"
                          className={cn(
                            "font-normal",
                            PROVENANCE_BADGE[provenanceClass(r.set_by)],
                          )}
                          title={r.set_by}
                        >
                          {SET_BY_LABELS[r.set_by] ?? r.set_by}
                        </Badge>
                      </TableCell>
                      <TableCell className="text-muted-foreground py-1.5">
                        {formatRelativeDate(r.set_at) ?? r.set_at}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          </ScrollArea>
        </>
      )}
      <div className="text-muted-foreground mt-auto flex flex-wrap justify-between gap-x-4 gap-y-1 text-xs tabular-nums">
        <span>
          {rows.length} series field{rows.length === 1 ? "" : "s"}
          {pinnedOnly ? ` · ${shown.length} pinned` : ""}
        </span>
        <span>
          Issue fields: {p.file} file · {p.provider} provider · {p.user} pinned
        </span>
      </div>
    </SectionCard>
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
  // Opens on what needs attention; "All" is one tap away.
  const [tier, setTier] = React.useState<TierFilter | undefined>("incomplete");
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
  const incomplete = t ? t.partial + t.needs_metadata : undefined;
  const chips: {
    key: string;
    label: string;
    count?: number;
    active: boolean;
    onClick: () => void;
    tone?: "destructive";
  }[] = [
    {
      key: "needs",
      label: "Needs attention",
      count: incomplete,
      active: tier === "incomplete",
      tone: incomplete != null && incomplete > 0 ? "destructive" : undefined,
      onClick: () => setTier(tier === "incomplete" ? undefined : "incomplete"),
    },
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
  const onlyAttentionFilter =
    tier === "incomplete" && without == null && !specials;
  return (
    <SectionCard
      title="Issue audit"
      description="Every issue with what is still missing. Opens on the issues that need attention; covers stay on the Issues tab."
      testId="series-metadata-audit"
    >
      <div
        className="flex flex-wrap items-center gap-1.5"
        role="group"
        aria-label="Filters"
      >
        {chips.map((c) => (
          <FilterPill
            key={c.key}
            active={c.active}
            count={c.count}
            onClick={c.onClick}
            className={cn(
              "h-7",
              !c.active && c.tone === "destructive" && "text-destructive",
            )}
          >
            {c.label}
          </FilterPill>
        ))}
      </div>

      {selectedRows.length > 0 ? (
        <div
          className="bg-secondary/60 border-border flex flex-wrap items-center gap-2 rounded-md border px-3 py-2 text-xs"
          role="status"
          data-testid="series-metadata-selection"
        >
          <span className="font-semibold tabular-nums">
            {selectedRows.length} selected
          </span>
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={selectionBatch.isPending}
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
              Mark complete
            </Button>
          ) : null}
          {selectedRows.every((r) => r.accepted) ? (
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
          <Button
            type="button"
            size="sm"
            variant="ghost"
            className="ml-auto"
            onClick={() => selection.clear()}
          >
            Clear
          </Button>
        </div>
      ) : null}

      {q.isLoading && !q.data ? (
        <div className="text-muted-foreground flex items-center gap-2 py-4 text-sm">
          <Loader2 className="h-4 w-4 animate-spin" /> Loading issues…
        </div>
      ) : items.length === 0 ? (
        onlyAttentionFilter && q.data ? (
          <EmptyState
            size="sm"
            icon={ListChecks}
            title="Every issue has complete metadata"
            description={`${q.data.series_total} issue${q.data.series_total === 1 ? "" : "s"}, nothing missing.`}
            action={
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => setTier(undefined)}
              >
                Show all issues
              </Button>
            }
          />
        ) : (
          <p className="text-muted-foreground py-4 text-sm">
            No issues match these filters.
          </p>
        )
      ) : (
        <ScrollArea
          className="border-border/60 rounded-md border"
          viewportClassName="max-h-[60vh]"
          viewportProps={{ tabIndex: 0 }}
          horizontal
        >
          <TooltipProvider delayDuration={200}>
            {/* See the provenance table: the Table wrapper must not scroll. */}
            <div className="[&>div]:overflow-visible">
              <Table
                className="min-w-210 text-sm"
                data-testid="series-metadata-issues"
              >
                <TableHeader className="bg-card sticky top-0 z-10">
                  <TableRow>
                    <TableHead className="h-9 w-10">
                      <Checkbox
                        aria-label="Select all shown"
                        checked={allSelected}
                        onCheckedChange={() =>
                          allSelected
                            ? selection.clear()
                            : selection.selectAll()
                        }
                      />
                    </TableHead>
                    <TableHead className="h-9">Issue</TableHead>
                    <TableHead className="h-9">Cover date</TableHead>
                    <TableHead className="h-9">Tier</TableHead>
                    <TableHead className="h-9">Missing core</TableHead>
                    <TableHead className="h-9">Provider ids</TableHead>
                    <TableHead className="h-9">Last fetched</TableHead>
                    <TableHead className="h-9" />
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {items.map((r) => (
                    <IssueRow
                      key={r.id}
                      row={r}
                      seriesSlug={seriesSlug}
                      selected={selection.isSelected(r.id)}
                      onToggle={(ev) => selection.toggle(r.id, ev)}
                    />
                  ))}
                </TableBody>
              </Table>
            </div>
          </TooltipProvider>
        </ScrollArea>
      )}

      {q.data && items.length > 0 ? (
        <div className="text-muted-foreground text-xs tabular-nums">
          {q.data.total} of {q.data.series_total} shown
          {q.data.total > 12 ? " · scroll for more" : ""}
        </div>
      ) : null}
    </SectionCard>
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
    <TableRow
      data-testid={`issue-row-${row.slug}`}
      data-state={selected ? "selected" : undefined}
      className={cn(selected && "bg-secondary/40")}
    >
      <TableCell className="py-1.5">
        <Checkbox
          aria-label={`Select ${row.label}`}
          checked={selected}
          onCheckedChange={() => onToggle({})}
          onClick={(e) => {
            if (e.shiftKey) {
              e.preventDefault();
              onToggle({ shiftKey: true });
            }
          }}
        />
      </TableCell>
      <TableCell className="py-1.5">
        <Link href={issueUrl(seriesSlug, row.slug)} className="hover:underline">
          {row.label}
        </Link>
        {row.title ? (
          <span className="text-muted-foreground ml-2 text-xs">
            {row.title}
          </span>
        ) : null}
      </TableCell>
      <TableCell className="py-1.5 tabular-nums">
        {row.cover_date ?? <span className="text-muted-foreground">none</span>}
      </TableCell>
      <TableCell className="py-1.5">
        <span
          className={cn(
            "inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-medium whitespace-nowrap",
            TIER_CLASSES[tierKey] ?? TIER_CLASSES.partial,
          )}
        >
          {TIER_LABELS[tierKey] ?? tierKey}
        </span>
      </TableCell>
      <TableCell className="py-1.5 text-xs">
        {row.missing_core.length > 0 ? (
          metadataFieldLabels(row.missing_core)
        ) : (
          <span className="text-muted-foreground">—</span>
        )}
      </TableCell>
      <TableCell className="py-1.5">
        <span className="inline-flex items-center gap-1">
          {SOURCES.map((s) => {
            const id = row.ids[s.key];
            return (
              <Tooltip key={s.key}>
                <TooltipTrigger asChild>
                  <span
                    tabIndex={0}
                    className={cn(
                      "inline-flex h-4.5 min-w-6.5 items-center justify-center rounded border px-1 text-[10px] font-semibold tracking-wide",
                      id
                        ? "border-success/40 bg-success/10 text-success"
                        : "border-border/60 text-muted-foreground/60",
                    )}
                  >
                    {s.short}
                  </span>
                </TooltipTrigger>
                <TooltipContent>
                  {id ? `${s.label} ${id}` : `No ${s.label} id`}
                </TooltipContent>
              </Tooltip>
            );
          })}
        </span>
      </TableCell>
      <TableCell className="text-muted-foreground py-1.5 text-xs whitespace-nowrap">
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
      </TableCell>
      <TableCell className="py-1.5">
        <Link
          href={`${issueUrl(seriesSlug, row.slug)}?tab=metadata`}
          className="text-primary inline-flex items-center gap-0.5 text-xs whitespace-nowrap hover:underline"
        >
          Open
          <ChevronRight className="h-3 w-3" />
        </Link>
      </TableCell>
    </TableRow>
  );
}
