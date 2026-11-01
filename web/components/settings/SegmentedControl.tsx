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
  ariaDescribedBy,
  disabled,
  fill = false,
  className,
}: {
  value: T;
  onChange: (next: T) => void;
  /** `disabled` greys one option out while the rest stay live (the
   *  caller explains why beside the control); `title` is its hover
   *  text. */
  options: ReadonlyArray<{
    value: T;
    label: string;
    disabled?: boolean;
    title?: string;
  }>;
  ariaLabel: string;
  /** Id of an element explaining the group (e.g. why an option is
   *  disabled); set as `aria-describedby` on the group. */
  ariaDescribedBy?: string;
  disabled?: boolean;
  /** Span the parent's width with equal-width segments whose labels
   *  truncate instead of overflowing. For a narrow container with a fixed
   *  option set (the reader settings popover on a phone); the default
   *  sizes to content and scrolls when it can't fit. */
  fill?: boolean;
  /** Extra classes on the group wrapper — e.g. `h-9` when the control
   *  sits in a toolbar row beside `<Button>` / `<Input>` siblings. */
  className?: string;
}) {
  return (
    <div
      role="group"
      aria-label={ariaLabel}
      aria-describedby={ariaDescribedBy}
      // Default: `max-w-full` + `overflow-x-auto` keep a long option set
      // (e.g. the per-series ranking dimensions: Writers…Cover artists +
      // Genres + Tags) from spilling off-screen on mobile — it scrolls
      // within the control instead of widening the page. No-op when the
      // options already fit. `[&>*]:shrink-0` stops the pills squishing;
      // scrollbar hidden to match the TabsList pattern.
      //
      // `fill`: the group is the row's width and the segments share it
      // equally (`flex-1 min-w-0`), so nothing can overflow and the group
      // needs no scroll container (which would clip the focus rings).
      className={cn(
        "border-input bg-background rounded-md border p-0.5",
        fill
          ? "flex w-full"
          : // `overflow-y-hidden`: see `TabsList` — the segments' touch tap
            // area would otherwise give the strip a few px of vertical scroll.
            "inline-flex max-w-full [scrollbar-width:none] overflow-x-auto overflow-y-hidden [&::-webkit-scrollbar]:hidden [&>*]:shrink-0",
        className,
      )}
    >
      {options.map((opt) => {
        const active = opt.value === value;
        return (
          <button
            key={opt.value}
            aria-pressed={active}
            type="button"
            disabled={disabled || opt.disabled}
            title={opt.title}
            onClick={() => onChange(opt.value)}
            className={cn(
              "folio-tap relative rounded py-1.5 text-sm font-medium transition-colors",
              // The label truncates in a span, not on the button: clipping
              // the button would also clip its touch tap area (`::after`).
              fill ? "min-w-0 flex-1 px-1.5" : "px-3",
              "focus-visible:ring-ring focus-visible:ring-2 focus-visible:ring-offset-1 focus-visible:outline-none",
              "disabled:cursor-not-allowed disabled:opacity-50",
              active
                ? "bg-primary text-primary-foreground shadow-sm"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            {fill ? (
              <span className="block truncate">{opt.label}</span>
            ) : (
              opt.label
            )}
          </button>
        );
      })}
    </div>
  );
}
