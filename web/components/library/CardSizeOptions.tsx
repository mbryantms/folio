"use client";

import { LayoutGrid } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Slider } from "@/components/ui/slider";

/** Reusable popover with a single "Card size" slider — drives the
 *  `minmax` of an auto-fill cover grid. Used by the series Issues
 *  panel and by saved-view detail pages so users get the same density
 *  control everywhere they see covers. */
export function CardSizeOptions({
  cardSize,
  onCardSize,
  min,
  max,
  step,
  defaultSize,
  description = "Adjust the cover grid to your screen and taste. Saved per browser.",
  triggerLabel = "View options",
  fieldId = "card-size",
}: {
  cardSize: number;
  onCardSize: (next: number) => void;
  min: number;
  max: number;
  step: number;
  defaultSize: number;
  description?: string;
  triggerLabel?: string;
  fieldId?: string;
}) {
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          size="sm"
          aria-label={triggerLabel}
          title={triggerLabel}
        >
          <LayoutGrid className="h-4 w-4" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-72">
        <div className="space-y-4">
          <div>
            <h3 className="text-sm font-semibold">View</h3>
            <p className="text-muted-foreground text-xs">{description}</p>
          </div>
          <div className="space-y-2">
            <div className="flex items-center justify-between">
              <Label htmlFor={fieldId} className="text-xs font-medium">
                Card size
              </Label>
              <span className="text-muted-foreground text-xs tabular-nums">
                {cardSize}px
              </span>
            </div>
            <Slider
              id={fieldId}
              min={min}
              max={max}
              step={step}
              value={[cardSize]}
              onValueChange={(v) => {
                if (v[0] !== undefined) onCardSize(v[0]);
              }}
            />
            <div className="text-muted-foreground/70 flex justify-between text-[10px] tracking-wider uppercase">
              <span>Compact</span>
              <span>Roomy</span>
            </div>
          </div>
          <div className="flex justify-end">
            <Button
              variant="ghost"
              size="sm"
              disabled={cardSize === defaultSize}
              onClick={() => onCardSize(defaultSize)}
            >
              Reset
            </Button>
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}
