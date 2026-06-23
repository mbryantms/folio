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
 * list of exceptions. Source-agnostic — GCD appears automatically once
 * that provider is added.
 *
 * Visible to anyone who can see the library; add / remove / detect are
 * admin-only (the API enforces it too). Renders nothing when there's no
 * coverage and the viewer can't edit.
 */

import { Loader2, Plus, Sparkles, Trash2 } from "lucide-react";
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  useAddProviderRangeSeries,
  useDeleteProviderRangeSeries,
  useDetectProviderRangesSeries,
} from "@/lib/api/mutations";
import { useMe, useProviderCoverageSeries } from "@/lib/api/queries";
import type { CoverageSegment, DetectResp } from "@/lib/api/types";
import { cn } from "@/lib/utils";

const SOURCES: Array<{ value: string; label: string }> = [
  { value: "metron", label: "Metron" },
  { value: "comicvine", label: "ComicVine" },
  { value: "gcd", label: "Grand Comics Database" },
  { value: "marvel", label: "Marvel" },
  { value: "locg", label: "League of Comic Geeks" },
];

/**
 * Categorical hues (HSL triplets) for the coverage bar — one per distinct
 * provider series within a provider, so every segment is coloured and the
 * split reads at a glance. First slot is the theme amber; the rest are
 * evenly-spread distinct hues that hold up on the dark + light themes.
 */
const SERIES_COLORS = [
  "38 92% 55%", // amber (theme primary)
  "199 89% 48%", // sky
  "262 83% 58%", // violet
  "160 84% 39%", // emerald
  "350 89% 60%", // rose
  "173 80% 40%", // teal
  "25 95% 53%", // orange
  "292 84% 61%", // fuchsia
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
  const detect = useDetectProviderRangesSeries(seriesSlug);
  const [detectResult, setDetectResult] = React.useState<DetectResp | null>(
    null,
  );

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
      { onSuccess: resetForm },
    );
  };

  const onConfirmRemove = () => {
    if (!confirmRemove) return;
    remove.mutate(
      { id: confirmRemove.id },
      { onSuccess: () => setConfirmRemove(null) },
    );
  };

  // Transparent in the common case: nothing mapped and the viewer can't edit.
  if (!coverage.isLoading && providers.length === 0 && !isAdmin) return null;

  return (
    <div className="space-y-4 text-sm">
      {coverage.isLoading ? (
        <div className="text-muted-foreground flex items-center gap-2 py-3">
          <Loader2 className="h-4 w-4 animate-spin" /> Loading…
        </div>
      ) : providers.length === 0 && !adding ? (
        <p className="text-muted-foreground text-sm">
          No provider series mapped yet. Match this series to a provider, then
          use <span className="font-medium">Detect from providers</span> — if a
          provider (Metron/GCD) splits part of the run (e.g. a legacy
          renumbering) into a separate series, it&rsquo;ll show here.
        </p>
      ) : (
        <div className="space-y-4">
          {providers.map((p) => {
            const total = p.segments.reduce((n, s) => n + s.issue_count, 0);
            // One colour per distinct provider series (stable across
            // non-contiguous segments of the same series).
            const seriesOrder = Array.from(
              new Set(p.segments.map((s) => s.provider_series_id)),
            );
            const colorOf = (id: string) =>
              `hsl(${SERIES_COLORS[seriesOrder.indexOf(id) % SERIES_COLORS.length]})`;
            return (
              <div key={p.source} className="space-y-1.5">
                <div className="flex items-baseline justify-between gap-2">
                  <span className="font-medium">{p.source_label}</span>
                  <span className="text-muted-foreground text-xs">
                    {total} issue{total === 1 ? "" : "s"} · {seriesOrder.length}{" "}
                    series
                  </span>
                </div>
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
                      className={cn(
                        "h-full",
                        i > 0 && "border-background border-l",
                      )}
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
                          style={{
                            backgroundColor: colorOf(seg.provider_series_id),
                          }}
                        />
                        <Badge
                          variant="outline"
                          className="font-normal tabular-nums"
                        >
                          {formatRange(seg.low, seg.high)}
                        </Badge>
                        <span className="text-muted-foreground text-xs">
                          {seg.issue_count} issue
                          {seg.issue_count === 1 ? "" : "s"}
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
                          className="text-muted-foreground/60 hover:text-foreground h-6 w-6 shrink-0"
                          onClick={() =>
                            setConfirmRemove({
                              id: seg.range_id!,
                              sourceLabel: p.source_label,
                              range: formatRange(seg.low, seg.high),
                              seriesName:
                                segmentSeriesLabel(seg) ??
                                `series ${seg.provider_series_id}`,
                            })
                          }
                          aria-label={`Remove ${p.source_label} ${formatRange(seg.low, seg.high)} mapping`}
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </Button>
                      )}
                    </li>
                  ))}
                </ul>
              </div>
            );
          })}
        </div>
      )}

      {isAdmin && detectResult && (
        <div className="border-border/60 text-muted-foreground space-y-1 rounded-md border p-2 text-xs">
          {detectResult.results.length === 0 ? (
            <p>
              No matched provider series to scan yet — apply a series match
              first.
            </p>
          ) : (
            detectResult.results.map((r) => (
              <p key={`${r.source}:${r.provider_series_id}`}>
                <span className="font-medium">{r.source_label}</span> ·{" "}
                {r.error
                  ? `error: ${r.error}`
                  : r.covered_count === 0
                    ? "provider returned no issue list (nothing to split)"
                    : `scanned ${r.covered_count} issues; ${
                        r.gaps.length
                          ? `gaps ${r.gaps.join(", ")}; `
                          : "no gaps; "
                      }created ${r.created.length} mapping(s)`}
              </p>
            ))
          )}
        </div>
      )}

      {isAdmin && !adding && (
        <div className="flex justify-end gap-1">
          <Button
            variant="ghost"
            size="sm"
            disabled={detect.isPending}
            onClick={() =>
              detect.mutate(undefined, {
                onSuccess: (data) => setDetectResult(data),
              })
            }
          >
            {detect.isPending ? (
              <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
            ) : (
              <Sparkles className="mr-1 h-3.5 w-3.5" />
            )}
            Detect from providers
          </Button>
          <Button variant="ghost" size="sm" onClick={() => setAdding(true)}>
            <Plus className="mr-1 h-3.5 w-3.5" /> Add mapping
          </Button>
        </div>
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
                  <Loader2 className="mr-1 h-3 w-3 animate-spin" /> Adding
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
                  <Loader2 className="mr-1 h-3 w-3 animate-spin" /> Removing
                </>
              ) : (
                "Remove"
              )}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
