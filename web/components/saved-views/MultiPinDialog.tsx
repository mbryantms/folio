"use client";

import * as React from "react";
import { Pin, Search } from "lucide-react";

import { Checkbox } from "@/components/ui/checkbox";
import { CollapsiblePickerSection } from "@/components/ui/CollapsiblePickerSection";
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
import { useMe, useMePages } from "@/lib/api/queries";
import { useTogglePinOnPage } from "@/lib/api/mutations";
import type { PageView, SavedViewView } from "@/lib/api/types";

// Fallback when /auth/me is still resolving. Mirrors the column
// default in m20261222_000001_user_max_rails_per_page.
const DEFAULT_RAIL_CAP = 12;

/** Multi-page rails M6 — multi-pin picker.
 *
 *  Replaces the legacy single "Pin to home" toggle on the saved-view
 *  detail page. Shows every user page (system + custom) with a
 *  checkbox; the current pin state pre-fills via `view.pinned_on_pages`
 *  on the saved view, and each toggle fires `useTogglePinOnPage`
 *  immediately so the server's per-page rail cap (the user's
 *  `max_rails_per_page` preference; default 12, max 50) surfaces
 *  inline.
 *
 *  Cap-disabled state: when a target page is already at the user's
 *  cap and the view isn't on it, the checkbox renders disabled with
 *  a hint.
 *
 *  Long-list ergonomics (mirrors the page-side `<ManagePinsDialog>`): a
 *  search input filters pages by name, and pages are grouped under
 *  collapsible System / Custom sections so a big page library stays
 *  navigable. */
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
  const me = useMe();
  const railCap = me.data?.max_rails_per_page ?? DEFAULT_RAIL_CAP;
  const toggle = useTogglePinOnPage();
  const [query, setQuery] = React.useState("");
  const pinnedSet = React.useMemo(
    () => new Set(view.pinned_on_pages),
    [view.pinned_on_pages],
  );

  // Reset the search when the dialog (re-)opens so reopening it doesn't
  // surprise the user with a stale filter. Render-phase setState — see
  // https://react.dev/learn/you-might-not-need-an-effect.
  const [lastOpen, setLastOpen] = React.useState(open);
  if (open !== lastOpen) {
    setLastOpen(open);
    if (open) setQuery("");
  }

  const pages = React.useMemo(() => pagesQ.data ?? [], [pagesQ.data]);
  const filtered = React.useMemo(() => {
    const q = query.trim().toLowerCase();
    if (q.length === 0) return pages;
    return pages.filter((p) => p.name.toLowerCase().includes(q));
  }, [pages, query]);

  // Two collapsible buckets: the system Home page first (most common
  // pin target), then the user's custom pages.
  const groups = React.useMemo(() => {
    const system = filtered.filter((p) => p.is_system);
    const custom = filtered.filter((p) => !p.is_system);
    return [
      { key: "system", label: "Home", items: system },
      { key: "custom", label: "Your pages", items: custom },
    ].filter((g) => g.items.length > 0);
  }, [filtered]);

  const searching = query.trim().length > 0;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>Pin to pages</DialogTitle>
          <DialogDescription>
            Each page holds up to {railCap} pinned rails. Toggle a page to add
            or remove this view from it.
          </DialogDescription>
        </DialogHeader>
        <div className="relative">
          <Search className="text-muted-foreground pointer-events-none absolute top-1/2 left-2.5 h-4 w-4 -translate-y-1/2" />
          <Input
            type="search"
            placeholder="Search pages…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            className="pl-8"
            aria-label="Filter pages"
          />
        </div>
        <div className="max-h-[50vh] space-y-2 overflow-y-auto py-1">
          {pagesQ.isLoading ? (
            <p className="text-muted-foreground py-4 text-sm">Loading pages…</p>
          ) : pages.length === 0 ? (
            <p className="text-muted-foreground py-4 text-sm">
              No pages yet. Use &ldquo;New page&rdquo; in the sidebar.
            </p>
          ) : groups.length === 0 ? (
            <p className="text-muted-foreground py-4 text-sm">
              No pages match <span className="text-foreground">{query}</span>.
            </p>
          ) : (
            groups.map((group) => (
              <CollapsiblePickerSection
                key={group.key}
                label={group.label}
                count={group.items.length}
                // While searching, force every section open so matches
                // aren't hidden behind a collapsed header.
                forceOpen={searching ? true : undefined}
              >
                {group.items.map((p) => (
                  <PageRow
                    key={p.id}
                    page={p}
                    railCap={railCap}
                    pinned={pinnedSet.has(p.id)}
                    onToggle={(next) =>
                      toggle.mutate({
                        viewId: view.id,
                        pageId: p.id,
                        pinned: next,
                      })
                    }
                  />
                ))}
              </CollapsiblePickerSection>
            ))
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

function PageRow({
  page,
  railCap,
  pinned,
  onToggle,
}: {
  page: PageView;
  railCap: number;
  pinned: boolean;
  onToggle: (next: boolean) => void;
}) {
  const capReached = !pinned && page.pin_count >= railCap;
  return (
    <label
      className={
        "hover:bg-secondary/50 flex cursor-pointer items-center gap-3 rounded-md px-2 py-2 " +
        (capReached ? "opacity-60" : "")
      }
    >
      <Checkbox
        checked={pinned}
        disabled={capReached}
        onCheckedChange={(next) => onToggle(next === true)}
      />
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium">{page.name}</p>
        <p className="text-muted-foreground text-xs">
          {page.pin_count} / {railCap} rails
          {capReached ? " — full" : ""}
        </p>
      </div>
      {pinned ? (
        <Pin className="text-muted-foreground h-4 w-4 shrink-0" />
      ) : null}
    </label>
  );
}
