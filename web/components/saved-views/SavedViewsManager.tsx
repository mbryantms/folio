"use client";

import * as React from "react";
import Link from "next/link";
import { Lock, Pencil, Trash2 } from "lucide-react";

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
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import {
  useDeleteSavedView,
  usePinSavedView,
  useSidebarSavedView,
} from "@/lib/api/mutations";
import { useSavedViews } from "@/lib/api/queries";
import type { SavedViewView } from "@/lib/api/types";

import { AddViewButton } from "./AddViewButton";

const PIN_CAP = 12;

/** Per-user saved-views **catalog**. Lives at `/settings/views`. Pure
 *  CRUD surface — create / open-to-edit / delete. Per-row Switches
 *  for "On home" and "In sidebar" expose the most common cross-flow
 *  without forcing a trip to `/settings/navigation`, but pin order and
 *  sidebar arrangement live exclusively on that page. */
export function SavedViewsManager() {
  const viewsQ = useSavedViews();
  const all = React.useMemo(
    () => viewsQ.data?.items ?? [],
    [viewsQ.data?.items],
  );
  const sorted = React.useMemo(() => {
    // Alphabetical, system views and user views interleaved. Same name
    // ordering the picker on `/settings/navigation` uses, so the user
    // can find a row in one place and recognize its position in the
    // other.
    return [...all].sort((a, b) =>
      a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
    );
  }, [all]);
  const pinnedCount = all.filter((v) => v.pinned).length;
  const atPinCap = pinnedCount >= PIN_CAP;

  if (viewsQ.isLoading) {
    return (
      <div className="text-muted-foreground py-12 text-sm">Loading views…</div>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-muted-foreground text-sm">
          {sorted.length} view{sorted.length === 1 ? "" : "s"}.{" "}
          <Link
            href="/settings/navigation"
            className="text-foreground underline underline-offset-2"
          >
            Reorder home rails and the sidebar
          </Link>
          .
        </p>
        <AddViewButton />
      </div>

      {sorted.length === 0 ? (
        <div className="border-border/60 text-muted-foreground rounded-md border border-dashed p-4 text-sm">
          No saved views yet. Click <strong>Add view</strong> to create one.
        </div>
      ) : (
        <ul className="border-border/60 divide-border/60 divide-y rounded-lg border">
          {sorted.map((view) => (
            <ViewRow key={view.id} view={view} atPinCap={atPinCap} />
          ))}
        </ul>
      )}
    </div>
  );
}

function ViewRow({
  view,
  atPinCap,
}: {
  view: SavedViewView;
  atPinCap: boolean;
}) {
  const pin = usePinSavedView();
  const sidebar = useSidebarSavedView();
  const del = useDeleteSavedView(view.id);
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const isCbl = view.kind === "cbl";

  return (
    <li className="bg-background flex items-center gap-3 px-3 py-2">
      <div className="flex min-w-0 flex-1 items-center gap-2">
        <Link
          href={`/views/${view.id}`}
          className="hover:text-foreground truncate text-sm font-medium"
          title={view.name}
        >
          {view.name}
        </Link>
        {view.is_system ? (
          <span
            className="text-muted-foreground bg-muted/40 inline-flex shrink-0 items-center rounded-md border px-2 py-0.5 text-xs"
            title="Built-in views can't be edited or deleted"
          >
            <Lock className="mr-1 h-3 w-3" /> Built-in
          </span>
        ) : null}
      </div>

      {/* Edit + Delete render first (left of the toggles) so the
       *  trailing `[On home] [In sidebar]` pair lines up on the right
       *  edge across every row, regardless of whether a row carries
       *  Edit/Delete (user views) or not (system views). */}
      {view.is_system ? null : (
        <>
          <Button type="button" variant="ghost" size="sm" asChild>
            <Link href={`/views/${view.id}`} title="Open and edit">
              <Pencil className="h-4 w-4 sm:mr-1" />
              <span className="hidden sm:inline">Edit</span>
            </Link>
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            onClick={() => setConfirmOpen(true)}
            aria-label="Delete view"
            className="text-destructive hover:text-destructive"
          >
            <Trash2 className="h-4 w-4" />
          </Button>
          <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Delete this view?</AlertDialogTitle>
                <AlertDialogDescription>
                  {isCbl
                    ? "Removes the saved view but keeps the underlying CBL list. You can re-import or re-pin later."
                    : "Removes the filter view permanently."}
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Cancel</AlertDialogCancel>
                <AlertDialogAction onClick={() => del.mutate()}>
                  Delete
                </AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        </>
      )}

      <ToggleControl
        label="On home"
        checked={view.pinned}
        disabled={!view.pinned && atPinCap}
        title={
          !view.pinned && atPinCap
            ? `Pin cap reached (${PIN_CAP}). Unpin one to add another.`
            : undefined
        }
        onCheckedChange={(next) => pin.mutate({ id: view.id, pinned: next })}
      />
      <ToggleControl
        label="In sidebar"
        checked={view.show_in_sidebar}
        onCheckedChange={(next) =>
          sidebar.mutate({ id: view.id, show: next })
        }
      />
    </li>
  );
}

/** Compact Switch + label cluster for the per-row cross-flow toggles
 *  ("On home", "In sidebar"). Visually de-emphasized so it reads as a
 *  secondary affordance against Edit/Delete; primary management lives
 *  on `/settings/navigation`. */
function ToggleControl({
  label,
  checked,
  disabled,
  title,
  onCheckedChange,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  title?: string;
  onCheckedChange: (next: boolean) => void;
}) {
  return (
    <label
      className="text-muted-foreground hidden shrink-0 cursor-pointer items-center gap-2 text-xs sm:inline-flex"
      title={title}
    >
      <span>{label}</span>
      <Switch
        checked={checked}
        disabled={disabled}
        onCheckedChange={onCheckedChange}
        aria-label={`${label} (${checked ? "on" : "off"})`}
      />
    </label>
  );
}
