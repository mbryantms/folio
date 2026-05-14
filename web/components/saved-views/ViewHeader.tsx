"use client";

import * as React from "react";
import {
  Lock,
  MoreHorizontal,
  PanelLeft,
  PanelLeftClose,
  Pencil,
  Pin,
  PinOff,
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
import { usePinSavedView, useSidebarSavedView } from "@/lib/api/mutations";
import { cn } from "@/lib/utils";
import type { SavedViewView } from "@/lib/api/types";

/** Detail-page header shared by both filter and CBL view types.
 *
 *  Layout:
 *    [title]                                      [extraActions] [⋯]
 *
 *  `extraActions` are the inline controls that stay visible at-a-glance
 *  (stats pills, CardSizeOptions, badges). Everything else — Edit,
 *  Sidebar toggle, Pin/Unpin, and any caller-supplied `extraMenuItems`
 *  (Export, Refresh, …) — collapses into the `⋯` overflow menu. Mobile
 *  used to overflow with seven inline buttons; collapsing keeps the
 *  same UI on both viewports.
 *
 *  System views (`is_system = true`) omit Edit and surface a small
 *  read-only `Built-in` chip inline so the constraint is visible
 *  instead of just dimmed.
 */
export function ViewHeader({
  view,
  onEdit,
  extraActions,
  extraMenuItems,
  className,
}: {
  view: SavedViewView;
  onEdit?: () => void;
  /** Inline controls placed to the right of the title and left of the
   *  overflow menu. Use for stats, density toggles, badges — anything
   *  the user wants visible at a glance. */
  extraActions?: React.ReactNode;
  /** Items appended to the overflow menu *between* Edit and the
   *  Sidebar/Pin actions. Each item should be a `<DropdownMenuItem>`
   *  (or `<DropdownMenuSeparator>`). Caller-specific actions like
   *  Export and Refresh live here. */
  extraMenuItems?: React.ReactNode;
  className?: string;
}) {
  const pin = usePinSavedView();
  const sidebar = useSidebarSavedView();
  const canEdit = onEdit !== undefined && !view.is_system;
  return (
    <header className={cn("space-y-3", className)}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 space-y-1">
          {/* `md:truncate` only — on narrow viewports let the long CBL
           *  titles wrap onto multiple lines instead of clipping off
           *  the right edge. Desktop keeps the single-line truncate so
           *  pathological titles don't push the action cluster
           *  off-screen. */}
          <h1
            className="text-2xl font-semibold tracking-tight md:truncate"
            title={view.name}
          >
            {view.name}
          </h1>
          {view.description ? (
            <p className="text-muted-foreground text-sm">{view.description}</p>
          ) : null}
        </div>
        {/* `min-w-0` lets the cluster shrink below content width on
         *  narrow viewports so the inner `flex-wrap` actually triggers
         *  (without it, `shrink-0` froze the cluster wider than the
         *  viewport). `md:shrink-0` restores the no-shrink behavior on
         *  desktop where the full set fits in one row. */}
        <div className="flex min-w-0 flex-wrap items-center justify-end gap-2 md:shrink-0">
          {extraActions}
          {view.is_system ? (
            <span
              className="text-muted-foreground bg-muted/40 inline-flex items-center rounded-md border px-2.5 py-1 text-xs font-medium"
              title="Built-in views can't be edited or deleted"
            >
              <Lock className="mr-1 h-3 w-3" /> Built-in
            </span>
          ) : null}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                type="button"
                variant="outline"
                size="icon"
                className="h-8 w-8"
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
                    <PanelLeftClose className="mr-2 h-4 w-4" /> Hide from
                    sidebar
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
                  pin.mutate({ id: view.id, pinned: !view.pinned });
                }}
              >
                {view.pinned ? (
                  <>
                    <PinOff className="mr-2 h-4 w-4" /> Unpin from home
                  </>
                ) : (
                  <>
                    <Pin className="mr-2 h-4 w-4" /> Pin to home
                  </>
                )}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
    </header>
  );
}
