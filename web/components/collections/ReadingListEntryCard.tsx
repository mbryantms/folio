"use client";

/**
 * Linked reading lists (PR 5): a reading list kept in a collection. It
 * is a detour — read after the entry before it, never inlined into the
 * collection's order — so the tile reads as an event: covers, progress,
 * Start / Resume. Reading it carries the collection as `?via=`, so the
 * list's last issue returns to the collection.
 */
import * as React from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { ListOrdered, MoreHorizontal, Trash2 } from "lucide-react";
import { toast } from "sonner";

import { CoverStack } from "@/components/cover-stack/CoverStack";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/folio/dropdown-menu";
import { Progress } from "@/components/ui/progress";
import { useCblListEntriesInfinite } from "@/lib/api/queries";
import type { ReadingListCardView } from "@/lib/api/types";
import { progressLabel } from "@/lib/cbl/linked-plan";
import { displayListName, ensureListView } from "@/lib/reader/list-context";
import { readerUrl } from "@/lib/urls";

const COVERS = 3;

export function ReadingListEntryCard({
  list,
  collectionId,
  onRemove,
}: {
  list: ReadingListCardView;
  /** The collection's id, carried as `?via=`. */
  collectionId: string;
  onRemove: () => void;
}) {
  const router = useRouter();
  const [busy, setBusy] = React.useState(false);
  const covers = useCblListEntriesInfinite(list.list_id, { limit: COVERS });
  const urls = (covers.data?.pages[0]?.items ?? [])
    .map((i) => i.issue?.cover_url)
    .filter((u): u is string => !!u);
  const name = displayListName(list.name);
  const percent = list.total > 0 ? (list.read / list.total) * 100 : 0;
  const via = encodeURIComponent(collectionId);
  const go = async (to: (viewId: string) => string) => {
    setBusy(true);
    try {
      const view = await ensureListView(list.list_id, list.view_id);
      if (view) router.push(to(view));
      else toast.error(`Couldn't open ${name}. Try again.`);
    } finally {
      setBusy(false);
    }
  };
  const first = list.first_unread;
  return (
    <article
      aria-label={`Reading list: ${name}`}
      className="cover-stack-host border-border bg-card flex h-full flex-col gap-2 rounded-md border p-2"
    >
      <button
        type="button"
        onClick={() => void go((v) => `/views/${v}?via=${via}`)}
        disabled={busy}
        className="block w-full"
        aria-label={`Open ${name}`}
      >
        <CoverStack urls={urls} Icon={ListOrdered} spill={false} />
      </button>
      <div className="min-w-0">
        <p className="text-muted-foreground flex items-center gap-1 text-xs">
          <ListOrdered className="size-3" aria-hidden /> Reading list
        </p>
        <h3 className="truncate text-sm font-medium" title={name}>
          {list.view_id ? (
            <Link
              href={`/views/${list.view_id}?via=${via}`}
              className="hover:underline"
            >
              {name}
            </Link>
          ) : (
            name
          )}
        </h3>
        <Progress
          value={percent}
          className="mt-1.5 h-1"
          aria-label={`${name} progress`}
        />
        <p className="text-muted-foreground mt-1 text-xs tabular-nums">
          {progressLabel(list)}
        </p>
      </div>
      <div className="mt-auto flex items-center gap-1">
        {first ? (
          <Button
            size="sm"
            className="flex-1"
            disabled={busy}
            onClick={() =>
              void go((v) =>
                readerUrl(first.series_slug, first.issue_slug, {
                  cbl: v,
                  via: collectionId,
                }),
              )
            }
          >
            {list.read > 0 ? "Resume" : "Start"}
          </Button>
        ) : (
          <span className="text-muted-foreground flex-1 text-xs">
            {list.in_library === 0 ? "None in your library" : "All read"}
          </span>
        )}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={`Actions for ${name}`}
            >
              <MoreHorizontal className="size-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem variant="destructive" onSelect={onRemove}>
              <Trash2 className="size-4" /> Remove from this collection
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </article>
  );
}
