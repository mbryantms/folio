"use client";

import * as React from "react";
import Link from "next/link";
import type { ColumnDef } from "@tanstack/react-table";
import { Search } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable } from "@/components/ui/data-table";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { useUserList, type UserListFilters } from "@/lib/api/queries";
import type { AdminUserView } from "@/lib/api/types";
import { cn } from "@/lib/utils";

type RoleFilter = "all" | "admin" | "user";
type StateFilter = "all" | "active" | "disabled" | "pending_verification";

function roleVariant(role: string): "default" | "secondary" {
  return role === "admin" ? "default" : "secondary";
}

function stateVariant(state: string): "default" | "secondary" | "destructive" {
  if (state === "disabled") return "destructive";
  if (state === "pending_verification") return "secondary";
  return "default";
}

export function UserTable() {
  const [role, setRole] = React.useState<RoleFilter>("all");
  const [state, setState] = React.useState<StateFilter>("all");
  const [q, setQ] = React.useState("");
  const [search, setSearch] = React.useState("");
  const [cursor, setCursor] = React.useState<string | undefined>(undefined);
  // History stack of cursors so the user can paginate backwards.
  const [history, setHistory] = React.useState<(string | undefined)[]>([]);

  React.useEffect(() => {
    const t = setTimeout(() => {
      setSearch(q.trim());
      setCursor(undefined);
      setHistory([]);
    }, 250);
    return () => clearTimeout(t);
  }, [q]);

  // Reset the cursor whenever the *filter* (not the cursor) changes. Comparing
  // the previous filter inline avoids a useEffect → cascading-render warning.
  const filterKey = `${role}|${state}`;
  const [prevFilterKey, setPrevFilterKey] = React.useState(filterKey);
  if (filterKey !== prevFilterKey) {
    setPrevFilterKey(filterKey);
    setCursor(undefined);
    setHistory([]);
  }

  const filters: UserListFilters = React.useMemo(
    () => ({
      role: role === "all" ? undefined : role,
      state: state === "all" ? undefined : state,
      q: search || undefined,
      cursor,
      limit: 50,
    }),
    [role, state, search, cursor],
  );

  const { data, isLoading, error, isFetching } = useUserList(filters);

  const columns = React.useMemo<ColumnDef<AdminUserView>[]>(
    () => [
      {
        accessorKey: "email",
        header: "Email",
        cell: ({ row }) => (
          <Link
            href={`/admin/users/${row.original.id}`}
            className="text-foreground font-medium hover:underline"
            onClick={(e) => e.stopPropagation()}
          >
            {row.original.email ?? "—"}
          </Link>
        ),
      },
      {
        accessorKey: "display_name",
        header: "Display name",
        cell: ({ row }) => (
          <span className="text-sm">{row.original.display_name}</span>
        ),
      },
      {
        accessorKey: "role",
        header: "Role",
        cell: ({ row }) => (
          <Badge variant={roleVariant(row.original.role)} className="uppercase">
            {row.original.role}
          </Badge>
        ),
      },
      {
        accessorKey: "state",
        header: "State",
        cell: ({ row }) => (
          <Badge
            variant={stateVariant(row.original.state)}
            className="uppercase"
          >
            {row.original.state.replace("_", " ")}
          </Badge>
        ),
      },
      {
        accessorKey: "last_login_at",
        header: "Last login",
        cell: ({ row }) =>
          row.original.last_login_at ? (
            <span className="text-muted-foreground text-xs">
              {new Date(row.original.last_login_at).toLocaleString()}
            </span>
          ) : (
            <span className="text-muted-foreground text-xs">Never</span>
          ),
      },
      {
        accessorKey: "library_count",
        header: "Libraries",
        cell: ({ row }) => {
          if (row.original.role === "admin") {
            return <span className="text-muted-foreground text-xs">All</span>;
          }
          return (
            <span className="text-muted-foreground text-xs">
              {row.original.library_count}
            </span>
          );
        },
      },
    ],
    [],
  );

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-muted-foreground text-xs">Role:</span>
        {(["all", "admin", "user"] as RoleFilter[]).map((r) => (
          <button
            key={r}
            type="button"
            onClick={() => setRole(r)}
            className={cn(
              "rounded-full border px-3 py-1 text-xs font-medium tracking-wider uppercase transition-colors",
              role === r
                ? "border-primary bg-primary/10 text-primary"
                : "border-border text-muted-foreground hover:text-foreground",
            )}
          >
            {r}
          </button>
        ))}
        <span className="text-muted-foreground ml-2 text-xs">State:</span>
        {(
          ["all", "active", "disabled", "pending_verification"] as StateFilter[]
        ).map((s) => (
          <button
            key={s}
            type="button"
            onClick={() => setState(s)}
            className={cn(
              "rounded-full border px-2.5 py-0.5 text-[11px] tracking-wider uppercase transition-colors",
              state === s
                ? "border-foreground/40 text-foreground"
                : "border-border text-muted-foreground hover:text-foreground",
            )}
          >
            {s.replace("_", " ")}
          </button>
        ))}
        <div className="ml-auto flex items-center gap-2">
          <div className="relative">
            <Search className="text-muted-foreground absolute top-1/2 left-2 size-3.5 -translate-y-1/2" />
            <Input
              type="search"
              value={q}
              onChange={(e) => setQ(e.target.value)}
              placeholder="Search email or name…"
              className="h-8 w-56 pl-7"
            />
          </div>
        </div>
      </div>

      {isLoading ? (
        <Skeleton className="h-64 w-full" />
      ) : error ? (
        <p className="text-destructive text-sm">{error.message}</p>
      ) : (
        <DataTable
          columns={columns}
          data={data?.items ?? []}
          emptyMessage="No users match the current filters."
        />
      )}

      <div className="text-muted-foreground flex items-center justify-end gap-2 text-xs">
        <Button
          size="sm"
          variant="ghost"
          disabled={history.length === 0 || isFetching}
          onClick={() => {
            setHistory((prev) => {
              const next = [...prev];
              const back = next.pop();
              setCursor(back);
              return next;
            });
          }}
        >
          ← Previous
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={!data?.next_cursor || isFetching}
          onClick={() => {
            setHistory((prev) => [...prev, cursor]);
            setCursor(data?.next_cursor ?? undefined);
          }}
        >
          Next →
        </Button>
      </div>
    </div>
  );
}
