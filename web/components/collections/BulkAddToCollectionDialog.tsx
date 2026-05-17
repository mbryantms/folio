"use client";

import * as React from "react";
import { BookmarkPlus, Folder, Search } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { useCollections } from "@/lib/api/queries";
import { useBulkAddToCollection } from "@/lib/api/mutations";
import { cn } from "@/lib/utils";
import type {
  CollectionEntryKind,
  SavedViewView,
} from "@/lib/api/types";

const WANT_TO_READ_KEY = "want_to_read";

export type BulkAddTarget = {
  entry_kind: CollectionEntryKind;
  ref_id: string;
};

/**
 * Multi-select Tranche M3: pick one collection, add many items.
 * Patterned after `<AddToCollectionDialog>` (single-item) but
 * shaped for the multi-select toolbar's "Add to collection…" action.
 *
 * Differs from the single-item dialog in three ways:
 *   1. Accepts `targets: BulkAddTarget[]` instead of one target.
 *   2. Calls `useBulkAddToCollection` (POST /…/members/bulk-add)
 *      rather than per-item `useAddCollectionEntry`.
 *   3. Doesn't offer the "Create new collection" inline path —
 *      multi-select is for organizing into *existing* buckets;
 *      creating a new collection just to dump N items into it is
 *      a less common flow and adding the create-then-add chain
 *      doubles the dialog's surface area. Users who need a new
 *      target can create the collection from `/collections`
 *      first, then re-enter select mode. Future revisit if users
 *      ask.
 *
 * Plan: `~/.claude/plans/multi-select-bulk-actions-1.0.md` (M3).
 */
export function BulkAddToCollectionDialog({
  open,
  onOpenChange,
  targets,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  targets: BulkAddTarget[];
}) {
  const collectionsQ = useCollections();
  const [search, setSearch] = React.useState("");

  React.useEffect(() => {
    if (open) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setSearch("");
    }
  }, [open]);

  const all = collectionsQ.data ?? [];
  const wantToRead = all.find((c) => c.system_key === WANT_TO_READ_KEY);
  const others = all.filter((c) => c.system_key !== WANT_TO_READ_KEY);
  const needle = search.trim().toLowerCase();
  const filtered = needle
    ? others.filter((c) => c.name.toLowerCase().includes(needle))
    : others;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>
            Add {targets.length} item{targets.length === 1 ? "" : "s"} to
            collection
          </DialogTitle>
          <DialogDescription>
            Pick a collection. Items already in it are silently skipped.
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-3">
          <div className="relative">
            <Search className="text-muted-foreground absolute top-2.5 left-2.5 h-4 w-4" />
            <Input
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Search collections"
              className="pl-9"
              autoFocus
            />
          </div>
          <ul
            role="list"
            className="border-border/60 divide-border/60 max-h-72 divide-y overflow-y-auto rounded-md border"
          >
            {wantToRead && !needle ? (
              <li>
                <BulkPickRow
                  targets={targets}
                  collection={wantToRead}
                  onDone={() => onOpenChange(false)}
                  icon={
                    <BookmarkPlus
                      className="text-muted-foreground h-4 w-4 shrink-0"
                      aria-hidden="true"
                    />
                  }
                />
              </li>
            ) : null}
            {filtered.map((collection) => (
              <li key={collection.id}>
                <BulkPickRow
                  targets={targets}
                  collection={collection}
                  onDone={() => onOpenChange(false)}
                  icon={
                    <Folder
                      className="text-muted-foreground h-4 w-4 shrink-0"
                      aria-hidden="true"
                    />
                  }
                />
              </li>
            ))}
            {filtered.length === 0 &&
            (needle || !wantToRead) &&
            !collectionsQ.isLoading ? (
              <li className="text-muted-foreground px-3 py-4 text-center text-sm">
                No collections{needle ? " match" : " yet"}.
              </li>
            ) : null}
          </ul>
          <div className="flex items-center justify-end">
            <Button
              type="button"
              variant="ghost"
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function BulkPickRow({
  targets,
  collection,
  onDone,
  icon,
}: {
  targets: BulkAddTarget[];
  collection: SavedViewView;
  onDone: () => void;
  icon: React.ReactNode;
}) {
  const add = useBulkAddToCollection(collection.id);
  return (
    <button
      type="button"
      onClick={() => {
        add.mutate(
          { members: targets },
          {
            onSuccess: () => {
              onDone();
            },
          },
        );
      }}
      disabled={add.isPending}
      className={cn(
        "hover:bg-accent/40 focus-visible:bg-accent/40 flex w-full items-center gap-3 px-3 py-2 text-left text-sm transition-colors focus-visible:outline-none disabled:opacity-60",
      )}
    >
      {icon}
      <span className="min-w-0 flex-1 truncate">{collection.name}</span>
    </button>
  );
}
