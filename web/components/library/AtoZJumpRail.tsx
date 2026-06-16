"use client";

import { cn } from "@/lib/utils";

/** Buckets the rail offers: `#` (non-letter / digit-leading) then A–Z.
 *  Values are lowercase to match the server's case-insensitive
 *  `starts_with` (`#` is its own sentinel). */
const BUCKETS: { value: string; label: string }[] = [
  { value: "#", label: "#" },
  ...Array.from({ length: 26 }, (_, i) => {
    const letter = String.fromCharCode(97 + i); // a–z
    return { value: letter, label: letter.toUpperCase() };
  }),
];

/**
 * A–Z jump rail (audit B9). Maps each letter (and `#`) to the server
 * `starts_with` filter so reaching "S" in a long alphabetical list is one
 * tap, not a scroll marathon. Clicking the active bucket clears it.
 *
 * Rendered as a wrapping row of compact buttons so it stays usable on
 * mobile (a fixed vertical rail would overflow narrow viewports); the
 * caller decides where to place it relative to the grid.
 */
export function AtoZJumpRail({
  value,
  onSelect,
  className,
}: {
  /** The active bucket (`#` or a lowercase letter), or null for "all". */
  value: string | null;
  onSelect: (bucket: string | null) => void;
  className?: string;
}) {
  return (
    <nav
      aria-label="Jump to letter"
      // Mobile: wrap into left-packed rows. md+: a single row that spreads
      // edge-to-edge so the rail fills the content width instead of
      // trailing off ~3/4 across on wide screens.
      className={cn(
        "flex flex-wrap items-center gap-0.5 md:flex-nowrap md:justify-between",
        className,
      )}
    >
      {BUCKETS.map((b) => {
        const active = value === b.value;
        return (
          <button
            key={b.value}
            type="button"
            aria-pressed={active}
            aria-label={
              b.value === "#"
                ? "Names starting with a number or symbol"
                : b.label
            }
            onClick={() => onSelect(active ? null : b.value)}
            className={cn(
              "grid size-6 place-items-center rounded text-xs font-medium tabular-nums transition-colors",
              active
                ? "bg-foreground text-background"
                : "text-muted-foreground hover:bg-muted hover:text-foreground",
            )}
          >
            {b.label}
          </button>
        );
      })}
    </nav>
  );
}
