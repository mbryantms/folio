"use client";

import { Star } from "lucide-react";
import * as React from "react";

import { cn } from "@/lib/utils";

const STARS = 5;

/**
 * Half-star-precision rating control. Hover the left half of a star for an
 * X.5 preview, the right half for X+1. Click commits. Backed by a `radio`
 * group so keyboard users can `Tab` to it and use arrow keys / digits 0-5
 * to pick a value (Shift+digit gives the half-step).
 *
 * The component is fully controlled: pass `value` (0..=5 in 0.5 steps, or
 * `null` for unset) and react to `onChange`. `readOnly` renders the same
 * filled stars but disables interaction — useful for surfaces that show a
 * rating without an editing affordance.
 *
 * Click the currently-selected value again to clear (a common rating-UI
 * convention; mirrors GitHub Stars / Letterboxd / Goodreads).
 */
export function RatingStars({
  value,
  onChange,
  size = "md",
  label = "Rating",
  readOnly = false,
  className,
}: {
  value: number | null;
  onChange?: (next: number | null) => void;
  size?: "sm" | "md" | "lg";
  /** Accessible group label. Surfaced via `aria-label` so screen readers
   *  can announce "Rating: 3.5 of 5 stars". */
  label?: string;
  readOnly?: boolean;
  className?: string;
}) {
  const [hover, setHover] = React.useState<number | null>(null);
  const display = hover ?? value ?? 0;
  const sizes = {
    sm: { star: "h-3.5 w-3.5", gap: "gap-0.5" },
    md: { star: "h-5 w-5", gap: "gap-0.5" },
    lg: { star: "h-7 w-7", gap: "gap-1" },
  } as const;
  const dim = sizes[size];

  const commit = (next: number) => {
    if (readOnly) return;
    // Click the same value to clear — a common rating-UI affordance.
    if (value !== null && Math.abs(value - next) < 0.001) {
      onChange?.(null);
    } else {
      onChange?.(next);
    }
  };

  return (
    <div
      role="radiogroup"
      aria-label={`${label}: ${value ?? 0} of ${STARS} stars`}
      aria-readonly={readOnly || undefined}
      className={cn(
        "inline-flex items-center",
        dim.gap,
        readOnly && "pointer-events-none",
        className,
      )}
      onMouseLeave={() => setHover(null)}
      onKeyDown={(e) => {
        if (readOnly) return;
        // Digit keys: 0..5 for whole stars, Shift+digit for half-step.
        if (/^[0-5]$/.test(e.key)) {
          e.preventDefault();
          const n = Number(e.key);
          commit(e.shiftKey && n > 0 ? n - 0.5 : n);
        }
        if (e.key === "ArrowRight" || e.key === "ArrowUp") {
          e.preventDefault();
          commit(Math.min(STARS, (value ?? 0) + 0.5));
        }
        if (e.key === "ArrowLeft" || e.key === "ArrowDown") {
          e.preventDefault();
          commit(Math.max(0, (value ?? 0) - 0.5));
        }
      }}
      tabIndex={readOnly ? -1 : 0}
    >
      {Array.from({ length: STARS }, (_, i) => {
        const starIndex = i + 1;
        const halfValue = starIndex - 0.5;
        const fullValue = starIndex;
        // Fill state for the rendered star: full ≥ starIndex, half if
        // exactly halfValue, otherwise empty.
        const isFull = display >= fullValue - 0.001;
        const isHalf = !isFull && display >= halfValue - 0.001;
        return (
          <span
            key={i}
            className={cn(
              "relative inline-block",
              dim.star,
              !readOnly && "cursor-pointer",
            )}
          >
            {/* Empty base */}
            <Star
              className={cn(
                "absolute inset-0 transition-colors",
                dim.star,
                "text-muted-foreground/40",
              )}
              aria-hidden
            />
            {/* Filled overlay, clipped by `clip-path` for the half state */}
            <Star
              className={cn(
                "absolute inset-0 transition-colors",
                dim.star,
                isFull || isHalf ? "text-amber-400" : "text-transparent",
              )}
              fill="currentColor"
              style={
                isHalf
                  ? { clipPath: "polygon(0 0, 50% 0, 50% 100%, 0 100%)" }
                  : undefined
              }
              aria-hidden
            />
            {/* Hit zones — left half = X.5, right half = X */}
            {!readOnly && (
              <>
                <button
                  type="button"
                  className="absolute inset-y-0 left-0 w-1/2 cursor-pointer focus-visible:outline-none"
                  aria-label={`${halfValue} stars`}
                  role="radio"
                  aria-checked={value === halfValue}
                  onMouseEnter={() => setHover(halfValue)}
                  onClick={() => commit(halfValue)}
                  tabIndex={-1}
                />
                <button
                  type="button"
                  className="absolute inset-y-0 right-0 w-1/2 cursor-pointer focus-visible:outline-none"
                  aria-label={`${fullValue} stars`}
                  role="radio"
                  aria-checked={value === fullValue}
                  onMouseEnter={() => setHover(fullValue)}
                  onClick={() => commit(fullValue)}
                  tabIndex={-1}
                />
              </>
            )}
          </span>
        );
      })}
    </div>
  );
}
