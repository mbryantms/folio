"use client";

import * as React from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import {
  Bookmark as BookmarkIcon,
  Download,
  Highlighter,
  ListChecks,
  Search as SearchIcon,
  Star,
  StickyNote,
  Trash2,
  X,
  MoreVertical,
} from "lucide-react";
import { toast } from "sonner";

import {
  CoverMenuButton,
  type CoverMenuAction,
} from "@/components/CoverMenuButton";
import { useCoverLongPressActions } from "@/components/CoverLongPressActions";
import {
  CardSizeFields,
  CardSizeOptions,
} from "@/components/library/CardSizeOptions";
import {
  PhoneControlsBar,
  PhoneMenuAction,
} from "@/components/library/PhoneControlsBar";
import { SelectionCheckbox } from "@/components/library/SelectionCheckbox";
import { SelectionToolbar } from "@/components/library/SelectionToolbar";
import { StickyToolbarRow } from "@/components/library/StickyToolbarRow";
import { useCardSize } from "@/components/library/use-card-size";
import { PageHeader } from "@/components/admin/PageHeader";
import { SelectModeButton } from "@/components/library/SelectModeButton";
import { NotesExportMenu } from "@/components/markers/NotesExportMenu";
import { FilterPill } from "@/components/ui/filter-pill";
import { Kbd } from "@/components/ui/kbd";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { useMarkerTags } from "@/lib/api/queries";
import {
  useBulkDeleteMarkers,
  useDeleteMarker,
  useRestoreMarkers,
  useUpdateMarker,
} from "@/lib/api/mutations";
import { shouldSkipHotkey } from "@/lib/reader/keybinds";
import { useContainerWidth } from "@/lib/use-container-width";
import { useSelection } from "@/lib/selection/use-selection";
import { useCoarsePointerActionsHint } from "@/lib/ui/use-coarse-pointer";
import { UNDO_TOAST_DURATION_MS } from "@/lib/api/toast-strings";
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
import {
  markerPermalink,
  notesExportHref,
  pageBytesUrl,
  readerUrl,
} from "@/lib/urls";
import { cn } from "@/lib/utils";
import {
  useBookmarkCounts,
  useBookmarkSeries,
  useBookmarksFeedInfinite,
} from "@/lib/api/queries";
import type {
  BookmarkEntryView,
  BookmarkKindCounts,
  BookmarksFeedFilters,
  BookmarksFeedKind,
  BookmarksFeedSort,
  ClosingNoteEntryView,
} from "@/lib/api/types";
import { Combobox } from "@/components/folio/combobox";
import { NativeSelect } from "@/components/ui/native-select";
import {
  ClampedBody,
  ClosingNoteRow,
} from "@/components/markers/ClosingNoteRow";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

type KindFilter = BookmarksFeedKind | "all" | "favorite";

const SORT_OPTIONS: { value: BookmarksFeedSort; label: string }[] = [
  { value: "newest", label: "Newest" },
  { value: "oldest", label: "Oldest" },
  { value: "reading", label: "Reading order" },
];

const FILTER_OPTIONS: { value: KindFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "bookmark", label: "Bookmarks" },
  { value: "note", label: "Notes" },
  // Issue-level closing notes (`user_issue_notes`) — written from the
  // reader's end-of-issue Wrap up pane; rendered as text rows, not
  // picture tiles (`ClosingNoteRow`).
  { value: "closing_note", label: "Closing notes" },
  // v0.3.44: favorite is now a kind again (decoupled from bookmark)
  // but legacy `is_favorite=true` rows still exist on other kinds.
  // The chip wires through `is_favorite=true` so the server's union
  // query (`is_favorite OR kind='favorite'`) covers both shapes.
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

/** Preset server-side scope for an embedded list (WP-5.2). */
export type MarkersListScope =
  { kind: "series"; seriesId: string } | { kind: "issue"; issueId: string };

/** Global feed for the `/bookmarks` page. Renders every kind grouped by
 *  series with a filter chip row and debounced search. Each card jumps
 *  to the reader at the right page; inline kebab handles delete (full
 *  edit lives in the reader to reuse the marker editor sheet).
 *
 *  With a `scope` (WP-5.2) the same list is embedded as the "Your notes"
 *  tab on a series or issue page: the scope rides as a server-side filter
 *  (`series_id` / `issue_id`), the page header and export menu are
 *  dropped, and cards render newest-first without series grouping. */
export function MarkersList({ scope }: { scope?: MarkersListScope } = {}) {
  const embedded = scope !== undefined;
  // One-time touch hint for the now-persistent cover kebab (audit B16).
  useCoarsePointerActionsHint();
  const [filter, setFilter] = React.useState<KindFilter>("all");
  const [rawSearch, setRawSearch] = React.useState("");
  const [debouncedSearch, setDebouncedSearch] = React.useState("");
  const [selectedTags, setSelectedTags] = React.useState<string[]>([]);
  const [tagMatch, setTagMatch] = React.useState<MarkerTagMatch>("all");
  // Sort + Series are server-side like every other filter, so they
  // survive pagination (a client re-sort of the loaded pages would lie
  // past the first page).
  const [sort, setSort] = React.useState<BookmarksFeedSort>("newest");
  const [seriesFilter, setSeriesFilter] = React.useState<string>("");
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
  const kindFilter: BookmarksFeedKind | undefined =
    filter === "all" || filter === "favorite" ? undefined : filter;
  // Infinite scroll — the previous single-shot 200-cap meant power
  // users with > 200 markers silently lost access to older ones.
  // Filters (kind, favorite, tags, search, series, sort) all drive
  // server-side query params, not client-side `.filter()` over the
  // loaded page. The feed unions markers with closing notes.
  const feedFilters: BookmarksFeedFilters = {
    series_id:
      scope?.kind === "series" ? scope.seriesId : seriesFilter || undefined,
    issue_id: scope?.kind === "issue" ? scope.issueId : undefined,
    kind: kindFilter,
    is_favorite: isFavoriteFilter ? true : undefined,
    q: debouncedSearch || undefined,
    tags: selectedTags.length > 0 ? selectedTags.join(",") : undefined,
    tag_match: selectedTags.length > 0 ? tagMatch : undefined,
    sort,
  };
  const query = useBookmarksFeedInfinite(feedFilters);
  // Per-kind counts under the other filters (chips + header), and the
  // series the caller has entries in (the Series filter).
  const counts = useBookmarkCounts(feedFilters);
  const seriesQuery = useBookmarkSeries(!embedded);
  const tagsQuery = useMarkerTags();
  const availableTags = tagsQuery.data?.items ?? [];

  const entries = React.useMemo<BookmarkEntryView[]>(
    () => query.data?.pages.flatMap((p) => p.items) ?? [],
    [query.data],
  );
  // Markers only — the selection / bulk-delete surface is marker-backed;
  // closing notes delete from their own row menu.
  const items = React.useMemo<MarkerView[]>(
    () => entries.flatMap((e) => (e.type === "marker" ? [e.marker] : [])),
    [entries],
  );
  const groups = React.useMemo(() => groupEntriesBySeries(entries), [entries]);
  const runs = React.useMemo(() => splitIntoRuns(entries), [entries]);
  const hasFilterOrSearch =
    filter !== "all" ||
    debouncedSearch.length > 0 ||
    selectedTags.length > 0 ||
    seriesFilter.length > 0;
  const activeSeriesName = seriesQuery.data?.items.find(
    (s) => s.id === seriesFilter,
  )?.name;
  const countFor = (value: KindFilter): number | undefined => {
    const by = counts.data?.by_kind;
    if (!by) return undefined;
    if (value === "all") return counts.data?.total;
    return by[value as keyof BookmarkKindCounts];
  };

  // --- Layout + multi-select (audit B11) ---
  // Grouped-by-series (default) ⇄ flat newest-first. The server already
  // returns markers newest-updated first, so "flat" just renders the
  // loaded order without the per-series grouping pass.
  const [layout, setLayout] = React.useState<"grouped" | "flat">(
    embedded ? "flat" : "grouped",
  );
  // Phone-only "Filters" bottom sheet (layout toggle + tags). On
  // desktop those controls sit inline under the header.
  const [phoneFiltersOpen, setPhoneFiltersOpen] = React.useState(false);
  const phoneFilterCount =
    selectedTags.length +
    (!embedded && layout !== "grouped" ? 1 : 0) +
    (sort !== "newest" ? 1 : 0) +
    (seriesFilter ? 1 : 0);
  const selection = useSelection(items);
  const bulkDelete = useBulkDeleteMarkers();
  const restoreMarkers = useRestoreMarkers();
  // Header total — global (the list endpoint carries no per-filter
  // count), so it reads as "you've saved N markers" regardless of the
  // active chips.
  const selectButtonRef = React.useRef<HTMLButtonElement | null>(null);
  const wasSelectModeRef = React.useRef(false);

  const runBulkDelete = React.useCallback(() => {
    const ids = Array.from(selection.selected);
    if (ids.length === 0) return;
    // Snapshot the selected rows BEFORE the delete invalidates the
    // cache, so Undo can restore them from stable values. Marker
    // deletes are the AlertDialog-confirm exception — they use an Undo
    // toast instead (CLAUDE.md notifications convention).
    const snapshots = items.filter((m) => selection.isSelected(m.id));
    bulkDelete.mutate(ids, {
      onSuccess: (resp) => {
        selection.exit();
        const n = resp?.deleted ?? ids.length;
        toast.success(`Deleted ${n} marker${n === 1 ? "" : "s"}`, {
          duration: UNDO_TOAST_DURATION_MS,
          action: {
            label: "Undo",
            // One request for the whole selection (WP-8.4).
            onClick: () => restoreMarkers.mutate(snapshots),
          },
        });
      },
    });
  }, [bulkDelete, restoreMarkers, items, selection]);

  // Esc exits select mode; Cmd/Ctrl+A selects every loaded card.
  // Dormant while focus is in a form field (the search input).
  React.useEffect(() => {
    if (!selection.selectMode) return;
    const onKey = (e: KeyboardEvent) => {
      if (shouldSkipHotkey(e)) return;
      if (e.key === "Escape") {
        e.preventDefault();
        selection.exit();
      } else if (e.key === "a" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        selection.selectAll();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selection]);
  // Restore focus to the Select trigger after leaving select mode.
  React.useEffect(() => {
    if (wasSelectModeRef.current && !selection.selectMode) {
      selectButtonRef.current?.focus();
    }
    wasSelectModeRef.current = selection.selectMode;
  }, [selection.selectMode]);

  // IntersectionObserver sentinel for infinite scroll. Same rootMargin
  // as `IssuesPanel` / `CblViewDetail` so behavior stays consistent.
  // Depend on the three fields, not the whole result object — TanStack
  // returns a fresh object identity per render, so `[query]` tore the
  // observer down and rebuilt it on every state change.
  const sentinelRef = React.useRef<HTMLDivElement | null>(null);
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = query;
  React.useEffect(() => {
    const el = sentinelRef.current;
    if (!el) return;
    const obs = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          if (hasNextPage && !isFetchingNextPage) {
            void fetchNextPage();
          }
        }
      },
      { rootMargin: "400px" },
    );
    obs.observe(el);
    return () => obs.disconnect();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

  function toggleTag(tag: string) {
    setSelectedTags((prev) =>
      prev.includes(tag) ? prev.filter((t) => t !== tag) : [...prev, tag],
    );
  }

  const headerActions = (
    <>
      {/* `w-full` only below `sm` (the embedded tab stacks it above the
          buttons). From `sm` up the input flexes between 10rem and
          16rem so the row fits on one line down to an iPad-landscape
          column with the sidebar open (~694px) — a fixed `w-64` wrapped
          the row there, and a wrapped row does not pin (rule 3). */}
      <div className="relative w-full sm:max-w-64 sm:min-w-40 sm:flex-1 sm:basis-40">
        <SearchIcon
          className="text-muted-foreground pointer-events-none absolute top-1/2 left-2 h-4 w-4 -translate-y-1/2"
          aria-hidden="true"
        />
        <Input
          type="search"
          value={rawSearch}
          onChange={(e) => setRawSearch(e.target.value)}
          placeholder="Filter saved markers"
          aria-label="Filter saved markers"
          className="pl-8"
        />
      </div>
      <NativeSelect
        aria-label="Sort"
        value={sort}
        onChange={(v) => setSort(v as BookmarksFeedSort)}
        options={SORT_OPTIONS}
        className="h-9 w-auto"
      />
      {embedded ? null : (
        <SeriesFilter
          value={seriesFilter}
          onChange={setSeriesFilter}
          options={seriesQuery.data?.items ?? []}
        />
      )}
      {items.length > 0 || selection.selectMode ? (
        <SelectModeButton
          ref={selectButtonRef}
          active={selection.selectMode}
          onEnter={() => selection.enter()}
          onExit={() => selection.exit()}
        />
      ) : null}
      {embedded ? null : <NotesExportMenu />}
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
    </>
  );

  return (
    <div className="space-y-6">
      {embedded ? (
        // Embedded in a series / issue tab: the tab already names the
        // surface, so only the controls row renders (no second h1).
        <div className="flex flex-wrap items-center justify-end gap-2">
          {headerActions}
        </div>
      ) : (
        <PageHeader
          title="Bookmarks"
          description={headerDescription({
            all: counts.data?.all,
            shown: countFor(filter),
            filtered: hasFilterOrSearch,
            seriesName: activeSeriesName,
          })}
          phoneDescription="hidden"
          // Below `sm` the whole header goes: the description is already
          // hidden there and the title rides in the PhoneControlsBar's
          // leading slot, so title + controls share one sticky row
          // instead of stacking two bordered bands.
          className="max-sm:hidden"
        />
      )}

      {embedded ? null : (
        // `sm` and up: search / sort / series / Select / Export / card
        // size in their own row under the header, pinned under the
        // topbar while it fits one line (the rule in
        // docs/dev/phone-layout.md: rows that hold search, sort or
        // filters pin at every width). It used to sit inside the
        // header's actions and scrolled away with the title.
        <StickyToolbarRow enabled={!selection.selectMode}>
          {headerActions}
        </StickyToolbarRow>
      )}

      {embedded ? null : (
        <>
          {/* Phone (< sm): one sticky row — title · search / filters /
              View options
              — in place of the page header, its toolbar, the layout
              toggle and the Tags row. The selection toolbar takes its
              place while selecting (both are sticky under the topbar). */}
          <PhoneControlsBar
            hidden={selection.selectMode}
            leading={
              <h1 className="min-w-0 truncate text-xl font-semibold tracking-tight">
                Bookmarks
              </h1>
            }
            search={{
              value: rawSearch,
              onChange: setRawSearch,
              placeholder: "Filter saved markers",
              label: "Filter saved markers",
            }}
            filters={{
              count: phoneFilterCount,
              onOpen: () => setPhoneFiltersOpen(true),
            }}
            menu={(close) => (
              <>
                <CardSizeFields
                  cardSize={cardSize}
                  onCardSize={setCardSize}
                  min={CARD_SIZE_MIN}
                  max={CARD_SIZE_MAX}
                  step={CARD_SIZE_STEP}
                  defaultSize={CARD_SIZE_DEFAULT}
                  fieldId="bookmarks-phone-card-size"
                />
                <div className="grid gap-2">
                  {items.length > 0 ? (
                    <PhoneMenuAction
                      icon={<ListChecks className="size-4" />}
                      onClick={() => {
                        close();
                        selection.enter();
                      }}
                    >
                      Select
                    </PhoneMenuAction>
                  ) : null}
                  {/* Same two `<a download>` links as `NotesExportMenu`. */}
                  <PhoneMenuAction asChild>
                    <a href={notesExportHref("md")} download>
                      <Download className="size-4" aria-hidden="true" />
                      Export notes as Markdown (.md)
                    </a>
                  </PhoneMenuAction>
                  <PhoneMenuAction asChild>
                    <a href={notesExportHref("json")} download>
                      <Download className="size-4" aria-hidden="true" />
                      Export notes as JSON (.json)
                    </a>
                  </PhoneMenuAction>
                </div>
              </>
            )}
          />
          <Sheet open={phoneFiltersOpen} onOpenChange={setPhoneFiltersOpen}>
            <SheetContent
              side="bottom"
              className="rounded-t-lg pb-[max(1.5rem,var(--safe-bottom))]"
            >
              <SheetHeader>
                <SheetTitle>Filters</SheetTitle>
              </SheetHeader>
              <div className="space-y-5">
                <div className="space-y-2">
                  <p className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
                    Layout
                  </p>
                  <LayoutToggle
                    layout={layout}
                    onLayout={setLayout}
                    className="grid w-full grid-cols-2 [&>button]:h-9"
                  />
                </div>
                <div className="space-y-2">
                  <p className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
                    Sort
                  </p>
                  <NativeSelect
                    aria-label="Sort"
                    value={sort}
                    onChange={(v) => setSort(v as BookmarksFeedSort)}
                    options={SORT_OPTIONS}
                    className="folio-control"
                  />
                </div>
                <div className="space-y-2">
                  <p className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
                    Series
                  </p>
                  <NativeSelect
                    aria-label="Series"
                    value={seriesFilter}
                    onChange={setSeriesFilter}
                    options={[
                      { value: "", label: "All series" },
                      ...(seriesQuery.data?.items ?? []).map((s) => ({
                        value: s.id,
                        label: `${s.name} (${s.count})`,
                      })),
                    ]}
                    className="folio-control"
                  />
                </div>
                {availableTags.length > 0 ? (
                  <div className="space-y-2">
                    <div className="flex items-center justify-between gap-2">
                      <p className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
                        Tags
                      </p>
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
                    <div className="flex flex-wrap gap-1.5">
                      {availableTags.map((t) => (
                        <FilterPill
                          key={t.tag}
                          active={selectedTags.includes(t.tag)}
                          aria-pressed={selectedTags.includes(t.tag)}
                          onClick={() => toggleTag(t.tag)}
                          count={t.count}
                        >
                          <span>{t.tag}</span>
                        </FilterPill>
                      ))}
                    </div>
                    {selectedTags.length > 1 ? (
                      <TagMatchToggle
                        tagMatch={tagMatch}
                        onTagMatch={setTagMatch}
                        className="grid w-full grid-cols-2 [&>button]:h-9"
                      />
                    ) : null}
                  </div>
                ) : null}
                <Button
                  type="button"
                  className="h-11 w-full"
                  onClick={() => setPhoneFiltersOpen(false)}
                >
                  Done
                </Button>
              </div>
            </SheetContent>
          </Sheet>
        </>
      )}

      <SelectionToolbar
        open={selection.selectMode}
        count={selection.count}
        total={items.length}
        primary={[
          {
            id: "delete",
            label: "Delete",
            icon: Trash2,
            onClick: runBulkDelete,
            disabled: bulkDelete.isPending || selection.count === 0,
            destructive: true,
          },
        ]}
        onDone={() => selection.exit()}
        onClear={() => selection.clear()}
        onSelectAll={() => selection.selectAll()}
        isPending={bulkDelete.isPending}
      />

      <div className="flex flex-wrap items-center gap-2">
        {/* Phone: one horizontally scrolling strip (edge to edge inside
            the shell's `px-4`) instead of wrapping onto two rows. */}
        <div
          role="tablist"
          aria-label="Filter by kind"
          className={cn(
            "flex flex-wrap gap-1 max-sm:min-w-0 max-sm:flex-1 max-sm:[scrollbar-width:none] max-sm:flex-nowrap max-sm:overflow-x-auto max-sm:overflow-y-hidden max-sm:[&>*]:shrink-0",
            !embedded && "max-sm:-mx-4 max-sm:px-4",
          )}
        >
          {FILTER_OPTIONS.map((opt) => (
            <FilterPill
              key={opt.value}
              role="tab"
              active={filter === opt.value}
              aria-selected={filter === opt.value}
              onClick={() => setFilter(opt.value)}
              count={countFor(opt.value)}
            >
              {opt.label}
            </FilterPill>
          ))}
        </div>
        {/* Grouped-by-series ⇄ flat newest-first (audit B11). Hidden
            when scoped to one series / issue — grouping is moot there.
            On phones it lives in the Filters sheet instead. */}
        <LayoutToggle
          hidden={embedded}
          layout={layout}
          onLayout={setLayout}
          className="ml-auto max-sm:hidden"
        />
      </div>

      {availableTags.length > 0 ? (
        // On the /bookmarks page the Tags row moves into the phone
        // Filters sheet; embedded lists keep it inline everywhere.
        <div
          className={cn(
            "flex flex-wrap items-center gap-2",
            !embedded && "max-sm:hidden",
          )}
        >
          <span className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
            Tags
          </span>
          <div className="flex flex-wrap gap-1">
            {availableTags.map((t) => (
              <FilterPill
                key={t.tag}
                active={selectedTags.includes(t.tag)}
                aria-pressed={selectedTags.includes(t.tag)}
                onClick={() => toggleTag(t.tag)}
                count={t.count}
              >
                <span>{t.tag}</span>
              </FilterPill>
            ))}
          </div>
          {selectedTags.length > 1 ? (
            <TagMatchToggle
              tagMatch={tagMatch}
              onTagMatch={setTagMatch}
              className="ml-1"
            />
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
      ) : entries.length === 0 ? (
        <EmptyState hasFilter={hasFilterOrSearch} scope={scope} />
      ) : (
        <div className="space-y-8">
          {layout === "grouped"
            ? groups.map((group) => (
                <section key={group.key} className="space-y-3">
                  <h2 className="text-muted-foreground text-xs font-medium tracking-wide uppercase">
                    {group.label}
                  </h2>
                  {/* Closing notes lead the group as full-width text rows:
                    "what I thought of the issues, then what I marked in
                    them". */}
                  {group.notes.map((n) => (
                    <ClosingNoteRow key={`note-${n.issue_id}`} note={n} />
                  ))}
                  {group.pageNotes.map((m) => (
                    <PageNoteRow
                      key={m.id}
                      marker={m}
                      selectMode={
                        selection.selectMode
                          ? {
                              isActive: true,
                              isSelected: selection.isSelected(m.id),
                              onToggle: (ev) => selection.toggle(m.id, ev),
                            }
                          : undefined
                      }
                      onEnterSelectMode={(id) => selection.toggle(id)}
                    />
                  ))}
                  {group.markers.length > 0 ? (
                    <RowPackedSection
                      items={group.markers}
                      cardSize={cardSize}
                      selection={selection}
                    />
                  ) : null}
                </section>
              ))
            : // Flat (Newest / Oldest): closing notes interleave as rows
              // between runs of packed tiles, in server order.
              runs.map((run) =>
                run.type === "note" ? (
                  <ClosingNoteRow
                    key={`note-${run.note.issue_id}`}
                    note={run.note}
                  />
                ) : run.type === "pageNote" ? (
                  <PageNoteRow
                    key={run.marker.id}
                    marker={run.marker}
                    selectMode={
                      selection.selectMode
                        ? {
                            isActive: true,
                            isSelected: selection.isSelected(run.marker.id),
                            onToggle: (ev) =>
                              selection.toggle(run.marker.id, ev),
                          }
                        : undefined
                    }
                    onEnterSelectMode={(id) => selection.toggle(id)}
                  />
                ) : (
                  <RowPackedSection
                    key={run.key}
                    items={run.markers}
                    cardSize={cardSize}
                    selection={selection}
                  />
                ),
              )}
          <div
            ref={sentinelRef}
            aria-hidden
            className={query.hasNextPage ? "h-12" : "hidden"}
          />
          {query.isFetchingNextPage ? (
            <p
              role="status"
              className="text-muted-foreground text-center text-xs"
            >
              Loading more markers…
            </p>
          ) : null}
        </div>
      )}
    </div>
  );
}

/** Grouped-by-series ⇄ flat newest-first segmented toggle. Rendered
 *  inline on desktop and inside the phone Filters sheet. */
function LayoutToggle({
  layout,
  onLayout,
  hidden,
  className,
}: {
  layout: "grouped" | "flat";
  onLayout: (next: "grouped" | "flat") => void;
  hidden?: boolean;
  className?: string;
}) {
  return (
    <div
      hidden={hidden}
      role="group"
      aria-label="Marker layout"
      className={cn(
        "bg-muted inline-flex items-center gap-0.5 rounded-md p-0.5",
        className,
      )}
    >
      {(
        [
          { value: "grouped", label: "By series" },
          { value: "flat", label: "Newest" },
        ] as const
      ).map((opt) => (
        <button
          key={opt.value}
          type="button"
          aria-pressed={layout === opt.value}
          onClick={() => onLayout(opt.value)}
          className={cn(
            "folio-tap rounded px-2 py-0.5 text-xs transition-colors",
            layout === opt.value
              ? "bg-background text-foreground shadow-sm"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          {opt.label}
        </button>
      ))}
    </div>
  );
}

/** "Match all" ⇄ "Match any" toggle for multi-tag filters. */
function TagMatchToggle({
  tagMatch,
  onTagMatch,
  className,
}: {
  tagMatch: MarkerTagMatch;
  onTagMatch: (next: MarkerTagMatch) => void;
  className?: string;
}) {
  return (
    <div
      role="group"
      aria-label="Tag match mode"
      className={cn(
        "bg-muted inline-flex items-center gap-0.5 rounded-md p-0.5",
        className,
      )}
    >
      {(["all", "any"] as const).map((mode) => (
        <button
          key={mode}
          type="button"
          aria-pressed={tagMatch === mode}
          onClick={() => onTagMatch(mode)}
          className={cn(
            "folio-tap rounded px-2 py-0.5 text-xs transition-colors",
            tagMatch === mode
              ? "bg-background text-foreground shadow-sm"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          Match {mode}
        </button>
      ))}
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
 *  use the whole-page aspect.
 *
 *  When the region carries the page's natural pixel size (`page_w`/`page_h`,
 *  stamped at capture time), we use the *real* page aspect instead of the 2:3
 *  approximation — this both sizes the tile correctly AND un-distorts the
 *  rendered crop (the renderer scales the page to fill the tile, so the tile
 *  aspect drives the displayed page aspect). Markers saved before the stamp,
 *  and page-level markers, fall back to `PAGE_ASPECT`. */
export function markerTileAspect(marker: MarkerView): number {
  const region = marker.region;
  if (!region || region.w <= 0 || region.h <= 0) {
    return PAGE_ASPECT;
  }
  const pageAspect =
    region.page_w && region.page_h && region.page_h > 0
      ? region.page_w / region.page_h
      : PAGE_ASPECT;
  return (region.w / region.h) * pageAspect;
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

/** Render one series's markers as a justified row-packed grid where
 *  card width tracks the highlighted region's aspect ratio. Wide
 *  panel selections produce wide tiles; narrow vertical strips
 *  produce tall narrow tiles. */
/** The slice of `useSelection` the card grid needs. Kept structural so
 *  callers can pass the hook return directly. */
type CardSelection = {
  selectMode: boolean;
  isSelected: (id: string) => boolean;
  toggle: (id: string, ev?: React.MouseEvent) => void;
};

function RowPackedSection({
  items,
  cardSize,
  selection,
}: {
  items: ReadonlyArray<MarkerView>;
  cardSize: number;
  selection?: CardSelection;
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
                  selectMode={
                    selection?.selectMode
                      ? {
                          isActive: true,
                          isSelected: selection.isSelected(slot.ref.marker.id),
                          onToggle: (ev) =>
                            selection.toggle(slot.ref.marker.id, ev),
                        }
                      : undefined
                  }
                  onEnterSelectMode={
                    selection ? (id) => selection.toggle(id) : undefined
                  }
                />
              </div>
            ))}
          </div>
        ))
      )}
    </div>
  );
}

/** Header copy: the unfiltered total, or "Showing N of M" once any chip,
 *  tag, search or series filter narrows the feed. */
export function headerDescription({
  all,
  shown,
  filtered,
  seriesName,
}: {
  all: number | undefined;
  shown: number | undefined;
  filtered: boolean;
  seriesName?: string;
}): string {
  if (all === undefined) {
    return "Every page bookmark, note, favorite, highlight, and closing note you’ve saved across your library.";
  }
  if (!filtered || shown === undefined) {
    return `${all} saved across your library — every page bookmark, note, favorite, highlight, and closing note.`;
  }
  const suffix = seriesName ? ` — filtered to ${seriesName}` : "";
  return `Showing ${shown} of ${all} saved${suffix}.`;
}

/** Series filter for the desktop toolbar: a searchable combobox over the
 *  series the caller has entries in, with counts. */
function SeriesFilter({
  value,
  onChange,
  options,
}: {
  value: string;
  onChange: (next: string) => void;
  options: ReadonlyArray<{ id: string; name: string; count: number }>;
}) {
  const active = options.find((s) => s.id === value);
  return (
    <div className="flex items-center gap-1">
      <Combobox
        value={value}
        onChange={(v) => onChange(v === value ? "" : v)}
        placeholder="Series"
        searchPlaceholder="Filter series…"
        emptyText="No series with markers."
        options={options.map((s) => ({
          value: s.id,
          label: s.name,
          description: `${s.count} saved`,
        }))}
        triggerClassName={cn(
          "h-9 w-auto min-w-28 max-w-56",
          active && "border-primary bg-primary/10",
        )}
        // The popover defaults to the trigger's width, which is the
        // placeholder's — too narrow to read a series name or the
        // search field. Fixed width instead.
        contentClassName="w-72"
      />
      {active ? (
        <button
          type="button"
          aria-label="Clear series filter"
          onClick={() => onChange("")}
          // `folio-button` so the clear target floors at 44px on touch
          // like the combobox trigger beside it.
          className="folio-button text-muted-foreground hover:text-foreground hover:bg-muted/50 inline-flex h-9 w-9 items-center justify-center rounded-md transition-colors"
        >
          <X className="h-3.5 w-3.5" aria-hidden="true" />
        </button>
      ) : null}
    </div>
  );
}

/** A long or multi-line snippet gets a three-line clamp plus More / Less
 *  that expands in place, so reading a page note never requires the
 *  reader. */
const SNIPPET_CLAMP_CHARS = 120;
function ExpandableSnippet({ text }: { text: string }) {
  const [expanded, setExpanded] = React.useState(false);
  const clamp = text.length > SNIPPET_CLAMP_CHARS || text.includes("\n");
  return (
    <>
      <p
        className={cn(
          "text-muted-foreground text-xs whitespace-pre-line",
          clamp && !expanded && "line-clamp-3",
        )}
      >
        {text}
      </p>
      {clamp ? (
        <button
          type="button"
          aria-expanded={expanded}
          onClick={() => setExpanded((e) => !e)}
          className="text-primary self-start text-xs font-medium hover:underline"
        >
          {expanded ? "Less" : "More"}
        </button>
      ) : null}
    </>
  );
}

/** A page note with text renders as a text row (`PageNoteRow`), the
 *  same shape as a closing note; bookmarks, favorites, highlights and a
 *  bodyless note stay picture tiles. */
export function isPageNoteRow(m: MarkerView): boolean {
  return m.kind === "note" && !!m.body?.trim();
}

/**
 * A page note as a full-width text row: page thumb (the crop when the
 * note has a region) linking to the page, Note badge, issue + page,
 * tags, date, the same kebab as the tile, and the body clamped at four
 * lines with More / Less. In select mode the thumb carries the checkbox
 * and the whole row toggles.
 */
function PageNoteRow({
  marker,
  selectMode,
  onEnterSelectMode,
}: {
  marker: MarkerView;
  selectMode?: {
    isActive: boolean;
    isSelected: boolean;
    onToggle: (ev?: React.MouseEvent) => void;
  };
  onEnterSelectMode?: (id: string) => void;
}) {
  const { actions, jumpHref } = useMarkerActions(marker, onEnterSelectMode);
  const kindMeta = KIND_META[marker.kind];
  const KindIcon = kindMeta.icon;
  const issueLabel = `${formatIssueLabel(marker)} · Page ${marker.page_index + 1}`;
  const inSelect = selectMode?.isActive ?? false;
  const body = marker.body?.trim() ?? "";
  const thumb = (
    <div className="group relative w-14 shrink-0">
      <MarkerThumbnail marker={marker} height={84} />
      {selectMode?.isActive ? (
        <SelectionCheckbox
          isSelected={selectMode.isSelected}
          selectMode
          onToggle={selectMode.onToggle}
          label={`Note · page ${marker.page_index + 1}`}
        />
      ) : null}
    </div>
  );
  return (
    <article
      data-page-note-row=""
      data-selected={inSelect && selectMode?.isSelected ? "" : undefined}
      onClick={inSelect ? (ev) => selectMode?.onToggle(ev) : undefined}
      className={cn(
        "border-border bg-card flex gap-3.5 rounded-lg border p-3",
        inSelect && "cursor-pointer",
        inSelect && selectMode?.isSelected && "ring-primary ring-2",
      )}
    >
      {!inSelect && jumpHref ? (
        <Link
          href={jumpHref}
          aria-label={`Jump to page ${marker.page_index + 1}`}
          className="shrink-0"
        >
          {thumb}
        </Link>
      ) : (
        thumb
      )}
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
          <span
            className={cn(
              "inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-semibold tracking-wide uppercase",
              kindMeta.badge,
            )}
          >
            <KindIcon className="h-3 w-3" aria-hidden="true" />
            {kindMeta.shortLabel}
          </span>
          <span
            className="text-muted-foreground truncate text-xs"
            title={issueLabel}
          >
            {issueLabel}
          </span>
          {marker.is_favorite ? (
            <Star
              aria-label="Favorite"
              className="h-3.5 w-3.5 fill-current text-rose-400"
            />
          ) : null}
          {marker.tags.slice(0, 4).map((t) => (
            <span
              key={t}
              className="bg-muted text-muted-foreground inline-flex items-center rounded px-1.5 py-0.5 text-[10px]"
            >
              {t}
            </span>
          ))}
          <span className="ml-auto flex shrink-0 items-center gap-1.5">
            <span className="text-muted-foreground text-xs whitespace-nowrap">
              {new Date(marker.updated_at).toLocaleDateString(undefined, {
                year: "numeric",
                month: "short",
                day: "numeric",
              })}
            </span>
            {!inSelect ? (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button
                    type="button"
                    aria-label={`Actions for note on page ${marker.page_index + 1}`}
                    className="folio-tap text-muted-foreground hover:text-foreground hover:bg-muted/50 inline-flex h-7 w-7 shrink-0 items-center justify-center rounded transition-colors"
                  >
                    <MoreVertical className="h-4 w-4" aria-hidden="true" />
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  {actions.map((a) => (
                    <DropdownMenuItem
                      key={a.label}
                      disabled={a.disabled}
                      onSelect={a.onSelect}
                      className={
                        a.destructive
                          ? "text-destructive focus:text-destructive"
                          : undefined
                      }
                    >
                      {a.label}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            ) : null}
          </span>
        </div>
        <ClampedBody text={body} lines={4} />
      </div>
    </article>
  );
}

/** Group feed entries by series in first-appearance order (the server
 *  already sorted the stream); closing notes, page-note rows and picture
 *  markers kept apart so the group renders rows above the packed tiles. */
export function groupEntriesBySeries(
  entries: ReadonlyArray<BookmarkEntryView>,
): {
  key: string;
  label: string;
  notes: ClosingNoteEntryView[];
  pageNotes: MarkerView[];
  markers: MarkerView[];
}[] {
  const map = new Map<
    string,
    {
      label: string;
      notes: ClosingNoteEntryView[];
      pageNotes: MarkerView[];
      markers: MarkerView[];
    }
  >();
  for (const e of entries) {
    const key = e.type === "marker" ? e.marker.series_id : e.note.series_id;
    const label =
      (e.type === "marker" ? e.marker.series_name : e.note.series_name) ??
      "Unknown series";
    const entry = map.get(key) ?? {
      label,
      notes: [],
      pageNotes: [],
      markers: [],
    };
    if (e.type === "closing_note") entry.notes.push(e.note);
    else if (isPageNoteRow(e.marker)) entry.pageNotes.push(e.marker);
    else entry.markers.push(e.marker);
    map.set(key, entry);
  }
  return Array.from(map.entries()).map(([key, value]) => ({ key, ...value }));
}

/** Flat layout: consecutive picture markers become one packed run; each
 *  closing note and each page note is its own full-width row, in server
 *  order. */
export type FeedRun =
  | { type: "markers"; key: string; markers: MarkerView[] }
  | { type: "note"; note: ClosingNoteEntryView }
  | { type: "pageNote"; marker: MarkerView };
export function splitIntoRuns(
  entries: ReadonlyArray<BookmarkEntryView>,
): FeedRun[] {
  const out: FeedRun[] = [];
  for (const e of entries) {
    if (e.type === "closing_note") {
      out.push({ type: "note", note: e.note });
      continue;
    }
    if (isPageNoteRow(e.marker)) {
      out.push({ type: "pageNote", marker: e.marker });
      continue;
    }
    const last = out[out.length - 1];
    if (last && last.type === "markers") last.markers.push(e.marker);
    else
      out.push({
        type: "markers",
        key: `run-${e.marker.id}`,
        markers: [e.marker],
      });
  }
  return out;
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

/** The per-marker action list (Jump, Copy link, Favorite, Copy / Save
 *  image for crops, Delete with Undo) shared by the picture tile and the
 *  page-note row. */
function useMarkerActions(
  marker: MarkerView,
  onEnterSelectMode?: (id: string) => void,
): { actions: CoverMenuAction[]; jumpHref: string | null; hasRegion: boolean } {
  const del = useDeleteMarker(marker.id, marker.issue_id, { silent: true });
  const restore = useRestoreMarkers();
  const update = useUpdateMarker(marker.id, marker.issue_id);
  const router = useRouter();
  const jumpHref = buildJumpHref(marker);
  // Image-bearing markers — anything with a region — get Copy/Save
  // affordances. We only want them for crops where the user
  // intentionally selected an area (any shape: rect / text / image);
  // page-level bookmarks would just dump the whole page, which the
  // reader already offers more directly. Page-level markers fall back
  // to the original two-action menu.
  const hasRegion = !!marker.region;

  const actions: CoverMenuAction[] = React.useMemo(() => {
    const list: CoverMenuAction[] = [];
    // Touch / long-press path into multi-select (no hover affordance on
    // coarse pointers). Mirrors the grid cards' "Select" entry.
    if (onEnterSelectMode) {
      list.push({
        label: "Select",
        onSelect: () => onEnterSelectMode(marker.id),
      });
    }
    list.push(
      {
        label: "Jump to page",
        onSelect: () => {
          if (jumpHref) router.push(jumpHref);
        },
        disabled: !jumpHref,
      },
      {
        // Permalink (WP-5.1): `/markers/{id}` 303s to the reader at this
        // page in peek mode, so the link survives slug changes.
        label: "Copy link",
        onSelect: () => {
          void copyMarkerLink(marker.id);
        },
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
    );
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
        // so Undo can restore from a stable value (we can't read
        // `marker` after the cache invalidation drops it).
        const snapshot = marker;
        del.mutate(undefined, {
          onSuccess: () =>
            toast.success("Removed", {
              duration: UNDO_TOAST_DURATION_MS,
              action: {
                label: "Undo",
                onClick: () => restore.mutate([snapshot]),
              },
            }),
        });
      },
      disabled: del.isPending,
    });
    return list;
  }, [
    restore,
    del,
    hasRegion,
    jumpHref,
    marker,
    onEnterSelectMode,
    router,
    update,
  ]);
  return { actions, jumpHref, hasRegion };
}

function MarkerCard({
  marker,
  thumbHeight,
  selectMode,
  onEnterSelectMode,
}: {
  marker: MarkerView;
  thumbHeight: number;
  /** When active, the whole card is a selection toggle (audit B11). */
  selectMode?: {
    isActive: boolean;
    isSelected: boolean;
    onToggle: (ev?: React.MouseEvent) => void;
  };
  /** Long-press / menu "Select" entry point into select mode. */
  onEnterSelectMode?: (id: string) => void;
}) {
  const inSelect = selectMode?.isActive ?? false;
  const { actions, jumpHref } = useMarkerActions(marker, onEnterSelectMode);
  const router = useRouter();
  const kindMeta = KIND_META[marker.kind];
  const KindIcon = kindMeta.icon;
  const snippet = marker.body?.trim() || marker.selection?.text?.trim() || null;
  const issueLabel = formatIssueLabel(marker);

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
    <div className="relative" {...(inSelect ? {} : longPress.wrapperProps)}>
      {/* Thumbnail height comes from the row-packed layout above;
       *  width is `100%` of the parent slot, which itself was sized
       *  to `aspect × height`. So the wrapper ends up an exact tile
       *  with no letterboxing. */}
      <MarkerThumbnail marker={marker} height={thumbHeight} />
      {selectMode?.isActive ? (
        // In select mode the checkbox takes the top-left slot the kind
        // badge usually owns; the whole card is the toggle (parent
        // button), so this is a decorative span.
        <SelectionCheckbox
          isSelected={selectMode.isSelected}
          selectMode
          onToggle={selectMode.onToggle}
          label={`${kindMeta.shortLabel} · page ${marker.page_index + 1}`}
        />
      ) : (
        <span
          className={cn(
            "absolute top-2 left-2 inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-medium uppercase",
            kindMeta.badge,
          )}
        >
          <KindIcon className="h-3 w-3" aria-hidden="true" />
          {kindMeta.shortLabel}
        </span>
      )}
      {marker.is_favorite ? (
        <span
          aria-label="Favorite"
          title="Favorite"
          className="absolute right-2 bottom-2 inline-flex h-5 w-5 items-center justify-center rounded-full bg-rose-500/90 text-rose-50 shadow-sm ring-1 ring-black/20"
        >
          <Star className="h-3 w-3 fill-current" aria-hidden="true" />
        </span>
      ) : null}
      {/* No per-card menu / long-press while selecting — the card click
          toggles selection instead. */}
      {!inSelect ? (
        <>
          <CoverMenuButton
            label={`Actions for ${kindMeta.shortLabel} on page ${marker.page_index + 1}`}
            actions={actions}
          />
          {longPress.sheet}
        </>
      ) : null}
    </div>
  );

  return (
    <div className="group flex h-full flex-col gap-2 rounded-md p-1">
      {selectMode?.isActive ? (
        <button
          type="button"
          onClick={(ev) => {
            ev.preventDefault();
            selectMode.onToggle(ev);
          }}
          aria-pressed={selectMode.isSelected}
          aria-label={`${selectMode.isSelected ? "Deselect" : "Select"} ${kindMeta.shortLabel} on page ${marker.page_index + 1}`}
          className={cn(
            "focus-visible:ring-ring block rounded-md text-left focus-visible:ring-2 focus-visible:outline-none",
            selectMode.isSelected && "ring-primary ring-2",
          )}
        >
          {cardBody}
        </button>
      ) : jumpHref ? (
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
        {snippet ? <ExpandableSnippet text={snippet} /> : null}
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

function EmptyState({
  hasFilter,
  scope,
}: {
  hasFilter: boolean;
  scope?: MarkersListScope;
}) {
  return (
    <div className="border-border/60 text-muted-foreground rounded-lg border border-dashed p-8 text-center text-sm">
      {hasFilter ? (
        <>Nothing matches the current filters.</>
      ) : scope ? (
        <>
          No markers{" "}
          {scope.kind === "series" ? "in this series" : "in this issue"} yet. In
          the reader, press <Kbd>b</Kbd> to bookmark a page, <Kbd>n</Kbd> to add
          a note, or <Kbd>h</Kbd> to highlight.
        </>
      ) : (
        <>
          You haven&apos;t saved any markers yet. Open the reader and press{" "}
          <Kbd>b</Kbd> to bookmark a page, <Kbd>n</Kbd> to add a note, or{" "}
          <Kbd>h</Kbd> to start a highlight — or use the bookmark, star, and
          marker buttons in the reader&apos;s top toolbar.
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

/** Copy the absolute marker permalink to the clipboard. */
export async function copyMarkerLink(id: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(
      markerPermalink(id, window.location.origin),
    );
    toast.success("Link copied");
  } catch {
    toast.error("Couldn't copy the link");
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
 *  exact panel/page the marker references. Adds `?peek=1` so the
 *  reader opens in peek mode — progress writes and session tracking
 *  stay suppressed until the user clicks "Continue from here" in the
 *  peek banner. Keeps a quick glance at a bookmark from polluting On
 *  Deck, last-read state, or the reading log feed.
 *
 *  Returns null when the marker is missing the slug fields (shouldn't
 *  happen via /me/markers — the server hydrates them — but defensive
 *  against stale cache rows). */
export function buildJumpHref(m: MarkerView): string | null {
  if (!m.series_slug || !m.issue_slug) return null;
  const base = readerUrl(m.series_slug, m.issue_slug);
  return `${base}?page=${m.page_index}&peek=1`;
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
  favorite: {
    icon: Star,
    shortLabel: "Favorite",
    badge: "bg-amber-500/90 text-amber-50",
  },
  highlight: {
    icon: Highlighter,
    shortLabel: "Highlight",
    badge: "bg-yellow-500/90 text-yellow-950",
  },
};
