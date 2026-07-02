"use client";

import * as React from "react";
import { useRouter } from "next/navigation";
import { Plus } from "lucide-react";

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
import { useCreatePage } from "@/lib/api/mutations";
import { useMe } from "@/lib/api/queries";

// Fallback when /auth/me is still resolving. Mirrors the column
// default in m20261222_000001_user_max_rails_per_page.
const DEFAULT_RAIL_CAP = 12;

const MAX_NAME_LEN = 80;

/** Multi-page rails M6 — entry point for creating a new page.
 *
 *  Renders an outline button next to the saved-views picker on
 *  `/settings/views`. Opens a single-input modal; on submit calls
 *  `POST /me/pages` and navigates to `/pages/{slug}` so the user lands
 *  on the new (empty) page ready to pin saved views. */
export function NewPageButton() {
  const router = useRouter();
  const create = useCreatePage();
  const me = useMe();
  const railCap = me.data?.max_rails_per_page ?? DEFAULT_RAIL_CAP;
  const [open, setOpen] = React.useState(false);
  const [name, setName] = React.useState("");
  const [submitting, setSubmitting] = React.useState(false);
  const trimmed = name.trim();
  const valid = trimmed.length > 0 && trimmed.length <= MAX_NAME_LEN;

  const reset = () => {
    setName("");
    setSubmitting(false);
  };

  const onSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!valid || submitting) return;
    setSubmitting(true);
    try {
      const created = await create.mutateAsync({ name: trimmed });
      if (created) {
        setOpen(false);
        reset();
        router.push(`/pages/${created.slug}`);
      } else {
        setSubmitting(false);
      }
    } catch {
      // useApiMutation surfaces the error toast; keep the dialog open
      // so the user can retry without retyping.
      setSubmitting(false);
    }
  };

  return (
    <>
      <Button
        type="button"
        variant="outline"
        onClick={() => setOpen(true)}
        title="Add page"
      >
        <Plus className="mr-1 h-4 w-4" /> New page
      </Button>
      <Dialog
        open={open}
        onOpenChange={(o) => {
          setOpen(o);
          if (!o) reset();
        }}
      >
        <DialogContent className="max-w-sm">
          <DialogHeader>
            <DialogTitle>New page</DialogTitle>
            <DialogDescription>
              Pages hold up to {railCap} pinned saved-view rails. You can rename
              or delete them later from the page header.
            </DialogDescription>
          </DialogHeader>
          <form onSubmit={onSubmit} className="space-y-3">
            <div className="space-y-1.5">
              <Label htmlFor="new-page-name">Name</Label>
              <Input
                id="new-page-name"
                autoFocus
                placeholder="e.g. Marvel, Indie, Manga"
                value={name}
                onChange={(e) => setName(e.target.value)}
                maxLength={MAX_NAME_LEN + 8}
                disabled={submitting}
              />
            </div>
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                onClick={() => setOpen(false)}
                disabled={submitting}
              >
                Cancel
              </Button>
              <Button type="submit" disabled={!valid || submitting}>
                {submitting ? "Creating…" : "Create"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </>
  );
}
