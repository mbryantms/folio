"use client";

import * as React from "react";
import { Check, RotateCcw } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { useSetSavedViewIcon } from "@/lib/api/mutations";
import type { SavedViewView } from "@/lib/api/types";
import { cn } from "@/lib/utils";

import {
  RAIL_ICONS,
  RAIL_ICON_CATEGORY_LABELS,
  type RailIconCategory,
  type RailIconEntry,
  defaultIconKeyForKind,
  railIconFor,
} from "./rail-icons";

/**
 * Click-the-icon-to-pick-a-new-one affordance shown on every rail header.
 *
 * The trigger is the rail's current icon rendered as a small button (same
 * 14px footprint as the static icon it replaces), so the visual weight of
 * the header doesn't change when picking is available. Clicking opens a
 * popover with the registry grouped by category; click any tile to save
 * and close.
 *
 * The mutation is optimistic — clicking a tile invalidates the
 * `saved-views` queries so the home rail + sidebar both reflect the new
 * choice without a refresh.
 */
export function RailIconPicker({
  view,
  size = 14,
}: {
  view: SavedViewView;
  /** Icon pixel size for the trigger. The picker tiles always render at
   *  20px regardless. */
  size?: number;
}) {
  const setIcon = useSetSavedViewIcon();
  const [open, setOpen] = React.useState(false);
  const current = railIconFor(view);
  const defaultKey = defaultIconKeyForKind(view.kind);
  const usingDefault = !view.icon || view.icon === defaultKey;

  const choose = (entry: RailIconEntry) => {
    setIcon.mutate(
      { id: view.id, icon: entry.key === defaultKey ? null : entry.key },
      { onSuccess: () => setOpen(false) },
    );
  };
  const reset = () => {
    setIcon.mutate(
      { id: view.id, icon: null },
      { onSuccess: () => setOpen(false) },
    );
  };

  const grouped = React.useMemo(() => groupByCategory(RAIL_ICONS), []);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label={`Change icon (currently ${current.label})`}
          title={`Icon: ${current.label} (click to change)`}
          // No `onClick` override here: Radix's PopoverTrigger composes
          // its own click handler onto this child via `asChild`. If we
          // call `preventDefault`, Radix sees `event.defaultPrevented`
          // and skips opening the popover. The trigger lives outside
          // the title `<Link>` in every surface that mounts it (rail
          // header, sidebar — not nested), so neither preventDefault
          // nor stopPropagation is needed for correctness.
          className="text-muted-foreground hover:text-foreground hover:bg-accent/40 focus-visible:ring-ring inline-flex shrink-0 cursor-pointer items-center justify-center rounded-md p-0.5 transition-colors focus-visible:ring-2 focus-visible:outline-none"
        >
          <current.Icon
            aria-hidden="true"
            style={{ height: size, width: size }}
          />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        // Slightly wider than the default so the 6-column tile grid has
        // breathing room and the rightmost column never clips. Plain
        // `overflow-y-auto` instead of `<ScrollArea>` because the
        // Radix viewport's intrinsic width can extend past its parent
        // when wrapped, which was what clipped the right column.
        // `overscroll-contain` prevents the page from scrolling once
        // we've hit the picker's top/bottom.
        className="flex max-h-[28rem] w-[22rem] flex-col gap-3 overflow-hidden p-3"
      >
        <div className="flex items-start justify-between gap-2">
          <div>
            <h3 className="text-sm font-semibold">Rail icon</h3>
            <p className="text-muted-foreground text-xs">
              Shown on the home rail header and sidebar.
            </p>
          </div>
          <Button
            variant="ghost"
            size="sm"
            className="text-muted-foreground hover:text-foreground shrink-0"
            onClick={reset}
            disabled={usingDefault || setIcon.isPending}
            title="Reset to default"
          >
            <RotateCcw className="h-3 w-3" />
            <span className="ml-1 text-xs">Reset</span>
          </Button>
        </div>
        <div className="space-y-3 overflow-y-auto overscroll-contain pr-1">
          {grouped.map(([category, entries]) => (
            <section key={category}>
              <h4 className="text-muted-foreground mb-1.5 text-[10px] font-semibold tracking-wider uppercase">
                {RAIL_ICON_CATEGORY_LABELS[category]}
              </h4>
              <div className="grid grid-cols-6 gap-1">
                {entries.map((entry) => {
                  const isActive = entry.key === current.key;
                  return (
                    <button
                      key={entry.key}
                      type="button"
                      aria-label={entry.label}
                      title={entry.label}
                      onClick={() => choose(entry)}
                      disabled={setIcon.isPending}
                      className={cn(
                        "relative inline-flex aspect-square cursor-pointer items-center justify-center rounded-md transition-colors",
                        isActive
                          ? "bg-primary/15 text-primary ring-primary/40 ring-1"
                          : "text-muted-foreground hover:bg-accent/40 hover:text-foreground",
                      )}
                    >
                      <entry.Icon aria-hidden="true" className="h-5 w-5" />
                      {isActive && (
                        <Check className="absolute right-0.5 bottom-0.5 h-2.5 w-2.5" />
                      )}
                    </button>
                  );
                })}
              </div>
            </section>
          ))}
        </div>
      </PopoverContent>
    </Popover>
  );
}

function groupByCategory(
  entries: ReadonlyArray<RailIconEntry>,
): ReadonlyArray<[RailIconCategory, RailIconEntry[]]> {
  const buckets = new Map<RailIconCategory, RailIconEntry[]>();
  for (const e of entries) {
    const list = buckets.get(e.category) ?? [];
    list.push(e);
    buckets.set(e.category, list);
  }
  // Preserve a stable category order matching the labels map insertion.
  return Array.from(
    Object.keys(RAIL_ICON_CATEGORY_LABELS) as RailIconCategory[],
  )
    .filter((cat) => buckets.has(cat))
    .map((cat) => [cat, buckets.get(cat)!]);
}
