"use client";

import { useWakePreference } from "@/lib/reader/use-wake-lock";
import Link from "next/link";
import { useEffect, useId, useState } from "react";
import { Keyboard, RotateCcw } from "lucide-react";
import { useReaderStore, type FitMode } from "@/lib/reader/store";
import type { Direction, ViewMode } from "@/lib/reader/detect";
import { SegmentedControl } from "@/components/settings/SegmentedControl";
import { Separator } from "@/components/ui/separator";
import { Slider } from "@/components/folio/slider";
import { Switch } from "@/components/ui/switch";
import {
  FILTER_RANGES,
  type FilterRange,
  isDefaultFilters,
} from "@/lib/reader/filters";
import {
  hasSeriesOverrides,
  clearSeriesOverrides,
} from "@/lib/reader/series-overrides";
import {
  EMPTY_SPREAD_OVERRIDES,
  isEmptySpreadOverrides,
} from "@/lib/reader/spreads";
import { useSpreadOverrides } from "@/lib/reader/use-spread-overrides";
import {
  useResetIssuePageOverrides,
  useSetIssuePageOverrides,
  useResetSeriesReaderPrefs,
} from "@/lib/api/mutations";

const VIEW_OPTIONS: ReadonlyArray<{ value: ViewMode; label: string }> = [
  { value: "single", label: "Single" },
  { value: "double", label: "Double" },
  { value: "webtoon", label: "Webtoon" },
];

const FIT_OPTIONS: ReadonlyArray<{ value: FitMode; label: string }> = [
  { value: "width", label: "Width" },
  { value: "height", label: "Height" },
  { value: "original", label: "Original" },
  { value: "contain", label: "Screen" },
];

/** Webtoon is width-fit by construction, so the reader renders `contain`
 *  as `width` there (audit N-R10). The option stays visible but greyed
 *  out, with the reason under the control, rather than silently doing
 *  something else. */
export const WEBTOON_CONTAIN_HINT =
  "Webtoon always fits the page to the width; Screen fit applies in single and double view.";
const FIT_OPTIONS_WEBTOON: ReadonlyArray<{
  value: FitMode;
  label: string;
  disabled?: boolean;
  title?: string;
}> = FIT_OPTIONS.map((o) =>
  o.value === "contain"
    ? { ...o, disabled: true, title: WEBTOON_CONTAIN_HINT }
    : o,
);

const DIRECTION_OPTIONS: ReadonlyArray<{ value: Direction; label: string }> = [
  { value: "ltr", label: "LTR" },
  { value: "rtl", label: "RTL" },
];

/**
 * Contents of the reader's settings Popover (M2). Surfaces the same options
 * the standalone `/settings/reading` page exposes, scoped to the current
 * session — direction/fit/view are per-series, the strip + auto-hide
 * toggles are per-tab.
 */
export function ReaderSettings({ seriesId }: { seriesId: string | null }) {
  const awake = useWakePreference();
  const fitMode = useReaderStore((s) => s.fitMode);
  const viewMode = useReaderStore((s) => s.viewMode);
  const direction = useReaderStore((s) => s.direction);
  const pageStripVisible = useReaderStore((s) => s.pageStripVisible);
  const chromeAutoHide = useReaderStore((s) => s.chromeAutoHide);
  const brightness = useReaderStore((s) => s.brightness);
  const sepia = useReaderStore((s) => s.sepia);
  const grayscale = useReaderStore((s) => s.grayscale);
  const invert = useReaderStore((s) => s.invert);
  const warmth = useReaderStore((s) => s.warmth);
  const contrast = useReaderStore((s) => s.contrast);
  const coverSolo = useReaderStore((s) => s.coverSolo);
  const markersHidden = useReaderStore((s) => s.markersHidden);
  const zoomPersist = useReaderStore((s) => s.zoomPersist);
  const setFitMode = useReaderStore((s) => s.setFitMode);
  const setViewMode = useReaderStore((s) => s.setViewMode);
  const setDirection = useReaderStore((s) => s.setDirection);
  const togglePageStrip = useReaderStore((s) => s.togglePageStrip);
  const setChromeAutoHide = useReaderStore((s) => s.setChromeAutoHide);
  const setBrightness = useReaderStore((s) => s.setBrightness);
  const setSepia = useReaderStore((s) => s.setSepia);
  const setGrayscale = useReaderStore((s) => s.setGrayscale);
  const setInvert = useReaderStore((s) => s.setInvert);
  const setWarmth = useReaderStore((s) => s.setWarmth);
  const setContrast = useReaderStore((s) => s.setContrast);
  const resetFilters = useReaderStore((s) => s.resetFilters);
  const setCoverSolo = useReaderStore((s) => s.setCoverSolo);
  const setMarkersHidden = useReaderStore((s) => s.setMarkersHidden);
  const setZoomPersist = useReaderStore((s) => s.setZoomPersist);
  const fitHintId = useId();

  return (
    <div className="space-y-4 text-sm">
      <Section title="Display">
        <SwitchRow
          label="Keep screen awake"
          description="While reading, when allowed by your device."
          checked={awake.enabled}
          onChange={awake.setEnabled}
        />
        <Field label="View">
          <SegmentedControl
            value={viewMode}
            onChange={setViewMode}
            options={VIEW_OPTIONS}
            ariaLabel="View mode"
          />
        </Field>
        <Field label="Fit">
          <SegmentedControl
            // Webtoon shows the fit the reader actually applies: a stored
            // `contain` renders as width there, so Width is the pressed pill.
            value={
              viewMode === "webtoon" && fitMode === "contain"
                ? "width"
                : fitMode
            }
            onChange={setFitMode}
            options={viewMode === "webtoon" ? FIT_OPTIONS_WEBTOON : FIT_OPTIONS}
            ariaLabel="Fit mode"
            ariaDescribedBy={viewMode === "webtoon" ? fitHintId : undefined}
          />
        </Field>
        {viewMode === "webtoon" ? (
          <p id={fitHintId} className="text-xs text-neutral-400">
            {WEBTOON_CONTAIN_HINT}
          </p>
        ) : null}
        <SwitchRow
          label="Keep zoom between pages"
          description="Turning the page keeps your zoom level and starts at the top corner of the next page."
          checked={zoomPersist}
          onChange={setZoomPersist}
        />
        <Field label="Direction">
          <SegmentedControl
            value={direction}
            onChange={setDirection}
            options={DIRECTION_OPTIONS}
            ariaLabel="Reading direction"
          />
        </Field>
      </Section>

      <Separator className="bg-neutral-800" />

      <Section title="Overlay">
        <SwitchRow
          label="Page strip"
          description="Thumbnail strip at the bottom."
          checked={pageStripVisible}
          onChange={togglePageStrip}
        />
        <SwitchRow
          label="Auto-hide chrome"
          description="Hide controls after a few seconds of no input."
          checked={chromeAutoHide}
          onChange={setChromeAutoHide}
        />
        <SwitchRow
          label="Hide markers"
          description="Hide bookmark / note / highlight overlays while reading. Your markers stay saved."
          checked={markersHidden}
          onChange={setMarkersHidden}
        />
      </Section>

      {viewMode === "double" ? (
        <>
          <Separator className="bg-neutral-800" />
          <Section title="Spreads">
            <SwitchRow
              label="First page is cover (always solo)"
              description="Aligns pairs around the cover, like a printed comic."
              checked={coverSolo}
              onChange={setCoverSolo}
            />
            <IssueSpreadOverrides />
          </Section>
        </>
      ) : null}

      <Separator className="bg-neutral-800" />

      <Section title="Vision">
        <SwitchRow
          label="Night mode"
          description="Invert the page colours: black on white becomes white on black."
          checked={invert}
          onChange={setInvert}
        />
        <FilterSlider
          label="Brightness"
          value={brightness}
          display={`${Math.round(brightness * 100)}%`}
          range={FILTER_RANGES.brightness}
          onChange={setBrightness}
        />
        <FilterSlider
          label="Contrast"
          value={contrast}
          display={`${contrast}%`}
          range={FILTER_RANGES.contrast}
          onChange={setContrast}
        />
        <FilterSlider
          label="Grayscale"
          value={grayscale}
          display={`${grayscale}%`}
          range={FILTER_RANGES.grayscale}
          onChange={setGrayscale}
        />
        <FilterSlider
          label="Sepia"
          value={sepia}
          display={`${Math.round(sepia * 100)}%`}
          range={FILTER_RANGES.sepia}
          onChange={setSepia}
        />
        <FilterSlider
          label="Warmth"
          value={warmth}
          display={warmthLabel(warmth)}
          range={FILTER_RANGES.warmth}
          onChange={setWarmth}
        />
        {!isDefaultFilters({
          brightness,
          sepia,
          grayscale,
          invert,
          warmth,
          contrast,
        }) ? (
          <button
            type="button"
            onClick={resetFilters}
            className="inline-flex items-center gap-2 text-xs text-neutral-400 underline-offset-4 hover:text-neutral-200 hover:underline"
          >
            <RotateCcw className="size-3" />
            Reset filters
          </button>
        ) : null}
      </Section>

      <Separator className="bg-neutral-800" />

      <Section title="Shortcuts">
        <Link
          href={`/settings/keybinds`}
          className="inline-flex items-center gap-2 text-xs text-neutral-300 underline-offset-4 hover:text-neutral-100 hover:underline"
        >
          <Keyboard className="size-3.5" />
          Customize keyboard shortcuts
        </Link>
      </Section>

      <SeriesOverridesRow seriesId={seriesId} />
    </div>
  );
}

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="space-y-2">
      <h3 className="text-xs font-semibold tracking-[0.12em] text-neutral-400 uppercase">
        {title}
      </h3>
      <div className="space-y-2">{children}</div>
    </section>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-3">
      <span className="text-xs text-neutral-300">{label}</span>
      <div className="flex justify-end">{children}</div>
    </div>
  );
}

/** "Warmer +20" / "Cooler −20" / "Neutral": the sign alone reads badly
 *  next to a percentage column. */
export function warmthLabel(warmth: number): string {
  if (warmth === 0) return "Neutral";
  return warmth > 0 ? `Warmer +${warmth}` : `Cooler −${-warmth}`;
}

/** One colour-filter row: visible label + current value, and a slider
 *  whose thumb carries the same label (the `Slider` wrapper names the
 *  thumb; the visible text is for sighted users). */
function FilterSlider({
  label,
  value,
  display,
  range,
  onChange,
}: {
  label: string;
  value: number;
  display: string;
  range: FilterRange;
  onChange: (next: number) => void;
}) {
  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between text-xs text-neutral-300">
        <span>{label}</span>
        <span className="text-neutral-400">{display}</span>
      </div>
      <Slider
        value={[value]}
        min={range.min}
        max={range.max}
        step={range.step}
        onValueChange={([next]) =>
          typeof next === "number" ? onChange(next) : undefined
        }
        thumbLabel={label}
      />
    </div>
  );
}

function SwitchRow({
  label,
  description,
  checked,
  onChange,
}: {
  label: string;
  description?: string;
  checked: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <div className="flex items-center justify-between gap-3">
      <div className="space-y-0.5">
        <p className="text-xs font-medium text-neutral-200">{label}</p>
        {description ? (
          <p className="text-xs text-neutral-400">{description}</p>
        ) : null}
      </div>
      <Switch checked={checked} onCheckedChange={onChange} aria-label={label} />
    </div>
  );
}

/**
 * WP-4.3 per-issue spread controls: the "shift pairing by one" switch
 * and a reset for every manual override on this issue (the per-page
 * spread/single toggles live on the page strip). Saved to the server
 * per user, so they follow the reader to other devices.
 */
function IssueSpreadOverrides() {
  const issueId = useReaderStore((s) => s.issueId);
  const overrides = useSpreadOverrides(issueId);
  const setOverrides = useSetIssuePageOverrides(issueId);
  const resetOverrides = useResetIssuePageOverrides(issueId);
  if (!issueId) return null;
  const shift = overrides?.shift_pairing ?? false;
  const pageCount =
    (overrides?.spread_pages.length ?? 0) +
    (overrides?.single_pages.length ?? 0);
  return (
    <>
      <SwitchRow
        label="Shift pairing by one"
        description="For this issue: fixes scans where every pair is off by a page. Mark single pages as spreads from the page strip."
        checked={shift}
        onChange={(next) =>
          setOverrides.mutate({
            ...(overrides ?? EMPTY_SPREAD_OVERRIDES),
            shift_pairing: next,
          })
        }
      />
      {!isEmptySpreadOverrides(overrides) ? (
        <button
          type="button"
          onClick={() => resetOverrides.mutate()}
          disabled={resetOverrides.isPending}
          className="inline-flex items-center gap-2 text-xs text-neutral-500 underline-offset-4 hover:text-neutral-200 hover:underline disabled:opacity-50"
        >
          <RotateCcw className="size-3" />
          Reset spread overrides for this issue
          {pageCount > 0
            ? ` (${pageCount} page${pageCount === 1 ? "" : "s"})`
            : ""}
        </button>
      ) : null}
    </>
  );
}

function SeriesOverridesRow({ seriesId }: { seriesId: string | null }) {
  const [hasOverrides, setHasOverrides] = useState(false);
  // Post-audit T4.4: the overrides also live on the account; a reset
  // clears both or the next open would mirror them straight back.
  const resetServer = useResetSeriesReaderPrefs(seriesId ?? "");

  useEffect(() => {
    if (!seriesId || typeof window === "undefined") return;
    // SSR-safe: localStorage isn't readable on the server; we re-read post-mount
    // (matches the SeriesOverridesCard pattern in `ReadingPrefs.tsx`).
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setHasOverrides(hasSeriesOverrides(window.localStorage, seriesId));
  }, [seriesId]);

  if (!seriesId || !hasOverrides) return null;

  function reset() {
    if (!seriesId || typeof window === "undefined") return;
    clearSeriesOverrides(window.localStorage, seriesId);
    setHasOverrides(false);
    resetServer.mutate(undefined);
    // Don't mutate the active store — the reset takes effect on next mount,
    // matching the behavior of the standalone settings page.
  }

  return (
    <button
      type="button"
      onClick={reset}
      className="inline-flex items-center gap-2 text-xs text-neutral-400 underline-offset-4 hover:text-neutral-200 hover:underline"
    >
      <RotateCcw className="size-3" />
      Reset this series to defaults
    </button>
  );
}
