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
  invalidateCollectionEntries,
  useBulkAddToCollection,
  useCreateCollection,
  type BulkAddMembersResp,
} from "@/lib/api/mutations";
import { TOAST } from "@/lib/api/toast-strings";
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
 * Multi-select Tranche M3 (with v0.3.19 create-new extension): pick
 * one collection, add many items. Patterned after
 * `<AddToCollectionDialog>` (single-item) but shaped for the
 * multi-select toolbar's "Add to collection…" action.
 *
 * Differs from the single-item dialog in two ways:
 *   1. Accepts `targets: BulkAddTarget[]` instead of one target.
 *   2. Calls `useBulkAddToCollection` (POST /…/members/bulk-add)
 *      rather than per-item `useAddCollectionEntry`.
 *
 * Same two-mode flow as the single-item dialog: `"pick"` lists the
 * user's existing collections with a sticky "Create new collection"
 * footer; `"create"` swaps to a name input that chains
 * `useCreateCollection` → bulk-add on submit.
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
  const [mode, setMode] = React.useState<"pick" | "create">("pick");
  const [search, setSearch] = React.useState("");

  React.useEffect(() => {
    if (open) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setMode("pick");
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
            {mode === "pick"
              ? `Add ${targets.length} item${targets.length === 1 ? "" : "s"} to collection`
              : "New collection"}
          </DialogTitle>
          <DialogDescription>
            {mode === "pick"
              ? "Pick a collection. Items already in it are silently skipped."
              : `Create a collection and add ${targets.length} item${targets.length === 1 ? "" : "s"} to it.`}
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
          <BulkCreateForm
            targets={targets}
            initialName={search.trim()}
            onCancel={() => setMode("pick")}
            onCreated={() => onOpenChange(false)}
          />
        )}
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

function BulkCreateForm({
  targets,
  initialName,
  onCancel,
  onCreated,
}: {
  targets: BulkAddTarget[];
  initialName: string;
  onCancel: () => void;
  onCreated: () => void;
}) {
  const qc = useQueryClient();
  // Silent so the chained "N added to <name>" toast below is the only
  // success signal — same pattern as the single-item dialog's
  // `CreateForm` (otherwise we'd surface both "Collection X created"
  // *and* the bulk-add summary for a single click).
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
      const created = await create.mutateAsync({ name: trimmed });
      if (!created) {
        throw new Error("Server didn't return the new collection");
      }
      // Chain the bulk-add via apiMutate. `useBulkAddToCollection`
      // binds at hook-call time and we don't know the id until
      // create resolves — same indirection as the single-item
      // CreateForm uses.
      const summary = await apiMutate<BulkAddMembersResp>({
        path: `/me/collections/${created.id}/members/bulk-add`,
        method: "POST",
        body: { members: targets },
      });
      invalidateCollectionEntries(qc, created.id);
      qc.invalidateQueries({ queryKey: queryKeys.collections });

      const addedCount = summary?.added ?? targets.length;
      toast.success(`${addedCount} added to ${trimmed}`, {
        // Undo discards the whole collection — the user's intent was
        // "add to a new place"; leaving an empty collection isn't
        // what they asked for. Mirrors single-item CreateForm.
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
              // best-effort; user can delete the stray collection
              // from the catalog if this fails.
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
        <Label htmlFor="bulk-new-collection-name">Collection name</Label>
        <Input
          id="bulk-new-collection-name"
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
