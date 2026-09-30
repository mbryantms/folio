"use client";

import * as React from "react";
import { Bookmark, Highlighter, Star, StickyNote } from "lucide-react";

import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { useIssueMarkers } from "@/lib/api/queries";
import type { MarkerKind } from "@/lib/api/types";
import {
  DRAWER_NAV_KEYS,
  drawerSnippet,
  nextDrawerIndex,
  sortDrawerMarkers,
  useMarkerDrawer,
} from "@/lib/reader/marker-drawer";
import { cn } from "@/lib/utils";

const KIND_LABEL: Record<MarkerKind, string> = {
  bookmark: "Bookmark",
  note: "Note",
  favorite: "Favorite",
  highlight: "Highlight",
};

const KIND_ICON: Record<MarkerKind, typeof Bookmark> = {
  bookmark: Bookmark,
  note: StickyNote,
  favorite: Star,
  highlight: Highlighter,
};

/**
 * In-reader marker drawer (roadmap WP-5.2): every marker on this issue in
 * page order; activating one jumps the reader to its page.
 *
 * Non-modal like the page-text panel, so the reader keeps turning pages
 * while it's open and outside taps don't dismiss it. Keyboard: the list is
 * a single tab stop with roving focus — ↑ / ↓ move between markers (wrap),
 * Home / End jump to the first / last, Enter or Space opens the focused
 * one. Those keys are stopped from reaching the reader keymap while focus
 * is in the list; Esc, the close button and the `l` shortcut close it.
 */
export function MarkerDrawer({
  issueId,
  currentPage,
  onJump,
}: {
  issueId: string;
  currentPage: number;
  onJump: (pageIndex: number) => void;
}) {
  const open = useMarkerDrawer((s) => s.open);
  const setOpen = useMarkerDrawer((s) => s.setOpen);
  const query = useIssueMarkers(issueId);
  const items = React.useMemo(
    () => sortDrawerMarkers(query.data?.items ?? []),
    [query.data],
  );

  // Roving tabindex: one item is tabbable; arrows move it.
  const [active, setActive] = React.useState(0);
  const itemRefs = React.useRef<(HTMLButtonElement | null)[]>([]);
  const activeIdx = Math.min(active, Math.max(0, items.length - 1));

  const onKeyDown = (e: React.KeyboardEvent<HTMLUListElement>) => {
    if (!DRAWER_NAV_KEYS.has(e.key)) return;
    e.stopPropagation();
    const next = nextDrawerIndex(e.key, activeIdx, items.length);
    if (next === null) return; // Enter / Space: the button's own click.
    e.preventDefault();
    setActive(next);
    itemRefs.current[next]?.focus();
  };

  return (
    <Sheet open={open} onOpenChange={setOpen} modal={false}>
      <SheetContent
        side="right"
        className="flex w-full flex-col gap-0 sm:max-w-sm"
        onInteractOutside={(e) => e.preventDefault()}
        // Keep the reader keymap's `quitReader` from also firing on Esc.
        onEscapeKeyDown={(e) => e.stopPropagation()}
      >
        <SheetHeader className="border-border border-b pb-4">
          <SheetTitle>Markers in this issue</SheetTitle>
          <SheetDescription>
            Bookmarks, notes, favorites, and highlights in page order. Use ↑ and
            ↓ to move, Enter to jump to the page.
          </SheetDescription>
        </SheetHeader>
        <div className="min-h-0 flex-1 overflow-y-auto py-3">
          {query.isLoading ? (
            <p className="text-muted-foreground px-4 text-sm">
              Loading markers…
            </p>
          ) : query.isError ? (
            <p className="text-destructive px-4 text-sm">
              Couldn&apos;t load markers.
            </p>
          ) : items.length === 0 ? (
            <p className="text-muted-foreground px-4 text-sm">
              No markers on this issue yet.
            </p>
          ) : (
            <ul
              aria-label="Markers in this issue"
              className="space-y-1 px-2"
              onKeyDown={onKeyDown}
            >
              {items.map((m, i) => {
                const Icon = KIND_ICON[m.kind];
                const snippet = drawerSnippet(m);
                const here = m.page_index === currentPage;
                return (
                  <li key={m.id}>
                    <button
                      ref={(el) => {
                        itemRefs.current[i] = el;
                      }}
                      type="button"
                      tabIndex={i === activeIdx ? 0 : -1}
                      aria-current={here ? "page" : undefined}
                      onFocus={() => setActive(i)}
                      onClick={() => onJump(m.page_index)}
                      className={cn(
                        "hover:bg-accent/40 focus-visible:ring-ring flex w-full items-start gap-3 rounded-md px-3 py-2 text-left focus-visible:ring-2 focus-visible:outline-none",
                        here && "bg-accent/30",
                      )}
                    >
                      <Icon
                        aria-hidden="true"
                        className="text-muted-foreground mt-0.5 h-4 w-4 shrink-0"
                      />
                      <span className="min-w-0 flex-1">
                        <span className="block text-sm font-medium">
                          Page {m.page_index + 1} · {KIND_LABEL[m.kind]}
                          {m.is_favorite && m.kind !== "favorite" ? (
                            <span className="sr-only"> (favorite)</span>
                          ) : null}
                        </span>
                        {snippet ? (
                          <span className="text-muted-foreground line-clamp-2 block text-xs">
                            {snippet}
                          </span>
                        ) : null}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </SheetContent>
    </Sheet>
  );
}
