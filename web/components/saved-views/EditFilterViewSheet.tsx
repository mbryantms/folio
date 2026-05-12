"use client";

import * as React from "react";
import { toast } from "sonner";
import { useQueryClient } from "@tanstack/react-query";

import {
  FilterBuilder,
  type FilterBuilderState,
} from "@/components/filters/filter-builder";
import { PopoverPortalContainer } from "@/components/ui/popover";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { useUpdateSavedView } from "@/lib/api/mutations";
import type {
  Condition,
  MatchMode,
  SavedViewSortField,
  SavedViewView,
  SortOrder,
} from "@/lib/api/types";

/** Edit sheet for filter-series views. Pre-fills the M5 FilterBuilder
 *  with the saved view's current state and PATCHes the change back.
 *  A right-side sheet — instead of a centered modal — gives the
 *  builder + preview grid full-height vertical space, which the
 *  long-form layout (details / conditions / sort / preview) was
 *  outgrowing inside a max-w-4xl modal.
 */
export function EditFilterViewSheet({
  view,
  open,
  onOpenChange,
}: {
  view: SavedViewView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const update = useUpdateSavedView(view.id);
  const qc = useQueryClient();
  const [portalContainer, setPortalContainer] =
    React.useState<HTMLElement | null>(null);
  const initial: Partial<FilterBuilderState> = {
    name: view.name,
    description: view.description ?? "",
    matchMode: (view.match_mode ?? "all") as MatchMode,
    conditions: (view.conditions ?? []) as Condition[],
    sortField: (view.sort_field ?? "created_at") as SavedViewSortField,
    sortOrder: (view.sort_order ?? "desc") as SortOrder,
    resultLimit: view.result_limit ?? 12,
  };
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        ref={setPortalContainer}
        side="right"
        // Override the shadcn default `w-3/4 sm:max-w-sm` — the
        // builder + 4-column preview grid needs a roomier workspace.
        // `overflow-visible` so the field-picker popover (portaled
        // into this content node via `PopoverPortalContainer`) can
        // extend past the sheet's bounds when collision avoidance
        // flips it outward; the inner body div owns long-form scroll.
        // Zero the default `p-6` so the header gets its own divider
        // rule and the scroll body owns its own padding.
        className="flex w-full flex-col gap-0 overflow-visible p-0 sm:max-w-2xl lg:max-w-3xl xl:max-w-4xl"
      >
        <SheetHeader className="border-border/60 border-b px-6 py-4 pr-12">
          <SheetTitle>Edit filter view</SheetTitle>
          <SheetDescription>
            Tweak the details, conditions, and sort. Pinned-rail previews
            refresh automatically when you save.
          </SheetDescription>
        </SheetHeader>
        <PopoverPortalContainer value={portalContainer}>
          <div className="min-h-0 flex-1 overflow-y-auto px-6 py-5">
            <FilterBuilder
              // Re-mount on view change so the form picks up server-side
              // refreshes (e.g. someone edited the same view in another tab).
              key={`${view.id}-${view.updated_at}`}
              saveLabel="Save"
              initial={initial}
              onCancel={() => onOpenChange(false)}
              onSave={async (state) => {
                try {
                  await update.mutateAsync({
                    name: state.name,
                    description: state.description.trim() || null,
                    filter: {
                      match_mode: state.matchMode,
                      conditions: state.conditions,
                    },
                    sort_field: state.sortField,
                    sort_order: state.sortOrder,
                    result_limit: state.resultLimit,
                  });
                  qc.invalidateQueries({ queryKey: ["saved-views"] });
                  toast.success("View updated");
                  onOpenChange(false);
                } catch (e) {
                  toast.error(
                    e instanceof Error ? e.message : "Failed to update view",
                  );
                }
              }}
            />
          </div>
        </PopoverPortalContainer>
      </SheetContent>
    </Sheet>
  );
}
