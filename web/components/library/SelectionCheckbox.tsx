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
 *     hover (`group-hover:opacity-100`). The checkbox renders as a
 *     `<button role="checkbox">` and tapping it enters select mode
 *     while selecting this card. Mobile devices don't have hover
 *     and don't reveal anything in this state.
 *   - **Select mode on:** always visible, regardless of pointer
 *     type. The parent card itself is a `<button>` whose tap
 *     toggles selection, so this checkbox renders as a decorative
 *     `<span aria-hidden="true">` to avoid a nested-interactive
 *     element (button-in-button is invalid HTML and triggers a
 *     hydration warning in React 19). The outer button carries
 *     `aria-pressed`, so the toggle state is still announced.
 *
 * The visible icon is 14 px but the tap target is padded to 28 px,
 * which paired with the parent card's padding lands at ≥44 CSS px
 * of actionable area — meets iOS HIG + Material guidance for touch
 * targets.
 *
 * Out of select mode the click handler stops propagation so a
 * stray bubble-up doesn't also navigate the parent `<Link>`.
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
   *  whether the checkbox is persistently visible (mobile),
   *  whether desktop reveals it on hover, AND whether it renders
   *  as an interactive `<button>` or a decorative `<span>`. */
  selectMode: boolean;
  /** Receives the click event so the consumer can detect
   *  `shiftKey` for range-select. Unused when `selectMode` is
   *  true — the outer card-button handles the click. */
  onToggle: (ev?: React.MouseEvent) => void;
  /** Accessible name. Typically the card's title ("Saga #1"). */
  label: string;
  className?: string;
}) {
  const visualClasses = cn(
    "absolute top-2 left-2 z-10 flex h-7 w-7 items-center justify-center rounded-md border-2 transition-all",
    isSelected
      ? "border-primary bg-primary text-primary-foreground"
      : "border-border bg-background/90 backdrop-blur-sm",
    className,
  );

  if (selectMode) {
    // Parent is a `<button>` (the whole card is the click target in
    // select mode). Render as a decorative span so we don't nest
    // interactive elements. State is announced via the parent
    // button's `aria-pressed`.
    return (
      <span aria-hidden="true" className={cn(visualClasses, "opacity-100")}>
        {isSelected && <Check className="h-4 w-4" />}
      </span>
    );
  }

  // Out of select mode the parent is a `<Link>` (anchor). The
  // checkbox is the way to ENTER select mode — keep it as a
  // semantic button so keyboard users can Tab to it.
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={isSelected}
      aria-label={isSelected ? `Deselect ${label}` : `Select ${label}`}
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
        visualClasses,
        "focus-visible:ring-ring focus-visible:ring-2 focus-visible:outline-none",
        "opacity-0 pointer-events-none group-hover:opacity-100 group-hover:pointer-events-auto",
      )}
    >
      {isSelected && <Check className="h-4 w-4" />}
    </button>
  );
}
