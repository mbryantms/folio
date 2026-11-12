/**
 * The grid every row of `GroupedViewsBoard`'s rows presentation shares:
 * the column header, folder rows and item rows each lay out on these
 * tracks, so a column lines up down the whole table without a `<table>`.
 * The board fills the grip cell (and the lead / main / menu cells of a
 * folder row); the caller's `renderRow` / `renderGroupCells` place their
 * cells with `ROW_CELL`.
 *
 * The tier comes from the board's own width, not the viewport's: the
 * board root is the `@container/board` query container, so the app
 * sidebar being open or closed changes nothing. Three tiers:
 *
 *                grip  lead   main             stat   stat2   menu
 *   wide  ≥42rem   ▪   cover  name             read   library ⋯
 *   medium ≥30rem  ▪   cover  name + caption   read           ⋯
 *   narrow         —   cover  name                            ⋯
 *                      (2 rows) read + caption, under the name
 *
 * Only the number columns have fixed tracks; everything else goes to the
 * name, whose track has a minimum so it can never collapse to a sliver.
 * Narrow drops the grip (press-and-hold drags the row, as on cards);
 * medium folds In library into a caption under the name.
 *
 * Every class is written out in full: Tailwind finds classes by scanning
 * the source.
 */
export const ROW_GRID = [
  "grid items-center gap-x-3 gap-y-1.5",
  "grid-cols-[auto_minmax(0,1fr)_auto]",
  "@min-[30rem]/board:gap-y-0 @min-[30rem]/board:grid-cols-[1.25rem_auto_minmax(10rem,1fr)_9rem_2rem]",
  "@min-[42rem]/board:grid-cols-[1.25rem_auto_minmax(12rem,1fr)_8rem_8rem_2rem]",
].join(" ");

/** Horizontal / vertical padding of a row inside the table. */
export const ROW_PAD = "px-2 py-2 @min-[30rem]/board:px-3";

export const ROW_CELL = {
  grip: "hidden @min-[30rem]/board:inline-flex @min-[30rem]/board:col-start-1 @min-[30rem]/board:row-start-1",
  /** Cover (item) or chevron + folder icon (folder), indented by the
   *  row's `--row-depth` (0 at the root, up to 4: a list in a fourth-level
   *  folder). Spans both narrow rows. The step shrinks with the tier so
   *  the deepest indent never eats the name's track: 0.5rem narrow (2rem
   *  at most, beside a ≥14rem name on a phone), 0.75rem medium (3rem at
   *  most: a 30rem table still fits the name's 10rem minimum), 1.25rem
   *  wide (5rem at most: a 42rem table still fits its 12rem minimum). */
  lead: [
    "col-start-1 row-start-1 row-span-2 self-start flex items-center gap-1",
    "pl-[calc(var(--row-depth,0)*0.5rem)]",
    "@min-[30rem]/board:col-start-2 @min-[30rem]/board:row-span-1 @min-[30rem]/board:self-center @min-[30rem]/board:pl-[calc(var(--row-depth,0)*0.75rem)]",
    "@min-[42rem]/board:pl-[calc(var(--row-depth,0)*1.25rem)]",
  ].join(" "),
  main: "col-start-2 row-start-1 min-w-0 @min-[30rem]/board:col-start-3",
  /** The one data column every tier shows (under the name when narrow). */
  stat: "col-start-2 col-span-2 row-start-2 min-w-0 @min-[30rem]/board:col-start-4 @min-[30rem]/board:col-span-1 @min-[30rem]/board:row-start-1",
  stat2:
    "hidden min-w-0 @min-[42rem]/board:block @min-[42rem]/board:col-start-5 @min-[42rem]/board:row-start-1",
  menu: "col-start-3 row-start-1 self-start justify-self-end @min-[30rem]/board:col-start-5 @min-[30rem]/board:row-start-1 @min-[30rem]/board:self-center @min-[42rem]/board:col-start-6",
} as const;

/** Show an element only in some tiers. */
export const TIER = {
  /** Under 30rem. */
  narrow: "@min-[30rem]/board:hidden",
  /** Under 42rem. */
  belowWide: "@min-[42rem]/board:hidden",
  /** 30rem to 42rem. */
  medium: "hidden @min-[30rem]/board:block @min-[42rem]/board:hidden",
  /** 30rem and up. */
  mediumUp: "hidden @min-[30rem]/board:block",
  /** 42rem and up, as a block / as a flex row. */
  wide: "hidden @min-[42rem]/board:block",
  wideFlex: "hidden @min-[42rem]/board:flex",
} as const;

/** The query container the tiers read; on the board root. */
export const BOARD_CONTAINER = "@container/board";
