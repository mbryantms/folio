"use client";

import * as React from "react";
import { Check } from "lucide-react";

import { cn } from "@/lib/utils";

/**
 * Card overlay checkbox used by the multi-select flow. Absolutely
 * positioned in the top-left of the parent (which must be
 * `position: relative`). Two visibility states:
 *
 *   - **Select mode off, desktop:** hidden by default, revealed on
 *     hover (`group-hover:opacity-100`). Mobile devices don't have
 *     hover and don't reveal anything in this state.
 *   - **Select mode on:** always visible, regardless of pointer
 *     type, and the parent card's primary tap toggles selection
 *     instead of navigating.
 *
 * The visible icon is 14 px but the tap target is padded to 28 px,
 * which paired with the parent card's padding lands at ≥44 CSS px
 * of actionable area — meets iOS HIG + Material guidance for touch
 * targets.
 *
 * Stops event propagation so a click on the checkbox doesn't also
 * fire the underlying card's onClick (which would re-toggle, net
 * zero, but feels glitchy on slow devices).
 *
 * Plan: `~/.claude/plans/multi-select-bulk-actions-1.0.md` (M1).
 */
export function SelectionCheckbox({
  isSelected,
  selectMode,
  onToggle,
  label,
  className,
}: {
  isSelected: boolean;
  /** True when the parent surface is in select mode. Controls
   *  whether the checkbox is persistently visible (mobile) and
   *  whether desktop reveals it on hover. */
  selectMode: boolean;
  /** Receives the click event so the consumer can detect
   *  `shiftKey` for range-select. */
  onToggle: (ev?: React.MouseEvent) => void;
  /** Accessible name. Typically the card's title ("Saga #1"). */
  label: string;
  className?: string;
}) {
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={isSelected}
      aria-label={
        isSelected ? `Deselect ${label}` : `Select ${label}`
      }
      onClick={(ev) => {
        ev.stopPropagation();
        ev.preventDefault();
        onToggle(ev);
      }}
      onKeyDown={(ev) => {
        if (ev.key === " " || ev.key === "Enter") {
          ev.stopPropagation();
          ev.preventDefault();
          onToggle();
        }
      }}
      className={cn(
        "absolute top-2 left-2 z-10 flex h-7 w-7 items-center justify-center rounded-md border-2 transition-all",
        "focus-visible:ring-ring focus-visible:ring-2 focus-visible:outline-none",
        // Persistent on mobile when in select mode; hover-revealed on desktop.
        selectMode
          ? "opacity-100"
          : "opacity-0 pointer-events-none group-hover:opacity-100 group-hover:pointer-events-auto",
        isSelected
          ? "border-primary bg-primary text-primary-foreground"
          : "border-border bg-background/90 backdrop-blur-sm",
        className,
      )}
    >
      {isSelected && <Check className="h-4 w-4" />}
    </button>
  );
}
