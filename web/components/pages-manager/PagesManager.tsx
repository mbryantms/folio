"use client";

import * as React from "react";
import Link from "next/link";
import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import {
  ExternalLink,
  GripVertical,
  ListPlus,
  MessageSquare,
  MoreHorizontal,
  PanelLeft,
  PanelLeftClose,
  Pencil,
  Plus,
  Trash2,
  X,
} from "lucide-react";

import { NewPageButton } from "@/components/library/NewPageButton";
import { EditDescriptionDialog } from "@/components/saved-views/EditDescriptionDialog";
import { ManagePinsDialog } from "@/components/saved-views/ManagePinsDialog";
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
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  useDeletePage,
  useReorderSavedViews,
  useTogglePageSidebar,
  useTogglePinOnPage,
  useUpdatePage,
  useUpdatePreferences,
} from "@/lib/api/mutations";
import { useMe, useMePages, useSavedViews } from "@/lib/api/queries";
import { cn } from "@/lib/utils";
import type { PageView, SavedViewView } from "@/lib/api/types";
import { Label } from "@/components/ui/label";

/** Server-side fallback when the `/auth/me` lookup hasn't resolved
 *  yet. Matches the column default in
 *  `m20261222_000001_user_max_rails_per_page`; keeping it in sync
 *  with that migration's `.default(12)` is enforced visually rather
 *  than programmatically.
 */
const DEFAULT_RAIL_CAP = 12;
const RAIL_CAP_MIN = 1;
const RAIL_CAP_MAX = 50;

/** Settings → Pages.
 *
 *  Tabbed view of every page the user owns (system Home + custom).
 *  Each tab opens a panel listing the saved views pinned to that page —
 *  drag-reorder them, remove with a row button, or add new ones via
 *  the picker. Page-level controls (rename, description, sidebar
 *  visibility, delete) live in a kebab menu inside each panel. */
export function PagesManager() {
  const pagesQ = useMePages();
  const me = useMe();
  const railCap = me.data?.max_rails_per_page ?? DEFAULT_RAIL_CAP;
  const allPages = React.useMemo(() => pagesQ.data ?? [], [pagesQ.data]);
  const sortedPages = React.useMemo(() => {
    // System Home first, then custom pages in their stored order.
    const system = allPages.filter((p) => p.is_system);
    const custom = allPages.filter((p) => !p.is_system);
    return [...system, ...custom];
  }, [allPages]);
  const [activeTab, setActiveTab] = React.useState<string | null>(null);
  // Default + repair the active tab in render. Both situations are
  // "derive state from props" cases — using the render-phase setState
  // idiom (https://react.dev/learn/you-might-not-need-an-effect) keeps
  // the lint clean and avoids an extra mount.
  const firstPageId = sortedPages[0]?.id ?? null;
  if (firstPageId !== null) {
    const stillValid =
      activeTab !== null && sortedPages.some((p) => p.id === activeTab);
    if (!stillValid) {
      setActiveTab(firstPageId);
    }
  } else if (activeTab !== null) {
    setActiveTab(null);
  }

  if (pagesQ.isLoading) {
    return (
      <div className="text-muted-foreground py-12 text-sm">Loading pages…</div>
    );
  }
  if (sortedPages.length === 0 || activeTab === null) {
    return (
      <div className="flex flex-col gap-4">
        <NewPageButton />
        <p className="text-muted-foreground text-sm">
          No pages yet. Click <strong>New page</strong> to add one.
        </p>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-muted-foreground text-sm">
          Each page holds up to {railCap} pinned saved-view rails. Drag rows to
          reorder; use the kebab menu for page settings.
        </p>
        <NewPageButton />
      </div>

      <RailCapControl value={railCap} />

      <Tabs value={activeTab} onValueChange={setActiveTab}>
        <TabsList>
          {sortedPages.map((p) => (
            <TabsTrigger key={p.id} value={p.id}>
              {p.name}
              {p.is_system ? (
                <Badge
                  variant="outline"
                  className="text-muted-foreground ml-2 text-[10px] font-medium tracking-wider uppercase"
                >
                  Home
                </Badge>
              ) : null}
              <span className="text-muted-foreground/80 ml-2 text-xs tabular-nums">
                {p.pin_count}
              </span>
            </TabsTrigger>
          ))}
        </TabsList>
        {sortedPages.map((p) => (
          <TabsContent key={p.id} value={p.id}>
            <PageTabContent page={p} />
          </TabsContent>
        ))}
      </Tabs>
    </div>
  );
}

function PageTabContent({ page }: { page: PageView }) {
  return (
    <div className="border-border/60 rounded-lg border">
      <PageHeader page={page} />
      <PageRailsList pageId={page.id} pageName={page.name} />
    </div>
  );
}

function PageHeader({ page }: { page: PageView }) {
  const updatePage = useUpdatePage(page.id);
  const toggleSidebar = useTogglePageSidebar(page.id);
  const del = useDeletePage(page.id);
  const [editing, setEditing] = React.useState(false);
  const [draft, setDraft] = React.useState(page.name);
  const [lastSeen, setLastSeen] = React.useState(page.name);
  if (lastSeen !== page.name) {
    setLastSeen(page.name);
    setDraft(page.name);
  }
  const [descOpen, setDescOpen] = React.useState(false);
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const inputRef = React.useRef<HTMLInputElement | null>(null);
  React.useEffect(() => {
    if (editing) inputRef.current?.select();
  }, [editing]);

  const commit = async () => {
    const trimmed = draft.trim();
    setEditing(false);
    if (trimmed.length === 0 || trimmed === page.name) {
      setDraft(page.name);
      return;
    }
    try {
      await updatePage.mutateAsync({ name: trimmed });
    } catch {
      setDraft(page.name);
    }
  };

  const href = page.is_system ? "/" : `/pages/${page.slug}`;
  const hasDescription =
    typeof page.description === "string" && page.description.length > 0;

  return (
    <header className="border-border/60 flex flex-wrap items-start justify-between gap-3 border-b p-4">
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          {editing ? (
            <Input
              ref={inputRef}
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onBlur={commit}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  void commit();
                } else if (e.key === "Escape") {
                  e.preventDefault();
                  setDraft(page.name);
                  setEditing(false);
                }
              }}
              maxLength={88}
              className="h-8 max-w-xs text-base font-semibold"
              aria-label="Page name"
            />
          ) : (
            <button
              type="button"
              onClick={() => setEditing(true)}
              className="group flex items-center gap-1.5 text-left"
              title="Click to rename"
            >
              <span className="text-base font-semibold">{page.name}</span>
              <Pencil className="text-muted-foreground/0 group-hover:text-muted-foreground h-3.5 w-3.5 transition-colors" />
            </button>
          )}
          <Link
            href={href}
            className="text-muted-foreground hover:text-foreground inline-flex items-center gap-1 text-xs"
          >
            <ExternalLink className="h-3 w-3" />
            Open
          </Link>
        </div>
        {hasDescription ? (
          <p className="text-muted-foreground mt-1 text-sm">
            {page.description}
          </p>
        ) : null}
      </div>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="outline"
            size="icon"
            className="h-8 w-8 shrink-0"
            aria-label="Page actions"
          >
            <MoreHorizontal className="h-4 w-4" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="min-w-[14rem]">
          <DropdownMenuItem
            onSelect={(e) => {
              e.preventDefault();
              setDescOpen(true);
            }}
          >
            <MessageSquare className="mr-2 h-4 w-4" />
            {hasDescription ? "Edit description" : "Add description"}
          </DropdownMenuItem>
          {hasDescription ? (
            <DropdownMenuItem
              onSelect={(e) => {
                e.preventDefault();
                updatePage.mutate({ description: "" });
              }}
            >
              <X className="mr-2 h-4 w-4" />
              Clear description
            </DropdownMenuItem>
          ) : null}
          {!page.is_system && (
            <DropdownMenuItem
              onSelect={(e) => {
                e.preventDefault();
                toggleSidebar.mutate({ show: !page.show_in_sidebar });
              }}
            >
              {page.show_in_sidebar ? (
                <>
                  <PanelLeftClose className="mr-2 h-4 w-4" /> Hide from sidebar
                </>
              ) : (
                <>
                  <PanelLeft className="mr-2 h-4 w-4" /> Show in sidebar
                </>
              )}
            </DropdownMenuItem>
          )}
          {!page.is_system && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                onSelect={(e) => {
                  e.preventDefault();
                  setConfirmOpen(true);
                }}
                className="text-destructive focus:text-destructive"
              >
                <Trash2 className="mr-2 h-4 w-4" /> Delete page…
              </DropdownMenuItem>
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
      <EditDescriptionDialog
        open={descOpen}
        onOpenChange={setDescOpen}
        pageId={page.id}
        initial={page.description ?? ""}
      />
      <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete this page?</AlertDialogTitle>
            <AlertDialogDescription>
              Pins on this page will be removed. The saved views themselves stay
              — you can pin them to other pages from Saved views.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={(e) => {
                e.preventDefault();
                del.mutate();
                setConfirmOpen(false);
              }}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              Delete page
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </header>
  );
}

/** Drag-reorderable list of pinned saved views for one page. */
function PageRailsList({
  pageId,
  pageName,
}: {
  pageId: string;
  pageName: string;
}) {
  const me = useMe();
  const railCap = me.data?.max_rails_per_page ?? DEFAULT_RAIL_CAP;
  const railsQ = useSavedViews({ pinnedOn: pageId });
  const reorder = useReorderSavedViews();
  const toggle = useTogglePinOnPage();
  const [pinOpen, setPinOpen] = React.useState(false);
  const [optimistic, setOptimistic] = React.useState<string[] | null>(null);

  const pinned = React.useMemo(
    () => railsQ.data?.items ?? [],
    [railsQ.data?.items],
  );
  // pinned_on_pages filtered by this page means everything in the
  // response is currently pinned here; preserve the server order until
  // an active drag-end overrides it.
  const pinnedIds = React.useMemo(() => pinned.map((v) => v.id), [pinned]);
  const renderIds = optimistic ?? pinnedIds;
  const viewById = React.useMemo(() => {
    const m = new Map<string, SavedViewView>();
    for (const v of pinned) m.set(v.id, v);
    return m;
  }, [pinned]);
  const ordered = renderIds
    .map((id) => viewById.get(id))
    .filter((v): v is SavedViewView => v !== undefined);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  const handleDragEnd = (e: DragEndEvent) => {
    const { active, over } = e;
    if (!over || active.id === over.id) return;
    const oldIdx = renderIds.indexOf(String(active.id));
    const newIdx = renderIds.indexOf(String(over.id));
    if (oldIdx < 0 || newIdx < 0) return;
    const next = arrayMove(renderIds, oldIdx, newIdx);
    setOptimistic(next);
    reorder.mutate(
      { page_id: pageId, view_ids: next },
      {
        onError: () => setOptimistic(null),
        onSettled: () => setOptimistic(null),
      },
    );
  };

  const atCap = pinned.length >= railCap;

  return (
    <div className="flex flex-col">
      <div className="flex flex-wrap items-center justify-between gap-2 p-3">
        <p className="text-muted-foreground text-xs">
          {pinned.length} / {railCap} rails
        </p>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => setPinOpen(true)}
          disabled={atCap && pinned.length === 0}
        >
          <Plus className="mr-1 h-3.5 w-3.5" />
          Add view
        </Button>
      </div>
      {railsQ.isLoading ? (
        <div className="text-muted-foreground p-6 text-center text-sm">
          Loading…
        </div>
      ) : ordered.length === 0 ? (
        <div className="text-muted-foreground border-border/60 m-3 mt-0 flex flex-col items-center gap-2 rounded-md border border-dashed p-6 text-center text-sm">
          <ListPlus className="text-muted-foreground/60 h-5 w-5" />
          <p>
            No rails on <span className="text-foreground">{pageName}</span> yet.
          </p>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => setPinOpen(true)}
          >
            <Plus className="mr-1 h-3.5 w-3.5" />
            Add a view
          </Button>
        </div>
      ) : (
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragEnd={handleDragEnd}
        >
          <SortableContext
            items={renderIds}
            strategy={verticalListSortingStrategy}
          >
            <ul className="border-border/60 divide-border/60 divide-y border-t">
              {ordered.map((view, idx) => (
                <RailRow
                  key={view.id}
                  view={view}
                  pageId={pageId}
                  position={idx + 1}
                  onRemove={() =>
                    toggle.mutate({
                      viewId: view.id,
                      pageId,
                      pinned: false,
                    })
                  }
                />
              ))}
            </ul>
          </SortableContext>
        </DndContext>
      )}
      <ManagePinsDialog
        open={pinOpen}
        onOpenChange={setPinOpen}
        pageId={pageId}
      />
    </div>
  );
}

function RailRow({
  view,
  position,
  onRemove,
}: {
  view: SavedViewView;
  pageId: string;
  position: number;
  onRemove: () => void;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: view.id });
  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.6 : undefined,
  };
  return (
    <li
      ref={setNodeRef}
      style={style}
      className="bg-background flex items-center gap-3 px-3 py-2"
    >
      <button
        type="button"
        aria-label={`Drag ${view.name}`}
        className="text-muted-foreground hover:text-foreground hover:bg-secondary/50 flex h-7 w-7 shrink-0 cursor-grab items-center justify-center rounded-md transition-colors active:cursor-grabbing"
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        {...(attributes as any)}
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        {...(listeners as any)}
      >
        <GripVertical className="h-4 w-4" />
      </button>
      <span className="text-muted-foreground/70 w-5 shrink-0 text-right text-xs tabular-nums">
        {position}
      </span>
      <div className="min-w-0 flex-1">
        <Link
          href={`/views/${view.id}`}
          className="hover:text-foreground block truncate text-sm font-medium"
          title={view.name}
        >
          {view.name}
        </Link>
        {view.description ? (
          <p className="text-muted-foreground truncate text-xs">
            {view.description}
          </p>
        ) : null}
      </div>
      <Badge
        variant="outline"
        className={cn(
          "text-muted-foreground hidden shrink-0 text-[10px] font-medium tracking-wider uppercase sm:inline-flex",
        )}
      >
        {kindLabel(view.kind)}
      </Badge>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="text-muted-foreground hover:text-destructive h-7 w-7"
        onClick={onRemove}
        aria-label="Remove from page"
        title="Remove from page"
      >
        <X className="h-4 w-4" />
      </Button>
    </li>
  );
}

function kindLabel(kind: SavedViewView["kind"]): string {
  switch (kind) {
    case "filter_series":
      return "Filter";
    case "cbl":
      return "CBL";
    case "system":
      return "Built-in";
    case "collection":
      return "Collection";
    default:
      return "View";
  }
}

/** Adjust the maximum number of pinned saved-view rails per page.
 *  PATCH on blur (matches the activity-tracking-thresholds form). The
 *  server enforces the same 1..=50 range — invalid inputs are dropped
 *  client-side without an attempted PATCH so the field doesn't flicker. */
function RailCapControl({ value }: { value: number }) {
  const update = useUpdatePreferences({ silent: true });
  return (
    <div className="border-border/60 bg-card/40 flex flex-wrap items-center gap-3 rounded-md border p-3">
      <div className="min-w-0 flex-1">
        <Label
          htmlFor="rail-cap-input"
          className="text-foreground text-sm font-medium"
        >
          Maximum rails per page
        </Label>
        <p className="text-muted-foreground mt-0.5 text-xs">
          Off-screen rails lazy-load as you scroll, so higher values stay
          responsive. Range {RAIL_CAP_MIN}–{RAIL_CAP_MAX}; default{" "}
          {DEFAULT_RAIL_CAP}.
        </p>
      </div>
      <Input
        id="rail-cap-input"
        type="number"
        min={RAIL_CAP_MIN}
        max={RAIL_CAP_MAX}
        step={1}
        defaultValue={value}
        onBlur={(e) => {
          const v = Number(e.currentTarget.value);
          if (!Number.isFinite(v) || v < RAIL_CAP_MIN || v > RAIL_CAP_MAX) {
            // Reset the field to the last-saved value so the user sees the
            // rejection rather than a phantom edit.
            e.currentTarget.value = String(value);
            return;
          }
          if (v === value) return;
          update.mutate({ max_rails_per_page: v });
        }}
        disabled={update.isPending}
        className="w-20"
      />
    </div>
  );
}
