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
  creatorSlugs,
  orientation = "vertical",
  className,
}: {
  label?: string;
  items: string[] | undefined;
  emptyLabel?: string;
  variant?: "secondary" | "outline" | "default";
  /** `"horizontal"` lays the label in a fixed-width column with the chips
   *  flowing in the remaining width (a definition-list row) instead of
   *  stacking label-over-chips. Lets long lists (e.g. a 16-character cast)
   *  use the full row width. Collapses back to stacked on narrow screens. */
  orientation?: "vertical" | "horizontal";
  /** When set, chips become quick-apply links into the library grid.
   *  Accepts saved-view field ids (`"writer"`, `"genres"`, etc.); the
   *  component maps each to its corresponding `/series` query param. */
  filterField?: string;
  /** Name → creator slug map. When `filterField` is a credit role and
   *  `creatorSlugs[item]` is set, the chip links directly to
   *  `/creators/<slug>` instead of the legacy library-grid filter.
   *  Series + issue detail endpoints both surface this map. */
  creatorSlugs?: Record<string, string>;
  className?: string;
}) {
  const list = items ?? [];
  if (list.length === 0 && !emptyLabel) return null;
  const horizontal = orientation === "horizontal";
  return (
    <div
      className={cn(
        horizontal ? "flex flex-col gap-1 sm:flex-row sm:gap-4" : "space-y-2",
        className,
      )}
    >
      {label && (
        <h3
          className={cn(
            "text-muted-foreground text-xs font-semibold tracking-wider uppercase",
            horizontal && "sm:w-32 sm:shrink-0 sm:pt-1.5",
          )}
        >
          {label}
        </h3>
      )}
      {list.length === 0 ? (
        <p
          className={cn("text-muted-foreground text-sm", horizontal && "flex-1")}
        >
          {emptyLabel}
        </p>
      ) : (
        <div className={cn("flex flex-wrap gap-1.5", horizontal && "flex-1")}>
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
            // Credit-role chips link to `/creators/<slug>` when the
            // parent surface (series / issue detail) hands us a slug
            // for the chip's name in `creatorSlugs`. The slug is
            // resolved server-side via the `series_credits.person_id`
            // FK — same primary-key lookup every other detail-page
            // chip uses. Names without a slug (a freshly-scanned
            // credit between rollups) fall back to the legacy
            // library-grid `?credits=<name>` filter.
            if (isCreditRole(filterField)) {
              const slug = creatorSlugs?.[item];
              if (slug) {
                return (
                  <Link
                    key={item}
                    href={`/creators/${encodeURIComponent(slug)}`}
                    title={`Open ${item}'s creator page`}
                  >
                    {chip}
                  </Link>
                );
              }
              return (
                <Link
                  key={item}
                  href={`/?library=all&credits=${encodeURIComponent(item)}`}
                  title={`View all "${item}" series`}
                >
                  {chip}
                </Link>
              );
            }
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

/** Credit-role facets get a dedicated creator detail page —
 *  everything else (genres / tags / characters / teams / locations)
 *  stays on the filtered library grid. Keep this list aligned with
 *  the backend's ROLE_ORDER + the registry in `api/creators.rs`. */
function isCreditRole(filterField: string): boolean {
  switch (filterField) {
    case "writer":
    case "penciller":
    case "inker":
    case "colorist":
    case "letterer":
    case "cover_artist":
    case "editor":
    case "translator":
      return true;
    default:
      return false;
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
