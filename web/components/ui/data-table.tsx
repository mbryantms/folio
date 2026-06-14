"use client";

import * as React from "react";
import {
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  type ColumnDef,
  type SortingState,
  type Row,
  useReactTable,
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

export interface DataTableProps<TData, TValue> {
  columns: ColumnDef<TData, TValue>[];
  data: TData[];
  /** Render an expanded row body. When provided, rows toggle on click. */
  renderExpanded?: (row: Row<TData>) => React.ReactNode;
  emptyMessage?: string;
}

export function DataTable<TData, TValue>({
  columns,
  data,
  renderExpanded,
  emptyMessage = "No rows.",
}: DataTableProps<TData, TValue>) {
  const [sorting, setSorting] = React.useState<SortingState>([]);
  const [expanded, setExpanded] = React.useState<Record<string, boolean>>({});
  // Stable prefix for `aria-controls` wiring between an expander button and
  // the detail row it discloses.
  const tableId = React.useId();
  // The expander adds a leading column, so empty/detail rows must span it too.
  const totalCols = columns.length + (renderExpanded ? 1 : 0);
  // TanStack Table returns non-memoizable functions; React Compiler skips.
  // eslint-disable-next-line react-hooks/incompatible-library
  const table = useReactTable({
    data,
    columns,
    state: { sorting },
    onSortingChange: setSorting,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
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
                    data-state={row.getIsSelected() ? "selected" : undefined}
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
