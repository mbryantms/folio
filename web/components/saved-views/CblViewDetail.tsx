"use client";

import * as React from "react";
import dynamic from "next/dynamic";
import { useSearchParams } from "next/navigation";
import { toast } from "sonner";
import {
  BookOpen,
  Check,
  Circle,
  Download,
  Eye,
  EyeOff,
  FileCog,
  HardDriveDownload,
  Link2,
  ListChecks,
  Loader2,
  Pencil,
  LayoutGrid,
  ListTree,
  MoreHorizontal,
  RefreshCw,
  Search as SearchIcon,
  Sparkles,
  Waypoints,
  FolderPlus,
} from "lucide-react";
import { ApiMutationError } from "@/lib/api/mutations/_core";
import { DeriveLinksDialog } from "@/components/cbl/links/DeriveLinksDialog";
import {
  LostAnchorsNotice,
  MoveEventDialog,
  anchorSiblings,
  reorderWrites,
} from "@/components/cbl/links/MoveEventDialog";
import { PartOfLink } from "@/components/cbl/links/PartOfLink";
import { AddToCollectionDialog } from "@/components/collections/AddToCollectionDialog";

import { CblDetail, CblInfoRow } from "@/components/cbl/cbl-detail";
import { RefreshFailureNotice } from "@/components/cbl/RefreshFailureNotice";
import { CblIssueCard } from "@/components/cbl/cbl-issue-card";
import { CblStatsPills } from "@/components/cbl/CblStatsPills";
import {
  BulkMarkReadDialog,
  BULK_BACKFILL_PROMPT_THRESHOLD,
} from "@/components/library/BulkMarkReadDialog";
import {
  CoverSizeFields,
  CoverSizeOptions,
} from "@/components/library/CoverSizeOptions";
import {
  PhoneControlsBar,
  PhoneMenuAction,
} from "@/components/library/PhoneControlsBar";
import { SelectModeButton } from "@/components/library/SelectModeButton";
import { SelectionToolbar } from "@/components/library/SelectionToolbar";
import { useAdminSelectionActions } from "@/components/library/useAdminSelectionActions";
import { useWantToReadSelectionAction } from "@/components/collections/useWantToReadSelectionAction";
import {
  CoverGridSkeleton,
  LoadingRegion,
  PageHeaderSkeleton,
} from "@/components/skeletons/LoadingShapes";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/folio/dropdown-menu";
import { Input } from "@/components/ui/input";
import { PopoverPortalContainer } from "@/components/folio/popover";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/folio/sheet";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/folio/tooltip";
import {
  useCblList,
  useCblListEntriesInfinite,
  useCblListLinks,
  useCblListWindow,
  useMe,
} from "@/lib/api/queries";
import {
  useBulkMarkProgress,
  useCreateCblBreak,
  useCreateCblLink,
  useCreateSavedViewBatch,
  useDeleteCblBreak,
  useDeleteCblLink,
  useMarkCblReadThrough,
  useRefreshCblList,
  useReorderCblLinks,
  useSkipCblLink,
  useUpdateCblLink,
  useUpdateCblBreak,
} from "@/lib/api/mutations";
import { EventBand } from "@/components/cbl/links/EventBand";
import {
  chapterTops,
  scrollBehavior,
} from "@/components/cbl/links/use-current-chapter";
import {
  ChapterHeader,
  ChapterRail,
  ChapterRailButton,
  ChapterRailSheet,
  chapterAnchorId,
} from "@/components/cbl/links/ChapterParts";
import {
  ChapterNameDialog,
  InsertListDialog,
  MarkThroughDialog,
  RemoveBreakDialog,
  RemoveLinkDialog,
} from "@/components/cbl/links/LinkDialogs";
import {
  buildChapterBlocks,
  chapterKey,
  chapterStep,
  foldedChapters,
  isChapterDone,
  seekStart,
  type PlanItem,
} from "@/lib/cbl/linked-plan";
import {
  useChapterRailHidden,
  usePlainListView,
  useShowReadChapters,
} from "@/lib/cbl/use-show-read-chapters";
import { usePinnedControlsHeight } from "@/lib/ui/use-pinned-controls-height";
import { listShortcutFor } from "@/lib/keyboard/list-shortcuts";
import type { ReadingListLinkView } from "@/lib/api/types";
import { skippedEntryIds } from "@/lib/library/bulk-archive-skips";
import { COVER_GRID_STYLE } from "@/lib/library/cover-size";
import { shouldSkipHotkey } from "@/lib/reader/keybinds";
import { useSelection } from "@/lib/selection/use-selection";
import type { CblEntryHydratedView, SavedViewView } from "@/lib/api/types";
import { useCblHideMissing } from "@/lib/cbl/use-hide-missing";

import { StickyToolbarRow } from "@/components/library/StickyToolbarRow";
import { LoadMoreError } from "@/components/folio/infinite-sentinel";
import { useInfiniteSentinel } from "@/lib/ui/use-infinite-sentinel";
import { RailError } from "@/components/saved-views/RailError";

import { ViewActionsMenu } from "./ViewActionsMenu";
import { ViewHeader } from "./ViewHeader";

// Heavy archive-edit dialog (+dnd) — lazy so it stays out of the CBL view's
// initial bundle; the chunk loads on first open (G6).
const BulkArchiveEditDialog = dynamic(
  () =>
    import("@/components/library/BulkArchiveEditDialog").then(
      (m) => m.BulkArchiveEditDialog,
    ),
  { ssr: false },
);

// "Download for offline" dialog (WP-4.6) — lazy, like the archive dialog.
const OfflineDownloadDialog = dynamic(
  () =>
    import("@/components/offline/OfflineDownloadDialog").then(
      (m) => m.OfflineDownloadDialog,
    ),
  { ssr: false },
);

/** Read-first detail page for a CBL saved view. Mirrors filter views:
 *  the page is a consumption surface (issues in CBL position order),
 *  with Edit / Pin/Unpin / Refresh / Export in the header. The full
 *  management UI (Reading order / Resolution / History / Settings
 *  tabs) lives behind the Edit button via a wide dialog. */
export function CblViewDetail({ savedView }: { savedView: SavedViewView }) {
  const listId = savedView.cbl_list_id;
  if (!listId) {
    return (
      <div className="text-destructive rounded-md border p-4 text-sm">
        Saved view is marked as CBL but has no `cbl_list_id`.
      </div>
    );
  }
  return <CblViewDetailInner savedView={savedView} listId={listId} />;
}

function CblViewDetailInner({
  savedView,
  listId,
}: {
  savedView: SavedViewView;
  listId: string;
}) {
  const detail = useCblList(listId);
  const createBatch = useCreateSavedViewBatch();
  const [offlineOpen, setOfflineOpen] = React.useState(false);
  const [offlineMounted, setOfflineMounted] = React.useState(false);
  if (offlineOpen && !offlineMounted) setOfflineMounted(true);
  const fetchViewMetadata = () => {
    createBatch.mutate({ saved_view_id: savedView.id });
  };
  // Server-filter when `hideMissing` is on: only fetch entries we'll
  // render. Otherwise pull everything in position order. Entries
  // arrive with their `IssueSummaryView` already attached, so no
  // separate hydration round-trip is needed (the old
  // `useCblListIssues({ limit: 1000 })` is gone).
  const [hideMissing, setHideMissing] = useCblHideMissing(listId);
  const [q, setQ] = React.useState("");
  const [debouncedQ, setDebouncedQ] = React.useState("");
  React.useEffect(() => {
    const t = setTimeout(() => setDebouncedQ(q.trim()), 200);
    return () => clearTimeout(t);
  }, [q]);
  // Linked reading lists: events, next part and chapters. The entries
  // query waits for them so it can start at the reader's chapter.
  const linksQuery = useCblListLinks(listId);
  // Plain list: the flat cover grid, as if the list had no chapters or
  // events (the reader's choice, per list).
  const [plainView, setPlainView] = usePlainListView(listId);
  const allChapters = linksQuery.data?.chapters;
  const allEvents = linksQuery.data?.events;
  const chapters = React.useMemo(
    () => (plainView ? [] : (allChapters ?? [])),
    [plainView, allChapters],
  );
  const linkEvents = React.useMemo(
    () => (plainView ? [] : (allEvents ?? [])),
    [plainView, allEvents],
  );
  // Whether the list has structure to show or hide at all.
  const hasStructure =
    (allChapters?.length ?? 0) > 1 || (allEvents?.length ?? 0) > 0;
  const [showReadChapters, setShowReadChapters] = useShowReadChapters(listId);
  // Chapter headers and the rail pin under the topbar plus whichever
  // control row is pinned; this publishes that row's height.
  const pinnedRootRef = usePinnedControlsHeight<HTMLDivElement>();
  const [railHidden, setRailHidden] = useChapterRailHidden();
  const [foldOverrides, setFoldOverrides] = React.useState<
    Record<number, boolean>
  >({});
  // Which chapters were finished when the page opened (see foldedChapters).
  const [doneAtLoad, setDoneAtLoad] = React.useState<Set<number> | null>(null);
  if (doneAtLoad == null && linksQuery.data) {
    setDoneAtLoad(
      new Set((allChapters ?? []).filter(isChapterDone).map(chapterKey)),
    );
  }
  const foldingActive = debouncedQ.length === 0 && chapters.length > 1;
  const folded = React.useMemo(
    () =>
      foldingActive
        ? foldedChapters(
            chapters,
            foldOverrides,
            showReadChapters,
            doneAtLoad ?? undefined,
          )
        : new Set<number>(),
    [foldingActive, chapters, foldOverrides, showReadChapters, doneAtLoad],
  );
  const fromPosition = foldingActive ? seekStart(chapters, folded) : undefined;
  // Entries wait for the links (to start at the reader's chapter), but
  // not on a links request that is failing: the list still loads, flat.
  const linksSettled = !linksQuery.isPending || linksQuery.failureCount > 0;
  const entriesQuery = useCblListEntriesInfinite(
    linksSettled ? listId : "",
    {
      status: hideMissing ? "matched,ambiguous,manual" : undefined,
      q: debouncedQ || undefined,
      fromPosition,
    },
    { keepPrevious: true },
  );
  // Tiny piggy-back on the rail's window query so the detail page
  // highlights the same "Up next" anchor card as the home rail.
  // before=0 keeps the response cheap — we only need `current_index`
  // and the position of the entry it points at. `after=1` because the
  // server clamps `after` to [1, 40]; an `after=0` request would
  // round up anyway. The full window endpoint already filters by
  // library ACL + ignores unmatched entries, matching the
  // up-next-resolution rules.
  const upNextWindow = useCblListWindow(listId, { before: 0, after: 1 });
  const upNextPosition = (() => {
    const data = upNextWindow.data;
    if (!data || data.current_index == null) return null;
    return data.items[data.current_index]?.position ?? null;
  })();
  const refresh = useRefreshCblList(listId);
  const [editOpen, setEditOpen] = React.useState(false);
  // Re-anchors `ManualMatchPopover` (and any other descendant popover)
  // into the SheetContent subtree. Without this they portal to
  // document.body, where the Sheet's modal aria-hide makes the search
  // input render but reject focus/clicks.
  const [editPortalContainer, setEditPortalContainer] =
    React.useState<HTMLElement | null>(null);

  // Infinite-scroll sentinel — once it intersects the viewport,
  // fetch the next page. The shared hook hands back a callback ref, so
  // the observer attaches when the sentinel mounts: it renders only
  // after the list detail below has loaded, and the entries query can
  // settle first. With a `useRef` + query-state deps the observer was
  // never attached in that order and the view stopped at 100 entries.
  const sentinelRef = useInfiniteSentinel(entriesQuery);
  // The Up Next jump below walks pages with the same fields.
  const {
    hasNextPage: entriesHasNext,
    isFetchingNextPage: entriesFetchingNext,
    fetchNextPage: entriesFetchNext,
  } = entriesQuery;
  // The first page of the current query (a new start position, search or
  // filter) is still loading — no page of it is on screen yet.
  const entriesFirstLoading =
    entriesQuery.isPending ||
    (entriesQuery.isFetching &&
      !entriesQuery.isFetchingNextPage &&
      !entriesQuery.isFetchingPreviousPage &&
      entriesQuery.isPlaceholderData);

  // Hooks must run unconditionally — keep them above the early
  // returns and use empty arrays for selection-relevant state while
  // the detail/entries queries are still loading. Below the early
  // returns we only do plain JSX / derived values.
  //
  // Memoised so the downstream useMemo `selectedIssueIds` (deps include
  // `loadedEntries`) doesn't recompute on every render — flatMap creates
  // a new array each call.
  const loadedEntries = React.useMemo<CblEntryHydratedView[]>(
    () => entriesQuery.data?.pages.flatMap((p) => p.items) ?? [],
    [entriesQuery.data?.pages],
  );

  // Multi-select on the CBL detail page (M6 extension per user
  // request). Mark-read/unread only — Add-to-collection / Remove
  // are not surfaced here. Selectable entries are matched entries
  // with a resolved issue; placeholder / missing entries can be
  // selected but contribute no targets to the mutation.
  // Up Next jump machinery — see `scrollToUpNext` below for the why.
  // Hoisted above the early returns with the rest of the hooks
  // (rules-of-hooks).
  const [jumpTarget, setJumpTarget] = React.useState<number | null>(null);
  React.useEffect(() => {
    if (jumpTarget == null) return;
    // Search still active (or its 200ms debounce hasn't flushed the
    // cleared value yet): the loaded pages belong to the filtered
    // query key. Wait — the debounce effect re-runs this one.
    if (debouncedQ.length > 0) return;
    const loaded = loadedEntries;
    if (loaded.some((e) => e.position === jumpTarget)) {
      // Two rAFs: one for state→DOM commit, one for the layout that
      // follows — then look up the (now rendered) card. The state
      // clear rides the same callback so it isn't a synchronous
      // setState inside the effect body.
      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          document
            .getElementById(`cbl-entry-${jumpTarget}`)
            ?.scrollIntoView({ behavior: scrollBehavior(), block: "center" });
          setJumpTarget(null);
        }),
      );
      return;
    }
    const lastLoaded =
      loaded.length > 0 ? loaded[loaded.length - 1]!.position : -1;
    if (lastLoaded >= jumpTarget || (!entriesHasNext && !entriesFetchingNext)) {
      // Walked past the target position without finding it (entry
      // filtered out by hide-missing) or the list is exhausted —
      // nothing to scroll to; disarm instead of spinning forever.
      requestAnimationFrame(() => setJumpTarget(null));
      return;
    }
    if (entriesHasNext && !entriesFetchingNext) {
      void entriesFetchNext();
    }
  }, [
    jumpTarget,
    loadedEntries,
    debouncedQ,
    entriesHasNext,
    entriesFetchingNext,
    entriesFetchNext,
  ]);

  const selection = useSelection(loadedEntries);
  const bulkMark = useBulkMarkProgress();
  const selectButtonRef = React.useRef<HTMLButtonElement | null>(null);
  const wasSelectModeRef = React.useRef(false);
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
  React.useEffect(() => {
    if (wasSelectModeRef.current && !selection.selectMode) {
      selectButtonRef.current?.focus();
    }
    wasSelectModeRef.current = selection.selectMode;
  }, [selection.selectMode]);
  // Resolve selected entry.id → issue.id, dropping unmatched ones
  // (placeholder cards in CBLs reference a series + issue number
  // but have no resolved issue row).
  const selectedIssueIds = React.useMemo(() => {
    const out: string[] = [];
    for (const entry of loadedEntries) {
      if (!selection.isSelected(entry.id)) continue;
      if (entry.issue) out.push(entry.issue.id);
    }
    return out;
  }, [loadedEntries, selection]);
  const skippedSelectedCount = Math.max(
    0,
    selection.count - selectedIssueIds.length,
  );

  // Hooks must run unconditionally — declared above the early
  // returns so the loading / error branches don't break the
  // rules-of-hooks invariant.
  const [markReadOpen, setMarkReadOpen] = React.useState(false);
  const [archiveEditOpen, setArchiveEditOpen] = React.useState(false);
  // Mount the lazy archive-edit dialog on first open; keep it mounted so its
  // close animation still runs (G6).
  const [archiveEditMounted, setArchiveEditMounted] = React.useState(false);
  if (archiveEditOpen && !archiveEditMounted) setArchiveEditMounted(true);
  const me = useMe().data;
  const isAdmin = me?.role === "admin";
  const createLink = useCreateCblLink(listId);
  const deleteLink = useDeleteCblLink(listId);
  const skipLink = useSkipCblLink(listId);
  const markThrough = useMarkCblReadThrough(listId);
  const [chaptersOpen, setChaptersOpen] = React.useState(false);
  const [deriveOpen, setDeriveOpen] = React.useState(false);
  const [collectionOpen, setCollectionOpen] = React.useState(false);
  const [insertTarget, setInsertTarget] = React.useState<{
    entryId: string | null;
    label: string | null;
  } | null>(null);
  const [markTarget, setMarkTarget] = React.useState<{
    position: number;
    label: string;
  } | null>(null);
  const [insertError, setInsertError] = React.useState<string | null>(null);
  const [removeTarget, setRemoveTarget] =
    React.useState<ReadingListLinkView | null>(null);
  // Move an event to after another issue (also "Place…" for a lost one).
  const [moveTarget, setMoveTarget] =
    React.useState<ReadingListLinkView | null>(null);
  const moveLink = useUpdateCblLink(listId);
  const reorderLinks = useReorderCblLinks(listId);
  // Chapter breaks (PR 3a): naming a new one / renaming, and removing.
  const createBreak = useCreateCblBreak(listId);
  const updateBreak = useUpdateCblBreak(listId);
  const deleteBreak = useDeleteCblBreak(listId);
  const [nameTarget, setNameTarget] = React.useState<
    | { mode: "create"; entryId: string; startsAt: string; label: string }
    | { mode: "rename"; breakId: string; startsAt: string; label: string }
    | null
  >(null);
  const [removeBreak, setRemoveBreak] = React.useState<{
    breakId: string;
    label: string;
  } | null>(null);
  // The rail asked for a chapter; scroll once its header is in the DOM.
  const [pendingChapter, setPendingChapter] = React.useState<number | null>(
    null,
  );
  // `?event=<link id>` (an event page's "Part of" link): open at that
  // event's band, its chapter unfolded. Once, when the links are known.
  const searchParams = useSearchParams();
  const eventParam = searchParams.get("event");
  const [pendingEvent, setPendingEvent] = React.useState<string | null>(null);
  const [eventHandled, setEventHandled] = React.useState(false);
  if (!eventHandled && eventParam != null && chapters.length > 0) {
    setEventHandled(true);
    const i = chapters.findIndex((c) => c.event_link_ids.includes(eventParam));
    if (i >= 0) {
      setFoldOverrides((o) => ({ ...o, [chapterKey(chapters[i]!)]: false }));
      setPendingEvent(eventParam);
      setPendingChapter(i);
    }
  }
  // Drop `?event=` once it has been acted on, so a reload or Back to this
  // URL restores the scroll position instead of jumping again.
  React.useEffect(() => {
    if (!eventHandled || eventParam == null) return;
    const url = new URL(window.location.href);
    url.searchParams.delete("event");
    window.history.replaceState(
      window.history.state,
      "",
      url.pathname + url.search + url.hash,
    );
  }, [eventHandled, eventParam]);
  React.useEffect(() => {
    if (pendingChapter == null) return;
    // A rail jump that unfolds a chapter moves the entries query's start;
    // wait for the new first page so the scroll isn't lost to the reload.
    if (entriesFirstLoading) return;
    const eventSel = pendingEvent
      ? `[data-event-link="${CSS.escape(pendingEvent)}"]`
      : null;
    const el = eventSel
      ? document.querySelector(eventSel)
      : document.getElementById(chapterAnchorId(pendingChapter));
    if (!el) {
      // A search is being cleared (its debounce hasn't flushed): the
      // loaded pages are still the filtered ones. Wait for the real list.
      if (q.trim() !== debouncedQ) return;
      // Not loaded yet: walk the pages until its header renders.
      if (entriesHasNext) {
        if (!entriesFetchingNext) void entriesFetchNext();
        return;
      }
      if (entriesFetchingNext) return;
      // Every page is in and it still isn't there (filtered out): give up
      // rather than hold the jump, and the rail's highlight, forever.
      const giveUp = requestAnimationFrame(() => {
        setPendingChapter(null);
        setPendingEvent(null);
      });
      return () => cancelAnimationFrame(giveUp);
    }
    const raf = requestAnimationFrame(() => {
      // Scroll the chapter's section, not its header: the header is
      // sticky, so once pinned its box no longer says where the chapter
      // starts. Looked up again here: a fold change can swap elements.
      const target =
        (eventSel && document.querySelector(eventSel)) ||
        document.querySelector(`[data-chapter-index="${pendingChapter}"]`) ||
        document.getElementById(chapterAnchorId(pendingChapter));
      if (!target) return;
      target.scrollIntoView({ behavior: scrollBehavior(), block: "start" });
      setPendingChapter(null);
      setPendingEvent(null);
    });
    return () => cancelAnimationFrame(raf);
    // Re-checks after each page lands (fetching flips) and after a fold
    // override renders the header (pendingChapter set in the same batch).
  }, [
    pendingChapter,
    pendingEvent,
    entriesFirstLoading,
    entriesHasNext,
    entriesFetchingNext,
    entriesFetchNext,
    q,
    debouncedQ,
  ]);
  // `]` / `[`: next / previous chapter (PR 3a; listed in the `?` sheet
  // from `LIST_SHORTCUTS`). Positions come from the rendered chapter
  // sections, measured against the pinned chapter header's line.
  const chapterCount = chapters.length;
  React.useEffect(() => {
    if (chapterCount < 2) return;
    const onKey = (e: KeyboardEvent) => {
      if (shouldSkipHotkey(e) || e.metaKey || e.ctrlKey || e.altKey) return;
      const action = listShortcutFor(e.key);
      if (!action) return;
      if (document.querySelector('[role="dialog"][data-state="open"]')) return;
      const target = chapterStep(
        chapterTops(chapterCount),
        action === "nextChapter" ? 1 : -1,
      );
      if (target == null) return;
      e.preventDefault();
      // No unfolding: a finished chapter stays folded, and its header and
      // events are where the shortcut stops. A search hides chapters with
      // no matches, so it is cleared first (as the Up Next jump does).
      setQ("");
      setPendingChapter(target);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [chapterCount]);
  // Fetch metadata + Rescan (admin) over the matched selected entries.
  const adminBulk = useAdminSelectionActions(
    { issueIds: selectedIssueIds, seriesIds: [] },
    { scope: "issues", onDone: () => selection.exit() },
  );
  // Add to Want to Read — every user (their own list).
  const wantToRead = useWantToReadSelectionAction(
    selectedIssueIds.map((id) => ({
      entry_kind: "issue" as const,
      ref_id: id,
    })),
    {
      onDone: () => selection.exit(),
    },
  );

  if (detail.isLoading) {
    return (
      <LoadingRegion label="Loading view…" className="space-y-6">
        <PageHeaderSkeleton />
        <CoverGridSkeleton label="Loading entries…" />
      </LoadingRegion>
    );
  }
  if (detail.isError || !detail.data) {
    return (
      <RailError
        message="Couldn't load this view."
        onRetry={() => void detail.refetch()}
        className="rounded-md border p-4"
      />
    );
  }

  const list = detail.data;
  const filterTotal = entriesQuery.data?.pages[0]?.total ?? null;
  const missingCount = list.stats.missing;
  // Shared lists are refreshed by admins only (server enforces the same).
  const canRefresh =
    list.source_kind !== "upload" && (!savedView.is_system || isAdmin);
  const warnNoMatchedSelection = () => {
    toast.warning("No matched issues selected", {
      description: "Missing CBL entries cannot be marked read or unread.",
    });
  };
  const notifySkippedSelection = () => {
    if (skippedSelectedCount > 0) {
      toast.info(
        `${skippedSelectedCount} missing ${
          skippedSelectedCount === 1 ? "entry was" : "entries were"
        } skipped`,
      );
    }
  };
  const submitMarkRead = (backfill: boolean) => {
    if (selectedIssueIds.length === 0) {
      warnNoMatchedSelection();
      return;
    }
    bulkMark.mutate(
      { issue_ids: selectedIssueIds, finished: true, backfill },
      {
        onSuccess: () => {
          selection.clear();
          setMarkReadOpen(false);
          notifySkippedSelection();
        },
      },
    );
  };
  const runBulkMark = (finished: boolean) => {
    if (selectedIssueIds.length === 0) {
      warnNoMatchedSelection();
      return;
    }
    if (finished && selectedIssueIds.length >= BULK_BACKFILL_PROMPT_THRESHOLD) {
      setMarkReadOpen(true);
      return;
    }
    bulkMark.mutate(
      { issue_ids: selectedIssueIds, finished },
      {
        onSuccess: () => {
          selection.clear();
          notifySkippedSelection();
        },
      },
    );
  };

  // Jump the page to the Up Next anchor card.
  //
  // The entry list is an infinite query, so on a long CBL the anchor's
  // page may simply not be fetched yet — `getElementById` finds nothing
  // and the old implementation silently no-oped. Clicking arms
  // `jumpTarget` (hoisted with the hooks above), and its effect WALKS
  // pages (same auto-walk pattern as CollectionViewDetail's reorder
  // prerequisite) until the target position is loaded, then scrolls to
  // it. An active search is cleared first — the search is server-side,
  // so its query key holds a different (filtered) page set; the effect
  // waits for the debounced key to flip back to the unfiltered list
  // before walking.
  function scrollToUpNext() {
    if (upNextPosition == null) return;
    if (q.length > 0) setQ("");
    // A folded chapter isn't loaded; open the one holding Up Next first.
    const holder = chapters.find(
      (c) =>
        upNextPosition >= c.start_position && upNextPosition <= c.end_position,
    );
    if (holder && folded.has(holder.index)) {
      setFoldOverrides((o) => ({ ...o, [chapterKey(holder)]: false }));
    }
    setJumpTarget(upNextPosition);
  }

  // The server applies the page-local search (`q`) before pagination,
  // so loaded entries are already the filtered result set. Gap markers
  // are dropped while searching because the canonical CBL position
  // sequence no longer represents the visible rows.
  const filteredEntries = loadedEntries;

  // Build the render plan. When `hideMissing` is on, the server has
  // already filtered out missing entries; we still walk loaded
  // positions to insert a `gap` placeholder where the canonical CBL
  // index isn't contiguous. Position numbers on the visible cards
  // stay truthful regardless of how many were hidden.
  type RenderItem = PlanItem<CblEntryHydratedView>;
  const items: RenderItem[] = [];
  if (hideMissing && debouncedQ.length === 0) {
    let prevPos: number | null = null;
    for (const entry of filteredEntries) {
      if (prevPos !== null) {
        const gap = entry.position - prevPos - 1;
        if (gap > 0) {
          items.push({
            kind: "gap",
            key: `gap-${prevPos}-${entry.position}`,
            count: gap,
            position: entry.position,
          });
        }
      }
      items.push({ kind: "entry", entry, position: entry.position });
      prevPos = entry.position;
    }
  } else {
    for (const entry of filteredEntries) {
      items.push({ kind: "entry", entry, position: entry.position });
    }
  }

  // Linked reading lists: who may change links (the server enforces the
  // same), what is already linked, and the chapter blocks to draw.
  const canManageLinks =
    list.owner_user_id != null ? list.owner_user_id === me?.id : isAdmin;
  const nextPart = linksQuery.data?.continues ?? null;
  const linkedIds = new Set([
    ...linkEvents.map((e) => e.child_list_id),
    ...(nextPart ? [nextPart.child_list_id] : []),
  ]);
  const blocks = buildChapterBlocks(
    items,
    chapters,
    linkEvents,
    folded,
    foldingActive,
    !entriesQuery.hasNextPage,
  );
  const hasChapters = chapters.length > 1;
  const jumpToChapter = (index: number) => {
    const target = chapters[index];
    if (target && folded.has(index)) {
      setFoldOverrides((o) => ({ ...o, [chapterKey(target)]: false }));
    }
    // A search hides chapters with no matches; the jump needs the list.
    if (q.length > 0) setQ("");
    setPendingChapter(index);
  };
  // The rail's event rows jump to the event's band.
  const jumpToEvent = (linkId: string, chapterIndex: number) => {
    const target = chapters[chapterIndex];
    if (target && folded.has(chapterIndex)) {
      setFoldOverrides((o) => ({ ...o, [chapterKey(target)]: false }));
    }
    if (q.length > 0) setQ("");
    setPendingEvent(linkId);
    setPendingChapter(chapterIndex);
  };
  const entryLabel = (e: CblEntryHydratedView) =>
    `${e.series_name} #${e.issue_number}`;
  // Positions where a chapter already starts (the first issue, a break).
  const firstPosition = chapters[0]?.start_position ?? 0;
  const breakAt = new Set(
    (linksQuery.data?.breaks ?? [])
      .filter((b) => b.anchor.status === "placed")
      .map((b) => b.anchor.position),
  );
  const cardActions = (e: CblEntryHydratedView) => [
    {
      label: "Mark read up to here…",
      onSelect: () =>
        setMarkTarget({ position: e.position, label: entryLabel(e) }),
    },
    ...(canManageLinks
      ? [
          {
            label: "Link a reading list after this…",
            onSelect: () =>
              setInsertTarget({ entryId: e.id, label: entryLabel(e) }),
          },
        ]
      : []),
    ...(canManageLinks && e.position > firstPosition && !breakAt.has(e.position)
      ? [
          {
            label: "Start a chapter here…",
            onSelect: () =>
              setNameTarget({
                mode: "create",
                entryId: e.id,
                startsAt: entryLabel(e),
                label: "",
              }),
          },
        ]
      : []),
  ];
  const breakById = new Map(
    (linksQuery.data?.breaks ?? []).map((b) => [b.id, b]),
  );
  const chapterActions = (c: (typeof chapters)[number]) => {
    const b = c.break_id ? breakById.get(c.break_id) : undefined;
    if (!b || !canManageLinks) return undefined;
    const startsAt =
      b.anchor.series && b.anchor.number
        ? `${b.anchor.series} #${b.anchor.number}`
        : "this issue";
    return (
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={`Actions for chapter ${c.index + 1}`}
          >
            <MoreHorizontal className="size-4" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem
            onSelect={() =>
              setNameTarget({
                mode: "rename",
                breakId: b.id,
                startsAt,
                label: b.label ?? "",
              })
            }
          >
            Rename chapter…
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            variant="destructive"
            onSelect={() => setRemoveBreak({ breakId: b.id, label: c.label })}
          >
            Remove chapter break
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    );
  };
  const renderCard = (entry: CblEntryHydratedView) => (
    <CblIssueCard
      entry={entry}
      issue={entry.issue ?? undefined}
      cblSavedViewId={savedView.id}
      isCurrent={upNextPosition != null && entry.position === upNextPosition}
      selectMode={
        selection.selectMode
          ? {
              isActive: true,
              isSelected: selection.isSelected(entry.id),
              onToggle: (ev) => selection.toggle(entry.id, ev),
            }
          : undefined
      }
      onEnterSelectMode={() => selection.toggle(entry.id)}
      extraActions={cardActions(entry)}
    />
  );
  // "Read earlier / later here": only among several events after one
  // issue, and not past either end.
  const reorderActions = (link: ReadingListLinkView) => {
    const siblings = anchorSiblings(linkEvents, link);
    const i = siblings.findIndex((s) => s.id === link.id);
    if (i < 0) return {};
    const move = (dir: -1 | 1) => () => {
      const writes = reorderWrites(siblings, link, dir);
      if (writes.length > 0) reorderLinks.mutate({ writes });
    };
    return {
      onMoveEarlier: i > 0 ? move(-1) : undefined,
      onMoveLater: i < siblings.length - 1 ? move(1) : undefined,
    };
  };
  const renderEvent = (link: ReadingListLinkView) => (
    <EventBand
      key={link.id}
      link={link}
      parentListId={list.id}
      parentViewId={savedView.id}
      afterLabel={
        link.anchor?.series
          ? `${link.anchor.series} #${link.anchor.number ?? ""}`
          : null
      }
      canManage={canManageLinks}
      onSkip={(skip) => skipLink.mutate({ linkId: link.id, skip })}
      onMove={() => setMoveTarget(link)}
      {...reorderActions(link)}
      onRemove={() => setRemoveTarget(link)}
    />
  );

  // `⋯` menu body, shared by the phone bar's instance and the pinned
  // row's instance (one per breakpoint composition).
  const viewMenuItems = (
    <>
      {/* Starts a metadata batch: admin-only (audit 2026-11 E16). */}
      {isAdmin && (
        <DropdownMenuItem
          onSelect={fetchViewMetadata}
          disabled={createBatch.isPending}
        >
          <Sparkles className="size-4" /> Fetch metadata for view
        </DropdownMenuItem>
      )}
      <DropdownMenuItem onSelect={() => setCollectionOpen(true)}>
        <FolderPlus className="size-4" /> Add to collection…
      </DropdownMenuItem>
      <DropdownMenuItem onSelect={() => setOfflineOpen(true)}>
        <HardDriveDownload className="size-4" />
        Download for offline…
      </DropdownMenuItem>
      <DropdownMenuItem asChild>
        <a
          href={`/api/me/cbl-lists/${list.id}/export`}
          download
          title="Download as .cbl"
        >
          <Download className="size-4" /> Export
        </a>
      </DropdownMenuItem>
      {(allEvents?.length ?? 0) > 0 && (
        <DropdownMenuItem asChild>
          <a
            href={`/api/me/cbl-lists/${list.id}/export?inline_events=true`}
            download
            title="Download as .cbl with each event's issues in place"
          >
            <Download className="size-4" /> Export with events
          </a>
        </DropdownMenuItem>
      )}
      {missingCount > 0 ? (
        <DropdownMenuItem
          onSelect={(e) => {
            e.preventDefault();
            setHideMissing(!hideMissing);
          }}
        >
          {hideMissing ? (
            <Check className="size-4" />
          ) : (
            <EyeOff className="size-4" />
          )}
          {hideMissing
            ? `Showing matched only (${missingCount} hidden)`
            : `Hide ${missingCount} missing`}
        </DropdownMenuItem>
      ) : null}
      {hasChapters ? (
        <DropdownMenuItem
          onSelect={(e) => {
            e.preventDefault();
            setFoldOverrides({});
            setShowReadChapters(!showReadChapters);
          }}
        >
          {showReadChapters ? (
            <EyeOff className="size-4" />
          ) : (
            <Eye className="size-4" />
          )}
          {showReadChapters ? "Fold read chapters" : "Show read chapters"}
        </DropdownMenuItem>
      ) : null}
      {hasStructure ? (
        <DropdownMenuItem
          onSelect={() => {
            setFoldOverrides({});
            setPlainView(!plainView);
          }}
        >
          {plainView ? (
            <ListTree className="size-4" />
          ) : (
            <LayoutGrid className="size-4" />
          )}
          {plainView ? "Show chapters and events" : "Plain list"}
        </DropdownMenuItem>
      ) : null}
      {canManageLinks && list.catalog_source_id ? (
        <DropdownMenuItem onSelect={() => setDeriveOpen(true)}>
          <Waypoints className="size-4" /> Find events and parts…
        </DropdownMenuItem>
      ) : null}
      {canManageLinks && !nextPart ? (
        <DropdownMenuItem
          onSelect={() => setInsertTarget({ entryId: null, label: null })}
        >
          <Link2 className="size-4" /> Link the next part…
        </DropdownMenuItem>
      ) : null}
      {canRefresh ? (
        <DropdownMenuItem
          onSelect={(e) => {
            e.preventDefault();
            if (!refresh.isPending) refresh.mutate({});
          }}
          disabled={refresh.isPending}
        >
          <RefreshCw
            className={`mr-2 size-4 ${refresh.isPending ? "animate-spin" : ""}`}
          />
          Refresh
        </DropdownMenuItem>
      ) : null}
    </>
  );

  return (
    <div ref={pinnedRootRef} className="space-y-6">
      {offlineMounted && (
        <OfflineDownloadDialog
          open={offlineOpen}
          onOpenChange={setOfflineOpen}
          target={{
            kind: "list",
            list: {
              id: savedView.id,
              kind: "cbl",
              name: savedView.name,
              sourceId: list.id,
            },
            remaining:
              list.stats != null
                ? Math.max(0, list.stats.matched - list.stats.read_count)
                : null,
          }}
        />
      )}
      <ViewHeader
        view={savedView}
        onEdit={() => setEditOpen(true)}
        titleSuffix={renderYearRangeBadge(
          savedView.custom_year_start,
          savedView.custom_year_end,
        )}
        // The title row is title-only: the stats pills and the `⋯` menu
        // ride in the pinned control row (phone bar / StickyToolbarRow)
        // with the tools, at every width, so the list's header never
        // splits its controls across two lines.
        actions="none"
        phoneControls={
          <PhoneControlsBar
            hidden={selection.selectMode}
            leading={<CblStatsPills cblListId={list.id} size="rail" />}
            search={{
              value: q,
              onChange: setQ,
              placeholder: "Search this list…",
              label: "Search entries in this list",
            }}
            primary={
              upNextPosition != null ? (
                <Button
                  type="button"
                  variant="outline"
                  size="icon"
                  onClick={scrollToUpNext}
                  className="h-9 w-9 shrink-0"
                  disabled={jumpTarget != null}
                  aria-label="Scroll to Up Next entry"
                  title="Up Next"
                >
                  {jumpTarget != null ? (
                    <Loader2
                      className="size-4 animate-spin"
                      aria-hidden="true"
                    />
                  ) : (
                    <BookOpen className="size-4" aria-hidden="true" />
                  )}
                </Button>
              ) : null
            }
            more={
              <ViewActionsMenu
                view={savedView}
                onEdit={() => setEditOpen(true)}
                extraMenuItems={viewMenuItems}
              />
            }
            menu={(close) => (
              <>
                <CoverSizeFields fieldId="phone-card-size" />
                {loadedEntries.length > 0 || selection.selectMode ? (
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
                {hasChapters ? (
                  <PhoneMenuAction
                    icon={<ListTree className="size-4" />}
                    onClick={() => {
                      close();
                      setChaptersOpen(true);
                    }}
                  >
                    Chapters
                  </PhoneMenuAction>
                ) : null}
              </>
            )}
          />
        }
      />
      {/* `sm` and up: one control row under the title — stats pills and search on the left;
       *  Up Next, Select, cover size and `⋯` on the right (rule 11) —
       *  pinned under the topbar while it fits one line (phone-layout
       *  rule 3). The search flexes between 10rem and 14rem so the row
       *  still fits an iPad-landscape column with the sidebar open. */}
      <StickyToolbarRow enabled={!selection.selectMode}>
        {/* `size="header"` matches the h-9 controls beside it. */}
        <CblStatsPills cblListId={list.id} size="header" />
        {/* Page-local search input. Folded into the header's
         *  actions row alongside cover size + select so the
         *  toolbar reads as one cluster. Search is server-side,
         *  preserving pagination + totals for long CBLs. */}
        <div className="relative max-w-56 min-w-40 flex-1 basis-40">
          <SearchIcon
            aria-hidden="true"
            className="text-muted-foreground pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2"
          />
          <Input
            type="search"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder="Search this list…"
            aria-label="Search entries in this list"
            className="h-9 pl-8"
          />
        </div>
        {/* Right cluster, rule 11 order: primary · Select · View options · ⋯. */}
        <div className="ml-auto flex items-center gap-2">
          {upNextPosition != null && (
            <Button
              type="button"
              variant="outline"

              onClick={scrollToUpNext}
              disabled={jumpTarget != null}
              aria-label="Scroll to Up Next entry"
              title="Scroll to Up Next entry"
            >
              {jumpTarget != null ? (
                <Loader2
                  className="mr-1.5 size-4 animate-spin"
                  aria-hidden="true"
                />
              ) : (
                <BookOpen className="mr-1.5 size-4" aria-hidden="true" />
              )}
              Up Next
            </Button>
          )}
          {(loadedEntries.length > 0 || selection.selectMode) && (
            <SelectModeButton
              ref={selectButtonRef}
              active={selection.selectMode}
              onEnter={() => selection.enter()}
              onExit={() => selection.exit()}
            />
          )}
          {hasChapters && (
            <ChapterRailButton
              onClick={() => setChaptersOpen(true)}
              railHidden={railHidden}
            />
          )}
          <CoverSizeOptions />
          <ViewActionsMenu
            view={savedView}
            onEdit={() => setEditOpen(true)}
            extraMenuItems={viewMenuItems}
          />
        </div>
      </StickyToolbarRow>
      {/* Catalog/source path + import date — useful context on the
       *  desktop detail page, but redundant on mobile where the title
       *  already identifies the list. Hide to save vertical real
       *  estate; full info still lives on the CBL management page
       *  (`<CblDetail>`) which renders the same row at all widths. */}
      {/* Year range moved up next to the title via `titleSuffix`; this
       *  row keeps source + matchers + imported date. */}
      <div className="hidden md:block">
        <CblInfoRow list={list} />
      </div>
      {/* Linked into other lists: the way back (PR 4). */}
      <PartOfLink
        listId={list.id}
        parents={linksQuery.data?.parents ?? []}
        viaViewId={searchParams.get("via")}
      />
      {/* A failing refresh shows at every width: it is the one thing
       *  here the reader cannot learn elsewhere on the page. */}
      <RefreshFailureNotice
        list={list}
        onRefresh={
          canRefresh
            ? () => {
                if (!refresh.isPending) refresh.mutate({});
              }
            : undefined
        }
        refreshing={refresh.isPending}
      />

      <SelectionToolbar
        open={selection.selectMode}
        count={selection.count}
        total={loadedEntries.length}
        primary={[
          {
            id: "mark-read",
            label: "Mark read",
            icon: Check,
            onClick: () => runBulkMark(true),
            disabled: selectedIssueIds.length === 0,
          },
          {
            id: "mark-unread",
            label: "Mark unread",
            icon: Circle,
            onClick: () => runBulkMark(false),
            disabled: selectedIssueIds.length === 0,
          },
        ]}
        overflow={[wantToRead.action]}
        // Admin-only (audit 2026-11 E16): the archive edit rewrites files,
        // the fetch starts a metadata batch, the rescan queues scans.
        actionGroups={
          isAdmin
            ? [
                {
                  id: "editing",
                  label: "Editing",
                  icon: Pencil,
                  actions: [
                    {
                      id: "edit-archives",
                      label: "Edit archives…",
                      icon: FileCog,
                      onClick: () => setArchiveEditOpen(true),
                    },
                    ...adminBulk.actions,
                  ],
                },
              ]
            : []
        }
        onDone={() => selection.exit()}
        onClear={() => selection.clear()}
        onSelectAll={() => selection.selectAll()}
        isPending={
          bulkMark.isPending || adminBulk.isPending || wantToRead.isPending
        }
      />
      {adminBulk.dialogs}

      {!plainView && linksQuery.data ? (
        <LostAnchorsNotice
          events={linksQuery.data.events}
          breaks={linksQuery.data.breaks}
          canManage={canManageLinks}
          onPlace={setMoveTarget}
          onRemoveBreak={(b) =>
            setRemoveBreak({ breakId: b.id, label: b.label ?? "this chapter" })
          }
        />
      ) : null}
      {linksQuery.isError ? (
        <RailError
          message="Couldn't load this list's chapters and events. The issues are shown as a plain list."
          onRetry={() => void linksQuery.refetch()}
        />
      ) : null}
      {entriesQuery.isLoading || !linksSettled ? (
        <CoverGridSkeleton label="Loading entries…" />
      ) : list.stats.total === 0 ? (
        <div className="border-border/60 text-muted-foreground rounded-lg border border-dashed p-8 text-center text-sm">
          This list has no entries yet.
        </div>
      ) : items.length === 0 ? (
        <div className="border-border/60 text-muted-foreground rounded-lg border border-dashed p-8 text-center text-sm">
          {debouncedQ
            ? `No entries match "${debouncedQ}".`
            : hideMissing
              ? 'Every entry in this list is currently missing from your library. Toggle "Hide missing" off to see them.'
              : "No entries to show."}
        </div>
      ) : (
        <div className="flex items-start gap-6">
          <div className="min-w-0 flex-1 space-y-4">
            {blocks.map((block) => (
              <section
                key={block.chapter ? `chapter-${block.chapter.index}` : "all"}
                data-chapter-index={block.chapter?.index}
                aria-label={
                  block.chapter
                    ? `Chapter ${block.chapter.index + 1}: ${block.chapter.label}`
                    : undefined
                }
                className="scroll-mt-[calc(var(--topbar-h)+var(--pinned-controls-h,0px)+0.25rem)] space-y-4"
              >
                {block.chapter && hasChapters && (
                  <ChapterHeader
                    chapter={block.chapter}
                    actions={chapterActions(block.chapter)}
                    folded={foldingActive ? block.folded : undefined}
                    onToggle={
                      foldingActive
                        ? () => {
                            const key = chapterKey(block.chapter!);
                            setFoldOverrides((o) => ({
                              ...o,
                              [key]: !block.folded,
                            }));
                          }
                        : undefined
                    }
                  />
                )}
                {block.items.length > 0 && (
                  <ul
                    role="list"
                    className="grid gap-4"
                    style={COVER_GRID_STYLE}
                  >
                    {block.items.map((item) =>
                      item.kind === "entry" ? (
                        <li
                          key={item.entry.id}
                          id={`cbl-entry-${item.entry.position}`}
                        >
                          {renderCard(item.entry)}
                        </li>
                      ) : (
                        <li
                          key={item.key}
                          className="grid place-items-center"
                          aria-label={`${item.count} missing ${item.count === 1 ? "entry" : "entries"} hidden`}
                          title={`${item.count} missing ${item.count === 1 ? "entry" : "entries"} hidden`}
                        >
                          <div className="border-border/60 text-muted-foreground/80 inline-flex flex-col items-center rounded-md border border-dashed px-2.5 py-1.5">
                            <span className="font-mono text-sm leading-none tracking-widest">
                              •••
                            </span>
                            <span className="mt-1 text-[10px] tracking-wider uppercase">
                              {item.count} missing
                            </span>
                          </div>
                        </li>
                      ),
                    )}
                  </ul>
                )}
                {block.events.length > 0 && (
                  <div className="space-y-3">
                    {block.events.map(renderEvent)}
                  </div>
                )}
              </section>
            ))}
            {nextPart && !entriesQuery.hasNextPage && (
              <div
                data-testid="next-part"
                className="border-border/60 flex min-h-11 items-center gap-2 rounded-md border border-dashed px-3 py-2 text-sm"
              >
                <span className="text-muted-foreground">Continues in</span>
                {nextPart.child_view_id ? (
                  <a
                    href={`/views/${nextPart.child_view_id}`}
                    className="font-medium hover:underline"
                  >
                    {nextPart.child_name}
                  </a>
                ) : (
                  <span className="font-medium">{nextPart.child_name}</span>
                )}
                <span className="text-muted-foreground ml-auto text-xs">
                  {nextPart.read} / {nextPart.total} read
                </span>
                {canManageLinks && (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => setRemoveTarget(nextPart)}
                    aria-label={`Unlink ${nextPart.child_name}`}
                  >
                    Unlink
                  </Button>
                )}
              </div>
            )}
            <div
              ref={sentinelRef}
              aria-hidden
              className={entriesQuery.hasNextPage ? "h-12" : "hidden"}
            />
            <LoadMoreError query={entriesQuery} />
            {entriesQuery.isFetchingNextPage ? (
              <p className="text-muted-foreground flex items-center justify-center gap-2 text-xs">
                <Loader2 className="size-3 animate-spin" /> Loading more (
                {loadedEntries.length}
                {filterTotal != null ? ` of ${filterTotal}` : ""})…
              </p>
            ) : null}
          </div>
          {hasChapters && !railHidden && (
            <ChapterRail
              chapters={chapters}
              events={linkEvents}
              next={nextPart}
              jumpTarget={pendingChapter}
              onJump={jumpToChapter}
              onJumpEvent={jumpToEvent}
              onHide={() => setRailHidden(true)}
            />
          )}
        </div>
      )}

      {hasChapters && (
        <ChapterRailSheet
          open={chaptersOpen}
          onOpenChange={setChaptersOpen}
          onShowRail={railHidden ? () => setRailHidden(false) : undefined}
          chapters={chapters}
          events={linkEvents}
          next={nextPart}
          jumpTarget={pendingChapter}
          onJump={jumpToChapter}
          onJumpEvent={jumpToEvent}
        />
      )}
      <AddToCollectionDialog
        open={collectionOpen}
        onOpenChange={setCollectionOpen}
        target={{
          entry_kind: "list",
          ref_id: list.id,
          label: list.parsed_name,
        }}
      />
      {deriveOpen && (
        <DeriveLinksDialog
          listId={list.id}
          open={deriveOpen}
          onOpenChange={setDeriveOpen}
        />
      )}
      <MoveEventDialog
        listId={list.id}
        link={moveTarget}
        onOpenChange={(o) => !o && setMoveTarget(null)}
        isPending={moveLink.isPending}
        onPick={(afterEntryId) => {
          const t = moveTarget;
          if (!t) return;
          // Stays open until the move settles: a refusal shows here.
          moveLink.mutate(
            { linkId: t.id, after_entry_id: afterEntryId },
            {
              onSuccess: (data) => {
                setMoveTarget(null);
                // Bring the band into view at its new place, read from
                // the layout the server just returned (unfolding the
                // chapter it landed in).
                const landed = data?.chapters.find((c) =>
                  c.event_link_ids.includes(t.id),
                );
                if (landed && !plainView) {
                  setFoldOverrides((o) => ({
                    ...o,
                    [chapterKey(landed)]: false,
                  }));
                  setPendingEvent(t.id);
                  setPendingChapter(landed.index);
                }
              },
            },
          );
        }}
      />
      <InsertListDialog
        key={`insert-${insertTarget ? (insertTarget.entryId ?? "next-part") : "closed"}`}
        open={insertTarget != null}
        onOpenChange={(o) => {
          if (o) return;
          setInsertTarget(null);
          setInsertError(null);
        }}
        parent={list}
        afterLabel={insertTarget?.label ?? null}
        alreadyLinked={linkedIds}
        hasNextPart={nextPart != null}
        isPending={createLink.isPending}
        error={insertError}
        onPick={(childId, kind) => {
          const t = insertTarget;
          setInsertError(null);
          // The dialog stays open until the link is made: a refusal (a
          // list that has events of its own, a loop of parts) shows in
          // it, where the reader is looking, with the picker still there.
          createLink.mutate(
            {
              child_list_id: childId,
              kind,
              after_entry_id: kind === "event" ? (t?.entryId ?? null) : null,
            },
            {
              onSuccess: () => setInsertTarget(null),
              onError: (err) =>
                setInsertError(
                  err instanceof ApiMutationError && err.fields[0]
                    ? sentence(err.fields[0].message)
                    : sentence(err.message),
                ),
            },
          );
        }}
      />
      <ChapterNameDialog
        key={`name-${
          nameTarget
            ? nameTarget.mode === "create"
              ? nameTarget.entryId
              : nameTarget.breakId
            : "closed"
        }`}
        target={nameTarget}
        onOpenChange={(o) => !o && setNameTarget(null)}
        isPending={createBreak.isPending || updateBreak.isPending}
        onSave={(label) => {
          const t = nameTarget;
          if (!t) return;
          const done = { onSuccess: () => setNameTarget(null) };
          if (t.mode === "create") {
            createBreak.mutate({ entry_id: t.entryId, label }, done);
          } else {
            updateBreak.mutate({ breakId: t.breakId, label }, done);
          }
        }}
      />
      <RemoveBreakDialog
        label={removeBreak?.label ?? null}
        onOpenChange={(o) => !o && setRemoveBreak(null)}
        onConfirm={() => {
          const t = removeBreak;
          setRemoveBreak(null);
          if (t) deleteBreak.mutate({ breakId: t.breakId });
        }}
      />
      <MarkThroughDialog
        listId={listId}
        target={markTarget}
        onOpenChange={(o) => !o && setMarkTarget(null)}
        isPending={markThrough.isPending}
        onConfirm={(position) =>
          markThrough.mutate(
            { position },
            { onSuccess: () => setMarkTarget(null) },
          )
        }
      />
      <RemoveLinkDialog
        name={removeTarget?.child_name ?? null}
        onOpenChange={(o) => !o && setRemoveTarget(null)}
        onConfirm={() => {
          const t = removeTarget;
          setRemoveTarget(null);
          if (t) deleteLink.mutate({ linkId: t.id });
        }}
      />

      <Sheet open={editOpen} onOpenChange={setEditOpen}>
        <SheetContent
          size="5xl"
          ref={setEditPortalContainer}
          side="right"
          // `size="5xl"`: the Reading-order tab hosts a 6-column
          // virtualized table that pairs long list titles with long
          // series titles. `p-0` so the header gets its own divider and
          // the body owns its padding. `overflow-visible` so the
          // manual-match popover (portaled into this content via
          // PopoverPortalContainer) can extend past the sheet's edge
          // when collision detection flips it outward; tab bodies own
          // their own scroll.
          className="flex w-full flex-col gap-0 overflow-visible p-0"
        >
          <SheetHeader className="border-border/60 border-b px-6 py-4 pr-12">
            <SheetTitle>Manage {savedView.name}</SheetTitle>
            <SheetDescription>
              Resolve missing or ambiguous matches, review refresh history,
              tweak metadata.
            </SheetDescription>
          </SheetHeader>
          <PopoverPortalContainer value={editPortalContainer}>
            <div className="flex min-h-0 flex-1 flex-col px-6 py-4">
              <CblDetail savedView={savedView} />
            </div>
          </PopoverPortalContainer>
        </SheetContent>
      </Sheet>
      <BulkMarkReadDialog
        open={markReadOpen}
        onOpenChange={setMarkReadOpen}
        count={selectedIssueIds.length}
        onConfirm={submitMarkRead}
        isPending={bulkMark.isPending}
      />
      {archiveEditMounted && (
        <BulkArchiveEditDialog
          open={archiveEditOpen}
          onOpenChange={(next) => {
            setArchiveEditOpen(next);
            if (!next) selection.clear();
          }}
          issueIds={selectedIssueIds}
          onResult={(res) => {
            setArchiveEditOpen(false);
            if (res.skipped.length > 0) {
              selection.replace(
                skippedEntryIds(
                  res.skipped,
                  loadedEntries.map((e) => ({
                    entryId: e.id,
                    issueId: e.issue?.id,
                  })),
                ),
              );
            } else {
              selection.clear();
            }
          }}
        />
      )}
    </div>
  );
}

/** Compact year-range string for the title-adjacent header slot.
 *  `2002–2026` when both bounds exist, single-sided (`from 2002`,
 *  `up to 2026`) when only one does, equal-year collapse, `null`
 *  when neither is set. */
function formatYearRange(
  start?: number | null,
  end?: number | null,
): string | null {
  if (start != null && end != null) {
    return start === end ? `${start}` : `${start}–${end}`;
  }
  if (start != null) return `from ${start}`;
  if (end != null) return `up to ${end}`;
  return null;
}

/** Year-range badge for `ViewHeader.titleSuffix`. Renders the formatted
 *  range as a tooltip-trigger so the user can hover for a quick
 *  description of what the range is and where it came from. Returns
 *  `null` when the saved view has no year bounds — keeps the header
 *  tight for the common no-overlay case. */
function renderYearRangeBadge(
  start?: number | null,
  end?: number | null,
): React.ReactNode {
  const label = formatYearRange(start, end);
  if (!label) return null;
  return (
    <TooltipProvider delayDuration={200}>
      <Tooltip>
        <TooltipTrigger asChild>
          <span
            className="cursor-help underline decoration-dotted decoration-1 underline-offset-4"
            aria-label={`Year range: ${label}`}
          >
            {label}
          </span>
        </TooltipTrigger>
        <TooltipContent side="bottom" className="max-w-xs">
          Year range covered by this list. Auto-filled from the earliest and
          latest entry at import; edit in Settings.
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}

/** "a list can’t link to itself" → "A list can’t link to itself." */
function sentence(message: string): string {
  const m = message.trim();
  if (!m) return m;
  const cap = m[0]!.toUpperCase() + m.slice(1);
  return /[.!?]$/.test(cap) ? cap : `${cap}.`;
}
