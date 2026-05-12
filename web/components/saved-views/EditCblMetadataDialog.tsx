"use client";

import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { useUpdateCblList, useUpdateSavedView } from "@/lib/api/mutations";
import type { CblListView, SavedViewView } from "@/lib/api/types";

const REFRESH_OPTIONS: { value: string; label: string }[] = [
  { value: "manual", label: "Manual only" },
  { value: "@daily", label: "Daily" },
  { value: "@weekly", label: "Weekly" },
  { value: "@monthly", label: "Monthly" },
];

/** Edit dialog for CBL views. Touches both the saved_view (name,
 *  description, custom tags, year overlay) and the underlying cbl_list
 *  (refresh schedule). Entries themselves are read-only — they change
 *  via re-import or refresh. */
export function EditCblMetadataDialog({
  view,
  list,
  open,
  onOpenChange,
}: {
  view: SavedViewView;
  list: CblListView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="max-w-2xl"
        onPointerDownOutside={(e) => {
          if (
            (e.target as Element | null)?.closest(
              "[data-radix-popper-content-wrapper]",
            )
          ) {
            e.preventDefault();
          }
        }}
        onInteractOutside={(e) => {
          if (
            (e.target as Element | null)?.closest(
              "[data-radix-popper-content-wrapper]",
            )
          ) {
            e.preventDefault();
          }
        }}
      >
        <DialogHeader>
          <DialogTitle>Edit view metadata</DialogTitle>
          <DialogDescription>
            Adjust the name, description, tags, and refresh schedule. Entries
            stay sourced from the imported `.cbl` file — they update via
            refresh.
          </DialogDescription>
        </DialogHeader>
        {open ? (
          // Inner form gets a fresh mount each time the dialog opens so
          // we can seed defaults via `useState(...)` initializers
          // without dancing around setState-in-render lints.
          <EditForm
            view={view}
            list={list}
            onClose={() => onOpenChange(false)}
          />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function EditForm({
  view,
  list,
  onClose,
}: {
  view: SavedViewView;
  list: CblListView;
  onClose: () => void;
}) {
  const updateView = useUpdateSavedView(view.id);
  const updateList = useUpdateCblList(list.id);
  const qc = useQueryClient();

  const [name, setName] = React.useState(view.name);
  const [description, setDescription] = React.useState(view.description ?? "");
  const [tagsRaw, setTagsRaw] = React.useState(view.custom_tags.join(", "));
  const [yearStart, setYearStart] = React.useState(
    view.custom_year_start != null ? String(view.custom_year_start) : "",
  );
  const [yearEnd, setYearEnd] = React.useState(
    view.custom_year_end != null ? String(view.custom_year_end) : "",
  );
  const [schedule, setSchedule] = React.useState<string>(
    list.refresh_schedule ?? "manual",
  );

  async function save() {
    const tags = tagsRaw
      .split(",")
      .map((t) => t.trim())
      .filter(Boolean);
    try {
      await Promise.all([
        updateView.mutateAsync({
          name: name.trim() || null,
          description: description.trim() || null,
          custom_tags: tags,
          custom_year_start: yearStart ? parseInt(yearStart, 10) : null,
          custom_year_end: yearEnd ? parseInt(yearEnd, 10) : null,
        }),
        updateList.mutateAsync({
          refresh_schedule: schedule === "manual" ? null : schedule,
        }),
      ]);
      qc.invalidateQueries({ queryKey: ["saved-views"] });
      qc.invalidateQueries({ queryKey: ["cbl-lists"] });
      toast.success("View updated");
      onClose();
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "Failed to update view");
    }
  }

  const submitting = updateView.isPending || updateList.isPending;

  return (
    <>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <div className="flex flex-col gap-1 sm:col-span-2">
          <Label htmlFor="cbl-edit-name">Name</Label>
          <Input
            id="cbl-edit-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </div>
        <div className="flex flex-col gap-1 sm:col-span-2">
          <Label htmlFor="cbl-edit-desc">Description</Label>
          <Textarea
            id="cbl-edit-desc"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            rows={2}
          />
        </div>
        <div className="flex flex-col gap-1 sm:col-span-2">
          <Label htmlFor="cbl-edit-tags">Tags (comma-separated)</Label>
          <Input
            id="cbl-edit-tags"
            value={tagsRaw}
            onChange={(e) => setTagsRaw(e.target.value)}
            placeholder="event, big-two"
          />
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor="cbl-edit-year-start">Year from</Label>
          <Input
            id="cbl-edit-year-start"
            type="number"
            value={yearStart}
            onChange={(e) => setYearStart(e.target.value)}
          />
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor="cbl-edit-year-end">Year to</Label>
          <Input
            id="cbl-edit-year-end"
            type="number"
            value={yearEnd}
            onChange={(e) => setYearEnd(e.target.value)}
          />
        </div>
        <div className="flex flex-col gap-1 sm:col-span-2">
          <Label htmlFor="cbl-edit-schedule">Refresh schedule</Label>
          <Select value={schedule} onValueChange={setSchedule}>
            <SelectTrigger id="cbl-edit-schedule">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {REFRESH_OPTIONS.map((o) => (
                <SelectItem key={o.value} value={o.value}>
                  {o.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>
      <DialogFooter>
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button onClick={save} disabled={submitting || name.trim() === ""}>
          {submitting ? "Saving…" : "Save"}
        </Button>
      </DialogFooter>
    </>
  );
}
