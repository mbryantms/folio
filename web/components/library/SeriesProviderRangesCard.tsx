"use client";

/**
 * `<SeriesProviderRangesCard>` — provider series-boundary coverage.
 *
 * Shows, per metadata provider, how this local series' issues map onto
 * the provider's series — the default series for most issues plus any
 * range overrides where the provider splits the run into a separate
 * series (e.g. Fantastic Four #600–611 → Metron "Fantastic Four (2012)").
 * Driven by the per-provider coverage map so the reader sees the whole
 * picture at a glance instead of reconstructing the split from a bare
 * list of exceptions. Source-agnostic.
 *
 * "Analyze coverage" runs a background analysis across ComicVine, Metron
 * and GCD with no prior match required: it lists every candidate provider
 * series, assigns each local issue by number + cover date, and proposes
 * a main series + ranges per provider (`<ProviderCoverageAnalysis>`),
 * which the admin accepts per provider. Once nothing is left to act on
 * (every provider up to date) the analysis collapses to a one-line
 * disclosure under the mapping, so the Metadata tab shows the result, not
 * the working; it opens by itself while a decision is pending (see
 * `analysisAttention`). A manual open/close sticks for this visit.
 *
 * The series-level `external_ids` row for each provider IS that provider's
 * default series, so it is shown in the provider's header (linked, with
 * unlink) instead of in a separate Identifiers card; ids for sources that
 * are not metadata providers (Marvel, LOCG, ISBN, …) sit in one "Other
 * identifiers" row at the foot with the "+ Add ID" form. The editor is
 * `useExternalIdsEditor`, shared with the issue page's `<ExternalIdsCard>`.
 *
 * Visible to anyone who can see the library; mapping add / remove /
 * analyze / accept are admin-only (the API enforces it too), while
 * linking and unlinking ids is open to any signed-in user, as the
 * identifiers card always was. Renders nothing when there is no coverage,
 * no id, and the viewer can't edit.
 */

import {
  ChevronRight,
  ExternalLink,
  Loader2,
  Plus,
  Sparkles,
  Trash2,
} from "lucide-react";
import * as React from "react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
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
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/folio/select";
import { CoverageAfterMatchPrompt } from "@/components/library/CoverageAfterMatchPrompt";
import { useExternalIdsEditor } from "@/components/library/ExternalIdsCard";
import {
  ProviderCoverageAnalysis,
  SERIES_COLORS,
} from "@/components/library/ProviderCoverageAnalysis";
import {
  useAcceptProviderCoverage,
  useAddProviderRangeSeries,
  useAnalyzeProviderCoverage,
  useDeleteProviderRangeSeries,
} from "@/lib/api/mutations";
import {
  useMe,
  useProviderCoverageAnalysis,
  useProviderCoverageSeries,
} from "@/lib/api/queries";
import type {
  CoverageRangeRef,
  CoverageSegment,
  ExternalIdRow,
  ProviderCoverageResp,
} from "@/lib/api/types";
import { analysisAttention, staleRemovedSummary } from "@/lib/coverage-ranges";
import { cn } from "@/lib/utils";

const SOURCES: Array<{ value: string; label: string }> = [
  { value: "metron", label: "Metron" },
  { value: "comicvine", label: "ComicVine" },
  { value: "gcd", label: "Grand Comics Database" },
  { value: "marvel", label: "Marvel" },
  { value: "locg", label: "League of Comic Geeks" },
];

/** "#600–611", "#600+", "up to #50", or "all issues". */
function formatRange(
  low: string | null | undefined,
  high: string | null | undefined,
): string {
  if (low && high) return low === high ? `#${low}` : `#${low}–${high}`;
  if (low) return `#${low}+`;
  if (high) return `up to #${high}`;
  return "all issues";
}

/** "Fantastic Four (2012)" — or `null` when the provider series name
 *  isn't known (the row then shows just the linked id). */
function segmentSeriesLabel(seg: CoverageSegment): string | null {
  if (!seg.provider_series_name) return null;
  return seg.declared_year != null
    ? `${seg.provider_series_name} (${seg.declared_year})`
    : seg.provider_series_name;
}

type RemoveTarget = {
  id: string;
  sourceLabel: string;
  range: string;
  seriesName: string;
};

export function SeriesProviderRangesCard({
  seriesSlug,
}: {
  seriesSlug: string;
}) {
  const me = useMe();
  const isAdmin = me.data?.role === "admin";
  const coverage = useProviderCoverageSeries(seriesSlug);
  const add = useAddProviderRangeSeries(seriesSlug);
  const remove = useDeleteProviderRangeSeries(seriesSlug);
  const analyze = useAnalyzeProviderCoverage(seriesSlug);
  const acceptCoverage = useAcceptProviderCoverage(seriesSlug);
  const analysis = useProviderCoverageAnalysis(seriesSlug, {
    enabled: isAdmin,
  });
  const [autoAccept, setAutoAccept] = React.useState(false);
  const [acceptingSource, setAcceptingSource] = React.useState<string | null>(
    null,
  );
  const analysisState = analysis.data?.state;
  const analysing =
    analyze.isPending ||
    analysisState === "queued" ||
    analysisState === "running";
  const attention = analysis.data ? analysisAttention(analysis.data) : null;
  // `null` = follow the automatic rule; a tap sets it for this visit.
  const [analysisOpen, setAnalysisOpen] = React.useState<boolean | null>(null);
  const showAnalysis = analysisOpen ?? attention?.needsAttention ?? false;

  const runAnalyze = () => {
    // A new run is worth watching even if the last one was folded away.
    setAnalysisOpen(null);
    analyze.mutate({ auto_accept: autoAccept });
  };

  // Stale ranges accepts removed, kept on screen (the toast fades).
  const [removedNotes, setRemovedNotes] = React.useState<string[]>([]);
  const onAccept = (source: string, mainSeriesId: string | null) => {
    setAcceptingSource(source);
    acceptCoverage.mutate(
      { source, main_series_id: mainSeriesId },
      {
        onSuccess: (data) => {
          const note = staleRemovedSummary(data);
          if (note) setRemovedNotes((prev) => [...prev, note]);
        },
        onSettled: () => setAcceptingSource(null),
      },
    );
  };

  const onRemoveStale = (sourceLabel: string, row: CoverageRangeRef) =>
    setConfirmRemove({
      id: row.id,
      sourceLabel,
      range: formatRange(row.range_low, row.range_high),
      seriesName:
        row.provider_series_name ?? `series ${row.provider_series_id}`,
    });

  const providers = coverage.data?.providers ?? [];
  const [adding, setAdding] = React.useState(false);
  const [confirmRemove, setConfirmRemove] = React.useState<RemoveTarget | null>(
    null,
  );

  // Add-form fields.
  const [source, setSource] = React.useState("metron");
  const [providerSeriesId, setProviderSeriesId] = React.useState("");
  const [providerSeriesName, setProviderSeriesName] = React.useState("");
  const [providerSeriesUrl, setProviderSeriesUrl] = React.useState("");
  const [rangeLow, setRangeLow] = React.useState("");
  const [rangeHigh, setRangeHigh] = React.useState("");
  const [declaredYear, setDeclaredYear] = React.useState("");

  const resetForm = () => {
    setAdding(false);
    setProviderSeriesId("");
    setProviderSeriesName("");
    setProviderSeriesUrl("");
    setRangeLow("");
    setRangeHigh("");
    setDeclaredYear("");
  };

  const onAdd = (e: React.FormEvent) => {
    e.preventDefault();
    if (!providerSeriesId.trim()) return;
    const yearNum = declaredYear.trim() ? Number(declaredYear.trim()) : null;
    add.mutate(
      {
        source,
        provider_series_id: providerSeriesId.trim(),
        provider_series_name: providerSeriesName.trim() || null,
        provider_series_url: providerSeriesUrl.trim() || null,
        range_low: rangeLow.trim() || null,
        range_high: rangeHigh.trim() || null,
        declared_year:
          yearNum !== null && Number.isFinite(yearNum) ? yearNum : null,
      },
      {
        onSuccess: () => resetForm(),
      },
    );
  };

  const onConfirmRemove = () => {
    if (!confirmRemove) return;
    remove.mutate(
      { id: confirmRemove.id },
      {
        onSuccess: () => setConfirmRemove(null),
      },
    );
  };

  // Series-level ids: one per provider at most. The three metadata
  // providers get a block each (id and/or coverage); everything else is
  // an "other identifier".
  const ids = useExternalIdsEditor(
    { entityType: "series", seriesSlug },
    { idPrefix: "spr-eid" },
  );
  const idBySource = new Map(ids.rows.map((r) => [r.source, r]));
  const coverageBySource = new Map(providers.map((p) => [p.source, p]));
  const blocks: ProviderBlock[] = PROVIDERS.filter(
    (p) => coverageBySource.has(p.value) || idBySource.has(p.value),
  ).map((p) => ({
    source: p.value,
    label: p.label,
    id: idBySource.get(p.value),
    coverage: coverageBySource.get(p.value),
  }));
  for (const p of providers) {
    if (!PROVIDER_KEYS.has(p.source)) {
      blocks.push({
        source: p.source,
        label: p.source_label,
        id: idBySource.get(p.source),
        coverage: p,
      });
    }
  }
  const otherIds = ids.rows.filter((r) => !PROVIDER_KEYS.has(r.source));
  const loading = coverage.isLoading || ids.isLoading;

  // Transparent in the common case: nothing mapped, nothing linked, and
  // the viewer can't edit.
  if (!loading && blocks.length === 0 && otherIds.length === 0 && !isAdmin) {
    return null;
  }

  return (
    <>
      <Card data-testid="provider-mapping-card">
        <CardHeader className="flex flex-col gap-3 space-y-0 pb-3 sm:flex-row sm:items-start sm:justify-between">
          <div className="space-y-1">
            <CardTitle className="text-sm font-semibold">
              Provider series mapping
            </CardTitle>
            <CardDescription className="text-xs">
              Each provider&apos;s series for this run, and where it files
              annuals, specials and renumbered stretches. Unlinking a provider
              clears its default series.
            </CardDescription>
          </div>
          {isAdmin && !adding && (
            <div className="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-1 sm:justify-end">
              <div className="flex items-center gap-2">
                <Checkbox
                  id="cov-auto"
                  checked={autoAccept}
                  onCheckedChange={(v) => setAutoAccept(v === true)}
                  disabled={analysing}
                />
                <Label htmlFor="cov-auto" className="text-xs font-normal">
                  Accept high-confidence results
                </Label>
              </div>
              <Button
                variant="outline"
                size="sm"
                disabled={analysing || acceptingSource !== null}
                onClick={runAnalyze}
              >
                {analysing ? (
                  <Loader2 className="mr-1 size-3.5 animate-spin" />
                ) : (
                  <Sparkles className="mr-1 size-3.5" />
                )}
                Analyze coverage
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setAdding(true)}
              >
                <Plus className="mr-1 size-3.5" /> Add mapping
              </Button>
            </div>
          )}
        </CardHeader>
        <CardContent className="space-y-3 text-sm">
          {loading ? (
            <div className="text-muted-foreground flex items-center gap-2 py-3">
              <Loader2 className="size-4 animate-spin" /> Loading…
            </div>
          ) : blocks.length === 0 && !adding ? (
            <p className="text-muted-foreground text-sm">
              No provider series mapped yet. Use{" "}
              <span className="font-medium">Analyze coverage</span> to find
              which ComicVine, Metron and GCD series hold these issues, even
              when the folder mixes several volumes (e.g. a run plus its legacy
              renumbering).
            </p>
          ) : (
            <div className="divide-border/60 divide-y">
              {blocks.map((b) => (
                <ProviderBlockView
                  key={b.source}
                  block={b}
                  isAdmin={isAdmin}
                  onUnlinkId={ids.requestRemove}
                  onRemoveRange={setConfirmRemove}
                />
              ))}
            </div>
          )}

          {/* Identifiers that are not metadata providers (Marvel, LOCG,
              ISBN, …) — and the one "+ Add ID" entry point for every
              source, provider or not. */}
          <div
            className="border-border/60 flex flex-wrap items-center gap-x-3 gap-y-1.5 border-t pt-3"
            data-testid="other-identifiers"
          >
            <span className="text-muted-foreground text-[11px] font-semibold tracking-wide uppercase">
              Other identifiers
            </span>
            {otherIds.length === 0 ? (
              <span className="text-muted-foreground text-xs">
                None linked · Marvel, League of Comic Geeks, ISBN, UPC, ASIN…
              </span>
            ) : (
              otherIds.map((r) => (
                <span key={r.source} className="inline-flex items-center gap-1">
                  <IdBadge row={r} />
                  <Button
                    variant="ghost"
                    size="icon"
                    className="text-muted-foreground/60 hover:text-foreground size-6"
                    onClick={() => ids.requestRemove(r)}
                    aria-label={`Remove ${r.source_label} link`}
                  >
                    <Trash2 className="size-3" />
                  </Button>
                </span>
              ))
            )}
            {!ids.adding && (
              <Button
                variant="ghost"
                size="sm"
                className="ml-auto"
                onClick={ids.startAdding}
              >
                <Plus className="mr-1 size-3.5" /> Add ID
              </Button>
            )}
          </div>
          {ids.addForm}

          {isAdmin && analysis.data && attention && (
            <Collapsible
              open={showAnalysis}
              onOpenChange={setAnalysisOpen}
              className="border-t pt-3"
              data-testid="coverage-analysis-section"
            >
              <CollapsibleTrigger
                className="focus-visible:ring-ring flex w-full items-center gap-2 rounded text-left focus-visible:ring-2 focus-visible:outline-none"
                aria-label={`Coverage analysis: ${attention.summary}`}
              >
                <ChevronRight
                  className={cn(
                    "text-muted-foreground size-4 shrink-0 transition-transform",
                    showAnalysis && "rotate-90",
                  )}
                  aria-hidden
                />
                <span className="font-medium">Coverage analysis</span>
                <span className="text-muted-foreground min-w-0 truncate text-xs">
                  {analysing && (
                    <Loader2 className="mr-1 inline size-3 animate-spin" />
                  )}
                  {attention.summary}
                </span>
              </CollapsibleTrigger>
              <CollapsibleContent className="space-y-2 pt-2">
                {analysing ? (
                  <div className="text-muted-foreground flex items-center gap-2 text-xs">
                    <Loader2 className="size-3.5 animate-spin" />
                    {analysis.data.trigger === "analyze"
                      ? "Listing candidate series on ComicVine, Metron and GCD. This can take a minute (provider rate limits)."
                      : "Checking which issues the series you matched holds, and where the rest live…"}
                  </div>
                ) : analysis.data.state === "failed" ? (
                  <p className="text-destructive text-xs">
                    Analysis failed: {analysis.data.error ?? "unknown error"}
                  </p>
                ) : (
                  <>
                    <CoverageAfterMatchPrompt
                      data={analysis.data}
                      acceptingSource={acceptingSource}
                      onAccept={onAccept}
                    />
                    <ProviderCoverageAnalysis
                      data={analysis.data}
                      acceptingSource={acceptingSource}
                      onAccept={onAccept}
                      onRemoveStale={onRemoveStale}
                      removedNotes={removedNotes}
                    />
                  </>
                )}
              </CollapsibleContent>
            </Collapsible>
          )}

          {isAdmin && adding && (
            <form
              onSubmit={onAdd}
              className="grid gap-2 border-t pt-3 sm:grid-cols-2"
            >
              <div className="grid gap-1.5">
                <Label htmlFor="spr-source" className="text-xs">
                  Provider
                </Label>
                <Select value={source} onValueChange={setSource}>
                  <SelectTrigger id="spr-source">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {SOURCES.map((s) => (
                      <SelectItem key={s.value} value={s.value}>
                        {s.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="grid gap-1.5">
                <Label htmlFor="spr-id" className="text-xs">
                  Provider series ID
                </Label>
                <Input
                  id="spr-id"
                  value={providerSeriesId}
                  onChange={(e) => setProviderSeriesId(e.target.value)}
                  placeholder="e.g. 1713"
                  autoFocus
                />
              </div>
              <div className="grid gap-1.5 sm:col-span-2">
                <Label htmlFor="spr-name" className="text-xs">
                  Provider series name (shown to readers)
                </Label>
                <Input
                  id="spr-name"
                  value={providerSeriesName}
                  onChange={(e) => setProviderSeriesName(e.target.value)}
                  placeholder="e.g. Fantastic Four (2012)"
                />
              </div>
              <div className="grid gap-1.5">
                <Label htmlFor="spr-low" className="text-xs">
                  First issue # (blank = open)
                </Label>
                <Input
                  id="spr-low"
                  value={rangeLow}
                  onChange={(e) => setRangeLow(e.target.value)}
                  placeholder="e.g. 600"
                />
              </div>
              <div className="grid gap-1.5">
                <Label htmlFor="spr-high" className="text-xs">
                  Last issue # (blank = open)
                </Label>
                <Input
                  id="spr-high"
                  value={rangeHigh}
                  onChange={(e) => setRangeHigh(e.target.value)}
                  placeholder="e.g. 611"
                />
              </div>
              <div className="grid gap-1.5">
                <Label htmlFor="spr-year" className="text-xs">
                  Series start year
                </Label>
                <Input
                  id="spr-year"
                  value={declaredYear}
                  onChange={(e) => setDeclaredYear(e.target.value)}
                  placeholder="e.g. 2012"
                  inputMode="numeric"
                />
              </div>
              <div className="grid gap-1.5">
                <Label htmlFor="spr-url" className="text-xs">
                  Provider URL (optional)
                </Label>
                <Input
                  id="spr-url"
                  value={providerSeriesUrl}
                  onChange={(e) => setProviderSeriesUrl(e.target.value)}
                  placeholder="https://metron.cloud/series/…"
                />
              </div>
              <div className="flex gap-1 sm:col-span-2">
                <Button
                  type="submit"
                  size="sm"
                  disabled={add.isPending || !providerSeriesId.trim()}
                >
                  {add.isPending ? (
                    <>
                      <Loader2 className="mr-1 size-3 animate-spin" /> Adding
                    </>
                  ) : (
                    "Add"
                  )}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  onClick={resetForm}
                  disabled={add.isPending}
                >
                  Cancel
                </Button>
              </div>
            </form>
          )}
        </CardContent>
      </Card>

      {ids.confirmDialog}
      <AlertDialog
        open={confirmRemove !== null}
        onOpenChange={(o) => {
          if (!o) setConfirmRemove(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove provider range mapping?</AlertDialogTitle>
            <AlertDialogDescription>
              Removes the {confirmRemove?.sourceLabel} mapping for{" "}
              {confirmRemove?.range} ({confirmRemove?.seriesName}). Those issues
              will go back to matching the series&apos; default provider series.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={remove.isPending}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={onConfirmRemove}
              disabled={remove.isPending}
            >
              {remove.isPending ? (
                <>
                  <Loader2 className="mr-1 size-3 animate-spin" /> Removing
                </>
              ) : (
                "Remove"
              )}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

// ───────── provider block ─────────

/** The three metadata providers, in display order. */
const PROVIDERS = [
  { value: "comicvine", label: "ComicVine" },
  { value: "metron", label: "Metron" },
  { value: "gcd", label: "Grand Comics Database" },
] as const;
const PROVIDER_KEYS = new Set<string>(PROVIDERS.map((p) => p.value));

type CoverageProvider = ProviderCoverageResp["providers"][number];

type ProviderBlock = {
  source: string;
  label: string;
  /** The series-level id — the provider's default series. */
  id: ExternalIdRow | undefined;
  coverage: CoverageProvider | undefined;
};

/** The id as a chip: linked when the provider has a page for it. */
function IdBadge({ row }: { row: ExternalIdRow }) {
  const label = PROVIDER_KEYS.has(row.source)
    ? row.external_id
    : `${row.source_label} ${row.external_id}`;
  return row.external_url ? (
    <a
      href={row.external_url}
      target="_blank"
      rel="noreferrer"
      title={`Open ${row.source_label} ${row.external_id}`}
    >
      <Badge
        variant="secondary"
        className="hover:bg-secondary/80 cursor-pointer gap-1 font-normal tabular-nums"
      >
        {label}
        <ExternalLink className="size-3" aria-hidden />
      </Badge>
    </a>
  ) : (
    <Badge
      variant="secondary"
      className="cursor-default font-normal tabular-nums"
    >
      {label}
    </Badge>
  );
}

function ProviderBlockView({
  block,
  isAdmin,
  onUnlinkId,
  onRemoveRange,
}: {
  block: ProviderBlock;
  isAdmin: boolean;
  onUnlinkId: (row: ExternalIdRow) => void;
  onRemoveRange: (target: RemoveTarget) => void;
}) {
  const p = block.coverage;
  const total = p ? p.segments.reduce((n, s) => n + s.issue_count, 0) : 0;
  // One colour per distinct provider series (stable across
  // non-contiguous segments of the same series).
  const seriesOrder = p
    ? Array.from(new Set(p.segments.map((s) => s.provider_series_id)))
    : [];
  const colorOf = (id: string) =>
    `hsl(${SERIES_COLORS[seriesOrder.indexOf(id) % SERIES_COLORS.length]})`;
  return (
    <div
      className="space-y-2 py-3 first:pt-0 last:pb-0"
      data-testid={`provider-block-${block.source}`}
    >
      <div className="flex flex-wrap items-center gap-2">
        <span className="font-medium">{block.label}</span>
        {block.id ? (
          <>
            <IdBadge row={block.id} />
            <Button
              variant="ghost"
              size="icon"
              className="text-muted-foreground/60 hover:text-foreground size-6"
              onClick={() => onUnlinkId(block.id!)}
              aria-label={`Unlink ${block.label}`}
            >
              <Trash2 className="size-3" />
            </Button>
          </>
        ) : (
          <span className="text-muted-foreground text-xs">no series id</span>
        )}
        {p && (
          <span className="text-muted-foreground ml-auto text-xs tabular-nums">
            {total} issue{total === 1 ? "" : "s"} · {seriesOrder.length} series
          </span>
        )}
      </div>
      {p ? (
        <>
          {/* Proportional bar (decorative — the list below is the
              accessible representation): each series gets its own hue. */}
          <div
            aria-hidden
            className="border-border/40 flex h-2 overflow-hidden rounded-full border"
          >
            {p.segments.map((seg, i) => (
              <div
                key={`${seg.provider_series_id}-${seg.low}-${i}`}
                style={{
                  flexGrow: seg.issue_count,
                  backgroundColor: colorOf(seg.provider_series_id),
                }}
                title={`${formatRange(seg.low, seg.high)} → ${segmentSeriesLabel(seg) ?? `#${seg.provider_series_id}`}`}
                className={cn("h-full", i > 0 && "border-background border-l")}
              />
            ))}
          </div>
          <ul className="space-y-1">
            {p.segments.map((seg, i) => (
              <li
                key={`${seg.provider_series_id}-${seg.low}-${i}`}
                className="flex items-center justify-between gap-2"
              >
                <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5">
                  <span
                    aria-hidden
                    className="h-2.5 w-2.5 shrink-0 rounded-full"
                    style={{ backgroundColor: colorOf(seg.provider_series_id) }}
                  />
                  <Badge variant="outline" className="font-normal tabular-nums">
                    {formatRange(seg.low, seg.high)}
                  </Badge>
                  <span className="text-muted-foreground text-xs">
                    {seg.issue_count} issue{seg.issue_count === 1 ? "" : "s"}
                  </span>
                  {(() => {
                    const label = segmentSeriesLabel(seg);
                    const text = label ?? `#${seg.provider_series_id}`;
                    return (
                      <>
                        {seg.provider_series_url ? (
                          <a
                            href={seg.provider_series_url}
                            target="_blank"
                            rel="noreferrer"
                            className="truncate hover:underline"
                          >
                            {text} ↗
                          </a>
                        ) : (
                          <span className="truncate">{text}</span>
                        )}
                        {label && (
                          <code className="text-muted-foreground text-xs">
                            #{seg.provider_series_id}
                          </code>
                        )}
                      </>
                    );
                  })()}
                  {seg.via_range ? (
                    <Badge variant="secondary" className="font-normal">
                      override
                    </Badge>
                  ) : (
                    <span className="text-muted-foreground text-[10px] font-medium tracking-wider uppercase">
                      default
                    </span>
                  )}
                </div>
                {isAdmin && seg.via_range && seg.range_id && (
                  <Button
                    variant="ghost"
                    size="icon"
                    className="text-muted-foreground/60 hover:text-foreground size-6 shrink-0"
                    onClick={() =>
                      onRemoveRange({
                        id: seg.range_id!,
                        sourceLabel: block.label,
                        range: formatRange(seg.low, seg.high),
                        seriesName:
                          segmentSeriesLabel(seg) ??
                          `series ${seg.provider_series_id}`,
                      })
                    }
                    aria-label={`Remove ${block.label} ${formatRange(seg.low, seg.high)} mapping`}
                  >
                    <Trash2 className="size-3.5" />
                  </Button>
                )}
              </li>
            ))}
          </ul>
        </>
      ) : (
        <p className="text-muted-foreground text-xs">
          No issues mapped yet — Analyze coverage lists this provider&apos;s
          series and assigns each issue.
        </p>
      )}
    </div>
  );
}
