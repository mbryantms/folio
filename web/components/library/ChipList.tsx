import Link from "next/link";

import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

/**
 * Pill list for frequency-aggregated metadata fields (writers, genres,
 * tags, etc.). Renders nothing when there are no items unless
 * `emptyLabel` is provided.
 *
 * When `filterField` is set, each chip becomes a deep-link into the
 * library grid (`/?library=all&<param>=<value>`) so the user lands on
 * the filtered view immediately. The previous behaviour took users to
 * the saved-view creation flow — that meant every chip click was a
 * three-step "open dialog → confirm → apply" instead of a one-step
 * navigation.
 */
export function ChipList({
  label,
  items,
  emptyLabel,
  variant = "secondary",
  filterField,
  className,
}: {
  label?: string;
  items: string[] | undefined;
  emptyLabel?: string;
  variant?: "secondary" | "outline" | "default";
  /** When set, chips become quick-apply links into the library grid.
   *  Accepts saved-view field ids (`"writer"`, `"genres"`, etc.); the
   *  component maps each to its corresponding `/series` query param. */
  filterField?: string;
  className?: string;
}) {
  const list = items ?? [];
  if (list.length === 0 && !emptyLabel) return null;
  return (
    <div className={cn("space-y-2", className)}>
      {label && (
        <h3 className="text-muted-foreground text-xs font-semibold tracking-wider uppercase">
          {label}
        </h3>
      )}
      {list.length === 0 ? (
        <p className="text-muted-foreground text-sm">{emptyLabel}</p>
      ) : (
        <div className="flex flex-wrap gap-1.5">
          {list.map((item) => {
            const chip = (
              <Badge
                key={item}
                variant={variant}
                className={cn(
                  "font-normal",
                  filterField
                    ? "hover:bg-secondary/80 cursor-pointer"
                    : "cursor-default",
                )}
              >
                {item}
              </Badge>
            );
            if (!filterField) return chip;
            const param = libraryParamFor(filterField);
            const mode = libraryModeFor(filterField);
            const href =
              mode === "issues"
                ? `/?library=all&mode=issues&${param}=${encodeURIComponent(item)}`
                : `/?library=all&${param}=${encodeURIComponent(item)}`;
            const titleNoun = mode === "issues" ? "issues" : "series";
            return (
              <Link
                key={item}
                href={href}
                title={`View all "${item}" ${titleNoun}`}
              >
                {chip}
              </Link>
            );
          })}
        </div>
      )}
    </div>
  );
}

/** Saved-view field id → library-grid query param. Credit roles are
 *  singular in the saved-view DSL (`"writer"`) but plural on the
 *  listing endpoints (`writers`). Junction-table facets (`genres`,
 *  `tags`) are already plural everywhere. Falls through to the input
 *  so future additions don't silently break. */
function libraryParamFor(filterField: string): string {
  switch (filterField) {
    case "writer":
      return "writers";
    case "penciller":
      return "pencillers";
    case "inker":
      return "inkers";
    case "colorist":
      return "colorists";
    case "letterer":
      return "letterers";
    case "cover_artist":
      return "cover_artists";
    case "editor":
      return "editors";
    case "translator":
      return "translators";
    default:
      return filterField;
  }
}

/** Routes credit + cast/setting chips into the issues view ("show me
 *  every issue this writer worked on") and leaves genres/tags pointing
 *  at the series view, which matches how those facets are most often
 *  used (browsing the library by genre is a series-level activity).
 *  Anything unknown falls back to series. */
function libraryModeFor(filterField: string): "series" | "issues" {
  switch (filterField) {
    case "writer":
    case "penciller":
    case "inker":
    case "colorist":
    case "letterer":
    case "cover_artist":
    case "editor":
    case "translator":
    case "characters":
    case "teams":
    case "locations":
      return "issues";
    default:
      return "series";
  }
}
