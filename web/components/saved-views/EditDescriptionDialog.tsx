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
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { useUpdatePage } from "@/lib/api/mutations";

/** Multi-page rails follow-up — add / edit a page's description.
 *
 *  Mirrors the server contract: empty (or whitespace-only) text clears
 *  the description; a literal `null` (which the JSON serializer can't
 *  distinguish from a missing field anyway) is treated as "unchanged"
 *  and never sent. */
export function EditDescriptionDialog({
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
  // Reset the draft each time the dialog opens against new initial
  // content — covers reopen-on-different-page and revert-after-cancel.
  const [lastInitial, setLastInitial] = React.useState(initial);
  if (lastInitial !== initial) {
    setLastInitial(initial);
    setValue(initial);
  }

  const trimmed = value.trim();
  const submitting = update.isPending;

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (submitting) return;
    try {
      // Send the trimmed value (or empty string to clear). Server
      // normalizes whitespace; we trim here so the local state stays
      // honest after the mutation resolves.
      await update.mutateAsync({ description: trimmed });
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
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>
            {initial ? "Edit description" : "Add description"}
          </DialogTitle>
          <DialogDescription>
            Blurb rendered under the page title. Leave blank to clear it.
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={onSubmit} className="space-y-3">
          <div className="space-y-1.5">
            <Label htmlFor="page-description">Description</Label>
            <Textarea
              id="page-description"
              autoFocus
              rows={5}
              placeholder="e.g. Marvel — Hickman's run + adjacent titles"
              value={value}
              onChange={(e) => setValue(e.target.value)}
              disabled={submitting}
            />
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => onOpenChange(false)}
              disabled={submitting}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={submitting}>
              {submitting ? "Saving…" : "Save"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
