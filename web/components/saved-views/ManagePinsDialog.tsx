"use client";

import * as React from "react";
import Link from "next/link";
import { Search } from "lucide-react";

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
import { Input } from "@/components/ui/input";
import { useSavedViews, useMePages } from "@/lib/api/queries";
import { useTogglePinOnPage } from "@/lib/api/mutations";
import type { SavedViewView } from "@/lib/api/types";

const RAIL_CAP = 12;

/** Bucket order in the picker: built-in rails at the top so they're
 *  easy to surface on a fresh page, then filter views, then CBL lists,
 *  then user collections. */
const GROUP_ORDER: ReadonlyArray<SavedViewView["kind"]> = [
  "system",
  "filter_series",
  "cbl",
  "collection",
];

function groupLabel(kind: SavedViewView["kind"]): string {
  switch (kind) {
    case "system":
      return "Built-in";
    case "filter_series":
      return "Filter views";
    case "cbl":
      return "CBL lists";
    case "collection":
      return "Collections";
    default:
      return "Other";
  }
}

/** Multi-page rails follow-up — page-centric multi-pin picker.
 *
 *  Inverse of the per-view `<MultiPinDialog>` ("show me this view's
 *  page-pin state"). Opened from the page-detail kebab to manage
 *  WHICH SAVED VIEWS are pinned ON THIS page. Toggling a checkbox
 *  pins or unpins via `useTogglePinOnPage` immediately so the
 *  server's 12-rail per-page cap surfaces inline.
 *
 *  Long-list ergonomics: a search input at the top filters by name
 *  and description; rows are grouped by saved-view kind under sticky
 *  section labels so a 50-view library stays navigable. */
export function ManagePinsDialog({
  open,
  onOpenChange,
  pageId,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  pageId: string;
}) {
  const viewsQ = useSavedViews();
  const pagesQ = useMePages();
  const toggle = useTogglePinOnPage();
  const [query, setQuery] = React.useState("");

  // Reset the search when the dialog (re-)opens so reopening it
  // doesn't surprise the user with a stale filter. Render-phase
  // setState — see
  // https://react.dev/learn/you-might-not-need-an-effect.
  const [lastOpen, setLastOpen] = React.useState(open);
  if (open !== lastOpen) {
    setLastOpen(open);
    if (open) setQuery("");
  }

  const sortedViews = React.useMemo(() => {
    const all = viewsQ.data?.items ?? [];
    return [...all].sort((a, b) =>
      a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
    );
  }, [viewsQ.data]);
  const thisPage = pagesQ.data?.find((p) => p.id === pageId);
  const railCount = thisPage?.pin_count ?? 0;
  const atCap = railCount >= RAIL_CAP;

  const filtered = React.useMemo(() => {
    const q = query.trim().toLowerCase();
    if (q.length === 0) return sortedViews;
    return sortedViews.filter((v) => {
      if (v.name.toLowerCase().includes(q)) return true;
      if (v.description && v.description.toLowerCase().includes(q)) return true;
      return false;
    });
  }, [sortedViews, query]);

  const groups = React.useMemo(() => {
    const byKind = new Map<SavedViewView["kind"], SavedViewView[]>();
    for (const v of filtered) {
      const bucket = byKind.get(v.kind) ?? [];
      bucket.push(v);
      byKind.set(v.kind, bucket);
    }
    return GROUP_ORDER.map((kind) => ({
      kind,
      label: groupLabel(kind),
      items: byKind.get(kind) ?? [],
    })).filter((g) => g.items.length > 0);
  }, [filtered]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>
            Manage rails on {thisPage?.name ?? "this page"}
          </DialogTitle>
          <DialogDescription>
            {railCount} / {RAIL_CAP} rails pinned. Toggle a saved view to add
            or remove it from this page.
          </DialogDescription>
        </DialogHeader>
        <div className="relative">
          <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-2.5 h-4 w-4 -translate-y-1/2" />
          <Input
            type="search"
            placeholder="Search views…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            className="pl-8"
            aria-label="Filter saved views"
          />
        </div>
        <div className="max-h-[50vh] space-y-2 overflow-y-auto py-1">
          {viewsQ.isLoading ? (
            <p className="text-muted-foreground py-4 text-sm">Loading…</p>
          ) : sortedViews.length === 0 ? (
            <p className="text-muted-foreground py-4 text-sm">
              No saved views yet. Create one in{" "}
              <Link
                href="/settings/views"
                className="text-foreground underline"
                onClick={() => onOpenChange(false)}
              >
                Settings → Saved views
              </Link>
              .
            </p>
          ) : groups.length === 0 ? (
            <p className="text-muted-foreground py-4 text-sm">
              No views match <span className="text-foreground">{query}</span>.
            </p>
          ) : (
            groups.map((group) => (
              <div key={group.kind} className="space-y-1">
                <p className="bg-background text-muted-foreground/70 sticky top-0 z-10 px-2 pt-1 pb-0.5 text-[10px] font-medium tracking-widest uppercase">
                  {group.label}
                </p>
                {group.items.map((v) => {
                  const isPinned = v.pinned_on_pages.includes(pageId);
                  const disabled = !isPinned && atCap;
                  return (
                    <label
                      key={v.id}
                      className={
                        "hover:bg-secondary/50 flex cursor-pointer items-center gap-3 rounded-md px-2 py-2 " +
                        (disabled ? "opacity-60" : "")
                      }
                    >
                      <Checkbox
                        checked={isPinned}
                        disabled={disabled}
                        onCheckedChange={(next) => {
                          toggle.mutate({
                            viewId: v.id,
                            pageId,
                            pinned: next === true,
                          });
                        }}
                      />
                      <div className="min-w-0 flex-1">
                        <p className="truncate text-sm font-medium">{v.name}</p>
                        {v.description ? (
                          <p className="text-muted-foreground truncate text-xs">
                            {v.description}
                          </p>
                        ) : null}
                      </div>
                    </label>
                  );
                })}
              </div>
            ))
          )}
        </div>
        {atCap && (
          <p className="text-muted-foreground text-xs">
            Rail cap reached. Uncheck a view to free a slot.
          </p>
        )}
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
