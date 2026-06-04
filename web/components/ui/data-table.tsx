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
import { ArrowDown, ArrowUp, ChevronsUpDown } from "lucide-react";

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
              return (
                <React.Fragment key={row.id}>
                  <TableRow
                    data-state={row.getIsSelected() ? "selected" : undefined}
                    onClick={
                      renderExpanded
                        ? () =>
                            setExpanded((prev) => ({
                              ...prev,
                              [row.id]: !prev[row.id],
                            }))
                        : undefined
                    }
                    className={renderExpanded ? "cursor-pointer" : undefined}
                  >
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
                    <TableRow>
                      <TableCell
                        colSpan={columns.length}
                        className="bg-muted/30"
                      >
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
                colSpan={columns.length}
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
