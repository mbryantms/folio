"use client";

import * as React from "react";
import {
  MoreHorizontal,
  PanelLeft,
  PanelLeftClose,
  Pencil,
  Pin,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useSidebarSavedView } from "@/lib/api/mutations";
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
  const canEdit = onEdit !== undefined && !view.is_system;
  const pinnedCount = (view.pinned_on_pages ?? []).length;
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="outline"
            size="icon"
            // Toolbar-row convention: h-9 square button matches
            // the adjacent search input + CardSizeOptions trigger.
            className="h-9 w-9"
            aria-label="More actions"
            title="More actions"
          >
            <MoreHorizontal className="h-4 w-4" />
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
              <Pencil className="mr-2 h-4 w-4" /> Edit
            </DropdownMenuItem>
          ) : null}
          {extraMenuItems}
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
                <PanelLeftClose className="mr-2 h-4 w-4" /> Hide from sidebar
              </>
            ) : (
              <>
                <PanelLeft className="mr-2 h-4 w-4" /> Show in sidebar
              </>
            )}
          </DropdownMenuItem>
          <DropdownMenuItem
            onSelect={(e) => {
              e.preventDefault();
              setPinOpen(true);
            }}
          >
            <Pin className="mr-2 h-4 w-4" />
            {pinnedCount === 0
              ? "Pin to pages…"
              : pinnedCount === 1
                ? "Pinned to 1 page"
                : `Pinned to ${pinnedCount} pages`}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <MultiPinDialog view={view} open={pinOpen} onOpenChange={setPinOpen} />
    </>
  );
}
