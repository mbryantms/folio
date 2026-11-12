"use client";

import * as React from "react";
import Link from "next/link";
import {
  List,
  Lock,
  MoreHorizontal,
  PanelLeft,
  Pin,
  type LucideIcon,
} from "lucide-react";

import { Cover } from "@/components/Cover";
import { CblListsContext } from "@/components/cbl/cbl-lists-context";
import { ListProgressBar } from "@/components/cbl/ListProgressBar";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/folio/tooltip";
import { ROW_CELL } from "@/components/saved-views/board-rows";
import type {
  BoardRowContext,
  BoardRowsPresentation,
} from "@/components/saved-views/GroupedViewsBoard";
import {
  SavedViewMenu,
  isBuiltInView,
} from "@/components/saved-views/SavedViewMenu";
import { useViewCovers } from "@/lib/api/queries";
import type { CblListView, SavedViewView } from "@/lib/api/types";
import {
  compareByRead,
  inLibraryLabel,
  readCaption,
  readLabel,
  rollupStats,
  type RowStats,
} from "@/lib/cbl/list-rows";
import { formatRelativeDate } from "@/lib/format";
import type { BoardGroup } from "@/lib/saved-views/view-board";
import { cn } from "@/lib/utils";

/**
 * The reading lists as rows of `GroupedViewsBoard`'s tree table: one
 * header (Name · Read · In library · Updated), folders with roll-ups,
 * a row per list. `ROW_CELL` places each cell; see `board-rows.ts` for
 * what each width shows. `lists` is the complete `/me/cbl-lists`
 * response by list id (`null` while it loads), which is also what the
 * "Most read" sort reads.
 */
export function readingListRows(
  lists: Map<string, CblListView> | null,
): BoardRowsPresentation {
  const statsOf = (view: SavedViewView): RowStats | null =>
    (view.cbl_list_id ? lists?.get(view.cbl_list_id)?.stats : null) ?? null;
  return {
    densityKey: "folio.readingLists.density",
    columns: [
      { key: "read", label: "Read", cell: ROW_CELL.stat, sort: "read" },
      { key: "library", label: "In library", cell: ROW_CELL.stat2 },
      {
        key: "updated",
        label: "Updated",
        cell: ROW_CELL.stat3,
        sort: "updated",
      },
    ],
    sorts: [
      { value: "read", label: "Most read", compare: compareByRead(statsOf) },
    ],
    renderRow: (view, ctx) => <ReadingListRowCells view={view} ctx={ctx} />,
    renderGroupCells: (group) => (
      <FolderCells group={group} statsOf={statsOf} />
    ),
  };
}

/** A list row's cells after the board's grip. */
function ReadingListRowCells({
  view,
  ctx,
}: {
  view: SavedViewView;
  ctx: BoardRowContext;
}) {
  const lists = React.useContext(CblListsContext);
  const list = view.cbl_list_id ? (lists?.get(view.cbl_list_id) ?? null) : null;
  const stats = list?.stats ?? null;
  const compact = ctx.density === "compact";
  const updated = updatedAt(list);
  const pinnedCount = (view.pinned_on_pages ?? []).length;
  const builtIn = isBuiltInView(view, false);

  return (
    <>
      <div className={ROW_CELL.lead}>
        <RowCover viewId={view.id} />
      </div>

      {/* The whole name / description area opens the list. Compact (lg):
          one line, the name never giving way to the description. */}
      <Link
        href={`/views/${view.id}`}
        draggable={false}
        data-row-link=""
        className={cn(
          ROW_CELL.main,
          "focus-visible:ring-ring flex flex-col gap-0.5 rounded focus-visible:ring-2 focus-visible:outline-none",
          compact && "lg:flex-row lg:items-baseline lg:gap-2",
        )}
      >
        <span
          data-row-name=""
          title={view.name}
          className={cn(
            "text-foreground text-sm font-medium break-words max-sm:line-clamp-2",
            compact && "lg:max-w-full lg:shrink-0 lg:truncate",
          )}
        >
          {view.name}
        </span>
        {view.description ? (
          <span
            data-row-description=""
            title={view.description}
            className={cn(
              "text-muted-foreground text-xs",
              compact
                ? "max-lg:line-clamp-2 lg:min-w-0 lg:flex-1 lg:truncate"
                : "line-clamp-2",
            )}
          >
            {view.description}
          </span>
        ) : null}
      </Link>

      <div className={ROW_CELL.stat}>
        <ReadCell stats={stats} updated={updated} />
      </div>
      <div className={ROW_CELL.stat2}>
        <InLibraryCell stats={stats} />
      </div>
      <div className={ROW_CELL.stat3}>
        {updated ? (
          <span
            className="text-muted-foreground text-xs tabular-nums"
            title={updated.title}
          >
            <span className="sr-only">Updated </span>
            {updated.relative}
          </span>
        ) : (
          <Dash />
        )}
      </div>

      <div className={ROW_CELL.flags}>
        {builtIn ? (
          <Flag icon={Lock} label="Built-in: shared with everyone" />
        ) : null}
        {pinnedCount > 0 ? (
          <Flag
            icon={Pin}
            label={`Pinned to ${pinnedCount} ${pinnedCount === 1 ? "page" : "pages"}`}
          />
        ) : null}
        {view.show_in_sidebar ? (
          <Flag icon={PanelLeft} label="In the sidebar" />
        ) : null}
      </div>

      <div className={ROW_CELL.menu}>
        {ctx.overlay ? (
          <span className="text-muted-foreground inline-flex size-8 items-center justify-center">
            <MoreHorizontal className="size-4" aria-hidden="true" />
          </span>
        ) : (
          <SavedViewMenu
            view={view}
            isCollection={false}
            icon={MoreHorizontal}
            triggerClassName="size-8"
          />
        )}
      </div>
    </>
  );
}

/** A folder row's roll-ups over every list in it. */
function FolderCells({
  group,
  statsOf,
}: {
  group: BoardGroup;
  statsOf: (view: SavedViewView) => RowStats | null;
}) {
  const stats = rollupStats(group.views.map(statsOf));
  const known = group.views.some((v) => statsOf(v) !== null);
  return (
    <>
      <div className={ROW_CELL.stat}>
        <ReadCell stats={known ? stats : null} updated={null} />
      </div>
      <div className={ROW_CELL.stat2}>
        <InLibraryCell stats={known ? stats : null} />
      </div>
    </>
  );
}

function updatedAt(
  list: CblListView | null,
): { relative: string; title: string } | null {
  if (!list) return null;
  const iso = list.last_refreshed_at ?? list.imported_at;
  const relative = formatRelativeDate(iso);
  if (!relative) return null;
  const full = new Date(iso).toLocaleString();
  return {
    relative,
    title: list.last_refreshed_at
      ? `Last refreshed ${full}`
      : `Imported ${full}`,
  };
}

/** Read: the segmented bar and "42 of 102" (lg). Below lg the In library
 *  and Updated columns fold into a caption here: "42 of 102 read · 16
 *  missing", then the date (its own line from sm, inline on phones). */
function ReadCell({
  stats,
  updated,
}: {
  stats: RowStats | null;
  updated: { relative: string; title: string } | null;
}) {
  if (!stats) return <Dash />;
  return (
    <div className="space-y-1">
      {stats.total > 0 ? <ListProgressBar stats={stats} /> : null}
      <p className="text-muted-foreground text-xs tabular-nums max-lg:hidden">
        <span className="sr-only">Read </span>
        {readLabel(stats)}
      </p>
      <p
        data-row-caption=""
        className="text-muted-foreground text-xs tabular-nums lg:hidden"
      >
        {readCaption(stats)}
        {stats.missing > 0 ? (
          <span className="text-warning"> · {stats.missing} missing</span>
        ) : null}
        {updated ? (
          <span className="sm:block" title={updated.title}>
            <span className="sm:hidden"> · </span>
            {updated.relative}
          </span>
        ) : null}
      </p>
    </div>
  );
}

/** In library: "102 of 118", and the missing count in the warning colour
 *  when there is one. */
function InLibraryCell({ stats }: { stats: RowStats | null }) {
  if (!stats) return <Dash />;
  return (
    <div className="text-xs tabular-nums">
      <p className="text-muted-foreground">
        <span className="sr-only">In library </span>
        {inLibraryLabel(stats)}
      </p>
      {stats.missing > 0 ? (
        <p data-row-missing="" className="text-warning">
          {stats.missing} missing
        </p>
      ) : null}
    </div>
  );
}

function Dash() {
  return <span className="text-muted-foreground text-xs">—</span>;
}

/** A small status icon with its meaning in a tooltip and, for assistive
 *  tech, in its name. */
function Flag({ icon: Icon, label }: { icon: LucideIcon; label: string }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          role="img"
          aria-label={label}
          className="text-muted-foreground inline-flex"
        >
          <Icon className="size-3.5" aria-hidden="true" />
        </span>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

/** The list's front cover, small (larger on phones); the list icon on a
 *  muted tile when it has none. Decorative: the name is the label. */
function RowCover({ viewId }: { viewId: string }) {
  const covers = useViewCovers();
  const url = covers.data?.get(viewId)?.[0] ?? null;
  return (
    <div aria-hidden="true" className="w-12 shrink-0 sm:w-7">
      {url ? (
        <Cover src={url} alt="" />
      ) : (
        <div className="bg-muted border-border text-muted-foreground grid aspect-[2/3] place-items-center rounded-md border">
          <List className="size-3.5" />
        </div>
      )}
    </div>
  );
}
