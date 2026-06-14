"use client";

import { cn } from "@/lib/utils";

/**
 * Single-select pill group. Used by the settings forms in place of a
 * `<select>` because the option count is small and visible-at-a-glance is
 * the right ergonomics here.
 *
 * Generic over the value type so callers stay typed against their union
 * (e.g. `Direction = 'auto' | 'ltr' | 'rtl'`).
 */
export function SegmentedControl<T extends string>({
  value,
  onChange,
  options,
  ariaLabel,
  disabled,
}: {
  value: T;
  onChange: (next: T) => void;
  options: ReadonlyArray<{ value: T; label: string }>;
  ariaLabel: string;
  disabled?: boolean;
}) {
  return (
    <div
      role="group"
      aria-label={ariaLabel}
      // `max-w-full` + `overflow-x-auto` keep a long option set (e.g. the
      // per-series ranking dimensions: Writers…Cover artists + Genres + Tags)
      // from spilling off-screen on mobile — it scrolls within the control
      // instead of widening the page. No-op when the options already fit.
      // `[&>*]:shrink-0` stops the pills squishing; scrollbar hidden to match
      // the TabsList pattern.
      className="border-input bg-background inline-flex max-w-full overflow-x-auto rounded-md border p-0.5 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden [&>*]:shrink-0"
    >
      {options.map((opt) => {
        const active = opt.value === value;
        return (
          <button
            key={opt.value}
            aria-pressed={active}
            type="button"
            disabled={disabled}
            onClick={() => onChange(opt.value)}
            className={cn(
              "relative rounded px-3 py-1.5 text-sm font-medium transition-colors",
              "focus-visible:ring-ring focus-visible:ring-2 focus-visible:ring-offset-1 focus-visible:outline-none",
              "disabled:cursor-not-allowed disabled:opacity-50",
              active
                ? "bg-primary text-primary-foreground shadow-sm"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            {opt.label}
          </button>
        );
      })}
    </div>
  );
}
