"use client";

/**
 * Linked reading lists (PR 2b): the dialogs behind the list page's link
 * actions — insert a reading list after an issue (or as the next part),
 * confirm "mark read up to here", and confirm removing a link.
 */
import * as React from "react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { countCblReadThrough } from "@/lib/api/mutations";
import { useCblLists } from "@/lib/api/queries";
import type { CblListView } from "@/lib/api/types";

/** Lists the insert picker may offer for `parent`: never the list
 *  itself, and for a shared list only shared lists (the server refuses
 *  the rest). Pure, for tests. */
export function linkableLists(
  all: readonly CblListView[],
  parent: Pick<CblListView, "id" | "owner_user_id">,
  alreadyLinked: ReadonlySet<string>,
): CblListView[] {
  return all.filter(
    (l) =>
      l.id !== parent.id &&
      !alreadyLinked.has(l.id) &&
      (parent.owner_user_id != null || l.owner_user_id == null),
  );
}

export function InsertListDialog({
  open,
  onOpenChange,
  parent,
  afterLabel,
  alreadyLinked,
  hasNextPart,
  onPick,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  parent: Pick<CblListView, "id" | "owner_user_id">;
  /** "Excalibur #12" — `null` when inserting only as the next part. */
  afterLabel: string | null;
  alreadyLinked: ReadonlySet<string>;
  hasNextPart: boolean;
  onPick: (childListId: string, kind: "event" | "continues") => void;
}) {
  const lists = useCblLists();
  const [kind, setKind] = React.useState<"event" | "continues">(
    afterLabel ? "event" : "continues",
  );
  const options = linkableLists(lists.data?.items ?? [], parent, alreadyLinked);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Link a reading list</DialogTitle>
          <DialogDescription>
            {kind === "event" && afterLabel
              ? `Read it after ${afterLabel}. It stays its own list with its own progress.`
              : "Read it after this whole list ends."}
          </DialogDescription>
        </DialogHeader>
        <ToggleGroup
          type="single"
          variant="outline"
          value={kind}
          onValueChange={(v) => v && setKind(v as "event" | "continues")}
          aria-label="Link as"
        >
          <ToggleGroupItem value="event" disabled={!afterLabel}>
            Event here
          </ToggleGroupItem>
          <ToggleGroupItem value="continues" disabled={hasNextPart}>
            Next part
          </ToggleGroupItem>
        </ToggleGroup>
        <Command className="border-border rounded-md border">
          <CommandInput placeholder="House of M" />
          <CommandList className="max-h-72">
            <CommandEmpty>
              {lists.isLoading ? "Loading lists…" : "No lists to link."}
            </CommandEmpty>
            <CommandGroup>
              {options.map((l) => (
                <CommandItem
                  key={l.id}
                  value={`${l.parsed_name} ${l.id}`}
                  onSelect={() => onPick(l.id, kind)}
                >
                  <span className="truncate">{l.parsed_name}</span>
                  <span className="text-muted-foreground ml-auto shrink-0 text-xs">
                    {l.stats.total} issues
                  </span>
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </DialogContent>
    </Dialog>
  );
}

/** Counts first (dry run), then asks. */
export function MarkThroughDialog({
  listId,
  target,
  onOpenChange,
  onConfirm,
  isPending,
}: {
  listId: string;
  /** Position + label of the issue to mark through; `null` = closed. */
  target: { position: number; label: string } | null;
  onOpenChange: (open: boolean) => void;
  onConfirm: (position: number) => void;
  isPending: boolean;
}) {
  const [count, setCount] = React.useState<number | null>(null);
  const position = target?.position;
  React.useEffect(() => {
    if (position == null) return;
    let live = true;
    countCblReadThrough(listId, position)
      .then((r) => live && setCount(r?.marked ?? 0))
      .catch(() => live && setCount(null));
    return () => {
      live = false;
      setCount(null);
    };
  }, [listId, position]);
  return (
    <AlertDialog open={target != null} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Mark read up to here?</AlertDialogTitle>
          <AlertDialogDescription>
            {count == null
              ? "Counting the unread issues up to "
              : count === 0
                ? "Everything you own up to "
                : `${count} unread ${count === 1 ? "issue" : "issues"} up to and including `}
            {target?.label}
            {count === 0 ? " is already read." : "."} Events linked into this
            list aren&rsquo;t touched.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>
            {count === 0 ? "Close" : "Cancel"}
          </AlertDialogCancel>
          {count !== 0 && (
            <AlertDialogAction
              disabled={count == null || isPending}
              onClick={() => target && onConfirm(target.position)}
            >
              Mark {count ?? ""} read
            </AlertDialogAction>
          )}
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

export function RemoveLinkDialog({
  name,
  onOpenChange,
  onConfirm,
}: {
  /** Linked list's name; `null` = closed. */
  name: string | null;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  return (
    <AlertDialog open={name != null} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Remove {name} from this list?</AlertDialogTitle>
          <AlertDialogDescription>
            The list itself and your progress in it stay. Only its place in this
            list is removed.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction variant="destructive" onClick={onConfirm}>
            Remove
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
