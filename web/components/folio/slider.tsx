"use client";

import * as React from "react";
import { Slider as SliderPrimitive } from "radix-ui";

import { cn } from "@/lib/utils";

/**
 * Folio wrapper over the vendored `ui/slider`: the same structure and
 * classes, plus an accessible name for the thumb(s).
 *
 * Radix only names a thumb by itself when a slider has several
 * ("Minimum" / "Maximum"); a single thumb — every Folio slider — has
 * `role="slider"` and no name, which axe reports as
 * `aria-input-field-name` (WCAG 4.1.2). The vendored component renders
 * the thumbs itself and offers no way to label them, so the wrapper
 * re-renders the primitive. `thumbLabel` is required: the visible label
 * beside the control is for sighted users, the thumb needs its own.
 *
 * Import from here, not `@/components/ui/slider` (lint-enforced).
 */
export type SliderProps = React.ComponentProps<typeof SliderPrimitive.Root> & {
  /** Accessible name of the thumb; one per thumb for range sliders. */
  thumbLabel: string | string[];
};

export function Slider({
  className,
  defaultValue,
  value,
  min = 0,
  max = 100,
  thumbLabel,
  ...props
}: SliderProps) {
  const _values = React.useMemo(
    () =>
      Array.isArray(value)
        ? value
        : Array.isArray(defaultValue)
          ? defaultValue
          : [min, max],
    [value, defaultValue, min, max],
  );
  const labels = Array.isArray(thumbLabel) ? thumbLabel : [thumbLabel];

  return (
    <SliderPrimitive.Root
      data-slot="slider"
      defaultValue={defaultValue}
      value={value}
      min={min}
      max={max}
      className={cn(
        "relative flex w-full touch-none items-center select-none data-disabled:opacity-50 data-vertical:h-full data-vertical:min-h-40 data-vertical:w-auto data-vertical:flex-col",
        className,
      )}
      {...props}
    >
      <SliderPrimitive.Track
        data-slot="slider-track"
        className="bg-muted relative grow overflow-hidden rounded-full data-horizontal:h-1.5 data-horizontal:w-full data-vertical:h-full data-vertical:w-1.5"
      >
        <SliderPrimitive.Range
          data-slot="slider-range"
          className="bg-primary absolute select-none data-horizontal:h-full data-vertical:w-full"
        />
      </SliderPrimitive.Track>
      {Array.from({ length: _values.length }, (_, index) => (
        <SliderPrimitive.Thumb
          data-slot="slider-thumb"
          key={index}
          aria-label={labels[index] ?? labels[0]}
          className="border-primary ring-ring/50 block size-4 shrink-0 rounded-full border bg-white shadow-sm transition-[color,box-shadow] select-none hover:ring-4 focus-visible:ring-4 focus-visible:outline-hidden disabled:pointer-events-none disabled:opacity-50"
        />
      ))}
    </SliderPrimitive.Root>
  );
}
