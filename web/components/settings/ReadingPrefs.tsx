"use client";

import { useEffect, useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { HttpError, useMe, useSeriesMany } from "@/lib/api/queries";
import type { UseQueryResult } from "@tanstack/react-query";
import type { SeriesView } from "@/lib/api/types";
import { useUpdatePreferences } from "@/lib/api/mutations";
import type { PreferencesReq } from "@/lib/api/types";

import { SegmentedControl } from "./SegmentedControl";
import { SettingsSection } from "./SettingsSection";

type DirectionPref = "auto" | "ltr" | "rtl";
type FitPref = "auto" | "width" | "height" | "original";
type ViewPref = "auto" | "single" | "double" | "webtoon";

const directionOptions: ReadonlyArray<{ value: DirectionPref; label: string }> =
  [
    { value: "auto", label: "Auto" },
    { value: "ltr", label: "Left → right" },
    { value: "rtl", label: "Right → left" },
  ];
const fitOptions: ReadonlyArray<{ value: FitPref; label: string }> = [
  { value: "auto", label: "Auto" },
  { value: "width", label: "Fit width" },
  { value: "height", label: "Fit height" },
  { value: "original", label: "Original" },
];
const viewOptions: ReadonlyArray<{ value: ViewPref; label: string }> = [
  { value: "auto", label: "Auto" },
  { value: "single", label: "Single" },
  { value: "double", label: "Double" },
  { value: "webtoon", label: "Webtoon" },
];

const SERIES_OVERRIDE_PREFIX = "reader:";

function fitFromMe(v: string | null | undefined): FitPref {
  return v === "width" || v === "height" || v === "original" ? v : "auto";
}
function viewFromMe(v: string | null | undefined): ViewPref {
  return v === "single" || v === "double" || v === "webtoon" ? v : "auto";
}
function directionFromMe(v: string | null | undefined): DirectionPref {
  return v === "ltr" || v === "rtl" ? v : "auto";
}

function toReq(value: "auto" | string): string | null {
  return value === "auto" ? null : value;
}

export function ReadingPrefs() {
  const me = useMe();
  const update = useUpdatePreferences();

  function patch(body: PreferencesReq) {
    update.mutate(body);
  }

  if (me.isLoading) return <Skeleton className="h-72 w-full" />;
  if (me.error || !me.data) {
    return (
      <p className="text-destructive text-sm">Failed to load preferences.</p>
    );
  }
  const data = me.data;

  return (
    <div className="space-y-6">
      <SettingsSection
        title="Reading direction"
        description="Default for new series. Per-series overrides and the ComicInfo Manga flag still take precedence."
      >
        <SegmentedControl
          value={directionFromMe(data.default_reading_direction)}
          onChange={(next) =>
            patch({
              default_reading_direction: toReq(
                next,
              ) as PreferencesReq["default_reading_direction"],
            })
          }
          options={directionOptions}
          ariaLabel="Default reading direction"
          disabled={update.isPending}
        />
      </SettingsSection>

      <SettingsSection
        title="Fit mode"
        description="How a single page sizes inside the viewport."
      >
        <SegmentedControl
          value={fitFromMe(data.default_fit_mode)}
          onChange={(next) =>
            patch({
              default_fit_mode: toReq(
                next,
              ) as PreferencesReq["default_fit_mode"],
            })
          }
          options={fitOptions}
          ariaLabel="Default fit mode"
          disabled={update.isPending}
        />
      </SettingsSection>

      <SettingsSection
        title="View mode"
        description="Default layout. Auto picks single, double, or webtoon based on the series' page metadata."
      >
        <SegmentedControl
          value={viewFromMe(data.default_view_mode)}
          onChange={(next) =>
            patch({
              default_view_mode: toReq(
                next,
              ) as PreferencesReq["default_view_mode"],
            })
          }
          options={viewOptions}
          ariaLabel="Default view mode"
          disabled={update.isPending}
        />
      </SettingsSection>

      <SettingsSection
        title="Page strip"
        description="Open the reader with the page-thumbnail strip already visible."
      >
        <div className="flex items-center justify-between gap-6">
          <div className="space-y-0.5">
            <p className="text-foreground text-sm font-medium">
              Show page strip on open
            </p>
            <p className="text-muted-foreground text-sm">
              You can always toggle the strip with M while reading.
            </p>
          </div>
          <Switch
            checked={data.default_page_strip === true}
            onCheckedChange={(v) => patch({ default_page_strip: v })}
            disabled={update.isPending}
            aria-label="Show page strip by default"
          />
        </div>
      </SettingsSection>

      <SettingsSection
        title="Spreads"
        description="How double-page view aligns pairs around the cover and any inline spreads."
      >
        <div className="flex items-center justify-between gap-6">
          <div className="space-y-0.5">
            <p className="text-foreground text-sm font-medium">
              First page is cover (always solo)
            </p>
            <p className="text-muted-foreground text-sm">
              Aligns pairs around the cover, like a printed comic. Per-series
              overrides still win.
            </p>
          </div>
          <Switch
            checked={data.default_cover_solo !== false}
            onCheckedChange={(v) => patch({ default_cover_solo: v })}
            disabled={update.isPending}
            aria-label="First page is cover by default"
          />
        </div>
      </SettingsSection>

      <ActivityThresholdsCard />

      <SeriesOverridesCard />
    </div>
  );
}

/**
 * Per-user thresholds that gate when a reading session is recorded. The
 * server validates these on PATCH and again on every session upsert, so a
 * tampered client can't slip below the floor.
 */
function ActivityThresholdsCard() {
  const me = useMe();
  const update = useUpdatePreferences({ silent: true });

  if (me.isLoading) return <Skeleton className="h-40 w-full" />;
  if (me.error || !me.data) return null;

  const data = me.data;
  const minActiveSec = Math.round(
    (data.reading_min_active_ms ?? 30_000) / 1000,
  );
  const idleSec = Math.round((data.reading_idle_ms ?? 180_000) / 1000);
  const minPages = data.reading_min_pages ?? 3;

  function commit(body: PreferencesReq) {
    update.mutate(body);
  }

  return (
    <SettingsSection
      title="Activity tracking thresholds"
      description="A session is only recorded if you're active for at least this long AND turn at least this many distinct pages. The idle timeout ends a session when you stop interacting."
    >
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
        <div className="space-y-1">
          <Label htmlFor="min-active">Minimum active time (seconds)</Label>
          <Input
            id="min-active"
            type="number"
            min={1}
            max={600}
            step={5}
            defaultValue={minActiveSec}
            onBlur={(e) => {
              const v = Number(e.currentTarget.value);
              if (!Number.isFinite(v) || v < 1 || v > 600) return;
              commit({ reading_min_active_ms: Math.round(v * 1000) });
            }}
            disabled={update.isPending}
          />
          <p className="text-muted-foreground text-xs">1–600 seconds.</p>
        </div>
        <div className="space-y-1">
          <Label htmlFor="min-pages">Minimum distinct pages</Label>
          <Input
            id="min-pages"
            type="number"
            min={1}
            max={200}
            step={1}
            defaultValue={minPages}
            onBlur={(e) => {
              const v = Number(e.currentTarget.value);
              if (!Number.isFinite(v) || v < 1 || v > 200) return;
              commit({ reading_min_pages: v });
            }}
            disabled={update.isPending}
          />
          <p className="text-muted-foreground text-xs">1–200 pages.</p>
        </div>
        <div className="space-y-1">
          <Label htmlFor="idle">Idle timeout (seconds)</Label>
          <Input
            id="idle"
            type="number"
            min={30}
            max={1800}
            step={10}
            defaultValue={idleSec}
            onBlur={(e) => {
              const v = Number(e.currentTarget.value);
              if (!Number.isFinite(v) || v < 30 || v > 1800) return;
              commit({ reading_idle_ms: Math.round(v * 1000) });
            }}
            disabled={update.isPending}
          />
          <p className="text-muted-foreground text-xs">30–1800 seconds.</p>
        </div>
      </div>
    </SettingsSection>
  );
}

/**
 * Lists per-series overrides stored in localStorage and lets the user clear
 * them — one at a time, or in bulk for orphans (entries pointing at a
 * series that no longer exists in the database, typically left over from a
 * dev DB wipe). Best-effort introspection: the reader writes keys named
 * `reader:<slice>:<seriesId>` and we surface every distinct id we find.
 */
function SeriesOverridesCard() {
  const [overrides, setOverrides] = useState<string[]>([]);
  const refresh = useMemo(
    () => () => {
      if (typeof window === "undefined") return;
      const found = new Set<string>();
      for (let i = 0; i < window.localStorage.length; i += 1) {
        const k = window.localStorage.key(i);
        if (!k?.startsWith(SERIES_OVERRIDE_PREFIX)) continue;
        const parts = k.split(":");
        if (parts.length < 3) continue;
        const seriesId = parts[2];
        if (seriesId && seriesId !== "_default") found.add(seriesId);
      }
      setOverrides([...found].sort());
    },
    [],
  );
  useEffect(() => {
    // SSR-safe: localStorage isn't readable on the server, so the initial
    // render shows "no overrides" until we re-read post-mount. Calling the
    // setter inside the effect is intentional here.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    refresh();
  }, [refresh]);

  // Single batched lookup feeds every row + the "clear unknown" derivation,
  // sharing TanStack's per-id cache with anywhere else `useSeries(id)` is
  // called.
  const queries = useSeriesMany(overrides);

  // Only treat confirmed 404s as orphans — a transient network error or 5xx
  // shouldn't be conflated with "this series is gone." The HttpError class
  // surfaces the status from `jsonFetch` so we can be precise here.
  const missingIds = useMemo(() => {
    const out: string[] = [];
    for (let i = 0; i < overrides.length; i += 1) {
      const q = queries[i];
      const err = q?.error;
      if (err instanceof HttpError && err.status === 404) {
        out.push(overrides[i]!);
      }
    }
    return out;
  }, [overrides, queries]);

  function clearMany(ids: readonly string[]) {
    if (typeof window === "undefined" || ids.length === 0) return;
    const targets = new Set(ids);
    const toRemove: string[] = [];
    for (let i = 0; i < window.localStorage.length; i += 1) {
      const k = window.localStorage.key(i);
      if (!k?.startsWith(SERIES_OVERRIDE_PREFIX)) continue;
      const parts = k.split(":");
      if (parts.length < 3) continue;
      const seriesId = parts[2];
      if (seriesId && targets.has(seriesId)) toRemove.push(k);
    }
    for (const k of toRemove) window.localStorage.removeItem(k);
    refresh();
  }

  return (
    <SettingsSection
      title="Per-series overrides"
      description="Series you've manually adjusted in the reader. Clearing an override returns the series to your defaults."
    >
      {overrides.length === 0 ? (
        <p className="text-muted-foreground text-sm">
          No per-series overrides stored.
        </p>
      ) : (
        <div className="space-y-3">
          {missingIds.length > 0 && (
            <div className="border-border bg-muted/30 flex items-center justify-between gap-3 rounded-md border border-dashed px-3 py-2">
              <p className="text-muted-foreground text-xs">
                {missingIds.length}{" "}
                {missingIds.length === 1 ? "entry points" : "entries point"} at
                a series that no longer exists in your library.
              </p>
              <Button
                type="button"
                size="sm"
                variant="outline"
                onClick={() => clearMany(missingIds)}
              >
                Clear unknown
              </Button>
            </div>
          )}
          <ul className="divide-border divide-y">
            {overrides.map((seriesId, idx) => (
              <SeriesOverrideRow
                key={seriesId}
                seriesId={seriesId}
                query={queries[idx]!}
                onClear={(id) => clearMany([id])}
              />
            ))}
          </ul>
        </div>
      )}
    </SettingsSection>
  );
}

/**
 * One row in the SeriesOverridesCard. The series → name lookup is hoisted
 * to the parent so a single batched `useSeriesMany` powers every row plus
 * the "clear unknown" derivation. A row falls back to a short UUID hint
 * when the lookup 404s (orphan), so the user can still distinguish entries
 * before clearing them.
 */
function SeriesOverrideRow({
  seriesId,
  query,
  onClear,
}: {
  seriesId: string;
  query: UseQueryResult<SeriesView>;
  onClear: (id: string) => void;
}) {
  const isMissing =
    query.error instanceof HttpError && query.error.status === 404;
  const name = query.data?.name;
  return (
    <li className="flex items-center justify-between gap-3 py-2 text-sm">
      <span className="min-w-0 flex-1 truncate">
        {name ? (
          <span className="text-foreground">{name}</span>
        ) : isMissing ? (
          <span className="text-muted-foreground">
            Unknown series{" "}
            <code className="text-muted-foreground/70 font-mono text-xs">
              {shortId(seriesId)}
            </code>
          </span>
        ) : (
          <span className="text-muted-foreground/60">Loading…</span>
        )}
      </span>
      <Button
        type="button"
        size="sm"
        variant="outline"
        onClick={() => onClear(seriesId)}
      >
        Reset
      </Button>
    </li>
  );
}

function shortId(id: string): string {
  return id.length > 8 ? `${id.slice(0, 8)}…` : id;
}
