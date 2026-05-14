"use client";

import * as React from "react";
import { ArrowLeft, BookmarkPlus, Folder, Plus, Search } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
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
import { queryKeys, useCollections } from "@/lib/api/queries";
import {
  apiMutate,
  useAddCollectionEntry,
  useCreateCollection,
  useRemoveCollectionEntry,
} from "@/lib/api/mutations";
import { TOAST } from "@/lib/api/toast-strings";
import { cn } from "@/lib/utils";
import type {
  AddEntryReq,
  CollectionEntryKind,
  CollectionEntryView,
  SavedViewView,
} from "@/lib/api/types";

const WANT_TO_READ_KEY = "want_to_read";

export type AddToCollectionTarget = {
  entry_kind: CollectionEntryKind;
  ref_id: string;
  /** Display label used in toast messages, e.g. "Saga" or "Saga #1". */
  label: string;
};

/** Modal for adding a single series or issue to a collection. Two-mode
 *  flow inside one Dialog: "pick" lists the user's existing collections
 *  with a sticky "Create new collection" footer; "create" swaps to a
 *  name input that chains `useCreateCollection` → server add on submit.
 *  The 409-`already_in_collection` server response surfaces as the
 *  standard error toast through `useApiMutation`. */
export function AddToCollectionDialog({
  open,
  onOpenChange,
  target,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  target: AddToCollectionTarget;
}) {
  const collectionsQ = useCollections();
  const [mode, setMode] = React.useState<"pick" | "create">("pick");
  const [search, setSearch] = React.useState("");

  // Reset transient state each time the dialog opens to avoid leaking
  // half-typed names across invocations. Standard reset-on-prop-change
  // pattern — there's no derived-state alternative here because the
  // dialog body is mounted continuously and we still need to drop the
  // search filter when the user reopens it.
  React.useEffect(() => {
    if (open) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setMode("pick");
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setSearch("");
    }
  }, [open]);

  const all = collectionsQ.data ?? [];
  // Always show Want to Read first; the rest land alpha by name (server
  // already sorts that way, but re-sort defensively in case future
  // changes alter the order).
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
            {mode === "pick" ? "Add to collection" : "New collection"}
          </DialogTitle>
          <DialogDescription>
            {mode === "pick"
              ? `Add "${target.label}" to one of your collections.`
              : `Create a collection and add "${target.label}" to it.`}
          </DialogDescription>
        </DialogHeader>

        {mode === "pick" ? (
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
                  <PickRow
                    target={target}
                    collection={wantToRead}
                    onAdded={() => onOpenChange(false)}
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
                  <PickRow
                    target={target}
                    collection={collection}
                    onAdded={() => onOpenChange(false)}
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
            <Button
              type="button"
              variant="outline"
              onClick={() => setMode("create")}
              className="w-full justify-start"
            >
              <Plus className="mr-2 h-4 w-4" />
              Create new collection
            </Button>
          </div>
        ) : (
          <CreateForm
            target={target}
            initialName={search.trim()}
            onCancel={() => setMode("pick")}
            onCreated={() => onOpenChange(false)}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

function PickRow({
  target,
  collection,
  onAdded,
  icon,
}: {
  target: AddToCollectionTarget;
  collection: SavedViewView;
  onAdded: () => void;
  icon: React.ReactNode;
}) {
  const add = useAddCollectionEntry(collection.id);
  // Mirrors the IssueSettingsMenu / SeriesSettingsMenu / cover-menu
  // add-to-WTR pattern: every "Added to <name>" toast carries an Undo
  // action so the operation is one-click reversible.
  const remove = useRemoveCollectionEntry(collection.id);
  return (
    <button
      type="button"
      onClick={() => {
        add.mutate(
          { entry_kind: target.entry_kind, ref_id: target.ref_id },
          {
            onSuccess: (entry) => {
              if (!entry) {
                toast.success(`Added to ${collection.name}`);
                onAdded();
                return;
              }
              toast.success(`Added to ${collection.name}`, {
                action: {
                  label: "Undo",
                  onClick: () => remove.mutate({ entryId: entry.id }, {}),
                },
              });
              onAdded();
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

function CreateForm({
  target,
  initialName,
  onCancel,
  onCreated,
}: {
  target: AddToCollectionTarget;
  initialName: string;
  onCancel: () => void;
  onCreated: () => void;
}) {
  const qc = useQueryClient();
  // Silent so the chained "Added to X" toast below is the only success
  // signal — otherwise users get "Collection X created" *and* "Added to X"
  // for a single click.
  const create = useCreateCollection({ silent: true });
  const [name, setName] = React.useState(initialName);
  const [busy, setBusy] = React.useState(false);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) {
      toast.error(TOAST.NAME_REQUIRED);
      return;
    }
    setBusy(true);
    try {
      // mutateAsync resolves with the typed payload + still routes errors
      // through useApiMutation's toast wrappers.
      const created = await create.mutateAsync({ name: trimmed });
      if (!created) {
        // Treat null-return (204) as a server invariant violation — we
        // expect a 201 with the new row.
        throw new Error("Server didn't return the new collection");
      }
      // Chain the add via apiMutate (skips the hook indirection so we
      // don't need a second collection-scoped useAddCollectionEntry call
      // bound mid-flow). Errors propagate to the catch below.
      await apiMutate<CollectionEntryView>({
        path: `/me/collections/${created.id}/entries`,
        method: "POST",
        body: {
          entry_kind: target.entry_kind,
          ref_id: target.ref_id,
        } satisfies AddEntryReq,
      });
      qc.invalidateQueries({
        queryKey: ["collections", "entries", created.id],
      });
      qc.invalidateQueries({ queryKey: queryKeys.collections });
      // Undo discards the whole collection (not just the entry) since
      // the user's intent was "add to a new place" — leaving an empty
      // collection lying around isn't what they asked for. Uses apiMutate
      // directly because `useDeleteCollection(id)` binds at hook-call
      // time and we don't know the id until create resolves.
      toast.success(`Added to ${trimmed}`, {
        action: {
          label: "Undo",
          onClick: async () => {
            try {
              await apiMutate({
                path: `/me/collections/${created.id}`,
                method: "DELETE",
              });
              qc.invalidateQueries({ queryKey: queryKeys.collections });
              qc.invalidateQueries({ queryKey: ["saved-views"] });
            } catch {
              // apiMutate doesn't toast on error; swallow here too — the
              // undo affordance is best-effort. If it fails the user can
              // delete the stray collection from the catalog.
            }
          },
        },
      });
      onCreated();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-4">
      <div className="space-y-2">
        <Label htmlFor="new-collection-name">Collection name</Label>
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
      <DialogFooter className="flex items-center justify-between gap-2 sm:justify-between">
        <Button
          type="button"
          variant="ghost"
          onClick={onCancel}
          disabled={busy}
        >
          <ArrowLeft className="mr-1 h-4 w-4" /> Back
        </Button>
        <Button type="submit" disabled={busy}>
          Create &amp; add
        </Button>
      </DialogFooter>
    </form>
  );
}
