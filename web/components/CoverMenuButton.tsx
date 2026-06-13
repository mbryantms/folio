"use client";

import * as React from "react";
import { MoreHorizontal } from "lucide-react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useCoarsePointer } from "@/lib/ui/use-coarse-pointer";
import { cn } from "@/lib/utils";

export type CoverMenuAction = {
  label: string;
  onSelect: () => void;
  destructive?: boolean;
  disabled?: boolean;
};

/**
 * Hover/focus-revealed kebab menu that overlays a cover. Companion to
 * `<QuickReadOverlay>` — the two share the same reveal mechanics (key off
 * the nearest `.group` ancestor) and the same fixed footprint so that
 * neither affordance grows with cover size.
 *
 * Markup choice: dropdown trigger is a `<span role="button">` rather than
 * `<button>` so the trigger stays valid inside a parent `<Link>`'s
 * `<a>` element (the same pattern `<QuickReadOverlay>` uses for its own
 * activation surface). The dropdown menu still gets the full Radix
 * keyboard/aria behavior; the trigger just isn't a real `<button>` tag.
 *
 * Positioning: `absolute top-2 left-2` of the nearest positioned ancestor,
 * typically the cover wrapper. The cover wrapper must be `position:
 * relative` and the card root must carry `class="group"` for the
 * hover-reveal animation to work.
 *
 * Pass `actions` empty (or omit the component) to suppress the kebab on
 * cards that don't have meaningful actions.
 */
export function CoverMenuButton({
  actions,
  label = "Cover actions",
  className,
}: {
  actions: CoverMenuAction[];
  label?: string;
  className?: string;
}) {
  // Coarse pointers have no hover, so the kebab would never surface
  // (audit B16). Render it persistently there instead of waiting on a
  // hover that can't happen; hover-capable devices keep the reveal.
  const coarse = useCoarsePointer();
  if (actions.length === 0) return null;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <span
          role="button"
          tabIndex={0}
          aria-label={label}
          title={label}
          // The trigger lives inside a parent <Link> on most cards; stop
          // propagation + prevent the default anchor activation so the
          // dropdown opens without routing to the detail page first.
          onClick={(e) => {
            e.preventDefault();
            e.stopPropagation();
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              e.stopPropagation();
            }
          }}
          className={cn(
            "absolute top-2 left-2 z-10",
            // Fixed visual footprint (32px) — never grows with cover size.
            "bg-background/85 text-foreground inline-flex h-8 w-8 cursor-pointer items-center justify-center rounded-full shadow-sm ring-1 ring-black/10 backdrop-blur dark:ring-white/10",
            // On coarse pointers there's no hover to trigger the reveal,
            // so show it persistently (audit B16). Otherwise hidden by
            // default, revealed on the parent .group's hover/focus.
            coarse
              ? "scale-100 opacity-100"
              : cn(
                  "scale-90 opacity-0 transition-all duration-150 ease-out",
                  "group-hover:scale-100 group-hover:opacity-100",
                  "group-focus-within:scale-100 group-focus-within:opacity-100",
                  "focus-visible:scale-100 focus-visible:opacity-100 focus-visible:outline-none",
                ),
            className,
          )}
        >
          <MoreHorizontal className="h-4 w-4" aria-hidden="true" />
        </span>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        className="min-w-[11rem]"
        onClick={(e) => e.stopPropagation()}
      >
        {actions.map((a, i) => (
          <DropdownMenuItem
            key={i}
            disabled={a.disabled}
            onSelect={() => {
              // Let Radix close the menu first (default `onSelect`
              // behavior). If we `preventDefault()` here the dropdown
              // stays open over any follow-up toast — that swallowed
              // the first click on Sonner's "Undo" action for
              // Add-to-Want-to-Read because the menu sat on top of
              // the toast layer.
              if (!a.disabled) a.onSelect();
            }}
            className={cn(
              a.destructive
                ? "text-destructive focus:text-destructive"
                : undefined,
            )}
          >
            {a.label}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
