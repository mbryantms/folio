"use client";

import * as React from "react";
import { useRouter } from "next/navigation";
import { Library, Plus, Sparkles } from "lucide-react";
import { toast } from "sonner";

import { CblImportDialog } from "@/components/cbl/cbl-import-dialog";
import {
  FilterBuilder,
  type FilterBuilderState,
} from "@/components/filters/filter-builder";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { PopoverPortalContainer } from "@/components/ui/popover";
import { useCreateSavedView, usePinSavedView } from "@/lib/api/mutations";

/** "Add view" picker — opens either the FilterBuilder dialog or the
 *  CBL import dialog depending on the user's choice. Lives on the
 *  management page (`/settings/views`); the home page is read-only. */
export function AddViewButton({
  /** Optional override label. Defaults to "Add view". */
  label = "Add view",
  /** When true (default), the newly created view is pinned to home so
   *  it shows up immediately on the next render. Pass false to leave
   *  the pin state alone. */
  autoPin = true,
}: {
  label?: string;
  autoPin?: boolean;
}) {
  const [filterOpen, setFilterOpen] = React.useState(false);
  const [importOpen, setImportOpen] = React.useState(false);

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button type="button">
            <Plus className="mr-1 h-4 w-4" /> {label}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent className="w-56" align="end">
          <DropdownMenuItem onSelect={() => setFilterOpen(true)}>
            <Sparkles className="mr-2 h-4 w-4" />
            New filter view
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => setImportOpen(true)}>
            <Library className="mr-2 h-4 w-4" />
            Import CBL
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <NewFilterViewDialog
        open={filterOpen}
        onOpenChange={setFilterOpen}
        autoPin={autoPin}
      />
      <CblImportDialog open={importOpen} onOpenChange={setImportOpen} />
    </>
  );
}

/** Dialog wrapper around `<FilterBuilder>` that creates a saved view
 *  on submit. Used by `<AddViewButton>` for the "+ Add view" picker
 *  and by quick-apply links from chip lists (which pass an `initial`
 *  filter to seed the form). */
export function NewFilterViewDialog({
  open,
  onOpenChange,
  autoPin = true,
  initial,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  autoPin?: boolean;
  /** Optional seed for the FilterBuilder. Quick-apply links pass a
   *  one-condition prefill so the user lands on a half-filled form. */
  initial?: Partial<FilterBuilderState>;
}) {
  const router = useRouter();
  const create = useCreateSavedView();
  const pin = usePinSavedView();
  const [portalContainer, setPortalContainer] =
    React.useState<HTMLElement | null>(null);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        ref={setPortalContainer}
        // `overflow-visible` so the field-picker popover (portaled here
        // via PopoverPortalContainer) can extend past the dialog
        // bounds. The inner scroll div still owns long-form overflow.
        className="flex max-h-[90vh] max-w-4xl flex-col overflow-visible"
      >
        <DialogHeader>
          <DialogTitle>New filter view</DialogTitle>
          <DialogDescription>
            Chain conditions to define what shows up. Preview before saving.
          </DialogDescription>
        </DialogHeader>
        <PopoverPortalContainer value={portalContainer}>
          <div className="min-h-0 flex-1 overflow-y-auto px-1 py-2">
            <FilterBuilder
              // Re-mount when the prefilled seed changes (e.g. user
              // clicks a different chip without closing the dialog).
              key={JSON.stringify(initial ?? {})}
              saveLabel={autoPin ? "Save and pin" : "Save"}
              initial={initial}
              onCancel={() => onOpenChange(false)}
              onSave={async (state) => {
                try {
                  const view = await create.mutateAsync({
                    kind: "filter_series",
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
                  if (!view)
                    throw new Error("Saved view create returned empty");
                  if (autoPin) {
                    try {
                      await pin.mutateAsync({ id: view.id, pinned: true });
                    } catch (e) {
                      if (e instanceof Error && e.message.includes("pin_cap")) {
                        // Plan-limit notice per the variant policy in
                        // docs/dev/notifications-audit.md §3.
                        toast.info(
                          "View saved (pin cap reached — pin from /settings/views).",
                        );
                      }
                    }
                  }
                  toast.success("View created");
                  onOpenChange(false);
                  router.refresh();
                } catch (e) {
                  toast.error(
                    e instanceof Error ? e.message : "Failed to save view",
                  );
                }
              }}
            />
          </div>
        </PopoverPortalContainer>
      </DialogContent>
    </Dialog>
  );
}
