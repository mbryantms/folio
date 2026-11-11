"use client";

import * as React from "react";
import {
  MoreHorizontal,
  PanelLeft,
  PanelLeftClose,
  Pencil,
  Pin,
  Share2,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/folio/dropdown-menu";
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
import { useShareReadingList, useSidebarSavedView } from "@/lib/api/mutations";
import { useMe } from "@/lib/api/queries";
import type { SavedViewView } from "@/lib/api/types";

import { MultiPinDialog } from "./MultiPinDialog";

/** The saved view's `⋯` More-actions menu: Edit, caller-supplied
 *  `extraMenuItems` (Export, Refresh, …), Sidebar toggle, Pin. A
 *  dropdown at every width (phone-layout rule 2: `⋯` is an action
 *  list, never a settings sheet). `ViewHeader` renders it at the end of
 *  the title row by default; a page that passes `actions="none"` to
 *  the header renders it itself — the CBL page puts it in its pinned
 *  control row so the title row is title-only.
 *
 *  Owns its own Pin dialog state, so a page may mount two instances
 *  (one per breakpoint composition) without sharing state. */
export function ViewActionsMenu({
  view,
  onEdit,
  extraMenuItems,
}: {
  view: SavedViewView;
  onEdit?: () => void;
  extraMenuItems?: React.ReactNode;
}) {
  const sidebar = useSidebarSavedView();
  const [pinOpen, setPinOpen] = React.useState(false);
  const pinnedCount = (view.pinned_on_pages ?? []).length;
  const me = useMe();
  const isAdmin = me.data?.role === "admin";
  // Shared reading lists are system views that admins
  // still manage; built-in system rails stay uneditable.
  const canEdit =
    onEdit !== undefined &&
    (!view.is_system || (isAdmin && view.kind === "cbl"));
  const [shareOpen, setShareOpen] = React.useState(false);
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="outline"
            size="icon"
            // Toolbar-row convention: h-9 square button matches
            // the adjacent search input + CoverSizeOptions trigger.
            className="h-9 w-9"
            aria-label="More actions"
            title="More actions"
          >
            <MoreHorizontal className="size-4" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="min-w-[10rem]">
          <DropdownMenuLabel>Actions</DropdownMenuLabel>
          <DropdownMenuSeparator />
          {canEdit ? (
            <DropdownMenuItem
              onSelect={(e) => {
                e.preventDefault();
                onEdit?.();
              }}
            >
              <Pencil className="mr-2 size-4" /> Edit
            </DropdownMenuItem>
          ) : null}
          {extraMenuItems}
          {isAdmin && view.kind === "cbl" && !view.is_system ? (
            // Let the menu close; the confirm dialog opens on its own.
            <DropdownMenuItem onSelect={() => setShareOpen(true)}>
              <Share2 className="mr-2 size-4" /> Share with everyone…
            </DropdownMenuItem>
          ) : null}
          <DropdownMenuItem
            onSelect={(e) => {
              e.preventDefault();
              sidebar.mutate({
                id: view.id,
                show: !view.show_in_sidebar,
              });
            }}
          >
            {view.show_in_sidebar ? (
              <>
                <PanelLeftClose className="mr-2 size-4" /> Hide from sidebar
              </>
            ) : (
              <>
                <PanelLeft className="mr-2 size-4" /> Show in sidebar
              </>
            )}
          </DropdownMenuItem>
          <DropdownMenuItem
            onSelect={(e) => {
              e.preventDefault();
              setPinOpen(true);
            }}
          >
            <Pin className="mr-2 size-4" />
            {pinnedCount === 0
              ? "Pin to pages…"
              : pinnedCount === 1
                ? "Pinned to 1 page"
                : `Pinned to ${pinnedCount} pages`}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <MultiPinDialog view={view} open={pinOpen} onOpenChange={setPinOpen} />
      {isAdmin && view.kind === "cbl" && !view.is_system ? (
        <ShareReadingListDialog
          view={view}
          open={shareOpen}
          onOpenChange={setShareOpen}
        />
      ) : null}
    </>
  );
}

function ShareReadingListDialog({
  view,
  open,
  onOpenChange,
}: {
  view: SavedViewView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const share = useShareReadingList(view.id);
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            Share “{view.name}” with everyone?
          </AlertDialogTitle>
          <AlertDialogDescription>
            Every user will see this reading list under Reading Lists and can
            pin it, but only admins can edit, refresh or delete it. Sharing
            can&rsquo;t be undone from here.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            onClick={() =>
              share.mutate(undefined, { onSuccess: () => onOpenChange(false) })
            }
          >
            Share
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
