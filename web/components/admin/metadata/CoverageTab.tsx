"use client";

/**
 * `<CoverageTab>` — /admin/metadata › Coverage.
 *
 * Visibility only: how much of the library carries provider identifiers
 * (per provider, series and issues), how complete the issue metadata is
 * (the Collection grid's tiers, library-wide), where the gaps sit, which
 * tagging tool wrote the files' sidecars, the state of the tools that
 * close the gap over time, the live provider budgets, and what closing
 * each gap costs by route. Nothing here starts work: every count links to
 * the surface that does (the library grid's `provider_match` /
 * `metadata_completeness` facets, a series' Metadata tab, the settings).
 *
 * The snapshot is cached server-side for six hours; Recompute asks for a
 * fresh one.
 */

import { Loader2, RefreshCw } from "lucide-react";
import Link from "next/link";
import { useState, type ReactNode } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useAdminMetadataCoverage } from "@/lib/api/queries";
import type {
  FieldCount,
  FillCost,
  MetadataCoverageResp,
  TagSourceRow,
} from "@/lib/api/types";
import { formatRelativeDate } from "@/lib/format";

import { ProviderQuotaRow } from "./DashboardTab";

const FIELD_LABELS: Record<string, string> = {
  external_id: "Provider id",
  summary: "Summary",
  credits: "Any credit",
  cover_date: "Cover date",
  page_count: "Page count",
  title: "Title",
  teams: "Teams",
  genres: "Genre",
  story_arcs: "Story arc",
  cover_primary: "Cover",
};

const TOOL_LABELS: Record<string, string> = {
  folio: "Folio",
  comictagger_comicvine: "ComicTagger · ComicVine",
  comictagger_metron: "ComicTagger · Metron",
  comictagger_gcd: "ComicTagger · GCD",
  comictagger_other: "ComicTagger · other",
  mylar: "Mylar",
  scraper: "Other scrapers",
  untagged: "No tags",
};

/** The provider a tagging tool's notes name — its ids should have been
 *  read from the file; zero captured means the parser missed them. */
const TOOL_OWN_SOURCE: Record<string, "comicvine" | "metron" | "gcd"> = {
  comictagger_comicvine: "comicvine",
  comictagger_metron: "metron",
  comictagger_gcd: "gcd",
  mylar: "comicvine",
};

export function CoverageTab() {
  const [refresh, setRefresh] = useState(false);
  const q = useAdminMetadataCoverage(refresh);
  if (q.isLoading) {
    return (
      <div className="text-muted-foreground flex items-center gap-2 py-6 text-sm">
        <Loader2 className="h-4 w-4 animate-spin" /> Loading…
      </div>
    );
  }
  if (!q.data) {
    return null;
  }
  const d = q.data;
  return (
    <div className="space-y-4">
      <div className="text-muted-foreground flex flex-wrap items-center gap-x-4 gap-y-1 text-sm">
        <span>
          <Num n={d.series_total} /> series · <Num n={d.issues_total} /> issues
          · <Num n={d.libraries_total} /> libraries
        </span>
        <span>
          Computed {formatRelativeDate(d.computed_at) ?? d.computed_at}
          {d.cached ? " (cached)" : ""}
        </span>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-7 gap-1 px-2"
          onClick={() => {
            setRefresh(true);
            void q.refetch();
          }}
          disabled={q.isFetching}
        >
          <RefreshCw
            className={`h-3.5 w-3.5 ${q.isFetching ? "animate-spin" : ""}`}
          />
          Recompute
        </Button>
      </div>

      <IdentifierCoverageCard d={d} />
      <CompletenessCard d={d} />
      <div className="grid gap-4 lg:grid-cols-2">
        <LibraryGapsCard d={d} />
        <TopSeriesCard d={d} />
      </div>
      <TagSourcesCard rows={d.tag_sources} />
      <ToolsCard d={d} />
    </div>
  );
}

// ───────── identifier coverage ─────────

function IdentifierCoverageCard({ d }: { d: MetadataCoverageResp }) {
  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">
          Identifier coverage
        </CardTitle>
        <p className="text-muted-foreground text-xs">
          Which provider ids exist, at the series and issue level. Each series
          count links to the library grid filtered to the gap.
        </p>
      </CardHeader>
      <CardContent className="space-y-3">
        <div className="text-muted-foreground hidden grid-cols-[140px_1fr_1fr_minmax(0,1.2fr)] gap-4 text-xs tracking-wide uppercase md:grid">
          <span>Provider</span>
          <span>Series</span>
          <span>Issues</span>
          <span>Fill with</span>
        </div>
        {d.identifiers.map((row) => {
          const seriesMissing = Math.max(
            d.series_total - row.series_with_id,
            0,
          );
          const issuesMissing = Math.max(
            d.issues_total - row.issues_with_id,
            0,
          );
          return (
            <div
              key={row.source}
              className="border-border/60 grid gap-3 border-t py-3 md:grid-cols-[140px_1fr_1fr_minmax(0,1.2fr)] md:gap-4"
              data-testid={`identifier-${row.source}`}
            >
              <div className="font-medium">{row.label}</div>
              <Measure
                value={row.series_with_id}
                total={d.series_total}
                link={
                  seriesMissing > 0
                    ? {
                        href: `/?library=all&provider_match=unmatched&provider_source=${row.source}`,
                        label: `${seriesMissing.toLocaleString()} unmatched`,
                      }
                    : undefined
                }
              />
              <Measure
                value={row.issues_with_id}
                total={d.issues_total}
                link={
                  issuesMissing > 0
                    ? {
                        label: `${issuesMissing.toLocaleString()} without id`,
                      }
                    : undefined
                }
              />
              <div className="text-muted-foreground text-xs">
                {fillHint(row.source)}
              </div>
            </div>
          );
        })}
        <div className="text-muted-foreground border-border/60 flex flex-wrap gap-x-6 gap-y-1 border-t pt-3 text-xs">
          <span>
            All three ids: <Num n={d.all_sources_series} /> series ·{" "}
            <Num n={d.all_sources_issues} /> issues
          </span>
        </div>
      </CardContent>
    </Card>
  );
}

function fillHint(source: string): string {
  switch (source) {
    case "comicvine":
      return "The scanner reads ids from the files' tags; the rest comes from Refresh this series on the series' Metadata tab.";
    case "metron":
      return "Analyze coverage on each series (the series id and any ranges); issue ids come from the per-issue fetch.";
    default:
      return "Same as Metron. GCD is cover-less, so issue matches need the series id first.";
  }
}

// ───────── completeness ─────────

function CompletenessCard({ d }: { d: MetadataCoverageResp }) {
  const t = d.tiers;
  const total = Math.max(d.issues_total, 1);
  const incomplete = t.partial + t.needs_metadata;
  const pct = (n: number) => Math.round((n / total) * 1000) / 10;
  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">
          Issue completeness
        </CardTitle>
        <p className="text-muted-foreground text-xs">
          The same tiers as a series&rsquo; Collection grid: matched to a
          provider, with a cover date, summary, page count and a credit.
        </p>
      </CardHeader>
      <CardContent className="space-y-4">
        <div
          className="bg-secondary flex h-3 w-full overflow-hidden rounded-full"
          role="img"
          aria-label={`Complete ${pct(t.complete)} percent, partial ${pct(t.partial)} percent, needs metadata ${pct(t.needs_metadata)} percent`}
        >
          <div
            className="bg-emerald-600 dark:bg-emerald-500"
            style={{ width: `${pct(t.complete)}%` }}
          />
          <div
            className="bg-amber-500"
            style={{ width: `${pct(t.partial)}%` }}
          />
          <div
            className="bg-red-500 dark:bg-red-400"
            style={{ width: `${pct(t.needs_metadata)}%` }}
          />
        </div>
        <div className="flex flex-wrap items-center gap-x-6 gap-y-2 text-sm">
          <TierPill
            tone="complete"
            label="Complete"
            n={t.complete}
            pct={pct(t.complete)}
          />
          <TierPill
            tone="partial"
            label="Partial"
            n={t.partial}
            pct={pct(t.partial)}
            hint="has an id, gaps remain"
          />
          <TierPill
            tone="needs"
            label="Needs metadata"
            n={t.needs_metadata}
            pct={pct(t.needs_metadata)}
            hint="no provider id or mostly empty"
          />
          <TierPill
            tone="accepted"
            label="Accepted"
            n={t.accepted}
            pct={pct(t.accepted)}
            hint="marked complete by hand"
          />
          <Link
            href="/?library=all&metadata_completeness=needs_metadata"
            className="text-primary ml-auto text-xs hover:underline"
          >
            Open the needs-metadata worklist →
          </Link>
        </div>
        <div className="grid gap-6 md:grid-cols-2">
          <FieldBars
            title={`Core fields missing, among the ${incomplete.toLocaleString()} incomplete`}
            rows={d.missing_core}
            total={incomplete}
            tone="warn"
          />
          <FieldBars
            title="Recommended fields present, whole library"
            rows={d.recommended}
            total={d.issues_total}
            tone="ok"
            asPercent
          />
        </div>
        <p className="text-muted-foreground text-xs">
          Series: summary on <Num n={d.series_fields.with_summary} /> of{" "}
          <Num n={d.series_total} /> · series type on{" "}
          <Num n={d.series_fields.with_series_type} /> · ever synced with a
          provider <Num n={d.series_fields.synced} />.
        </p>
      </CardContent>
    </Card>
  );
}

function TierPill({
  tone,
  label,
  n,
  pct,
  hint,
}: {
  tone: "complete" | "partial" | "needs" | "accepted";
  label: string;
  n: number;
  pct: number;
  hint?: string;
}) {
  const dot = {
    complete: "bg-emerald-600 dark:bg-emerald-500",
    partial: "bg-amber-500",
    needs: "bg-red-500 dark:bg-red-400",
    accepted: "bg-muted-foreground/60",
  }[tone];
  return (
    <span className="inline-flex items-center gap-2">
      <span className={`inline-block h-2.5 w-2.5 rounded-full ${dot}`} />
      <span className="font-medium">{label}</span>
      <Num n={n} />
      <span className="text-muted-foreground text-xs">
        {pct}%{hint ? ` · ${hint}` : ""}
      </span>
    </span>
  );
}

function FieldBars({
  title,
  rows,
  total,
  tone,
  asPercent = false,
}: {
  title: string;
  rows: FieldCount[];
  total: number;
  tone: "warn" | "ok";
  asPercent?: boolean;
}) {
  const denom = Math.max(total, 1);
  return (
    <div className="space-y-2">
      <div className="text-muted-foreground text-xs tracking-wide uppercase">
        {title}
      </div>
      <div className="grid grid-cols-[120px_1fr_64px] items-center gap-x-3 gap-y-2 text-sm">
        {rows.map((r) => {
          const pct = Math.min(100, (r.issues / denom) * 100);
          const barTone =
            tone === "ok" && pct >= 50
              ? "bg-emerald-600 dark:bg-emerald-500"
              : "bg-amber-500";
          return (
            <FieldBarRow
              key={r.field}
              label={FIELD_LABELS[r.field] ?? r.field}
              pct={pct}
              barTone={barTone}
              value={
                asPercent ? `${Math.round(pct)}%` : r.issues.toLocaleString()
              }
            />
          );
        })}
      </div>
    </div>
  );
}

function FieldBarRow({
  label,
  pct,
  barTone,
  value,
}: {
  label: string;
  pct: number;
  barTone: string;
  value: string;
}) {
  return (
    <>
      <span className="truncate">{label}</span>
      <div className="bg-secondary h-2 overflow-hidden rounded-full">
        <div className={`h-full ${barTone}`} style={{ width: `${pct}%` }} />
      </div>
      <span className="text-right tabular-nums">{value}</span>
    </>
  );
}

// ───────── gaps ─────────

type GapSource = "comicvine" | "metron" | "gcd";
const GAP_SOURCES: { value: GapSource; label: string }[] = [
  { value: "comicvine", label: "ComicVine" },
  { value: "metron", label: "Metron" },
  { value: "gcd", label: "GCD" },
];

function LibraryGapsCard({ d }: { d: MetadataCoverageResp }) {
  const [source, setSource] = useState<GapSource>("comicvine");
  const without = (row: (typeof d.by_library)[number]) =>
    source === "comicvine"
      ? row.without_comicvine
      : source === "metron"
        ? row.without_metron
        : row.without_gcd;
  const rows = [...d.by_library]
    .filter((r) => without(r) > 0)
    .sort((a, b) => without(b) - without(a));
  return (
    <Card>
      <CardHeader className="pb-2">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <CardTitle className="text-sm font-medium">
            Issues without an id, by library
          </CardTitle>
          <SourceSwitch value={source} onChange={setSource} />
        </div>
      </CardHeader>
      <CardContent>
        {rows.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            Every issue carries a {labelFor(source)} id.
          </p>
        ) : (
          <table className="w-full text-sm">
            <thead className="text-muted-foreground text-xs tracking-wide uppercase">
              <tr>
                <th className="py-1 text-left font-medium">Library</th>
                <th className="py-1 text-right font-medium">Issues</th>
                <th className="py-1 text-right font-medium">Share</th>
              </tr>
            </thead>
            <tbody className="divide-border/60 divide-y">
              {rows.map((r) => (
                <tr key={r.library_id}>
                  <td className="py-1.5">
                    <Link
                      href={`/?library=${r.library_id}&provider_match=unmatched&provider_source=${source}`}
                      className="hover:underline"
                    >
                      {r.name}
                    </Link>
                  </td>
                  <td className="py-1.5 text-right tabular-nums">
                    {without(r).toLocaleString()}
                  </td>
                  <td className="text-muted-foreground py-1.5 text-right tabular-nums">
                    {r.issues_total > 0
                      ? `${Math.round((without(r) / r.issues_total) * 1000) / 10}%`
                      : "—"}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </CardContent>
    </Card>
  );
}

function TopSeriesCard({ d }: { d: MetadataCoverageResp }) {
  const [source, setSource] = useState<GapSource>("comicvine");
  const rows = d.top_series.find((t) => t.source === source)?.series ?? [];
  return (
    <Card>
      <CardHeader className="pb-2">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <CardTitle className="text-sm font-medium">
            Series with the most issues lacking an id
          </CardTitle>
          <SourceSwitch value={source} onChange={setSource} />
        </div>
      </CardHeader>
      <CardContent>
        {rows.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            Every issue carries a {labelFor(source)} id.
          </p>
        ) : (
          <table className="w-full text-sm">
            <thead className="text-muted-foreground text-xs tracking-wide uppercase">
              <tr>
                <th className="py-1 text-left font-medium">Series</th>
                <th className="py-1 text-right font-medium">Issues</th>
              </tr>
            </thead>
            <tbody className="divide-border/60 divide-y">
              {rows.map((r) => (
                <tr key={r.slug}>
                  <td className="py-1.5">
                    <Link
                      href={`/series/${r.slug}?tab=metadata`}
                      className="hover:underline"
                    >
                      {r.name}
                      {r.year != null ? (
                        <span className="text-muted-foreground">
                          {" "}
                          ({r.year})
                        </span>
                      ) : null}
                    </Link>
                  </td>
                  <td className="py-1.5 text-right tabular-nums">
                    {r.issues_without_id.toLocaleString()}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <p className="text-muted-foreground pt-3 text-xs">
          Each series page&rsquo;s Metadata tab has Refresh this series and
          Analyze coverage.
        </p>
      </CardContent>
    </Card>
  );
}

function SourceSwitch({
  value,
  onChange,
}: {
  value: GapSource;
  onChange: (v: GapSource) => void;
}) {
  return (
    <div
      className="bg-secondary inline-flex rounded-md p-0.5 text-xs"
      role="tablist"
      aria-label="Provider"
    >
      {GAP_SOURCES.map((s) => (
        <button
          key={s.value}
          type="button"
          role="tab"
          aria-selected={value === s.value}
          onClick={() => onChange(s.value)}
          className={`rounded px-2 py-1 ${
            value === s.value
              ? "bg-background text-foreground shadow-sm"
              : "text-muted-foreground"
          }`}
        >
          {s.label}
        </button>
      ))}
    </div>
  );
}

function labelFor(source: GapSource): string {
  return GAP_SOURCES.find((s) => s.value === source)?.label ?? source;
}

// ───────── tag sources ─────────

function TagSourcesCard({ rows }: { rows: TagSourceRow[] }) {
  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">
          Where the ids came from
        </CardTitle>
        <p className="text-muted-foreground text-xs">
          The tagging tool named in each archive&rsquo;s ComicInfo notes, and
          which ids the scanner read from it.
        </p>
      </CardHeader>
      <CardContent className="overflow-x-auto">
        <table className="w-full min-w-[640px] text-sm">
          <thead className="text-muted-foreground text-xs tracking-wide uppercase">
            <tr>
              <th className="py-1 text-left font-medium">Tool</th>
              <th className="py-1 text-right font-medium">Issues</th>
              <th className="py-1 text-right font-medium">ComicVine id</th>
              <th className="py-1 text-right font-medium">Metron id</th>
              <th className="py-1 text-right font-medium">GCD id</th>
              <th className="py-1 text-left font-medium"></th>
            </tr>
          </thead>
          <tbody className="divide-border/60 divide-y">
            {rows.map((r) => {
              const own = TOOL_OWN_SOURCE[r.tool];
              const captured =
                own === "comicvine"
                  ? r.with_comicvine
                  : own === "metron"
                    ? r.with_metron
                    : own === "gcd"
                      ? r.with_gcd
                      : null;
              const unread = own != null && r.issues > 0 && captured === 0;
              return (
                <tr key={r.tool} data-testid={`tag-source-${r.tool}`}>
                  <td className="py-1.5">{TOOL_LABELS[r.tool] ?? r.tool}</td>
                  <td className="py-1.5 text-right tabular-nums">
                    {r.issues.toLocaleString()}
                  </td>
                  <td className="py-1.5 text-right tabular-nums">
                    {r.with_comicvine.toLocaleString()}
                  </td>
                  <td className="py-1.5 text-right tabular-nums">
                    {r.with_metron.toLocaleString()}
                  </td>
                  <td className="py-1.5 text-right tabular-nums">
                    {r.with_gcd.toLocaleString()}
                  </td>
                  <td className="py-1.5">
                    {unread ? (
                      <Badge variant="outline" className="text-[10px]">
                        ids in tags, not read
                      </Badge>
                    ) : null}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </CardContent>
    </Card>
  );
}

// ───────── tools + budgets + cost ─────────

function ToolsCard({ d }: { d: MetadataCoverageResp }) {
  const t = d.tools;
  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">
          Tools that close the gap over time
        </CardTitle>
        <p className="text-muted-foreground text-xs">
          Status of what already exists. Nothing here starts work; each tile
          links to where it is switched on.
        </p>
      </CardHeader>
      <CardContent className="space-y-5">
        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
          <ToolTile
            label="Weekly refresh"
            value={t.weekly_refresh_enabled ? "On" : "Off"}
            hint={`Series scope · stale after ${t.stale_after_days} d · 200 per library per run`}
            href="/admin/metadata?tab=settings"
            linkLabel="Metadata settings →"
          />
          <ToolTile
            label="Issue refresh"
            value={t.issue_refresh_enabled ? "On" : "Off"}
            hint={`Direct lookups only · ${t.issue_refresh_per_provider_cap.toLocaleString()} issues per provider per run`}
            href="/admin/metadata?tab=settings"
            linkLabel="Metadata settings →"
          />
          <ToolTile
            label="Auto-apply strong matches"
            value={`${t.auto_apply_libraries} of ${t.libraries_total} libraries`}
            hint="A direct lookup on a known id is a strong match"
            href="/admin/libraries"
            linkLabel="Library settings →"
          />
          <ToolTile
            label="Coverage analysed"
            value={`${t.coverage_analysed_series.toLocaleString()} of ${d.series_total.toLocaleString()} series`}
            hint={`Ever fetched through Folio: ${t.series_synced.toLocaleString()} series · ${t.issues_synced.toLocaleString()} issues`}
            href="/admin/metadata?tab=runs"
            linkLabel="Runs →"
          />
        </div>

        {d.providers.length > 0 ? (
          <div>
            <div className="text-muted-foreground mb-1 text-xs tracking-wide uppercase">
              Provider budgets now
            </div>
            {d.providers.map((p) => (
              <ProviderQuotaRow key={p.id} provider={p} />
            ))}
          </div>
        ) : null}

        <FillCostTable rows={d.fill_cost} />
      </CardContent>
    </Card>
  );
}

function ToolTile({
  label,
  value,
  hint,
  href,
  linkLabel,
}: {
  label: string;
  value: string;
  hint: string;
  href: string;
  linkLabel: string;
}) {
  return (
    <div className="bg-secondary/40 flex flex-col gap-1 rounded-md p-3">
      <div className="text-muted-foreground text-xs tracking-wide uppercase">
        {label}
      </div>
      <div className="text-base font-semibold">{value}</div>
      <div className="text-muted-foreground text-xs">{hint}</div>
      <Link href={href} className="text-primary text-xs hover:underline">
        {linkLabel}
      </Link>
    </div>
  );
}

function FillCostTable({ rows }: { rows: FillCost[] }) {
  const days = (requests: number, daily: number) =>
    daily > 0 ? Math.max(1, Math.ceil(requests / daily)) : null;
  const label = (source: string) =>
    GAP_SOURCES.find((s) => s.value === source)?.label ?? source;
  return (
    <div className="overflow-x-auto">
      <table className="w-full min-w-[640px] text-sm">
        <thead className="text-muted-foreground text-xs tracking-wide uppercase">
          <tr>
            <th className="py-1 text-left font-medium">Issue ids to fill</th>
            <th className="py-1 text-left font-medium">
              Via provider issue lists
            </th>
            <th className="py-1 text-left font-medium">Via per-issue fetch</th>
          </tr>
        </thead>
        <tbody className="divide-border/60 divide-y">
          {rows.map((r) => (
            <tr key={r.source} data-testid={`fill-cost-${r.source}`}>
              <td className="py-1.5">
                {label(r.source)} · {r.issues_without_id.toLocaleString()}{" "}
                issues
              </td>
              <td className="py-1.5 tabular-nums">
                ≈ {r.via_lists_requests.toLocaleString()} requests
                <Days n={days(r.via_lists_requests, r.daily_budget)} />
              </td>
              <td className="py-1.5 tabular-nums">
                ≈ {r.via_detail_requests.toLocaleString()} requests
                <Days n={days(r.via_detail_requests, r.daily_budget)} />
                <span className="text-muted-foreground">
                  {" "}
                  · {r.via_detail_requests.toLocaleString()} archive rewrites
                </span>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="text-muted-foreground pt-2 text-xs">
        Lists: one series search per unmatched series plus one page per{" "}
        {rows[0]?.list_page_size ?? 100} issues. Days assume the
        provider&rsquo;s documented daily budget. Each fetch rewrites the
        archive and keeps a backup for the library&rsquo;s retention window.
      </p>
    </div>
  );
}

function Days({ n }: { n: number | null }) {
  if (n == null) return null;
  return (
    <span className="text-muted-foreground">
      {" "}
      · about {n} {n === 1 ? "day" : "days"}
    </span>
  );
}

// ───────── bits ─────────

function Measure({
  value,
  total,
  link,
}: {
  value: number;
  total: number;
  link?: { href?: string; label: string };
}) {
  const pct = total > 0 ? Math.min(100, (value / total) * 100) : 0;
  const tone =
    pct >= 90 ? "bg-emerald-600 dark:bg-emerald-500" : "bg-amber-500";
  return (
    <div className="space-y-1">
      <div className="flex items-baseline justify-between gap-2 text-sm">
        <span className="tabular-nums">
          {value.toLocaleString()} / {total.toLocaleString()}
        </span>
        {link ? (
          link.href ? (
            <Link
              href={link.href}
              className="text-primary text-xs tabular-nums hover:underline"
            >
              {link.label}
            </Link>
          ) : (
            <span className="text-muted-foreground text-xs tabular-nums">
              {link.label}
            </span>
          )
        ) : null}
      </div>
      <div className="bg-secondary h-2 overflow-hidden rounded-full">
        <div className={`h-full ${tone}`} style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

function Num({ n }: { n: number }): ReactNode {
  return <span className="tabular-nums">{n.toLocaleString()}</span>;
}
