"use client";

import * as React from "react";
import { Library as LibraryIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Skeleton } from "@/components/ui/skeleton";
import { useLibraryList } from "@/lib/api/queries";
import { useUpdateLibraryAccess } from "@/lib/api/mutations";
import type { AdminUserDetailView } from "@/lib/api/types";
import {
  isDirty,
  selectionDiff,
  toggleSelection,
} from "./library-access-logic";

export function LibraryAccessMatrix({ user }: { user: AdminUserDetailView }) {
  const { data: libraries, isLoading } = useLibraryList();
  const update = useUpdateLibraryAccess(user.id);

  const original = React.useMemo(
    () => new Set(user.library_access.map((g) => g.library_id)),
    [user.library_access],
  );
  const [selected, setSelected] = React.useState<Set<string>>(original);
  // Reset local selection when the upstream `original` set changes (e.g.
  // after a save invalidates the query and we re-fetch). Comparing the prev
  // value during render avoids the cascading-render footgun of useEffect.
  const [prevOriginal, setPrevOriginal] = React.useState(original);
  if (original !== prevOriginal) {
    setPrevOriginal(original);
    setSelected(new Set(original));
  }

  const dirty = isDirty(original, selected);
  const diff = selectionDiff(original, selected);
  const isAdmin = user.role === "admin";

  if (isLoading) return <Skeleton className="h-64 w-full" />;

  if (!libraries || libraries.length === 0) {
    return (
      <p className="text-muted-foreground text-sm">
        No libraries exist yet. Create one before granting access.
      </p>
    );
  }

  return (
    <div className="space-y-4">
      {isAdmin ? (
        <p className="border-border bg-muted/40 text-muted-foreground rounded-md border px-3 py-2 text-xs">
          Admins implicitly have access to every library. Per-library grants
          here are stored but have no effect while the role stays{" "}
          <code className="font-mono">admin</code>.
        </p>
      ) : null}

      <ul className="divide-border border-border bg-card divide-y rounded-md border">
        {libraries.map((lib) => {
          const checked = selected.has(lib.id);
          return (
            <li
              key={lib.id}
              className="hover:bg-muted/40 flex items-center gap-3 px-4 py-2"
            >
              <Checkbox
                id={`lib-access-${lib.id}`}
                checked={checked}
                onCheckedChange={() =>
                  setSelected((prev) => toggleSelection(prev, lib.id))
                }
              />
              <label
                htmlFor={`lib-access-${lib.id}`}
                className="flex flex-1 cursor-pointer items-center gap-2 text-sm"
              >
                <LibraryIcon className="text-muted-foreground size-3.5" />
                <span className="font-medium">{lib.name}</span>
                <span className="text-muted-foreground truncate font-mono text-xs">
                  {lib.root_path}
                </span>
              </label>
            </li>
          );
        })}
      </ul>

      <div className="flex items-center justify-between">
        <p className="text-muted-foreground text-xs">
          {dirty
            ? `${diff.added.length} to grant · ${diff.removed.length} to revoke`
            : "No pending changes"}
        </p>
        <div className="flex gap-2">
          <Button
            variant="ghost"
            size="sm"
            disabled={!dirty || update.isPending}
            onClick={() => setSelected(new Set(original))}
          >
            Reset
          </Button>
          <Button
            size="sm"
            disabled={!dirty || update.isPending}
            onClick={() => update.mutate({ library_ids: Array.from(selected) })}
          >
            {update.isPending ? "Saving…" : "Save changes"}
          </Button>
        </div>
      </div>
    </div>
  );
}
