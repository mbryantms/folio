"use client";

import * as React from "react";
import { Pin } from "lucide-react";

import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { useMePages } from "@/lib/api/queries";
import { useTogglePinOnPage } from "@/lib/api/mutations";
import type { SavedViewView } from "@/lib/api/types";

/** Multi-page rails M6 — multi-pin picker.
 *
 *  Replaces the legacy single "Pin to home" toggle on the saved-view
 *  detail page. Shows every user page (system + custom) with a
 *  checkbox; the current pin state pre-fills via `view.pinned_on_pages`
 *  on the saved view, and each toggle fires `useTogglePinOnPage`
 *  immediately so the server's 12-rail per-page cap surfaces inline.
 *
 *  Cap-disabled state: when a target page is already at 12 pins and
 *  the view isn't on it, the checkbox renders disabled with a hint. */
export function MultiPinDialog({
  view,
  open,
  onOpenChange,
}: {
  view: SavedViewView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const pagesQ = useMePages();
  const toggle = useTogglePinOnPage();
  const pinnedSet = React.useMemo(
    () => new Set(view.pinned_on_pages),
    [view.pinned_on_pages],
  );

  const pages = pagesQ.data ?? [];

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>Pin to pages</DialogTitle>
          <DialogDescription>
            Each page holds up to 12 pinned rails. Toggle a page to add or
            remove this view from it.
          </DialogDescription>
        </DialogHeader>
        <div className="max-h-[50vh] space-y-1 overflow-y-auto py-1">
          {pagesQ.isLoading ? (
            <p className="text-muted-foreground py-4 text-sm">
              Loading pages…
            </p>
          ) : pages.length === 0 ? (
            <p className="text-muted-foreground py-4 text-sm">
              No pages yet. Use &ldquo;New page&rdquo; in the sidebar.
            </p>
          ) : (
            pages.map((p) => {
              const isPinned = pinnedSet.has(p.id);
              const capReached = !isPinned && p.pin_count >= 12;
              return (
                <label
                  key={p.id}
                  className={
                    "hover:bg-secondary/50 flex cursor-pointer items-center gap-3 rounded-md px-2 py-2 " +
                    (capReached ? "opacity-60" : "")
                  }
                >
                  <Checkbox
                    checked={isPinned}
                    disabled={capReached}
                    onCheckedChange={(next) => {
                      toggle.mutate({
                        viewId: view.id,
                        pageId: p.id,
                        pinned: next === true,
                      });
                    }}
                  />
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium">
                      {p.name}
                      {p.is_system ? (
                        <span className="text-muted-foreground ml-2 text-[10px] font-medium tracking-wider uppercase">
                          Home
                        </span>
                      ) : null}
                    </p>
                    <p className="text-muted-foreground text-xs">
                      {p.pin_count} / 12 rails
                      {capReached ? " — full" : ""}
                    </p>
                  </div>
                  {isPinned ? (
                    <Pin className="text-muted-foreground h-4 w-4 shrink-0" />
                  ) : null}
                </label>
              );
            })
          )}
        </div>
        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            onClick={() => onOpenChange(false)}
          >
            Done
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
