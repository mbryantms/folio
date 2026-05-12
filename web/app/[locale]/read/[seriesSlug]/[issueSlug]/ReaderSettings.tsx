"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { Keyboard, RotateCcw } from "lucide-react";
import { useReaderStore, type FitMode } from "@/lib/reader/store";
import type { Direction, ViewMode } from "@/lib/reader/detect";
import { SegmentedControl } from "@/components/settings/SegmentedControl";
import { Separator } from "@/components/ui/separator";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import {
  hasSeriesOverrides,
  clearSeriesOverrides,
} from "@/lib/reader/series-overrides";

const VIEW_OPTIONS: ReadonlyArray<{ value: ViewMode; label: string }> = [
  { value: "single", label: "Single" },
  { value: "double", label: "Double" },
  { value: "webtoon", label: "Webtoon" },
];

const FIT_OPTIONS: ReadonlyArray<{ value: FitMode; label: string }> = [
  { value: "width", label: "Width" },
  { value: "height", label: "Height" },
  { value: "original", label: "Original" },
];

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
  const fitMode = useReaderStore((s) => s.fitMode);
  const viewMode = useReaderStore((s) => s.viewMode);
  const direction = useReaderStore((s) => s.direction);
  const pageStripVisible = useReaderStore((s) => s.pageStripVisible);
  const chromeAutoHide = useReaderStore((s) => s.chromeAutoHide);
  const brightness = useReaderStore((s) => s.brightness);
  const sepia = useReaderStore((s) => s.sepia);
  const coverSolo = useReaderStore((s) => s.coverSolo);
  const markersHidden = useReaderStore((s) => s.markersHidden);
  const setFitMode = useReaderStore((s) => s.setFitMode);
  const setViewMode = useReaderStore((s) => s.setViewMode);
  const setDirection = useReaderStore((s) => s.setDirection);
  const togglePageStrip = useReaderStore((s) => s.togglePageStrip);
  const setChromeAutoHide = useReaderStore((s) => s.setChromeAutoHide);
  const setBrightness = useReaderStore((s) => s.setBrightness);
  const setSepia = useReaderStore((s) => s.setSepia);
  const setCoverSolo = useReaderStore((s) => s.setCoverSolo);
  const setMarkersHidden = useReaderStore((s) => s.setMarkersHidden);

  return (
    <div className="space-y-4 text-sm">
      <Section title="Display">
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
            value={fitMode}
            onChange={setFitMode}
            options={FIT_OPTIONS}
            ariaLabel="Fit mode"
          />
        </Field>
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
          </Section>
        </>
      ) : null}

      <Separator className="bg-neutral-800" />

      <Section title="Vision">
        <div className="space-y-1">
          <div className="flex items-center justify-between text-xs text-neutral-300">
            <span>Brightness</span>
            <span className="text-neutral-500">
              {Math.round(brightness * 100)}%
            </span>
          </div>
          <Slider
            value={[brightness]}
            min={0.5}
            max={1.5}
            step={0.05}
            onValueChange={([next]) =>
              typeof next === "number" ? setBrightness(next) : undefined
            }
            aria-label="Brightness"
          />
        </div>
        <div className="space-y-1">
          <div className="flex items-center justify-between text-xs text-neutral-300">
            <span>Sepia</span>
            <span className="text-neutral-500">{Math.round(sepia * 100)}%</span>
          </div>
          <Slider
            value={[sepia]}
            min={0}
            max={1}
            step={0.05}
            onValueChange={([next]) =>
              typeof next === "number" ? setSepia(next) : undefined
            }
            aria-label="Sepia"
          />
        </div>
        {brightness !== 1 || sepia !== 0 ? (
          <button
            type="button"
            onClick={() => {
              setBrightness(1);
              setSepia(0);
            }}
            className="inline-flex items-center gap-2 text-[11px] text-neutral-500 underline-offset-4 hover:text-neutral-200 hover:underline"
          >
            <RotateCcw className="size-3" />
            Reset vision
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
      <h3 className="text-[10px] font-semibold tracking-[0.12em] text-neutral-500 uppercase">
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
          <p className="text-[11px] text-neutral-500">{description}</p>
        ) : null}
      </div>
      <Switch checked={checked} onCheckedChange={onChange} aria-label={label} />
    </div>
  );
}

function SeriesOverridesRow({ seriesId }: { seriesId: string | null }) {
  const [hasOverrides, setHasOverrides] = useState(false);

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
    // Don't mutate the active store — the reset takes effect on next mount,
    // matching the behavior of the standalone settings page.
  }

  return (
    <button
      type="button"
      onClick={reset}
      className="inline-flex items-center gap-2 text-[11px] text-neutral-500 underline-offset-4 hover:text-neutral-200 hover:underline"
    >
      <RotateCcw className="size-3" />
      Reset this series to defaults
    </button>
  );
}
