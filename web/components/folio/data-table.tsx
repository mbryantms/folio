"use client";

import * as React from "react";
import {
  columnVisibilityFeature,
  createCoreRowModel,
  createSortedRowModel,
  flexRender,
  rowSortingFeature,
  sortFns,
  tableFeatures,
  useTable,
  type ColumnDef,
  type Row,
  type RowData,
  type SortingState,
} from "@tanstack/react-table";
import { ArrowDown, ArrowUp, ChevronRight, ChevronsUpDown } from "lucide-react";

import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { cn } from "@/lib/utils";

// v9 features are opt-in and tree-shakeable, so the table declares exactly what
// this wrapper renders: sortable headers (`rowSortingFeature` + the `sortFns`
// registry the default comparators resolve through) and `getVisibleCells`
// (`columnVisibilityFeature`). Row models are slots on the same object.
// Deliberately absent: `rowSelectionFeature` — nothing here selects rows — and
// `rowExpandingFeature`, since expansion is local `expanded` state below rather
// than table state.
const dataTableFeatures = tableFeatures({
  rowSortingFeature,
  columnVisibilityFeature,
  coreRowModel: createCoreRowModel(),
  sortedRowModel: createSortedRowModel(),
  sortFns,
});

/** The feature set every `DataTable` column def is bound to. */
export type DataTableFeatures = typeof dataTableFeatures;

/**
 * Column def for a `DataTable`. v9 threads the feature set through as
 * `ColumnDef`'s first type argument; this alias keeps that an implementation
 * detail so call sites stay `DataTableColumn<MyView>[]`.
 */
export type DataTableColumn<TData extends RowData> = ColumnDef<
  DataTableFeatures,
  TData
>;

/** Row handed to `renderExpanded`. */
export type DataTableRow<TData extends RowData> = Row<DataTableFeatures, TData>;

export interface DataTableProps<TData extends RowData> {
  columns: DataTableColumn<TData>[];
  data: TData[];
  /** Render an expanded row body. When provided, rows toggle on click. */
  renderExpanded?: (row: DataTableRow<TData>) => React.ReactNode;
  emptyMessage?: string;
}

export function DataTable<TData extends RowData>({
  columns,
  data,
  renderExpanded,
  emptyMessage = "No rows.",
}: DataTableProps<TData>) {
  const [sorting, setSorting] = React.useState<SortingState>([]);
  const [expanded, setExpanded] = React.useState<Record<string, boolean>>({});
  // Stable prefix for `aria-controls` wiring between an expander button and
  // the detail row it discloses.
  const tableId = React.useId();
  // The expander adds a leading column, so empty/detail rows must span it too.
  const totalCols = columns.length + (renderExpanded ? 1 : 0);
  // v8's `useReactTable` returned non-memoizable functions, so React Compiler
  // had to skip this component (`react-hooks/incompatible-library`). v9 reads
  // through a store, so no opt-out is needed here any more.
  const table = useTable<DataTableFeatures, TData>({
    features: dataTableFeatures,
    data,
    columns,
    state: { sorting },
    onSortingChange: setSorting,
  });

  return (
    <div className="border-border bg-card rounded-md border">
      <Table>
        <TableHeader>
          {table.getHeaderGroups().map((headerGroup) => (
            <TableRow key={headerGroup.id}>
              {renderExpanded ? (
                <TableHead className="w-9">
                  <span className="sr-only">Expand row</span>
                </TableHead>
              ) : null}
              {headerGroup.headers.map((header) => {
                const canSort = header.column.getCanSort();
                const sortState = header.column.getIsSorted();
                const SortIcon =
                  sortState === "asc"
                    ? ArrowUp
                    : sortState === "desc"
                      ? ArrowDown
                      : ChevronsUpDown;
                const ariaSort = canSort
                  ? sortState === "asc"
                    ? "ascending"
                    : sortState === "desc"
                      ? "descending"
                      : "none"
                  : undefined;
                return (
                  <TableHead key={header.id} aria-sort={ariaSort}>
                    {header.isPlaceholder ? null : canSort ? (
                      <button
                        type="button"
                        onClick={header.column.getToggleSortingHandler()}
                        className={cn(
                          "hover:text-foreground focus-visible:ring-ring focus-visible:ring-offset-background inline-flex max-w-full items-center gap-1 rounded-sm text-left font-medium outline-none focus-visible:ring-2 focus-visible:ring-offset-2",
                          sortState
                            ? "text-foreground"
                            : "text-muted-foreground",
                        )}
                      >
                        <span className="truncate">
                          {flexRender(
                            header.column.columnDef.header,
                            header.getContext(),
                          )}
                        </span>
                        <SortIcon className="size-3.5 shrink-0" />
                      </button>
                    ) : (
                      flexRender(
                        header.column.columnDef.header,
                        header.getContext(),
                      )
                    )}
                  </TableHead>
                );
              })}
            </TableRow>
          ))}
        </TableHeader>
        <TableBody>
          {table.getRowModel().rows?.length ? (
            table.getRowModel().rows.map((row) => {
              const isOpen = !!expanded[row.id];
              const detailId = `${tableId}-detail-${row.id}`;
              const toggle = () =>
                setExpanded((prev) => ({ ...prev, [row.id]: !prev[row.id] }));
              return (
                <React.Fragment key={row.id}>
                  <TableRow
                    // Whole-row click stays a mouse affordance; the
                    // keyboard / screen-reader path is the real button in
                    // the leading cell below (audit E8).
                    onClick={renderExpanded ? toggle : undefined}
                    className={renderExpanded ? "cursor-pointer" : undefined}
                  >
                    {renderExpanded ? (
                      <TableCell className="w-9 align-middle">
                        <button
                          type="button"
                          // Stop the bubble so the row's onClick doesn't
                          // also fire and cancel this toggle.
                          onClick={(e) => {
                            e.stopPropagation();
                            toggle();
                          }}
                          aria-expanded={isOpen}
                          aria-controls={isOpen ? detailId : undefined}
                          aria-label={isOpen ? "Collapse row" : "Expand row"}
                          className="text-muted-foreground hover:text-foreground focus-visible:ring-ring inline-flex size-7 items-center justify-center rounded-md focus-visible:ring-2 focus-visible:outline-none"
                        >
                          <ChevronRight
                            aria-hidden="true"
                            className={cn(
                              "size-4 transition-transform",
                              isOpen && "rotate-90",
                            )}
                          />
                        </button>
                      </TableCell>
                    ) : null}
                    {row.getVisibleCells().map((cell) => (
                      <TableCell key={cell.id}>
                        {flexRender(
                          cell.column.columnDef.cell,
                          cell.getContext(),
                        )}
                      </TableCell>
                    ))}
                  </TableRow>
                  {renderExpanded && isOpen ? (
                    <TableRow id={detailId}>
                      <TableCell colSpan={totalCols} className="bg-muted/30">
                        {renderExpanded(row)}
                      </TableCell>
                    </TableRow>
                  ) : null}
                </React.Fragment>
              );
            })
          ) : (
            <TableRow>
              <TableCell
                colSpan={totalCols}
                className="text-muted-foreground h-24 text-center"
              >
                {emptyMessage}
              </TableCell>
            </TableRow>
          )}
        </TableBody>
      </Table>
    </div>
  );
}
