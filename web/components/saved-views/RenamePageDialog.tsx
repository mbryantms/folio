"use client";

import * as React from "react";

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
import { useUpdatePage } from "@/lib/api/mutations";

/** Longest page name the server takes (`PageHeading` uses the same). */
export const PAGE_NAME_MAX = 80;

/** Rename a custom page from its ⋯ menu. From `sm` up the title can also
 *  be edited in place (`PageHeading`); phones have only this. */
export function RenamePageDialog({
  open,
  onOpenChange,
  pageId,
  initial,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  pageId: string;
  initial: string;
}) {
  const update = useUpdatePage(pageId);
  const [value, setValue] = React.useState(initial);
  const [lastInitial, setLastInitial] = React.useState(initial);
  if (lastInitial !== initial) {
    setLastInitial(initial);
    setValue(initial);
  }
  const trimmed = value.trim();
  const valid = trimmed.length > 0 && trimmed.length <= PAGE_NAME_MAX;

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!valid || update.isPending) return;
    if (trimmed === initial) {
      onOpenChange(false);
      return;
    }
    try {
      await update.mutateAsync({ name: trimmed });
      onOpenChange(false);
    } catch {
      // toast surfaced by useApiMutation; keep the dialog open.
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) setValue(initial);
        onOpenChange(o);
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Rename page</DialogTitle>
          <DialogDescription>
            The name shows as the page&rsquo;s title and in the sidebar.
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={onSubmit} className="space-y-3">
          <div className="space-y-1.5">
            <Label htmlFor="page-name">Name</Label>
            <Input
              id="page-name"
              autoFocus
              value={value}
              maxLength={PAGE_NAME_MAX}
              onChange={(e) => setValue(e.target.value)}
              disabled={update.isPending}
            />
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button
              type="submit"
              disabled={!valid || trimmed === initial || update.isPending}
            >
              Rename
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
