"use client";

/**
 * "Add to Mylar" for a reading list's missing entries (add-to-mylar 1.0).
 *
 * Admin-only, and only once a Mylar address and key are stored
 * (`/admin/upcoming`). An entry is added one at a time from its row, or
 * in bulk: pick rows with Select, or take every missing entry of the
 * list. Folio queues the request; a worker sends it to Mylar, so the
 * row shows "Requested" first and "In Mylar" once Mylar accepted it.
 */

import { Check, Clock, Loader2, TriangleAlert } from "lucide-react";
import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { useAddToMylar } from "@/lib/api/mutations";
import { useMe, useMylarListStatus } from "@/lib/api/queries";
import type { CblEntryView } from "@/lib/api/types";
import {
  indexMylarStatus,
  mylarCanAdd,
  mylarEntryState,
  MYLAR_MAX_ENTRY_IDS,
  type MylarEntryState,
  type MylarStatusIndex,
} from "@/lib/cbl/mylar";

/** What the dialog is about to send. */
export type MylarAddTarget =
  | { kind: "all"; count: number }
  | { kind: "entries"; ids: string[]; label: string };

export type MylarForList = {
  /** The reader is an admin and Mylar is configured. */
  available: boolean;
  index: MylarStatusIndex;
  /** Missing entries with no ComicVine id (Mylar cannot take them). */
  withoutId: number;
};

/** Mylar state for a list's missing entries; inert for non-admins. */
export function useMylarForList(listId: string): MylarForList {
  const me = useMe();
  const isAdmin = me.data?.role === "admin";
  const status = useMylarListStatus(listId, { enabled: isAdmin });
  const index = React.useMemo(
    () => indexMylarStatus(status.data),
    [status.data],
  );
  return {
    available: isAdmin && status.data?.configured === true,
    index,
    withoutId: status.data?.without_comicvine_id ?? 0,
  };
}

/** The Mylar badge and/or button at the end of a missing entry's row. */
export function MylarRowControl({
  entry,
  index,
  onAdd,
}: {
  entry: CblEntryView;
  index: MylarStatusIndex;
  onAdd: () => void;
}) {
  const state = mylarEntryState(entry, index);
  return (
    <div className="flex shrink-0 items-center gap-2">
      <MylarStateBadge state={state} />
      {mylarCanAdd(state) ? (
        <Button type="button" size="sm" variant="outline" onClick={onAdd}>
          {state.kind === "failed"
            ? "Retry in Mylar"
            : state.kind === "series_in_mylar"
              ? "Want in Mylar"
              : "Add to Mylar"}
        </Button>
      ) : null}
    </div>
  );
}

function MylarStateBadge({ state }: { state: MylarEntryState }) {
  switch (state.kind) {
    case "no_id":
      return (
        <span
          className="text-muted-foreground text-xs"
          title="This entry has no ComicVine id, which Mylar needs."
        >
          No ComicVine id
        </span>
      );
    case "wanted":
      return (
        <Badge variant="secondary" className="gap-1">
          <Check className="size-3" aria-hidden /> Wanted in Mylar
        </Badge>
      );
    case "series_in_mylar":
      return (
        <Badge variant="outline" className="gap-1">
          <Check className="size-3" aria-hidden /> Series in Mylar
        </Badge>
      );
    case "requested":
      return (
        <Badge variant="outline" className="gap-1">
          <Clock className="size-3" aria-hidden /> Requested
        </Badge>
      );
    case "failed":
      return (
        <Badge
          variant="destructive"
          className="gap-1"
          title={state.error ?? undefined}
        >
          <TriangleAlert className="size-3" aria-hidden /> Mylar add failed
        </Badge>
      );
    case "addable":
      return null;
  }
}

/** The bar above the missing entries: Select, Add selected, Add all. */
export function MylarBar({
  missingCount,
  withoutId,
  selecting,
  selectedCount,
  onSelecting,
  onAddSelected,
  onAddAll,
}: {
  missingCount: number;
  withoutId: number;
  selecting: boolean;
  selectedCount: number;
  onSelecting: (on: boolean) => void;
  onAddSelected: () => void;
  onAddAll: () => void;
}) {
  const tooMany = selectedCount > MYLAR_MAX_ENTRY_IDS;
  return (
    <div
      className="border-border bg-muted/40 flex flex-wrap items-center gap-2 rounded-md border px-3 py-2"
      data-mylar-bar
    >
      <span className="text-sm font-medium">Mylar</span>
      <span className="text-muted-foreground min-w-0 flex-1 basis-40 text-xs">
        {selecting
          ? tooMany
            ? `Pick ${MYLAR_MAX_ENTRY_IDS} or fewer, or add all missing.`
            : `${selectedCount} selected`
          : withoutId > 0
            ? `${withoutId} of ${missingCount} missing ${
                missingCount === 1 ? "entry has" : "entries have"
              } no ComicVine id and cannot be added.`
            : "Add missing entries to Mylar's watchlist."}
      </span>
      {selecting ? (
        <>
          <Button
            type="button"
            size="sm"
            disabled={selectedCount === 0 || tooMany}
            onClick={onAddSelected}
          >
            Add {selectedCount > 0 ? selectedCount : ""} to Mylar…
          </Button>
          <Button
            type="button"
            size="sm"
            variant="ghost"
            onClick={() => onSelecting(false)}
          >
            Done
          </Button>
        </>
      ) : (
        <>
          <Button
            type="button"
            size="sm"
            variant="outline"
            onClick={() => onSelecting(true)}
          >
            Select
          </Button>
          <Button type="button" size="sm" variant="outline" onClick={onAddAll}>
            Add all {missingCount} missing…
          </Button>
        </>
      )}
    </div>
  );
}

/** Confirm a request and choose whether the issues are marked Wanted. */
export function MylarAddDialog({
  listId,
  target,
  onClose,
  onQueued,
}: {
  listId: string;
  target: MylarAddTarget | null;
  onClose: () => void;
  onQueued?: () => void;
}) {
  const add = useAddToMylar();
  const [wantIssues, setWantIssues] = React.useState(true);
  const submit = () => {
    if (!target) return;
    add.mutate(
      {
        cbl_list_id: listId,
        entry_ids: target.kind === "entries" ? target.ids : null,
        want_issues: wantIssues,
      },
      {
        onSuccess: () => {
          onQueued?.();
          onClose();
        },
      },
    );
  };
  const what =
    target == null
      ? ""
      : target.kind === "all"
        ? `all ${target.count} missing ${target.count === 1 ? "entry" : "entries"}`
        : target.label;
  return (
    <Dialog open={target != null} onOpenChange={(o) => (o ? null : onClose())}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add to Mylar</DialogTitle>
          <DialogDescription>
            Folio will add the series of {what} to Mylar&rsquo;s watchlist.
            Series already there are left alone.
          </DialogDescription>
        </DialogHeader>
        <div className="flex items-start gap-3">
          <Checkbox
            id="mylar-want-issues"
            checked={wantIssues}
            onCheckedChange={(v) => setWantIssues(v === true)}
            className="mt-0.5"
          />
          <div className="grid gap-1">
            <Label htmlFor="mylar-want-issues" className="cursor-pointer">
              Also mark these issues Wanted
            </Label>
            <p className="text-muted-foreground text-xs">
              Mylar then searches for each issue. Folio does this once Mylar has
              finished importing the series, which can take a while. Unticked,
              Mylar&rsquo;s own settings decide what it fetches.
            </p>
          </div>
        </div>
        <DialogFooter>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button type="button" onClick={submit} disabled={add.isPending}>
            {add.isPending ? (
              <Loader2 className="mr-2 size-4 animate-spin" />
            ) : null}
            Add to Mylar
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
