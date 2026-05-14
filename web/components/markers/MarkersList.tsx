"use client";

import * as React from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import {
  Bookmark as BookmarkIcon,
  Highlighter,
  Search as SearchIcon,
  Star,
  StickyNote,
} from "lucide-react";
import { toast } from "sonner";

import {
  CoverMenuButton,
  type CoverMenuAction,
} from "@/components/CoverMenuButton";
import { useCoverLongPressActions } from "@/components/CoverLongPressActions";
import { CardSizeOptions } from "@/components/library/CardSizeOptions";
import { useCardSize } from "@/components/library/use-card-size";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useMarkers, useMarkerTags } from "@/lib/api/queries";
import {
  useCreateMarker,
  useDeleteMarker,
  useUpdateMarker,
} from "@/lib/api/mutations";
import { markerToCreateReq } from "@/lib/markers/recreate";
import type {
  MarkerKind,
  MarkerRegion,
  MarkerTagMatch,
  MarkerView,
} from "@/lib/api/types";
import {
  copyMarkerImageToClipboard,
  downloadMarkerImage,
  markerCropFilename,
} from "@/lib/marker-crop";
import { pageBytesUrl, readerUrl } from "@/lib/urls";
import { cn } from "@/lib/utils";

type KindFilter = MarkerKind | "all" | "favorite";

const FILTER_OPTIONS: { value: KindFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "bookmark", label: "Bookmarks" },
  { value: "note", label: "Notes" },
  // Favorite is a flag, not a kind — the chip is convenience UX. Wired
  // below via `is_favorite=true` rather than `kind=favorite`.
  { value: "favorite", label: "Favorites" },
  { value: "highlight", label: "Highlights" },
];

const SEARCH_DEBOUNCE_MS = 200;

/** Card-size bounds for the marker grid. Default is intentionally
 *  smaller than the saved-view detail page (160) because marker
 *  thumbnails are pre-cropped to a region — each card is information-
 *  dense and benefits from fitting more per row. The thumb endpoint
 *  returns a fixed-size raster, so going much above ~220 starts to
 *  upscale visibly; the upper bound caps it before that happens. */
const CARD_SIZE_MIN = 100;
const CARD_SIZE_MAX = 220;
const CARD_SIZE_STEP = 20;
const CARD_SIZE_DEFAULT = 140;
const CARD_SIZE_STORAGE_KEY = "folio.bookmarks.cardSize";

/** Global feed for the `/bookmarks` page. Renders every kind grouped by
 *  series with a filter chip row and debounced search. Each card jumps
 *  to the reader at the right page; inline kebab handles delete (full
 *  edit lives in the reader to reuse the marker editor sheet). */
export function MarkersList() {
  const [filter, setFilter] = React.useState<KindFilter>("all");
  const [rawSearch, setRawSearch] = React.useState("");
  const [debouncedSearch, setDebouncedSearch] = React.useState("");
  const [selectedTags, setSelectedTags] = React.useState<string[]>([]);
  const [tagMatch, setTagMatch] = React.useState<MarkerTagMatch>("all");
  const [cardSize, setCardSize] = useCardSize({
    storageKey: CARD_SIZE_STORAGE_KEY,
    min: CARD_SIZE_MIN,
    max: CARD_SIZE_MAX,
    defaultSize: CARD_SIZE_DEFAULT,
  });

  React.useEffect(() => {
    const t = setTimeout(
      () => setDebouncedSearch(rawSearch.trim()),
      SEARCH_DEBOUNCE_MS,
    );
    return () => clearTimeout(t);
  }, [rawSearch]);

  // Translate the chip selection into the filter set the server
  // understands: kind chips map to `kind=…`, the favorite chip maps to
  // `is_favorite=true` (no kind constraint), and selected tags become
  // `tags=a,b&tag_match=…`.
  const isFavoriteFilter = filter === "favorite";
  const kindFilter: MarkerKind | undefined =
    filter === "all" || filter === "favorite"
      ? undefined
      : (filter as MarkerKind);
  const query = useMarkers({
    kind: kindFilter,
    is_favorite: isFavoriteFilter ? true : undefined,
    q: debouncedSearch || undefined,
    tags: selectedTags.length > 0 ? selectedTags.join(",") : undefined,
    tag_match: selectedTags.length > 0 ? tagMatch : undefined,
    limit: 200,
  });
  const tagsQuery = useMarkerTags();
  const availableTags = tagsQuery.data?.items ?? [];

  const items = query.data?.items ?? [];
  const groups = React.useMemo(() => groupBySeries(items), [items]);
  const hasFilterOrSearch =
    filter !== "all" || debouncedSearch.length > 0 || selectedTags.length > 0;

  function toggleTag(tag: string) {
    setSelectedTags((prev) =>
      prev.includes(tag) ? prev.filter((t) => t !== tag) : [...prev, tag],
    );
  }

  return (
    <div className="space-y-6">
      <header className="space-y-1">
        <h1 className="text-2xl font-semibold tracking-tight">Bookmarks</h1>
        <p className="text-muted-foreground text-sm">
          Every page bookmark, note, favorite, and highlight you&rsquo;ve saved
          across your library.
        </p>
      </header>

      <div className="flex flex-wrap items-center gap-2">
        <div
          role="tablist"
          aria-label="Filter by kind"
          className="flex flex-wrap gap-1"
        >
          {FILTER_OPTIONS.map((opt) => (
            <button
              key={opt.value}
              type="button"
              role="tab"
              aria-selected={filter === opt.value}
              onClick={() => setFilter(opt.value)}
              className={cn(
                "focus-visible:ring-ring inline-flex items-center rounded-full border px-3 py-1 text-sm transition-colors focus-visible:ring-2 focus-visible:outline-none",
                filter === opt.value
                  ? "border-foreground bg-foreground text-background"
                  : "border-border/60 hover:bg-accent/40",
              )}
            >
              {opt.label}
            </button>
          ))}
        </div>
        {/* Search input + card-size button share the trailing edge of
         *  the toolbar row so the controls flow as one cluster instead
         *  of stacking the size adjuster on its own line above. */}
        <div className="ml-auto flex items-center gap-2">
          <div className="relative w-full max-w-xs">
            <SearchIcon
              className="text-muted-foreground pointer-events-none absolute top-1/2 left-2 h-4 w-4 -translate-y-1/2"
              aria-hidden="true"
            />
            <Input
              type="search"
              value={rawSearch}
              onChange={(e) => setRawSearch(e.target.value)}
              placeholder="Search notes &amp; highlights"
              aria-label="Search markers"
              className="pl-8"
            />
          </div>
          <CardSizeOptions
            cardSize={cardSize}
            onCardSize={setCardSize}
            min={CARD_SIZE_MIN}
            max={CARD_SIZE_MAX}
            step={CARD_SIZE_STEP}
            defaultSize={CARD_SIZE_DEFAULT}
            fieldId="bookmarks-card-size"
            description="Tighten or loosen the bookmarks grid. Saved per browser."
          />
        </div>
      </div>

      {availableTags.length > 0 ? (
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Tags
          </span>
          <div className="flex flex-wrap gap-1">
            {availableTags.map((t) => {
              const active = selectedTags.includes(t.tag);
              return (
                <button
                  key={t.tag}
                  type="button"
                  onClick={() => toggleTag(t.tag)}
                  aria-pressed={active}
                  className={cn(
                    "focus-visible:ring-ring inline-flex items-center gap-1 rounded-full border px-2.5 py-0.5 text-xs transition-colors focus-visible:ring-2 focus-visible:outline-none",
                    active
                      ? "border-foreground bg-foreground text-background"
                      : "border-border/60 hover:bg-accent/40",
                  )}
                >
                  <span>{t.tag}</span>
                  <span
                    className={cn(
                      "tabular-nums",
                      active ? "text-background/70" : "text-muted-foreground",
                    )}
                  >
                    {t.count}
                  </span>
                </button>
              );
            })}
          </div>
          {selectedTags.length > 1 ? (
            <div
              role="radiogroup"
              aria-label="Tag match mode"
              className="ml-1 inline-flex items-center gap-0.5 rounded-md border p-0.5"
            >
              {(["all", "any"] as const).map((mode) => (
                <button
                  key={mode}
                  type="button"
                  role="radio"
                  aria-checked={tagMatch === mode}
                  onClick={() => setTagMatch(mode)}
                  className={cn(
                    "rounded px-2 py-0.5 text-xs transition-colors",
                    tagMatch === mode
                      ? "bg-foreground text-background"
                      : "text-muted-foreground hover:bg-accent/40",
                  )}
                >
                  Match {mode}
                </button>
              ))}
            </div>
          ) : null}
          {selectedTags.length > 0 ? (
            <button
              type="button"
              onClick={() => setSelectedTags([])}
              className="text-muted-foreground hover:text-foreground text-xs underline-offset-2 hover:underline"
            >
              Clear tags
            </button>
          ) : null}
        </div>
      ) : null}

      {query.isLoading ? (
        <div className="text-muted-foreground py-12 text-center text-sm">
          Loading markers…
        </div>
      ) : query.isError ? (
        <div className="text-destructive rounded-md border p-4 text-sm">
          Failed to load markers.
        </div>
      ) : items.length === 0 ? (
        <EmptyState hasFilter={hasFilterOrSearch} />
      ) : (
        <div className="space-y-8">
          {groups.map((group) => (
            <section key={group.key} className="space-y-3">
              <h2 className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
                {group.label}
              </h2>
              <RowPackedSection items={group.items} cardSize={cardSize} />
            </section>
          ))}
        </div>
      )}
    </div>
  );
}

/** Gap between cards inside a row + between rows. Keeps the inter-card
 *  rhythm tight enough that wide tiles read as part of the same row,
 *  not as standalone full-bleed bands. */
const ROW_GAP_PX = 12;

/** Page-aspect approximation used to compute each card's tile aspect.
 *  Real comic pages hover around 2:3; we don't carry natural dims on
 *  the marker payload, so the layout pretends every page is 2:3 here.
 *  The image rendered INSIDE each tile then uses positioning that
 *  scales the page to fit the tile — slight distortion on non-2:3
 *  pages, but the tiles stay consistently sized. */
const PAGE_ASPECT = 2 / 3; // page_width / page_height

/** Compute the displayed tile aspect (width / height) for a marker.
 *  Region markers use the region's content aspect; page-level markers
 *  use the whole-page aspect. */
function markerTileAspect(marker: MarkerView): number {
  const region = marker.region;
  if (!region || region.w <= 0 || region.h <= 0) {
    return PAGE_ASPECT;
  }
  return (region.w / region.h) * PAGE_ASPECT;
}

/** Justified / row-packed layout. Walks items left-to-right, packing
 *  each row with as many items as fit at the target row height, then
 *  scaling the row uniformly so the items fill the container width
 *  exactly. Last row keeps its natural target height (no stretching)
 *  so a short trailing row doesn't blow up into giant tiles. */
function packIntoRows<T extends { id: string; aspect: number }>(
  items: ReadonlyArray<T>,
  containerWidth: number,
  targetRowHeight: number,
  gap: number,
): {
  rowHeight: number;
  items: { ref: T; width: number; height: number }[];
}[] {
  if (containerWidth <= 0 || items.length === 0) return [];
  const rows: {
    rowHeight: number;
    items: { ref: T; width: number; height: number }[];
  }[] = [];
  let current: T[] = [];

  function commit(row: T[], stretchToFill: boolean) {
    if (row.length === 0) return;
    const totalAspect = row.reduce((s, it) => s + it.aspect, 0);
    const totalGap = (row.length - 1) * gap;
    // Height that makes the row's total width === containerWidth exactly.
    const fittedHeight = (containerWidth - totalGap) / totalAspect;
    // Clamp the fitted height: never grow more than ~2× the target
    // (a single very wide tile would otherwise stretch a row of one
    // into oversize territory) and never collapse below half target.
    const cappedHeight = Math.max(
      targetRowHeight * 0.5,
      Math.min(targetRowHeight * 2, fittedHeight),
    );
    const finalHeight = stretchToFill ? cappedHeight : targetRowHeight;
    rows.push({
      rowHeight: finalHeight,
      items: row.map((it) => ({
        ref: it,
        width: it.aspect * finalHeight,
        height: finalHeight,
      })),
    });
  }

  for (const item of items) {
    current.push(item);
    const totalAspect = current.reduce((s, it) => s + it.aspect, 0);
    const projectedWidth =
      totalAspect * targetRowHeight + (current.length - 1) * gap;
    if (projectedWidth >= containerWidth) {
      commit(current, true);
      current = [];
    }
  }
  // Trailing partial row — keep at target height instead of stretching.
  if (current.length > 0) {
    commit(current, false);
  }
  return rows;
}

/** Snapshot the container's content-box width and keep it in sync via
 *  ResizeObserver. Used by `RowPackedSection` to feed the layout
 *  algorithm. Returns `0` until the first layout pass — callers should
 *  short-circuit rendering during that interval to avoid a flash. */
function useContainerWidth<E extends HTMLElement>(): [
  React.RefObject<E | null>,
  number,
] {
  const ref = React.useRef<E | null>(null);
  const [width, setWidth] = React.useState(0);
  React.useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width;
      if (typeof w === "number") setWidth(w);
    });
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);
  return [ref, width];
}

/** Render one series's markers as a justified row-packed grid where
 *  card width tracks the highlighted region's aspect ratio. Wide
 *  panel selections produce wide tiles; narrow vertical strips
 *  produce tall narrow tiles. */
function RowPackedSection({
  items,
  cardSize,
}: {
  items: ReadonlyArray<MarkerView>;
  cardSize: number;
}) {
  const [ref, containerWidth] = useContainerWidth<HTMLDivElement>();
  // The cardSize slider represents the user's preferred card *width*.
  // Translate that into a target row height — pages are ~2:3 so the
  // natural height for a width=cardSize cover is cardSize × 1.5. We
  // anchor the row height on that so the slider keeps feeling like
  // "small/large cards" across both the cover surfaces and the
  // bookmarks page.
  const targetRowHeight = Math.max(80, cardSize * 1.5);
  const itemsWithAspect = React.useMemo(
    () =>
      items.map((m) => ({
        id: m.id,
        marker: m,
        aspect: markerTileAspect(m),
      })),
    [items],
  );
  const rows = React.useMemo(
    () =>
      packIntoRows(
        itemsWithAspect,
        containerWidth,
        targetRowHeight,
        ROW_GAP_PX,
      ),
    [itemsWithAspect, containerWidth, targetRowHeight],
  );

  return (
    <div ref={ref} className="flex flex-col" style={{ gap: ROW_GAP_PX }}>
      {/* Pre-mount the items even before container width is known so
       *  React doesn't unmount/remount cards on resize — keeps image
       *  loading state stable across breakpoints. */}
      {containerWidth <= 0 ? (
        <div className="h-8" aria-hidden="true" />
      ) : (
        rows.map((row, idx) => (
          <div
            key={idx}
            className="flex"
            style={{ gap: ROW_GAP_PX, alignItems: "flex-start" }}
          >
            {row.items.map((slot) => (
              <div
                key={slot.ref.id}
                style={{ width: slot.width, flexShrink: 0 }}
              >
                <MarkerCard
                  marker={slot.ref.marker}
                  thumbHeight={slot.height}
                />
              </div>
            ))}
          </div>
        ))
      )}
    </div>
  );
}

export function groupBySeries(items: MarkerView[]): {
  key: string;
  label: string;
  items: MarkerView[];
}[] {
  const map = new Map<string, { label: string; items: MarkerView[] }>();
  for (const m of items) {
    const key = m.series_id;
    const label = m.series_name ?? "Unknown series";
    const entry = map.get(key) ?? { label, items: [] };
    entry.items.push(m);
    map.set(key, entry);
  }
  // Stable order: series with the most recent marker first, then alphabetical
  // on label as a tiebreaker.
  return Array.from(map.entries())
    .map(([key, value]) => ({ key, ...value }))
    .sort((a, b) => {
      const aMax = Math.max(...a.items.map((m) => Date.parse(m.updated_at)));
      const bMax = Math.max(...b.items.map((m) => Date.parse(m.updated_at)));
      if (aMax !== bMax) return bMax - aMax;
      return a.label.localeCompare(b.label);
    });
}

/** Renders the marker's thumbnail at an explicit pixel height (driven
 *  by the row-packed layout). Width is `100%` of the parent — the
 *  parent slot has been sized to `aspect × height` already, so the
 *  wrapper ends up exactly tile-shaped.
 *
 *  - Region markers: page is rendered at full natural size, positioned
 *    so the region exactly fills the wrapper. Overflow is hidden so
 *    surrounding panels don't bleed in.
 *  - Page-level markers: full page covers the wrapper at the wrapper's
 *    aspect. The page-aspect approximation built into the layout means
 *    this rarely needs cropping on real comic pages. */
function MarkerThumbnail({
  marker,
  height,
}: {
  marker: MarkerView;
  height: number;
}) {
  const region = marker.region;
  const alt = `Page ${marker.page_index + 1}`;

  if (!region) {
    return (
      <div
        className="bg-muted relative w-full overflow-hidden rounded-md"
        style={{ height }}
      >
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img
          src={pageBytesUrl(marker.issue_id, marker.page_index)}
          alt={alt}
          loading="lazy"
          className="absolute inset-0 h-full w-full object-cover transition group-hover:brightness-110"
        />
      </div>
    );
  }

  // Region markers zoom into a fraction of the page. Strip thumbs cap
  // at 400px, so a 20% crop only has ~80px of source pixels to upscale
  // — visibly blurry. Switch to the full page bytes (typically
  // 1500-2000px wide) so even tight crops stay crisp.
  const src = pageBytesUrl(marker.issue_id, marker.page_index);

  // Reciprocal-scale of the region: how much wider/taller than the
  // wrapper the FULL page image must render so the cropped region
  // fits exactly. Clamped so a 1px stray drag doesn't try to upscale
  // 100× and produce mush.
  const scaleW = Math.min(100, 100 / Math.max(region.w, 1));
  const scaleH = Math.min(100, 100 / Math.max(region.h, 1));

  return (
    <div
      className="bg-muted relative w-full overflow-hidden rounded-md"
      style={{ height }}
    >
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img
        src={src}
        alt={alt}
        loading="lazy"
        className="max-w-none transition group-hover:brightness-110"
        // The img is positioned absolutely and zoomed so the region's
        // top-left corner sits at the wrapper's origin and the region
        // exactly fills the wrapper. Both width and height are scaled
        // so the math is consistent regardless of slight differences
        // between the assumed and actual page aspect.
        style={{
          position: "absolute",
          width: `${scaleW * 100}%`,
          height: `${scaleH * 100}%`,
          left: `${-region.x * scaleW}%`,
          top: `${-region.y * scaleH}%`,
        }}
      />
    </div>
  );
}

function MarkerCard({
  marker,
  thumbHeight,
}: {
  marker: MarkerView;
  thumbHeight: number;
}) {
  const del = useDeleteMarker(marker.id, marker.issue_id, { silent: true });
  const create = useCreateMarker();
  const update = useUpdateMarker(marker.id, marker.issue_id);
  const router = useRouter();
  const jumpHref = buildJumpHref(marker);
  const kindMeta = KIND_META[marker.kind];
  const KindIcon = kindMeta.icon;
  const snippet = marker.body?.trim() || marker.selection?.text?.trim() || null;
  const issueLabel = formatIssueLabel(marker);
  // Image-bearing markers — anything with a region — get Copy/Save
  // affordances. We only want them for crops where the user
  // intentionally selected an area (any shape: rect / text / image);
  // page-level bookmarks would just dump the whole page, which the
  // reader already offers more directly. Page-level markers fall back
  // to the original two-action menu.
  const hasRegion = !!marker.region;

  const actions: CoverMenuAction[] = React.useMemo(() => {
    const list: CoverMenuAction[] = [
      {
        label: "Jump to page",
        onSelect: () => {
          if (jumpHref) router.push(jumpHref);
        },
        disabled: !jumpHref,
      },
      {
        // Favorite toggle. Label flips based on current state so the
        // user knows what clicking will do. The toast that fires on
        // success ("Added to favorites" / "Removed from favorites")
        // comes from `useUpdateMarker`'s `successMessage`, which
        // tailors itself when `is_favorite` is the only field being
        // patched.
        label: marker.is_favorite ? "Remove from favorites" : "Favorite",
        onSelect: () => update.mutate({ is_favorite: !marker.is_favorite }),
        disabled: update.isPending,
      },
    ];
    if (hasRegion && marker.region) {
      const region = marker.region;
      list.push(
        {
          label: "Copy image",
          onSelect: () => {
            void copyMarkerRegion(marker, region);
          },
        },
        {
          label: "Save image…",
          onSelect: () => {
            void saveMarkerRegion(marker, region);
          },
        },
      );
    }
    list.push({
      label: "Delete",
      destructive: true,
      onSelect: () => {
        // Capture the snapshot before the row vanishes from the list
        // so Undo can recreate from a stable value (we can't read
        // `marker` after the cache invalidation drops it).
        const snapshot = marker;
        del.mutate(undefined, {
          onSuccess: () =>
            toast.success("Removed", {
              action: {
                label: "Undo",
                onClick: () => create.mutate(markerToCreateReq(snapshot)),
              },
            }),
        });
      },
      disabled: del.isPending,
    });
    return list;
  }, [create, del, hasRegion, jumpHref, marker, router, update]);

  const longPress = useCoverLongPressActions({
    primary: jumpHref
      ? {
          label: "Jump to page",
          onSelect: () => router.push(jumpHref),
        }
      : undefined,
    actions,
    label: `${kindMeta.shortLabel} · page ${marker.page_index + 1}`,
  });

  const cardBody = (
    <div className="relative" {...longPress.wrapperProps}>
      {/* Thumbnail height comes from the row-packed layout above;
       *  width is `100%` of the parent slot, which itself was sized
       *  to `aspect × height`. So the wrapper ends up an exact tile
       *  with no letterboxing. */}
      <MarkerThumbnail marker={marker} height={thumbHeight} />
      <span
        className={cn(
          "absolute top-2 left-2 inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-medium uppercase",
          kindMeta.badge,
        )}
      >
        <KindIcon className="h-3 w-3" aria-hidden="true" />
        {kindMeta.shortLabel}
      </span>
      {marker.is_favorite ? (
        <span
          aria-label="Favorite"
          title="Favorite"
          className="absolute right-2 bottom-2 inline-flex h-5 w-5 items-center justify-center rounded-full bg-rose-500/90 text-rose-50 ring-1 shadow-sm ring-black/20"
        >
          <Star className="h-3 w-3 fill-current" aria-hidden="true" />
        </span>
      ) : null}
      <CoverMenuButton
        label={`Actions for ${kindMeta.shortLabel} on page ${marker.page_index + 1}`}
        actions={actions}
      />
      {longPress.sheet}
    </div>
  );

  return (
    <div className="group flex h-full flex-col gap-2 rounded-md p-1">
      {jumpHref ? (
        <Link
          href={jumpHref}
          className="focus-visible:ring-ring rounded-md focus-visible:ring-2 focus-visible:outline-none"
        >
          {cardBody}
        </Link>
      ) : (
        cardBody
      )}
      <div className="min-w-0 space-y-0.5 px-1">
        <div className="text-muted-foreground text-xs font-medium">
          {issueLabel}
        </div>
        <div
          className="truncate text-sm font-medium"
          title={marker.issue_title ?? undefined}
        >
          Page {marker.page_index + 1}
        </div>
        {snippet ? (
          <p className="text-muted-foreground line-clamp-2 text-xs">
            {snippet}
          </p>
        ) : null}
        {marker.tags.length > 0 ? (
          <div className="flex flex-wrap gap-1 pt-1">
            {marker.tags.slice(0, 4).map((t) => (
              <span
                key={t}
                className="bg-muted text-muted-foreground inline-flex items-center rounded px-1.5 py-0.5 text-[10px]"
              >
                {t}
              </span>
            ))}
            {marker.tags.length > 4 ? (
              <span className="text-muted-foreground text-[10px]">
                +{marker.tags.length - 4}
              </span>
            ) : null}
          </div>
        ) : null}
      </div>
    </div>
  );
}

function EmptyState({ hasFilter }: { hasFilter: boolean }) {
  return (
    <div className="border-border/60 text-muted-foreground rounded-lg border border-dashed p-8 text-center text-sm">
      {hasFilter ? (
        <>No markers match the current filter.</>
      ) : (
        <>
          You haven&apos;t saved any markers yet. Open the reader and press{" "}
          <kbd className="bg-muted rounded px-1 font-mono text-xs">b</kbd> to
          bookmark a page,{" "}
          <kbd className="bg-muted rounded px-1 font-mono text-xs">n</kbd> to
          add a note, or{" "}
          <kbd className="bg-muted rounded px-1 font-mono text-xs">h</kbd> to
          start a highlight.
        </>
      )}
    </div>
  );
}

async function copyMarkerRegion(
  marker: MarkerView,
  region: MarkerRegion,
): Promise<void> {
  try {
    await copyMarkerImageToClipboard(
      marker.issue_id,
      marker.page_index,
      region,
    );
    toast.success("Copied image to clipboard");
  } catch (err) {
    const message =
      err instanceof Error && err.message === "clipboard-unsupported"
        ? "This browser can't copy images. Try Save instead."
        : "Couldn't copy the image";
    toast.error(message);
  }
}

async function saveMarkerRegion(
  marker: MarkerView,
  region: MarkerRegion,
): Promise<void> {
  try {
    const filename = markerCropFilename({
      seriesSlug: marker.series_slug,
      issueSlug: marker.issue_slug,
      issueId: marker.issue_id,
      pageIndex: marker.page_index,
    });
    await downloadMarkerImage(
      marker.issue_id,
      marker.page_index,
      region,
      filename,
    );
  } catch {
    toast.error("Couldn't save the image");
  }
}

/** Build the reader URL with `?page=<n>` so "Jump to page" lands on the
 *  exact panel/page the marker references. Returns null when the marker
 *  is missing the slug fields (shouldn't happen via /me/markers — the
 *  server hydrates them — but defensive against stale cache rows). */
export function buildJumpHref(m: MarkerView): string | null {
  if (!m.series_slug || !m.issue_slug) return null;
  const base = readerUrl(m.series_slug, m.issue_slug);
  return `${base}?page=${m.page_index}`;
}

export function formatIssueLabel(m: MarkerView): string {
  const parts: string[] = [];
  if (m.series_name) parts.push(m.series_name);
  if (m.issue_number) {
    parts.push(`#${m.issue_number}`);
  } else if (m.issue_title) {
    parts.push(m.issue_title);
  }
  return parts.length > 0 ? parts.join(" · ") : "Unknown issue";
}

const KIND_META: Record<
  MarkerKind,
  {
    icon: typeof BookmarkIcon;
    shortLabel: string;
    badge: string;
  }
> = {
  bookmark: {
    icon: BookmarkIcon,
    shortLabel: "Bookmark",
    badge: "bg-amber-500/90 text-amber-50",
  },
  note: {
    icon: StickyNote,
    shortLabel: "Note",
    badge: "bg-sky-500/90 text-sky-50",
  },
  highlight: {
    icon: Highlighter,
    shortLabel: "Highlight",
    badge: "bg-yellow-500/90 text-yellow-950",
  },
};
