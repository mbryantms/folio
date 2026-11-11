"use client";

import { LayoutGrid } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/folio/popover";
import { Slider } from "@/components/folio/slider";
import { Switch } from "@/components/ui/switch";
import { useCoverCollectionDot } from "@/components/library/use-cover-collection-dot";
import { useCoverSize } from "@/components/library/use-cover-size";
import { DEFAULT_COVER_SIZE_PX } from "@/lib/library/cover-size";

const DESCRIPTION =
  "One size for every cover grid and rail, set by how many covers fit a row of the library. Saved per browser.";

/**
 * The app-wide cover-size control (use-cover-size.ts). It takes no size
 * props: every instance, on every page, reads and sets the one shared
 * size, so the library, home rails, saved views, search and the series
 * page always agree. (Bookmarks have their own px slider:
 * `CardSizeOptions`.)
 *
 * The slider moves between **column counts** of the library grid, not
 * pixels: each stop is a number of covers that fills the row exactly, so
 * no size leaves a blank strip at the row's end.
 */
export function CoverSizeOptions({
  description = DESCRIPTION,
  triggerLabel = "View options",
  fieldId = "cover-size",
}: {
  description?: string;
  /** The trigger is icon-only: this is its accessible name and tooltip. */
  triggerLabel?: string;
  fieldId?: string;
}) {
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          aria-label={triggerLabel}
          title={triggerLabel}
          className="px-2.5"
        >
          <LayoutGrid className="size-4" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-72">
        <div className="space-y-4">
          <div>
            <h3 className="text-sm font-semibold">View</h3>
            <p className="text-muted-foreground text-xs">{description}</p>
          </div>
          <CoverSizeFields fieldId={fieldId} />
        </div>
      </PopoverContent>
    </Popover>
  );
}

/**
 * The popover's body without the popover. A page's phone menu renders this
 * inside its own accordion (docs/dev/phone-layout.md); `fieldId` must
 * differ from the toolbar copy's so the two labels stay unique.
 */
export function CoverSizeFields({
  fieldId = "cover-size",
}: {
  fieldId?: string;
}) {
  const { target, columns, stops, defaultColumns, setColumns, reset } =
    useCoverSize();
  const collectionDot = useCoverCollectionDot();
  const collectionFieldId = `${fieldId}-collection-dot`;
  // Slider left = Compact = the most covers per row.
  const steps = [...stops].reverse();
  const index = Math.max(0, steps.indexOf(columns));
  const valueText = columns > 0 ? `${columns} per row` : "";
  const isDefault =
    target === DEFAULT_COVER_SIZE_PX ||
    (columns > 0 && columns === defaultColumns);
  return (
    <>
      <div className="space-y-2">
        <div className="flex items-center justify-between">
          <Label htmlFor={fieldId} className="text-xs font-medium">
            Cover size
          </Label>
          <span className="text-muted-foreground text-xs tabular-nums">
            {valueText}
          </span>
        </div>
        <Slider
          id={fieldId}
          min={0}
          max={Math.max(1, steps.length - 1)}
          step={1}
          disabled={steps.length < 2}
          thumbLabel="Cover size"
          thumbValueText={valueText || undefined}
          value={[index]}
          onValueChange={(v) => {
            const next = v[0] === undefined ? undefined : steps[v[0]];
            if (next !== undefined && next !== columns) setColumns(next);
          }}
        />
        <div className="text-muted-foreground flex justify-between text-[10px] tracking-wider uppercase">
          <span>Compact</span>
          <span>Roomy</span>
        </div>
      </div>
      <div className="border-border space-y-1.5 border-t pt-3">
        <div className="flex items-center justify-between gap-3">
          <Label htmlFor={collectionFieldId} className="text-xs font-medium">
            Collection dot
          </Label>
          <Switch
            id={collectionFieldId}
            checked={collectionDot.enabled}
            onCheckedChange={collectionDot.setEnabled}
            aria-label="Show the collection ownership dot on series covers"
          />
        </div>
        <p className="text-muted-foreground/80 text-[11px] leading-snug">
          Show a small green or amber dot on series covers to mark collection
          ownership.
        </p>
      </div>
      <div className="flex justify-end">
        <Button variant="ghost" size="sm" disabled={isDefault} onClick={reset}>
          Reset
        </Button>
      </div>
    </>
  );
}
