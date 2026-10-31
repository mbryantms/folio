"use client";

import { Clock, X } from "lucide-react";

import { cn } from "@/lib/utils";

/** Recent-search chips: the search modal's and the `/search` page's
 *  empty-input state. Compact pills — the remove
 *  control is sized to the chip rather than a 44px touch box that
 *  ballooned the whole row. */
export function RecentSearches({
  recents,
  onPick,
  onRemove,
  onClear,
  className,
}: {
  recents: readonly string[];
  onPick: (q: string) => void;
  onRemove: (q: string) => void;
  onClear: () => void;
  className?: string;
}) {
  return (
    <div className={cn("min-h-0 flex-1 overflow-y-auto py-2", className)}>
      <div className="flex items-center justify-between px-3 pb-1">
        <h3 className="text-muted-foreground inline-flex items-center gap-1.5 text-[11px] font-semibold tracking-wide uppercase">
          <Clock className="size-3" aria-hidden="true" />
          Recent searches
        </h3>
        <button
          type="button"
          onClick={onClear}
          className="text-muted-foreground hover:text-foreground text-[11px] font-medium"
        >
          Clear all
        </button>
      </div>
      <ul className="flex flex-wrap gap-1.5 px-3 pb-3">
        {recents.map((q) => (
          <li key={q}>
            <span className="bg-muted/60 hover:bg-muted text-foreground border-border inline-flex items-center rounded-full border py-0.5 pr-0.5 pl-3 text-xs transition-colors">
              <button
                type="button"
                onClick={() => onPick(q)}
                className="py-1 focus-visible:outline-none"
                aria-label={`Search again for ${q}`}
              >
                {q}
              </button>
              <button
                type="button"
                onClick={() => onRemove(q)}
                aria-label={`Remove ${q} from recents`}
                className="text-muted-foreground hover:text-foreground hover:bg-background/70 ml-1 inline-flex size-6 items-center justify-center rounded-full transition-colors"
              >
                <X className="size-3" aria-hidden="true" />
              </button>
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}
