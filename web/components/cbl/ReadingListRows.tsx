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
import { ROW_CELL, TIER } from "@/components/saved-views/board-rows";
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
  compareByRefreshed,
  gapParts,
  inLibraryLabel,
  readCaption,
  readLabel,
  rollupStats,
  type RowStats,
} from "@/lib/cbl/list-rows";
import { formatRelativeDate } from "@/lib/format";
import { cn } from "@/lib/utils";

/**
 * The reading lists as rows of `GroupedViewsBoard`'s tree table: one
 * header (Name · Read · In library), folders with roll-ups, a row per
 * list. `ROW_CELL` places each cell; see `board-rows.ts` for what each
 * tier shows. There is no date column: the list's date is in the
 * tooltip of its In library figures, and "Recently updated" in the sort
 * menu orders by it. `lists` is the complete `/me/cbl-lists` response by
 * list id (`null` while it loads), which is also what the sorts read.
 */
export function readingListRows(
  lists: Map<string, CblListView> | null,
): BoardRowsPresentation {
  const listOf = (view: SavedViewView) =>
    (view.cbl_list_id ? lists?.get(view.cbl_list_id) : null) ?? null;
  const statsOf = (view: SavedViewView): RowStats | null =>
    listOf(view)?.stats ?? null;
  return {
    densityKey: "folio.readingLists.density",
    columns: [
      { key: "read", label: "Read", cell: ROW_CELL.stat, sort: "board:read" },
      { key: "library", label: "In library", cell: ROW_CELL.stat2 },
    ],
    sorts: [
      {
        value: "board:read",
        label: "Most read",
        compare: compareByRead(statsOf),
      },
      {
        value: "board:refreshed",
        label: "Recently updated",
        compare: compareByRefreshed((v) => listDate(listOf(v))),
      },
    ],
    // The view row's own `updated_at` is not the list's date; the
    // reading lists' "Recently updated" above replaces it.
    hiddenSorts: ["updated"],
    renderRow: (view, ctx) => <ReadingListRowCells view={view} ctx={ctx} />,
    renderGroupCells: (_group, views) => (
      <FolderCells views={views} statsOf={statsOf} />
    ),
  };
}

function listDate(list: CblListView | null): string | null {
  return list ? (list.last_refreshed_at ?? list.imported_at) : null;
}

/** "Updated 3 days ago" / "Imported 12 Mar 2025": the list's date, in
 *  the words the old card used to tell a refresh from an import. */
function dateTitle(list: CblListView | null): string | undefined {
  const iso = listDate(list);
  const relative = formatRelativeDate(iso);
  if (!list || !iso || !relative) return undefined;
  const full = new Date(iso).toLocaleDateString(undefined, {
    day: "numeric",
    month: "short",
    year: "numeric",
  });
  const verb = list.last_refreshed_at ? "Updated" : "Imported";
  return relative === full
    ? `${verb} ${full}`
    : `${verb} ${relative} (${full})`;
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
  const title = dateTitle(list);
  const pinnedCount = (view.pinned_on_pages ?? []).length;
  const builtIn = isBuiltInView(view, false);
  const flags =
    builtIn || pinnedCount > 0 || view.show_in_sidebar ? (
      // Only on rows they apply to, right after the name; a row without
      // any gives the room to its name. Dropped when narrow (the menu
      // says the same).
      <span className="ml-1.5 hidden gap-1 align-[-0.125em] @min-[30rem]/board:inline-flex">
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
      </span>
    ) : null;

  return (
    <>
      <div className={ROW_CELL.lead}>
        <RowCover viewId={view.id} />
      </div>

      {/* The whole name / description area opens the list.
          Comfortable: the full name, the description under it (two
          lines). Compact (wide tier): one line when name and
          description fit side by side; a name that does not fit wraps
          to a second line and pushes the description out. Narrow: two
          lines each. */}
      <Link
        href={`/views/${view.id}`}
        draggable={false}
        data-row-link=""
        className={cn(
          ROW_CELL.main,
          "focus-visible:ring-ring flex flex-col gap-0.5 rounded focus-visible:ring-2 focus-visible:outline-none",
          compact &&
            "@min-[42rem]/board:max-h-10 @min-[42rem]/board:flex-row @min-[42rem]/board:flex-wrap @min-[42rem]/board:items-baseline @min-[42rem]/board:gap-x-2 @min-[42rem]/board:gap-y-0 @min-[42rem]/board:overflow-hidden",
        )}
      >
        <span
          data-row-name=""
          title={view.name}
          className={cn(
            "text-foreground line-clamp-2 text-sm font-medium break-words @min-[30rem]/board:line-clamp-none",
            compact &&
              "@min-[42rem]/board:line-clamp-2 @min-[42rem]/board:min-w-0",
          )}
        >
          {view.name}
          {flags}
        </span>
        {view.description ? (
          <span
            data-row-description=""
            title={view.description}
            className={cn(
              "text-muted-foreground line-clamp-2 text-xs",
              compact &&
                "@min-[42rem]/board:block @min-[42rem]/board:min-w-24 @min-[42rem]/board:flex-1 @min-[42rem]/board:basis-0 @min-[42rem]/board:truncate",
            )}
          >
            {view.description}
          </span>
        ) : null}
      </Link>

      <div className={ROW_CELL.stat}>
        <ReadCell stats={stats} title={title} />
      </div>
      <div className={ROW_CELL.stat2}>
        <InLibraryCell stats={stats} title={title} />
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

/** A folder row's roll-ups over every list in it and in the folders
 *  under it. */
function FolderCells({
  views,
  statsOf,
}: {
  views: SavedViewView[];
  statsOf: (view: SavedViewView) => RowStats | null;
}) {
  const known = views.some((v) => statsOf(v) !== null);
  const stats = known ? rollupStats(views.map(statsOf)) : null;
  return (
    <>
      <div className={ROW_CELL.stat}>
        <ReadCell stats={stats} />
      </div>
      <div className={ROW_CELL.stat2}>
        <InLibraryCell stats={stats} />
      </div>
    </>
  );
}

/** Read: the segmented bar, then "42 of 102" (wide). Below the wide tier
 *  the In library column folds in here: "42 of 102 read · 16 missing ·
 *  3 to match". */
function ReadCell({
  stats,
  title,
}: {
  stats: RowStats | null;
  title?: string;
}) {
  if (!stats) return <Dash />;
  return (
    <div className="space-y-1">
      {stats.total > 0 ? <ListProgressBar stats={stats} /> : null}
      <p
        className={cn("text-muted-foreground text-xs tabular-nums", TIER.wide)}
      >
        <span className="sr-only">Read: </span>
        {readLabel(stats)}
      </p>
      <p
        data-row-caption=""
        title={title}
        className={cn(
          "text-muted-foreground text-xs tabular-nums",
          TIER.belowWide,
        )}
      >
        {[readCaption(stats), ...gapParts(stats)].map((part, i) => (
          <React.Fragment key={part}>
            {i > 0 ? " · " : null}
            <span className={i > 0 ? "text-warning" : undefined}>{part}</span>
          </React.Fragment>
        ))}
      </p>
    </div>
  );
}

/** In library: "102 of 118 · 16 missing · 3 to match" on one line where
 *  it fits, the gaps in the warning colour when there are any. The list's
 *  date is its tooltip. */
function InLibraryCell({
  stats,
  title,
}: {
  stats: RowStats | null;
  title?: string;
}) {
  if (!stats) return <Dash />;
  const gaps = gapParts(stats);
  return (
    <p className="text-muted-foreground text-xs tabular-nums" title={title}>
      <span className="sr-only">In library: </span>
      {inLibraryLabel(stats)}
      {gaps.map((part) => (
        <React.Fragment key={part}>
          {" · "}
          <span data-row-gap="" className="text-warning">
            {part}
          </span>
        </React.Fragment>
      ))}
    </p>
  );
}

function Dash() {
  return <span className="text-muted-foreground text-xs">—</span>;
}

/** A small status icon with its meaning in a tooltip and, for assistive
 *  tech, in its name. Not a tab stop: the menu says the same. */
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

/** The list's front cover, small (larger when narrow); the list icon on
 *  a muted tile when it has none. Decorative: the name is the label. */
function RowCover({ viewId }: { viewId: string }) {
  const covers = useViewCovers();
  const url = covers.data?.get(viewId)?.[0] ?? null;
  return (
    <div aria-hidden="true" className="w-12 shrink-0 @min-[30rem]/board:w-7">
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
