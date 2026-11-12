/**
 * The grid every row of `GroupedViewsBoard`'s rows presentation shares:
 * the column header, folder rows and item rows each lay out on these
 * tracks, so a column lines up down the whole table without a `<table>`.
 * The board fills the grip cell (and the lead / main / menu cells of a
 * folder row); the caller's `renderRow` / `renderGroupCells` place their
 * cells with `ROW_CELL`.
 *
 *              grip  lead   main   stat   stat2  stat3  flags  menu
 *   lg  (≥1024)  ▪    cover  name   read   lib    date   icons  ⋯
 *   sm  (≥640)   ▪    cover  name   read+caption        icons  ⋯
 *   phone        —    cover  name                              ⋯
 *                     (2 rows) read + caption, under the name
 *
 * Phones drop the grip (press-and-hold drags the row, as on cards) and
 * the flag icons; `stat2` / `stat3` only exist from `lg`, below which
 * the caller folds them into a caption in `stat`.
 */
export const ROW_GRID = [
  "grid items-center gap-x-3 gap-y-1.5",
  "grid-cols-[auto_minmax(0,1fr)_auto]",
  "sm:gap-y-0 sm:grid-cols-[1.25rem_auto_minmax(0,1fr)_11rem_2.75rem_2rem]",
  "lg:grid-cols-[1.25rem_auto_minmax(0,1fr)_8.5rem_7rem_5.5rem_2.75rem_2rem]",
].join(" ");

/** Horizontal / vertical padding of a row inside the table. */
export const ROW_PAD = "px-2 py-2 sm:px-3";

export const ROW_CELL = {
  grip: "max-sm:hidden sm:col-start-1 sm:row-start-1",
  /** Cover (item) or chevron + folder icon (folder), indented by the
   *  row's `--row-depth`. Spans both phone rows. */
  lead: [
    "col-start-1 row-start-1 row-span-2 self-start flex items-center gap-1",
    "pl-[calc(var(--row-depth,0)*0.75rem)]",
    "sm:col-start-2 sm:row-span-1 sm:self-center sm:pl-[calc(var(--row-depth,0)*1.75rem)]",
  ].join(" "),
  main: "col-start-2 row-start-1 min-w-0 sm:col-start-3",
  /** The one data column every width shows (under the name on phones). */
  stat: "col-start-2 col-span-2 row-start-2 min-w-0 sm:col-start-4 sm:col-span-1 sm:row-start-1",
  stat2: "hidden min-w-0 lg:col-start-5 lg:row-start-1 lg:block",
  stat3: "hidden min-w-0 lg:col-start-6 lg:row-start-1 lg:block",
  flags:
    "max-sm:hidden sm:col-start-5 sm:row-start-1 lg:col-start-7 flex items-center gap-1.5",
  menu: "col-start-3 row-start-1 self-start justify-self-end sm:col-start-6 sm:row-start-1 sm:self-center lg:col-start-8",
} as const;
