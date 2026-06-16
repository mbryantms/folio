"use client";

import * as React from "react";
import Link from "next/link";
import { Suspense } from "react";
import { Library, List, Plus, Sparkles, Trash2 } from "lucide-react";

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
import { Button } from "@/components/ui/button";
import { CblImportDialog } from "@/components/cbl/cbl-import-dialog";
import { NewFilterViewDialog } from "@/components/saved-views/AddViewButton";
import { CollectionsIndex } from "@/components/collections/CollectionsIndex";
import { QuickApplyPrefill } from "@/components/saved-views/QuickApplyPrefill";
import { ViewsSection } from "@/components/saved-views/views-section";
import { useDeleteSavedView } from "@/lib/api/mutations";
import { useSavedViews } from "@/lib/api/queries";
import type { SavedViewView } from "@/lib/api/types";

/** Split the single `useSavedViews()` list into the two index sections.
 *  `system` rails (continue_reading / on_deck) and `collection`s are
 *  excluded from both — system rails must never leak into Filter views,
 *  and collections render from their own `useCollections()` source. */
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
 * Unified `/views` index — the one library-context home for saved content
 * (audit A3). Three anchored sections — Filter views, Reading lists (CBL),
 * Collections — each with in-page create/import. Replaces the old redirect
 * to `/settings/views`, which now keeps arrangement only. `/collections`
 * redirects here to `#collections`.
 *
 * Filter views and Reading lists are two client-side kind-filters over the
 * single `useSavedViews()` call; Collections reuse `CollectionsIndex`
 * (which owns `useCollections()` + Want-to-Read-first + the create dialog).
 */
export function ViewsIndex() {
  const viewsQ = useSavedViews();
  const all = React.useMemo(() => viewsQ.data?.items ?? [], [viewsQ.data]);
  const { filterViews, readingLists } = React.useMemo(
    () => splitSavedViews(all),
    [all],
  );

  const [newFilterOpen, setNewFilterOpen] = React.useState(false);
  const [importOpen, setImportOpen] = React.useState(false);

  return (
    <div className="space-y-8">
      <PageHeader
        title="Views"
        description="Filter views update themselves from rules · Reading lists track an imported CBL · Collections are hand-picked. Manage where they appear under Settings → Saved views."
      />

      <ViewsSection
        id="filter-views"
        icon={Sparkles}
        title="Filter views"
        blurb="auto-update from rules"
        action={
          <Button type="button" onClick={() => setNewFilterOpen(true)}>
            <Plus className="mr-1 h-4 w-4" /> New filter view
          </Button>
        }
      >
        <ViewGrid
          loading={viewsQ.isLoading}
          views={filterViews}
          icon={Sparkles}
          empty="No filter views yet — create one to save a set of filters."
        />
      </ViewsSection>

      <ViewsSection
        id="reading-lists"
        icon={Library}
        title="Reading lists"
        blurb="track an imported CBL"
        action={
          <Button type="button" onClick={() => setImportOpen(true)}>
            <Plus className="mr-1 h-4 w-4" /> Import CBL
          </Button>
        }
      >
        <ViewGrid
          loading={viewsQ.isLoading}
          views={readingLists}
          icon={List}
          empty="No reading lists yet — import a CBL to track a reading order."
        />
      </ViewsSection>

      <CollectionsIndex embedded />

      <NewFilterViewDialog
        open={newFilterOpen}
        onOpenChange={setNewFilterOpen}
      />
      <CblImportDialog open={importOpen} onOpenChange={setImportOpen} />
      {/* Quick-apply: `?quick_field=&quick_value=` opens the New-filter-view
          dialog pre-filled (chip-list deep links). useSearchParams needs a
          Suspense boundary. Moved here from /settings/views with the create
          flow (A3). */}
      <Suspense fallback={null}>
        <QuickApplyPrefill />
      </Suspense>
    </div>
  );
}

function ViewGrid({
  loading,
  views,
  icon,
  empty,
}: {
  loading: boolean;
  views: SavedViewView[];
  icon: typeof Sparkles;
  empty: string;
}) {
  if (loading) {
    return <div className="text-muted-foreground py-6 text-sm">Loading…</div>;
  }
  if (views.length === 0) {
    return (
      <div className="border-border/60 text-muted-foreground rounded-lg border border-dashed p-6 text-sm">
        {empty}
      </div>
    );
  }
  return (
    <ul
      role="list"
      className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4"
    >
      {views.map((view) => (
        <li key={view.id}>
          <ViewCard view={view} Icon={icon} />
        </li>
      ))}
    </ul>
  );
}

/** A saved filter-view / reading-list card. The body links to the detail
 *  page (edit happens there); the hover-revealed Delete is the lifecycle
 *  action moved out of /settings/views (A3). Delete sits OUTSIDE the link
 *  (sibling, not nested) so the card stays valid HTML. */
function ViewCard({
  view,
  Icon,
}: {
  view: SavedViewView;
  Icon: typeof Sparkles;
}) {
  const del = useDeleteSavedView(view.id);
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const isCbl = view.kind === "cbl";
  return (
    <div className="group hover:bg-accent/40 border-border/60 flex h-full flex-col gap-2 rounded-lg border p-4 transition-colors">
      <div className="flex items-start justify-between gap-2">
        <Icon
          className="text-muted-foreground h-5 w-5 shrink-0"
          aria-hidden="true"
        />
        <Button
          type="button"
          variant="ghost"
          size="icon"
          onClick={() => setConfirmOpen(true)}
          aria-label={`Delete ${view.name}`}
          className="text-muted-foreground hover:text-destructive -mt-1 -mr-1 h-7 w-7 opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100"
        >
          <Trash2 className="h-4 w-4" />
        </Button>
      </div>
      <Link
        href={`/views/${view.id}`}
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
      <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete this view?</AlertDialogTitle>
            <AlertDialogDescription>
              {isCbl
                ? "Removes the saved view but keeps the underlying CBL list — you can re-import later."
                : "Removes the filter view permanently."}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={() => del.mutate()}>
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
