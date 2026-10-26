"use client";

import * as React from "react";
import Link from "next/link";
import { Suspense } from "react";
import { useSearchParams } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import {
  Check,
  Folder,
  FolderInput,
  FolderPlus,
  List,
  Lock,
  MoreVertical,
  PanelLeft,
  Pin,
  Plus,
  RefreshCw,
  Sparkles,
  Trash2,
  type LucideIcon,
} from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { PageHeader } from "@/components/admin/PageHeader";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/folio/dropdown-menu";
import { DropdownMenuResponsiveSub } from "@/components/folio/dropdown-menu-responsive-sub";
import { Progress } from "@/components/ui/progress";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/folio/tabs";
import { CblImportDialog } from "@/components/cbl/cbl-import-dialog";
import {
  CblUpdatesMenu,
  CblUpdatesPanel,
  CblUpdatesProvider,
  useCblUpdates,
} from "@/components/cbl/CblRefreshDashboard";
import { CblStatsPillsStatic } from "@/components/cbl/CblStatsPills";
import { NewFilterViewDialog } from "@/components/saved-views/AddViewButton";
import { NewCollectionDialog } from "@/components/collections/CollectionsIndex";
import {
  GroupedViewsBoard,
  useViewBoardActions,
} from "@/components/saved-views/GroupedViewsBoard";
import { MultiPinDialog } from "@/components/saved-views/MultiPinDialog";
import { QuickApplyPrefill } from "@/components/saved-views/QuickApplyPrefill";
import {
  useDeleteCollection,
  useDeleteSavedView,
  useSidebarSavedView,
} from "@/lib/api/mutations";
import { toast } from "sonner";

import {
  fetchCollectionSnapshot,
  type CollectionSnapshot,
} from "@/lib/collections/recreate";
import { useCollectionDeleteUndo } from "@/lib/collections/use-collection-undo";
import {
  queryKeys,
  useCblLists,
  useCollections,
  useSavedViews,
} from "@/lib/api/queries";
import type { CblListView, SavedViewView } from "@/lib/api/types";
import { BatchRefreshProvider } from "@/lib/cbl/use-batch-refresh";
import { formatRelativeDate } from "@/lib/format";
import { UNGROUPED_ID, type BoardGroup } from "@/lib/saved-views/view-board";

const WANT_TO_READ_KEY = "want_to_read";

const TAB_KEYS = ["filter-views", "reading-lists", "collections"] as const;
type TabKey = (typeof TAB_KEYS)[number];

function parseTab(raw: string | null): TabKey {
  return (TAB_KEYS as readonly string[]).includes(raw ?? "")
    ? (raw as TabKey)
    : "filter-views";
}

/** Split the single `useSavedViews()` list into the two non-collection
 *  index buckets. `system` rails (continue_reading / on_deck) and
 *  `collection`s are excluded from both — system rails must never leak
 *  into Filter views, and collections render from their own
 *  `useCollections()` source (the Collections tab). */
export function splitSavedViews(items: SavedViewView[]): {
  filterViews: SavedViewView[];
  readingLists: SavedViewView[];
} {
  return {
    filterViews: items.filter(
      (v) => v.kind === "filter_series" || v.kind === "filter_issues",
    ),
    readingLists: items.filter((v) => v.kind === "cbl"),
  };
}

/** CBL list rows keyed by list id, so each reading-list card can show
 *  stats without its own detail query. `null` until loaded. */
const CblListsContext = React.createContext<Map<string, CblListView> | null>(
  null,
);

/**
 * Unified saved-content index — the one management home for Views (audit
 * A3). Lives at `/settings/views`. Three tabs — Filter views, Reading
 * lists (CBL), Collections — each rendered as a grouped board
 * (`GroupedViewsBoard`): the user drags cards into named, reorderable
 * groups that persist per tab, and every card carries its own
 * arrangement controls (pin-to-pages + show-in-sidebar) so browse,
 * lifecycle, organization, and arrangement live in one place.
 */
export function ViewsIndex() {
  return (
    <div className="space-y-6">
      <PageHeader
        title="Views"
        description="Filter views update themselves from rules · Reading lists track an imported CBL · Collections are hand-picked. Drag cards into groups to organize them; pin any of them to a page or the sidebar from its ⋯ menu."
      />
      <Suspense
        fallback={
          <div className="text-muted-foreground py-6 text-sm">Loading…</div>
        }
      >
        <ViewsTabs />
      </Suspense>
      {/* Quick-apply: `?quick_field=&quick_value=` opens the New-filter-view
          dialog pre-filled (chip-list deep links). useSearchParams needs a
          Suspense boundary. */}
      <Suspense fallback={null}>
        <QuickApplyPrefill />
      </Suspense>
    </div>
  );
}

function ViewsTabs() {
  // Initial tab from the URL (`?type=`); switching is local state mirrored
  // back to the URL via replaceState so deep-links work without a redirect
  // or an RSC refetch on every tab change.
  const initialTab = parseTab(useSearchParams().get("type"));
  const [tab, setTab] = React.useState<TabKey>(initialTab);
  function onTabChange(next: string) {
    const t = parseTab(next);
    setTab(t);
    if (typeof window !== "undefined") {
      const url = new URL(window.location.href);
      url.searchParams.set("type", t);
      window.history.replaceState(null, "", url);
    }
  }

  const viewsQ = useSavedViews();
  const collectionsQ = useCollections();
  const cblListsQ = useCblLists();
  const { filterViews, readingLists } = React.useMemo(
    () => splitSavedViews(viewsQ.data?.items ?? []),
    [viewsQ.data],
  );
  const collections = React.useMemo(
    () => collectionsQ.data ?? [],
    [collectionsQ.data],
  );
  const cblListsById = React.useMemo(
    () =>
      cblListsQ.data
        ? new Map(cblListsQ.data.items.map((l) => [l.id, l]))
        : null,
    [cblListsQ.data],
  );

  const [newFilterOpen, setNewFilterOpen] = React.useState(false);
  const [importOpen, setImportOpen] = React.useState(false);
  const [newCollectionOpen, setNewCollectionOpen] = React.useState(false);

  const renderFilterCard = React.useCallback(
    (view: SavedViewView) => (
      <ViewCard view={view} Icon={Sparkles} isCollection={false} />
    ),
    [],
  );
  const renderCblCard = React.useCallback(
    (view: SavedViewView) => (
      <ViewCard view={view} Icon={List} isCollection={false} />
    ),
    [],
  );
  const renderCollectionCard = React.useCallback(
    (view: SavedViewView) => (
      <ViewCard view={view} Icon={Folder} isCollection />
    ),
    [],
  );

  return (
    <Tabs value={tab} onValueChange={onTabChange} className="space-y-4">
      <TabsList>
        <TabsTrigger value="filter-views">
          Filter views <TabCount n={filterViews.length} />
        </TabsTrigger>
        <TabsTrigger value="reading-lists">
          Reading lists <TabCount n={readingLists.length} />
        </TabsTrigger>
        <TabsTrigger value="collections">
          Collections <TabCount n={collections.length} />
        </TabsTrigger>
      </TabsList>

      <TabsContent value="filter-views">
        <GroupedViewsBoard
          scope="filter"
          views={filterViews}
          loading={viewsQ.isLoading}
          blurb="Auto-update from a set of filters."
          noun="view"
          icon={Sparkles}
          action={
            <Button type="button" onClick={() => setNewFilterOpen(true)}>
              <Plus className="mr-1 size-4" /> New filter view
            </Button>
          }
          renderCard={renderFilterCard}
          empty="No filter views yet — create one to save a set of filters."
        />
      </TabsContent>

      <TabsContent value="reading-lists">
        <CblListsContext.Provider value={cblListsById}>
          <BatchRefreshProvider>
            <ReadingListsTab
              views={readingLists}
              loading={viewsQ.isLoading}
              onImport={() => setImportOpen(true)}
              renderCard={renderCblCard}
            />
          </BatchRefreshProvider>
        </CblListsContext.Provider>
      </TabsContent>

      <TabsContent value="collections">
        <GroupedViewsBoard
          scope="collection"
          views={collections}
          loading={collectionsQ.isLoading}
          blurb="Hand-picked series and issues."
          noun="collection"
          icon={Folder}
          action={
            <Button type="button" onClick={() => setNewCollectionOpen(true)}>
              <Plus className="mr-1 size-4" /> New collection
            </Button>
          }
          renderCard={renderCollectionCard}
          empty="No collections yet — group series and issues into a manual list."
        />
      </TabsContent>

      <NewFilterViewDialog
        open={newFilterOpen}
        onOpenChange={setNewFilterOpen}
      />
      <CblImportDialog open={importOpen} onOpenChange={setImportOpen} />
      <NewCollectionDialog
        open={newCollectionOpen}
        onOpenChange={setNewCollectionOpen}
      />
    </Tabs>
  );
}

/** Reading-lists tab: the grouped board with the updates feature tucked
 *  into its toolbar. The "Updates" menu runs a check or a bulk update and
 *  reveals the updates panel under the toolbar; the panel stays hidden
 *  otherwise. Each group's ⋯ menu gets "Update these lists", which feeds
 *  the same runner and opens the same panel. */
function ReadingListsTab({
  views,
  loading,
  onImport,
  renderCard,
}: {
  views: SavedViewView[];
  loading: boolean;
  onImport: () => void;
  renderCard: (view: SavedViewView) => React.ReactNode;
}) {
  const targets = React.useMemo(
    () =>
      views.flatMap((v) =>
        v.cbl_list_id
          ? [{ id: v.cbl_list_id, name: v.name, viewId: v.id }]
          : [],
      ),
    [views],
  );
  return (
    <CblUpdatesProvider lists={targets}>
      <ReadingListsBoard
        views={views}
        loading={loading}
        onImport={onImport}
        renderCard={renderCard}
      />
    </CblUpdatesProvider>
  );
}

function ReadingListsBoard({
  views,
  loading,
  onImport,
  renderCard,
}: {
  views: SavedViewView[];
  loading: boolean;
  onImport: () => void;
  renderCard: (view: SavedViewView) => React.ReactNode;
}) {
  const { busy, startBatch } = useCblUpdates();
  const groupMenuExtra = React.useCallback(
    (group: BoardGroup) => {
      const ids = group.views.flatMap((v) =>
        v.cbl_list_id ? [{ id: v.cbl_list_id, name: v.name }] : [],
      );
      return (
        <DropdownMenuItem
          disabled={ids.length === 0 || busy}
          onSelect={() => startBatch(ids)}
        >
          <RefreshCw className="size-4" /> Update{" "}
          {ids.length === 1 ? "this list" : `these ${ids.length} lists`}
        </DropdownMenuItem>
      );
    },
    [busy, startBatch],
  );
  return (
    <GroupedViewsBoard
      scope="cbl"
      views={views}
      loading={loading}
      blurb="Track an imported CBL reading order."
      noun="list"
      icon={List}
      action={
        <>
          <CblUpdatesMenu />
          <Button type="button" onClick={onImport}>
            <Plus className="mr-1 size-4" /> Import CBL
          </Button>
        </>
      }
      panel={<CblUpdatesPanel />}
      renderCard={renderCard}
      groupMenuExtra={groupMenuExtra}
      empty="No reading lists yet — import a CBL to track a reading order."
    />
  );
}

function TabCount({ n }: { n: number }) {
  return (
    <Badge
      variant="secondary"
      className="ml-1.5 h-5 min-w-5 justify-center px-1.5 tabular-nums"
    >
      {n}
    </Badge>
  );
}

/** A saved-content card (filter view / reading list / collection). The body
 *  links to the detail page (edit happens there); the ⋯ menu folds in the
 *  arrangement + lifecycle actions: pin-to-pages (the search + collapsible
 *  `MultiPinDialog`), show-in-sidebar, Move to group (the keyboard / touch
 *  path for what drag-and-drop does), and Delete. Built-in rows (system
 *  rails + Want to Read) lock Delete. Reading-list cards also show the
 *  list's collection + read-progress stats and when it was last updated. */
function ViewCard({
  view,
  Icon,
  isCollection,
}: {
  view: SavedViewView;
  Icon: LucideIcon;
  isCollection: boolean;
}) {
  const qc = useQueryClient();
  // Both delete hooks are plain mutation builders (no work until `mutate`),
  // so calling both keeps the card a single component while routing the
  // delete through the right endpoint by kind.
  const delSaved = useDeleteSavedView(view.id);
  // Collections delete silently + show an Undo toast (audit B6). Saved
  // filter views keep their plain confirm — they're cheap to rebuild and
  // out of B6's scope.
  const delCollection = useDeleteCollection(view.id, { silent: true });
  const showDeleteUndo = useCollectionDeleteUndo();
  const sidebar = useSidebarSavedView();
  const board = useViewBoardActions();
  const cblLists = React.useContext(CblListsContext);
  const [pinOpen, setPinOpen] = React.useState(false);
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const [deleting, setDeleting] = React.useState(false);

  const isWantToRead = isCollection && view.system_key === WANT_TO_READ_KEY;
  const isBuiltIn = view.is_system || isWantToRead;
  const href = isCollection
    ? `/views/${isWantToRead ? "want-to-read" : view.id}`
    : `/views/${view.id}`;
  const pinnedCount = (view.pinned_on_pages ?? []).length;
  const cblList =
    view.kind === "cbl" && view.cbl_list_id
      ? (cblLists?.get(view.cbl_list_id) ?? null)
      : null;
  const currentGroup = board?.containerOf(view.id) ?? null;

  async function handleConfirmDelete() {
    if (!isCollection) {
      // Filter / CBL views: plain delete (the hook toasts on its own).
      delSaved.mutate(undefined, { onSuccess: () => setConfirmOpen(false) });
      return;
    }
    // Collections: snapshot the full member list *before* the delete drops
    // it, so the Undo toast can replay it. A snapshot failure (offline /
    // race) still deletes — just without the undo affordance.
    setDeleting(true);
    let snapshot: CollectionSnapshot | null = null;
    try {
      snapshot = await fetchCollectionSnapshot(
        view.id,
        view.name,
        view.description,
      );
    } catch {
      snapshot = null;
    }
    delCollection.mutate(undefined, {
      onSuccess: () => {
        setConfirmOpen(false);
        if (snapshot) showDeleteUndo(snapshot);
        else toast.success(`Collection "${view.name}" deleted`);
      },
      onSettled: () => setDeleting(false),
    });
  }

  function toggleSidebar(next: boolean) {
    sidebar.mutate(
      { id: view.id, show: next },
      // The sidebar mutation optimistically patches the saved-views cache;
      // collections read from a separate cache, so refresh it on settle.
      isCollection
        ? {
            onSettled: () =>
              qc.invalidateQueries({ queryKey: queryKeys.collections }),
          }
        : undefined,
    );
  }

  return (
    <div className="group hover:bg-accent/40 border-border/60 bg-card flex h-full flex-col gap-2 rounded-lg border p-4 transition-colors">
      <div className="flex items-start justify-between gap-2">
        <Icon
          className="text-muted-foreground size-5 shrink-0"
          aria-hidden="true"
        />
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              aria-label={`Actions for ${view.name}`}
              className="text-muted-foreground hover:text-foreground -mt-1 -mr-1 h-7 w-7"
            >
              <MoreVertical className="size-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-56">
            <DropdownMenuItem onSelect={() => setPinOpen(true)}>
              <Pin className="size-4" /> Pin to pages…
            </DropdownMenuItem>
            <DropdownMenuCheckboxItem
              checked={view.show_in_sidebar}
              onCheckedChange={(next) => toggleSidebar(next === true)}
            >
              Show in sidebar
            </DropdownMenuCheckboxItem>
            {board ? (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuResponsiveSub
                  icon={<FolderInput className="size-4" />}
                  label="Move to group"
                >
                  {board.groups.map((g) => (
                    <DropdownMenuItem
                      key={g.id}
                      disabled={currentGroup === g.id}
                      onSelect={() => board.moveToGroup(view.id, g.id)}
                    >
                      <span className="inline-flex w-4 justify-center">
                        {currentGroup === g.id ? (
                          <Check className="size-4" aria-hidden="true" />
                        ) : null}
                      </span>
                      <span className="truncate">{g.name}</span>
                    </DropdownMenuItem>
                  ))}
                  {board.groups.length > 0 ? (
                    <DropdownMenuItem
                      disabled={currentGroup === UNGROUPED_ID}
                      onSelect={() => board.moveToGroup(view.id, null)}
                    >
                      <span className="inline-flex w-4 justify-center">
                        {currentGroup === UNGROUPED_ID ? (
                          <Check className="size-4" aria-hidden="true" />
                        ) : null}
                      </span>
                      Ungrouped
                    </DropdownMenuItem>
                  ) : null}
                  {board.groups.length > 0 ? <DropdownMenuSeparator /> : null}
                  <DropdownMenuItem
                    onSelect={() => board.newGroupWithView(view.id)}
                  >
                    <FolderPlus className="size-4" /> New group…
                  </DropdownMenuItem>
                </DropdownMenuResponsiveSub>
              </>
            ) : null}
            {isBuiltIn ? null : (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  onSelect={() => setConfirmOpen(true)}
                  className="text-destructive focus:text-destructive"
                >
                  <Trash2 className="size-4" /> Delete
                </DropdownMenuItem>
              </>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      <Link
        href={href}
        draggable={false}
        className="focus-visible:ring-ring min-w-0 space-y-0.5 rounded focus-visible:ring-2 focus-visible:outline-none"
      >
        <div className="truncate font-medium" title={view.name}>
          {view.name}
        </div>
        {view.description ? (
          <p className="text-muted-foreground line-clamp-2 text-sm">
            {view.description}
          </p>
        ) : null}
      </Link>

      {cblList ? <CblCardStats list={cblList} /> : null}

      {isBuiltIn || pinnedCount > 0 || view.show_in_sidebar ? (
        <div className="mt-auto flex flex-wrap items-center gap-1.5 pt-1">
          {isBuiltIn ? (
            <Badge variant="outline" className="gap-1 font-normal">
              <Lock className="size-3" /> Built-in
            </Badge>
          ) : null}
          {pinnedCount > 0 ? (
            <Badge variant="secondary" className="gap-1 font-normal">
              <Pin className="size-3" /> {pinnedCount}{" "}
              {pinnedCount === 1 ? "page" : "pages"}
            </Badge>
          ) : null}
          {view.show_in_sidebar ? (
            <Badge variant="secondary" className="gap-1 font-normal">
              <PanelLeft className="size-3" /> Sidebar
            </Badge>
          ) : null}
        </div>
      ) : null}

      <MultiPinDialog view={view} open={pinOpen} onOpenChange={setPinOpen} />
      <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              Delete {isCollection ? "this collection" : "this view"}?
            </AlertDialogTitle>
            <AlertDialogDescription>
              {isCollection
                ? "Removes the collection and its hand-picked list — the series and issues themselves are untouched. You can undo this right after."
                : view.kind === "cbl"
                  ? "Removes the saved view but keeps the underlying CBL list — you can re-import later."
                  : "Removes the filter view permanently."}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleting}>Cancel</AlertDialogCancel>
            <AlertDialogAction
              // Keep the dialog mounted through the async snapshot+delete
              // so the pending state shows; we close it on success.
              onClick={(e) => {
                e.preventDefault();
                void handleConfirmDelete();
              }}
              disabled={deleting}
            >
              {deleting ? "Deleting…" : "Delete"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

/** Reading-list card footer: the collection / read-progress pills, a thin
 *  read-progress bar, and the last refresh time — enough to triage a
 *  long list of lists without opening each one. */
function CblCardStats({ list }: { list: CblListView }) {
  const { matched, read_count } = list.stats;
  const percent = matched > 0 ? Math.round((read_count / matched) * 100) : 0;
  const when = formatRelativeDate(list.last_refreshed_at ?? list.imported_at);
  return (
    <div className="space-y-1.5 pt-0.5">
      <div className="flex flex-wrap items-center justify-between gap-1.5">
        <CblStatsPillsStatic stats={list.stats} />
        {when ? (
          <span
            className="text-muted-foreground text-xs"
            title={
              list.last_refreshed_at
                ? `Last refreshed ${new Date(list.last_refreshed_at).toLocaleString()}`
                : `Imported ${new Date(list.imported_at).toLocaleString()}`
            }
          >
            {list.last_refreshed_at ? "Updated" : "Imported"} {when}
          </span>
        ) : null}
      </div>
      {matched > 0 ? (
        <Progress
          value={percent}
          className="h-1"
          aria-label={`${percent}% read`}
        />
      ) : null}
    </div>
  );
}
