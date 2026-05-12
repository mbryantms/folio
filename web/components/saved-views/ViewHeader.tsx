"use client";

import * as React from "react";
import {
  Lock,
  PanelLeft,
  PanelLeftClose,
  Pencil,
  Pin,
  PinOff,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { usePinSavedView, useSidebarSavedView } from "@/lib/api/mutations";
import { cn } from "@/lib/utils";
import type { SavedViewView } from "@/lib/api/types";

/** Detail-page header shared by both filter and CBL view types. The
 *  caller supplies the Edit handler; pin/unpin is wired locally so the
 *  same affordance works on either kind without duplicating the
 *  mutation plumbing. CBL views slot extra controls (e.g. Refresh)
 *  into `extraActions`. System views replace Edit with a "Built-in"
 *  badge so the read-only constraint is visible, not just dimmed. */
export function ViewHeader({
  view,
  onEdit,
  extraActions,
  className,
}: {
  view: SavedViewView;
  onEdit?: () => void;
  extraActions?: React.ReactNode;
  className?: string;
}) {
  const pin = usePinSavedView();
  const sidebar = useSidebarSavedView();
  return (
    <header className={cn("space-y-3", className)}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 space-y-1">
          <h1
            className="truncate text-2xl font-semibold tracking-tight"
            title={view.name}
          >
            {view.name}
          </h1>
          {view.description ? (
            <p className="text-muted-foreground text-sm">{view.description}</p>
          ) : null}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {extraActions}
          {view.is_system ? (
            <span
              className="text-muted-foreground bg-muted/40 inline-flex items-center rounded-md border px-2.5 py-1 text-xs font-medium"
              title="Built-in views can't be edited or deleted"
            >
              <Lock className="mr-1 h-3 w-3" /> Built-in
            </span>
          ) : onEdit ? (
            <Button type="button" variant="outline" size="sm" onClick={onEdit}>
              <Pencil className="mr-1 h-4 w-4" /> Edit
            </Button>
          ) : null}
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() =>
              sidebar.mutate({
                id: view.id,
                show: !view.show_in_sidebar,
              })
            }
            title={
              view.show_in_sidebar ? "Hide from sidebar" : "Show in sidebar"
            }
          >
            {view.show_in_sidebar ? (
              <>
                <PanelLeftClose className="mr-1 h-4 w-4" />
                Hide
              </>
            ) : (
              <>
                <PanelLeft className="mr-1 h-4 w-4" />
                Sidebar
              </>
            )}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => pin.mutate({ id: view.id, pinned: !view.pinned })}
            title={view.pinned ? "Unpin from home" : "Pin to home"}
          >
            {view.pinned ? (
              <>
                <PinOff className="mr-1 h-4 w-4" />
                Unpin
              </>
            ) : (
              <>
                <Pin className="mr-1 h-4 w-4" />
                Pin
              </>
            )}
          </Button>
        </div>
      </div>
    </header>
  );
}
