"use client";

import * as React from "react";
import Link from "next/link";
import { Check, Loader2, Plus } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import {
  useImportStarterList,
  useStarterLists,
  type StarterListView,
} from "@/lib/api/starter-lists";
import { cn } from "@/lib/utils";

/** Order publishers by how much of their picks this library holds, so
 *  the shelf leads with what the reader can actually start today. */
export function groupStarterLists(items: readonly StarterListView[]) {
  const groups = new Map<string, StarterListView[]>();
  for (const item of items) {
    const g = groups.get(item.publisher) ?? [];
    g.push(item);
    groups.set(item.publisher, g);
  }
  const score = (list: StarterListView[]) =>
    list.reduce((n, i) => n + Number(i.available), 0);
  return [...groups.entries()]
    .map(([publisher, lists]) => ({
      publisher,
      lists: [...lists].sort(
        (a, b) => Number(b.available > 0) - Number(a.available > 0),
      ),
    }))
    .sort((a, b) => score(b.lists) - score(a.lists));
}

/**
 * The starter reading-list shelf: the tour's last step. Each pick says
 * why it's a good first read and how much of it this library has (only
 * issues the reader can open). "Add" imports it as a reading list in one
 * click; an already-added pick links to its list instead.
 */
export function StarterListsPanel() {
  const q = useStarterLists();
  const importList = useImportStarterList();
  const [pending, setPending] = React.useState<string | null>(null);
  const items = q.data?.items ?? [];
  const anyAvailable = items.some((i) => Number(i.available) > 0);
  const [onlyAvailable, setOnlyAvailable] = React.useState(true);
  const filtered =
    anyAvailable && onlyAvailable
      ? items.filter(
          (i) =>
            Number(i.available) > 0 ||
            i.imported_view_id ||
            // Still being checked: keep it until we know.
            i.coverage_status === "pending",
        )
      : items;
  const groups = groupStarterLists(filtered);

  if (q.isLoading) {
    return (
      <div className="space-y-2" aria-busy="true">
        {Array.from({ length: 4 }, (_, i) => (
          <Skeleton key={i} className="h-16 w-full" />
        ))}
      </div>
    );
  }
  if (q.isError) {
    return (
      <p className="text-destructive text-sm">
        Couldn&rsquo;t load the starter lists. You can import reading lists
        later from Settings → Views.
      </p>
    );
  }
  if (items.length === 0) {
    return (
      <p className="text-muted-foreground text-sm">
        No starter lists are set up on this server. You can import reading lists
        any time from Settings → Views.
      </p>
    );
  }

  return (
    <div className="space-y-4">
      {anyAvailable ? (
        <label className="text-muted-foreground flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={onlyAvailable}
            onChange={(e) => setOnlyAvailable(e.target.checked)}
            className="accent-primary size-4"
          />
          Only lists with issues in this library
        </label>
      ) : null}
      {groups.map((g) => (
        <section key={g.publisher} aria-label={g.publisher}>
          <h3 className="text-muted-foreground mb-1.5 text-[11px] font-semibold tracking-widest uppercase">
            {g.publisher}
          </h3>
          <ul
            role="list"
            className="divide-border/60 divide-y rounded-md border"
          >
            {g.lists.map((item) => {
              const key = `${item.catalog_source_id}:${item.catalog_path}`;
              return (
                <li key={key} className="flex items-start gap-3 p-3">
                  <div className="min-w-0 flex-1 space-y-1">
                    <div className="font-medium">{item.title}</div>
                    {item.reason ? (
                      <p className="text-muted-foreground text-sm">
                        {item.reason}
                      </p>
                    ) : null}
                    <Coverage item={item} />
                  </div>
                  {item.imported_view_id ? (
                    <Button asChild size="sm" variant="outline">
                      <Link href={`/views/${item.imported_view_id}`}>
                        <Check className="mr-1 size-4" /> Open
                      </Link>
                    </Button>
                  ) : (
                    <Button
                      type="button"
                      size="sm"
                      disabled={pending !== null}
                      aria-label={`Add ${item.title} to your reading lists`}
                      onClick={() => {
                        setPending(key);
                        importList.mutate(item, {
                          onSettled: () => setPending(null),
                        });
                      }}
                    >
                      {pending === key ? (
                        <Loader2 className="mr-1 size-4 animate-spin" />
                      ) : (
                        <Plus className="mr-1 size-4" />
                      )}
                      Add
                    </Button>
                  )}
                </li>
              );
            })}
          </ul>
        </section>
      ))}
    </div>
  );
}

function Coverage({ item }: { item: StarterListView }) {
  if (item.coverage_status === "pending") {
    return (
      <p className="text-muted-foreground flex items-center gap-1.5 text-xs">
        <Loader2 className="size-3 animate-spin" aria-hidden="true" />
        Checking this library…
      </p>
    );
  }
  if (item.coverage_status === "error" && item.total === 0) {
    return (
      <p className="text-muted-foreground text-xs">
        Couldn&rsquo;t check this library right now.
      </p>
    );
  }
  const available = Number(item.available);
  const pct =
    item.total > 0
      ? Math.min(100, Math.round((available / item.total) * 100))
      : 0;
  return (
    <div className="flex items-center gap-2">
      <div
        className="bg-muted h-1.5 w-24 overflow-hidden rounded-full"
        role="meter"
        aria-valuemin={0}
        aria-valuemax={item.total}
        aria-valuenow={available}
        aria-label={`${available} of ${item.total} issues in this library`}
      >
        <div
          className={cn(
            "h-full rounded-full",
            pct >= 75 ? "bg-primary" : "bg-primary/60",
          )}
          style={{ width: `${pct}%` }}
        />
      </div>
      <span className="text-muted-foreground text-xs tabular-nums">
        {available === 0
          ? `None of ${item.total} in this library yet`
          : `${available} of ${item.total} in this library`}
      </span>
    </div>
  );
}
