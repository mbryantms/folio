"use client";

import * as React from "react";
import { Folder, Plus } from "lucide-react";
import { toast } from "sonner";

import { PageHeader } from "@/components/admin/PageHeader";
import { CollectionCardSkeleton } from "@/components/collections/CollectionCardSkeleton";
import { GroupedViewsBoard } from "@/components/saved-views/GroupedViewsBoard";
import { ViewCard } from "@/components/saved-views/ViewsIndex";
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
import { collectServerErrors } from "@/lib/api/form-errors";
import { useCollections } from "@/lib/api/queries";
import { useCreateCollection } from "@/lib/api/mutations";
import { TOAST } from "@/lib/api/toast-strings";
import type { SavedViewView } from "@/lib/api/types";

/** Standalone collections index — the day-to-day browse surface for the
 *  user's collections, reached from the main-UI sidebar "Collections"
 *  link (`/collections`). The same grouped, drag-to-arrange board as
 *  Settings → Views → Collections (one shared order and groups, `scope =
 *  "collection"`), with large cover-led cards: drag a card to reorder or
 *  regroup; its ⋯ menu pins, shows in the sidebar, moves between groups
 *  and deletes (with Undo). */
export function CollectionsIndex() {
  const collectionsQ = useCollections();
  const [createOpen, setCreateOpen] = React.useState(false);
  // The sidebar shares the collections query, so the cache can already
  // hold them when this route's segment hydrates; render the server's
  // (loading) markup until hydrated, or React reports a mismatch.
  const hydrated = React.useSyncExternalStore(
    noopSubscribe,
    () => true,
    () => false,
  );
  const collections = hydrated ? (collectionsQ.data ?? []) : [];

  return (
    <div className="space-y-6">
      <PageHeader
        title="Collections"
        description="Manual reading lists of mixed series and issues. Drag cards to arrange them; add items from the ⋯ menu on any cover."
      />
      {collectionsQ.isError ? (
        <div className="text-destructive rounded-md border p-4 text-sm">
          Failed to load collections.
        </div>
      ) : (
        <GroupedViewsBoard
          scope="collection"
          size="large"
          views={collections}
          loading={!hydrated || collectionsQ.isLoading}
          blurb="Hand-picked series and issues."
          noun="collection"
          icon={Folder}
          action={
            <Button type="button" onClick={() => setCreateOpen(true)}>
              <Plus className="mr-1 size-4" /> New collection
            </Button>
          }
          renderCard={renderCollectionCard}
          empty={<EmptyState onCreate={() => setCreateOpen(true)} />}
        />
      )}
      <NewCollectionDialog open={createOpen} onOpenChange={setCreateOpen} />
    </div>
  );
}

const noopSubscribe = () => () => {};

function renderCollectionCard(view: SavedViewView) {
  return <ViewCard view={view} Icon={Folder} isCollection size="large" />;
}

/** Fixed-width cards (15rem) rather than fluid columns, so a wide screen
 *  adds more cards per row instead of blowing each one up. Phones keep
 *  two flexible columns. Larger than the compact Settings → Views cards
 *  (the covers are the point here) but a predictable size. */
export { CollectionCardSkeleton };

const COLLECTION_GRID =
  "grid grid-cols-2 gap-3 sm:grid-cols-[repeat(auto-fill,15rem)] sm:gap-4";

/** The collection card grid in its loading state — same responsive columns
 *  as the real `<ul>` so nothing reflows when data arrives. */
export function CollectionsGridSkeleton({ count = 8 }: { count?: number }) {
  return (
    <ul role="list" aria-hidden className={COLLECTION_GRID}>
      {Array.from({ length: count }, (_, i) => (
        <li key={i}>
          <CollectionCardSkeleton />
        </li>
      ))}
    </ul>
  );
}

function EmptyState({ onCreate }: { onCreate: () => void }) {
  return (
    // The board draws the dashed frame around this.
    <div className="py-4 text-center">
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
        <Plus className="mr-1 size-4" /> Create your first collection
      </Button>
    </div>
  );
}

/** Create-collection dialog. Used by the standalone `/collections` index
 *  and by the Collections tab of the `/settings/views` manager. */
export function NewCollectionDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
}) {
  const create = useCreateCollection();
  const [name, setName] = React.useState("");
  const [description, setDescription] = React.useState("");
  // Server 422 field errors (`error.details`) bound under their inputs;
  // the mutation hook still toasts the summary.
  const [errors, setErrors] = React.useState<Record<string, string>>({});

  React.useEffect(() => {
    if (open) {
      // Reset form fields each time the dialog reopens.
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setName("");
      setDescription("");
      setErrors({});
    }
  }, [open]);

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) {
      toast.error(TOAST.NAME_REQUIRED);
      return;
    }
    setErrors({});
    create.mutate(
      { name: trimmed, description },
      {
        onSuccess: () => {
          toast.success(`Collection "${trimmed}" created`);
          onOpenChange(false);
        },
        onError: (err) => setErrors(collectServerErrors(err, ["name"])),
      },
    );
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-xl">
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
              aria-invalid={errors.name ? true : undefined}
              aria-describedby={
                errors.name ? "new-collection-name-error" : undefined
              }
            />
            {errors.name ? (
              <p
                id="new-collection-name-error"
                className="text-destructive text-sm"
              >
                {errors.name}
              </p>
            ) : null}
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
