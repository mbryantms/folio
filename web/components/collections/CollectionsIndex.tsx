"use client";

import * as React from "react";
import Link from "next/link";
import { BookmarkPlus, Folder, Plus } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { useCollections } from "@/lib/api/queries";
import { useCreateCollection } from "@/lib/api/mutations";
import type { SavedViewView } from "@/lib/api/types";

const WANT_TO_READ_KEY = "want_to_read";

/** Grid of the user's collections + "New collection" button. Want to
 *  Read is pinned to the front of the list with a distinct icon so the
 *  built-in collection is visually separated from user-curated lists. */
export function CollectionsIndex() {
  const collectionsQ = useCollections();
  const [createOpen, setCreateOpen] = React.useState(false);

  const collections = collectionsQ.data ?? [];

  return (
    <div className="space-y-6">
      <header className="flex flex-wrap items-end justify-between gap-3">
        <div className="space-y-1">
          <h1 className="text-2xl font-semibold tracking-tight">Collections</h1>
          <p className="text-muted-foreground text-sm">
            Manual reading lists of mixed series and issues. Use the kebab menu
            on any cover to add items.
          </p>
        </div>
        <Button type="button" onClick={() => setCreateOpen(true)}>
          <Plus className="mr-1 h-4 w-4" /> New collection
        </Button>
      </header>

      {collectionsQ.isLoading ? (
        <div className="text-muted-foreground py-12 text-sm">Loading…</div>
      ) : collectionsQ.isError ? (
        <div className="text-destructive rounded-md border p-4 text-sm">
          Failed to load collections.
        </div>
      ) : collections.length === 0 ? (
        <EmptyState onCreate={() => setCreateOpen(true)} />
      ) : (
        <ul
          role="list"
          className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4"
        >
          {collections.map((collection) => (
            <li key={collection.id}>
              <CollectionCard collection={collection} />
            </li>
          ))}
        </ul>
      )}

      <NewCollectionDialog open={createOpen} onOpenChange={setCreateOpen} />
    </div>
  );
}

function CollectionCard({ collection }: { collection: SavedViewView }) {
  const isWantToRead = collection.system_key === WANT_TO_READ_KEY;
  const Icon = isWantToRead ? BookmarkPlus : Folder;
  return (
    <Link
      href={`/views/${isWantToRead ? "want-to-read" : collection.id}`}
      className="group hover:bg-accent/40 focus-visible:ring-ring border-border/60 flex h-full flex-col gap-2 rounded-lg border p-4 transition-colors focus-visible:ring-2 focus-visible:outline-none"
    >
      <div className="flex items-start justify-between gap-3">
        <Icon
          className="text-muted-foreground h-5 w-5 shrink-0"
          aria-hidden="true"
        />
        {isWantToRead ? (
          <span className="text-muted-foreground bg-muted/40 inline-flex items-center rounded-md px-2 py-0.5 text-[10px] font-medium tracking-wider uppercase">
            Built-in
          </span>
        ) : null}
      </div>
      <div className="min-w-0 space-y-0.5">
        <div className="truncate font-medium" title={collection.name}>
          {collection.name}
        </div>
        {collection.description ? (
          <p className="text-muted-foreground line-clamp-2 text-sm">
            {collection.description}
          </p>
        ) : null}
      </div>
    </Link>
  );
}

function EmptyState({ onCreate }: { onCreate: () => void }) {
  return (
    <div className="border-border/60 rounded-lg border border-dashed p-10 text-center">
      <Folder
        className="text-muted-foreground/60 mx-auto h-10 w-10"
        aria-hidden="true"
      />
      <h2 className="mt-4 text-base font-medium">No collections yet</h2>
      <p className="text-muted-foreground mx-auto mt-1 max-w-sm text-sm">
        Group series and issues into reading lists. You can also add to a
        collection from any cover&rsquo;s kebab menu.
      </p>
      <Button type="button" className="mt-4" onClick={onCreate}>
        <Plus className="mr-1 h-4 w-4" /> Create your first collection
      </Button>
    </div>
  );
}

function NewCollectionDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
}) {
  const create = useCreateCollection();
  const [name, setName] = React.useState("");
  const [description, setDescription] = React.useState("");

  React.useEffect(() => {
    if (open) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setName("");
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setDescription("");
    }
  }, [open]);

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) {
      toast.error("Name is required");
      return;
    }
    create.mutate(
      { name: trimmed, description },
      {
        onSuccess: () => {
          toast.success(`Collection "${trimmed}" created`);
          onOpenChange(false);
        },
      },
    );
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>New collection</DialogTitle>
          <DialogDescription>
            Collections are manual lists of mixed series and issues.
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="space-y-2">
            <Label htmlFor="new-collection-name">Name</Label>
            <Input
              id="new-collection-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. My Capes"
              autoFocus
              required
              maxLength={200}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="new-collection-description">Description</Label>
            <Textarea
              id="new-collection-description"
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              rows={3}
              placeholder="Optional"
            />
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="ghost"
              onClick={() => onOpenChange(false)}
              disabled={create.isPending}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={create.isPending}>
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
