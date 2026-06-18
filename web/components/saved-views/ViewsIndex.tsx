"use client";

import * as React from "react";
import Link from "next/link";
import { Suspense } from "react";
import { useSearchParams } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import {
  Folder,
  List,
  Lock,
  MoreVertical,
  PanelLeft,
  Pin,
  Plus,
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
} from "@/components/ui/dropdown-menu";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { CblImportDialog } from "@/components/cbl/cbl-import-dialog";
import { NewFilterViewDialog } from "@/components/saved-views/AddViewButton";
import { NewCollectionDialog } from "@/components/collections/CollectionsIndex";
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
import { queryKeys, useCollections, useSavedViews } from "@/lib/api/queries";
import type { SavedViewView } from "@/lib/api/types";

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
    filterViews: items.filter((v) => v.kind === "filter_series"),
    readingLists: items.filter((v) => v.kind === "cbl"),
  };
}

/**
 * Unified saved-content index — the one management home for Views (audit
 * A3). Lives at `/settings/views`; replaces the old arrangement-only
 * manager. Three tabs — Filter views, Reading lists (CBL), Collections —
 * each with in-page create/import, and each card carries its own
 * arrangement controls (pin-to-pages + show-in-sidebar) so browse,
 * lifecycle, and arrangement live in one place.
 *
 * Tabs (vs the earlier stacked sections) keep a long Reading-lists library
 * from pushing Collections below the fold. The standalone `/collections`
 * page stays the day-to-day collections browse surface.
 */
export function ViewsIndex() {
  return (
    <div className="space-y-6">
      <PageHeader
        title="Views"
        description="Filter views update themselves from rules · Reading lists track an imported CBL · Collections are hand-picked. Pin any of them to a page or the sidebar from its ⋯ menu."
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
  const { filterViews, readingLists } = React.useMemo(
    () => splitSavedViews(viewsQ.data?.items ?? []),
    [viewsQ.data],
  );
  const collections = React.useMemo(
    () => collectionsQ.data ?? [],
    [collectionsQ.data],
  );

  const [newFilterOpen, setNewFilterOpen] = React.useState(false);
  const [importOpen, setImportOpen] = React.useState(false);
  const [newCollectionOpen, setNewCollectionOpen] = React.useState(false);

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
        <TabPanel
          blurb="Auto-update from a set of filters."
          action={
            <Button type="button" onClick={() => setNewFilterOpen(true)}>
              <Plus className="mr-1 h-4 w-4" /> New filter view
            </Button>
          }
          loading={viewsQ.isLoading}
          views={filterViews}
          icon={Sparkles}
          empty="No filter views yet — create one to save a set of filters."
        />
      </TabsContent>

      <TabsContent value="reading-lists">
        <TabPanel
          blurb="Track an imported CBL reading order."
          action={
            <Button type="button" onClick={() => setImportOpen(true)}>
              <Plus className="mr-1 h-4 w-4" /> Import CBL
            </Button>
          }
          loading={viewsQ.isLoading}
          views={readingLists}
          icon={List}
          empty="No reading lists yet — import a CBL to track a reading order."
        />
      </TabsContent>

      <TabsContent value="collections">
        <TabPanel
          blurb="Hand-picked series and issues."
          action={
            <Button type="button" onClick={() => setNewCollectionOpen(true)}>
              <Plus className="mr-1 h-4 w-4" /> New collection
            </Button>
          }
          loading={collectionsQ.isLoading}
          views={collections}
          icon={Folder}
          isCollection
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

function TabPanel({
  blurb,
  action,
  loading,
  views,
  icon,
  empty,
  isCollection = false,
}: {
  blurb: string;
  action: React.ReactNode;
  loading: boolean;
  views: SavedViewView[];
  icon: LucideIcon;
  empty: string;
  isCollection?: boolean;
}) {
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-muted-foreground text-sm">{blurb}</p>
        {action}
      </div>
      {loading ? (
        <div className="text-muted-foreground py-6 text-sm">Loading…</div>
      ) : views.length === 0 ? (
        <div className="border-border/60 text-muted-foreground rounded-lg border border-dashed p-6 text-sm">
          {empty}
        </div>
      ) : (
        <ul
          role="list"
          className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4"
        >
          {views.map((view) => (
            <li key={view.id}>
              <ViewCard view={view} Icon={icon} isCollection={isCollection} />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** A saved-content card (filter view / reading list / collection). The body
 *  links to the detail page (edit happens there); the ⋯ menu folds in the
 *  arrangement + lifecycle actions that used to live on `/settings/views`:
 *  pin-to-pages (the search + collapsible `MultiPinDialog`), show-in-sidebar,
 *  and Delete. Built-in rows (system rails + Want to Read) lock Delete. */
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
  const [pinOpen, setPinOpen] = React.useState(false);
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const [deleting, setDeleting] = React.useState(false);

  const isWantToRead = isCollection && view.system_key === WANT_TO_READ_KEY;
  const isBuiltIn = view.is_system || isWantToRead;
  const href = isCollection
    ? `/views/${isWantToRead ? "want-to-read" : view.id}`
    : `/views/${view.id}`;
  const pinnedCount = (view.pinned_on_pages ?? []).length;

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
    <div className="group hover:bg-accent/40 border-border/60 flex h-full flex-col gap-2 rounded-lg border p-4 transition-colors">
      <div className="flex items-start justify-between gap-2">
        <Icon
          className="text-muted-foreground h-5 w-5 shrink-0"
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
              <MoreVertical className="h-4 w-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-48">
            <DropdownMenuItem onSelect={() => setPinOpen(true)}>
              <Pin className="h-4 w-4" /> Pin to pages…
            </DropdownMenuItem>
            <DropdownMenuCheckboxItem
              checked={view.show_in_sidebar}
              onCheckedChange={(next) => toggleSidebar(next === true)}
            >
              Show in sidebar
            </DropdownMenuCheckboxItem>
            {isBuiltIn ? null : (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  onSelect={() => setConfirmOpen(true)}
                  className="text-destructive focus:text-destructive"
                >
                  <Trash2 className="h-4 w-4" /> Delete
                </DropdownMenuItem>
              </>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      <Link
        href={href}
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

      {isBuiltIn || pinnedCount > 0 || view.show_in_sidebar ? (
        <div className="mt-auto flex flex-wrap items-center gap-1.5 pt-1">
          {isBuiltIn ? (
            <Badge variant="outline" className="gap-1 font-normal">
              <Lock className="h-3 w-3" /> Built-in
            </Badge>
          ) : null}
          {pinnedCount > 0 ? (
            <Badge variant="secondary" className="gap-1 font-normal">
              <Pin className="h-3 w-3" /> {pinnedCount}{" "}
              {pinnedCount === 1 ? "page" : "pages"}
            </Badge>
          ) : null}
          {view.show_in_sidebar ? (
            <Badge variant="secondary" className="gap-1 font-normal">
              <PanelLeft className="h-3 w-3" /> Sidebar
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
