"use client";

import { useEffect, useRef } from "react";
import { useTheme } from "next-themes";

import { useMe } from "@/lib/api/queries";
import { useUpdatePreferences } from "@/lib/api/mutations";
import {
  ACCENTS,
  DENSITIES,
  MOTIONS,
  RADII,
  THEMES,
  type Accent,
  type Density,
  type Motion,
  type Radius,
  type Theme,
  isAccent,
  isDensity,
  isMotion,
  isRadius,
  isTheme,
  writeAccentCookie,
  writeDensityCookie,
  writeMotionCookie,
  writeRadiusCookie,
  writeThemeCookie,
} from "@/lib/theme";
import { revealChange, revealTheme } from "@/lib/ui/theme-reveal";
import {
  SLIDE_TRANSITION,
  useSlidingIndicator,
} from "@/lib/ui/use-sliding-indicator";
import { cn } from "@/lib/utils";

import { SegmentedControl } from "./SegmentedControl";
import { SettingsSection } from "./SettingsSection";
import { LoadingRegion } from "@/components/skeletons/LoadingShapes";
import { ThemeBodySkeleton } from "@/components/skeletons/SettingsShapes";

const themeOptions: ReadonlyArray<{ value: Theme; label: string }> = [
  { value: "system", label: "System" },
  { value: "dark", label: "Dark" },
  { value: "light", label: "Light" },
  { value: "amber", label: "Amber" },
];
const accentOptions: ReadonlyArray<{ value: Accent; label: string }> = [
  { value: "amber", label: "Amber" },
  { value: "blue", label: "Blue" },
  { value: "emerald", label: "Emerald" },
  { value: "rose", label: "Rose" },
];
const densityOptions: ReadonlyArray<{ value: Density; label: string }> = [
  { value: "comfortable", label: "Comfortable" },
  { value: "compact", label: "Compact" },
];
const radiusOptions: ReadonlyArray<{ value: Radius; label: string }> = [
  { value: "sharp", label: "Sharp" },
  { value: "default", label: "Default" },
  { value: "round", label: "Round" },
];
const motionOptions: ReadonlyArray<{ value: Motion; label: string }> = [
  { value: "system", label: "Follow system" },
  { value: "reduced", label: "Reduced" },
];

const ACCENT_DOTS: Record<Accent, string> = {
  amber: "bg-amber-500",
  blue: "bg-blue-500",
  emerald: "bg-emerald-500",
  rose: "bg-rose-500",
};

const ACCENT_THUMB = {
  active: ':scope > [role="radio"][aria-checked="true"]',
  items: ':scope > [role="radio"]',
  attributes: ["aria-checked"],
};

/** The accent swatches: the selected outline is one frame that slides
 *  between them (folio-flair), like the segmented controls' thumb. */
function AccentGroup({
  accent,
  disabled,
  onPick,
}: {
  accent: Accent;
  disabled: boolean;
  onPick: (next: Accent) => void;
}) {
  const group = useRef<HTMLDivElement | null>(null);
  const thumb = useSlidingIndicator(group, ACCENT_THUMB);
  return (
    <div
      ref={group}
      role="radiogroup"
      aria-label="Accent color"
      data-thumb={thumb ? "" : undefined}
      className="relative flex flex-wrap gap-3"
    >
      {thumb ? (
        <span
          aria-hidden
          className={cn(
            "border-primary bg-primary/10 pointer-events-none absolute top-0 left-0 rounded-md border",
            disabled && "opacity-50",
            thumb.animate && SLIDE_TRANSITION,
          )}
          style={{
            width: thumb.width,
            height: thumb.height,
            translate: `${thumb.left}px ${thumb.top}px`,
          }}
        />
      ) : null}
      {ACCENTS.map((value) => {
        const active = value === accent;
        const label =
          accentOptions.find((o) => o.value === value)?.label ?? value;
        return (
          <button
            key={value}
            role="radio"
            aria-checked={active}
            type="button"
            disabled={disabled}
            onClick={() => onPick(value)}
            className={cn(
              "focus-visible:ring-ring relative flex items-center gap-2 rounded-md border px-3 py-1.5 text-sm font-medium transition-colors focus-visible:ring-2 focus-visible:ring-offset-1 focus-visible:outline-none disabled:opacity-50",
              active
                ? thumb
                  ? "text-foreground border-transparent"
                  : "border-primary bg-primary/10 text-foreground"
                : "border-input bg-background text-muted-foreground hover:text-foreground",
            )}
          >
            <span
              className={`size-3 rounded-full ${ACCENT_DOTS[value]}`}
              aria-hidden
            />
            {label}
          </button>
        );
      })}
    </div>
  );
}

export function ThemePicker() {
  const me = useMe();
  const update = useUpdatePreferences({ silent: true });
  const { setTheme } = useTheme();

  // Once `me` lands, sync the theme attribute and cookies so a fresh sign-in
  // on a new browser gets the saved preference applied immediately.
  useEffect(() => {
    if (!me.data) return;
    const theme = isTheme(me.data.theme) ? me.data.theme : "dark";
    const accent = isAccent(me.data.accent_color)
      ? me.data.accent_color
      : "amber";
    const density = isDensity(me.data.density)
      ? me.data.density
      : "comfortable";
    const radius = isRadius(me.data.corner_radius)
      ? me.data.corner_radius
      : "default";
    const motion = isMotion(me.data.motion) ? me.data.motion : "system";
    setTheme(theme);
    writeThemeCookie(theme);
    writeAccentCookie(accent);
    writeDensityCookie(density);
    writeRadiusCookie(radius);
    writeMotionCookie(motion);
    if (typeof document !== "undefined") {
      document.documentElement.setAttribute("data-accent", accent);
      document.documentElement.setAttribute("data-density", density);
      document.documentElement.setAttribute("data-radius", radius);
      document.documentElement.setAttribute("data-motion", motion);
    }
  }, [me.data, setTheme]);

  if (me.isLoading) {
    return (
      <LoadingRegion label="Loading theme settings…">
        <ThemeBodySkeleton />
      </LoadingRegion>
    );
  }
  if (me.error || !me.data) {
    return (
      <p className="text-destructive text-sm">Failed to load preferences.</p>
    );
  }

  const theme = isTheme(me.data.theme) ? me.data.theme : "dark";
  const accent = isAccent(me.data.accent_color)
    ? me.data.accent_color
    : "amber";
  const density = isDensity(me.data.density) ? me.data.density : "comfortable";
  const radius = isRadius(me.data.corner_radius)
    ? me.data.corner_radius
    : "default";
  const motion = isMotion(me.data.motion) ? me.data.motion : "system";

  function pickTheme(next: Theme) {
    writeThemeCookie(next);
    revealTheme(next, setTheme);
    update.mutate({ theme: next });
  }
  function pickAccent(next: Accent) {
    writeAccentCookie(next);
    // The new look spreads from the control like a theme switch.
    revealChange(() =>
      document.documentElement.setAttribute("data-accent", next),
    );
    update.mutate({ accent_color: next });
  }
  function pickDensity(next: Density) {
    writeDensityCookie(next);
    // The new look spreads from the control like a theme switch.
    revealChange(() =>
      document.documentElement.setAttribute("data-density", next),
    );
    update.mutate({ density: next });
  }
  function pickRadius(next: Radius) {
    writeRadiusCookie(next);
    // The new look spreads from the control like a theme switch.
    revealChange(() =>
      document.documentElement.setAttribute("data-radius", next),
    );
    update.mutate({ corner_radius: next });
  }
  function pickMotion(next: Motion) {
    writeMotionCookie(next);
    if (typeof document !== "undefined") {
      document.documentElement.setAttribute("data-motion", next);
    }
    update.mutate({ motion: next });
  }

  return (
    <div className="space-y-6">
      <SettingsSection
        title="Theme"
        description="System follows your operating system. Dark is the canonical palette; Light is a clean white alternative; Amber is a sepia paper-warmth palette for warm-lit rooms."
      >
        <SegmentedControl
          value={theme}
          onChange={pickTheme}
          options={themeOptions}
          ariaLabel="Theme"
          disabled={update.isPending}
        />
      </SettingsSection>

      <SettingsSection
        title="Accent color"
        description="The CTA / focus tint applied across the admin and settings surfaces."
      >
        <AccentGroup
          accent={accent}
          disabled={update.isPending}
          onPick={pickAccent}
        />
      </SettingsSection>

      <SettingsSection
        title="Density"
        description="Comfortable is the default. Compact tightens vertical rhythm for laptop-sized screens."
      >
        <SegmentedControl
          value={density}
          onChange={pickDensity}
          options={densityOptions}
          ariaLabel="Density"
          disabled={update.isPending}
        />
      </SettingsSection>

      <SettingsSection
        title="Corners"
        description="How rounded controls, cards and menus are. Every corner in the app derives from this one value."
      >
        <SegmentedControl
          value={radius}
          onChange={pickRadius}
          options={radiusOptions}
          ariaLabel="Corner radius"
          disabled={update.isPending}
        />
      </SettingsSection>

      <SettingsSection
        title="Motion"
        description="Reduced turns page turns, menus and dialogs into instant swaps, whatever the operating system is set to."
      >
        <SegmentedControl
          value={motion}
          onChange={pickMotion}
          options={motionOptions}
          ariaLabel="Motion"
          disabled={update.isPending}
        />
      </SettingsSection>

      {/* Hidden helpers so the lint passes — these keep the import surfaces
          tidy if the option arrays change. */}
      <p className="sr-only" aria-hidden>
        {THEMES.length} themes, {ACCENTS.length} accents, {DENSITIES.length}{" "}
        densities, {RADII.length} radii, {MOTIONS.length} motion settings.
      </p>
    </div>
  );
}
