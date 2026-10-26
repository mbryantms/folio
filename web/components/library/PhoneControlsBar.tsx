"use client";

import * as React from "react";
import {
  ArrowDownUp,
  Check,
  LayoutGrid,
  Search,
  SlidersHorizontal,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
} from "@/components/folio/sheet";
import type { SortOrder } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import { CHROME_BAND_CLASS } from "@/lib/ui/chrome-band";

export type PhoneSortOption = { value: string; label: string };

/**
 * Phone-only (`sm:hidden`) grid controls: one 36px row instead of the
 * desktop toolbar folding into six. Every control is an icon button;
 * search expands in place, sort opens a bottom sheet, and everything
 * secondary (card size, Select, Save as view, exports…) lives in the
 * four-squares "View options" sheet. Pages keep their desktop markup and wrap
 * it in `max-sm:hidden`, so the two compositions never both render.
 *
 * `sticky` pins the row under the topbar so the controls stay reachable
 * after scrolling — that is what makes the compaction a net gain. The
 * negative horizontal margin lets the sticky band run edge to edge
 * inside the shell's `px-4` main column.
 */
export function PhoneControlsBar({
  leading,
  search,
  sort,
  filters,
  primary,
  menu,
  menuTitle = "View options",
  menuIcon,
  sticky = true,
  hidden = false,
  className,
}: {
  /** Leading content, e.g. the Series / Issues mode toggle. */
  leading?: React.ReactNode;
  search?: {
    value: string;
    onChange: (next: string) => void;
    placeholder?: string;
    label?: string;
  };
  /** Untyped on purpose: callers narrow `next` back to their own sort
   *  union (`v as SeriesSort`) — a generic here fought inference when a
   *  page switches between two sort unions by mode. */
  sort?: {
    value: string;
    options: ReadonlyArray<PhoneSortOption>;
    onChange: (next: string) => void;
    order?: SortOrder;
    onOrder?: (next: SortOrder) => void;
    disabled?: boolean;
  };
  filters?: { count: number; onOpen: () => void };
  /** Extra inline icon button(s) that must stay one tap away (CBL's
   *  "Up Next"). Keep it to one. */
  primary?: React.ReactNode;
  /** Body of the View options sheet. Receives `close` so actions that leave the
   *  sheet (entering Select mode, opening a dialog) can dismiss it. */
  menu?: (close: () => void) => React.ReactNode;
  menuTitle?: string;
  /** Icon for the menu trigger. Defaults to the four-squares
   *  `LayoutGrid` glyph — the same icon `CardSizeOptions` uses on
   *  desktop — so "View options" looks the same at every width and
   *  never reads as a second `⋯` beside a header's More-actions menu
   *  (`⋯` is reserved for action lists, which are dropdowns). */
  menuIcon?: React.ReactNode;
  sticky?: boolean;
  /** Hide the row entirely — the selection toolbar takes its place. */
  hidden?: boolean;
  className?: string;
}) {
  const [searchOpen, setSearchOpen] = React.useState(false);
  const [sortOpen, setSortOpen] = React.useState(false);
  const [menuOpen, setMenuOpen] = React.useState(false);
  // Keystrokes stay local; `search.value` from the caller is usually
  // the *debounced* value, so echoing it back into the input would
  // drop trailing characters mid-typing. `lastSent` tells our own
  // echo apart from an external change (Clear filters) that must sync.
  const externalValue = search?.value ?? "";
  const [searchInput, setSearchInput] = React.useState(externalValue);
  const lastSent = React.useRef(externalValue);
  React.useEffect(() => {
    if (externalValue !== lastSent.current) {
      setSearchInput(externalValue);
      lastSent.current = externalValue;
    }
  }, [externalValue]);
  const searchActive = searchInput.trim().length > 0;
  const sortLabel =
    sort?.options.find((o) => o.value === sort.value)?.label ?? "Sort";

  if (hidden) return null;

  return (
    <div
      data-phone-controls=""
      className={cn(
        "sm:hidden",
        sticky &&
          cn(
            CHROME_BAND_CLASS,
            "sticky top-(--topbar-h) z-20 -mx-4 border-b px-4 py-2",
          ),
        className,
      )}
    >
      {search && (searchOpen || searchActive) ? (
        <div className="flex items-center gap-2">
          <Input
            type="search"
            autoFocus={searchOpen}
            value={searchInput}
            onChange={(e) => {
              const next = e.target.value;
              setSearchInput(next);
              lastSent.current = next.trim();
              search.onChange(next);
            }}
            placeholder={search.placeholder ?? "Search…"}
            aria-label={search.label ?? search.placeholder ?? "Search"}
            className="h-9 min-w-0 flex-1"
          />
          <Button
            type="button"
            variant="ghost"

            className="shrink-0"
            onClick={() => {
              setSearchInput("");
              lastSent.current = "";
              search.onChange("");
              setSearchOpen(false);
            }}
          >
            Cancel
          </Button>
        </div>
      ) : (
        <div className="flex items-center gap-2">
          {leading ? (
            // Rule 6: the leading strip scrolls, it never wraps or runs
            // under the icon cluster. The CBL page's two stats pills plus
            // four icons are ~7px wider than a 375px phone's column, and a
            // `min-w-0` flex child whose content is `shrink-0` otherwise
            // paints over its neighbour.
            <div className="flex min-w-0 [scrollbar-width:none] items-center gap-2 overflow-x-auto overflow-y-hidden [&::-webkit-scrollbar]:hidden">
              {leading}
            </div>
          ) : null}
          <div className="ml-auto flex shrink-0 items-center gap-1">
            {search ? (
              <IconButton
                label={search.label ?? "Search"}
                onClick={() => setSearchOpen(true)}
              >
                <Search className="size-4" />
              </IconButton>
            ) : null}
            {sort ? (
              <IconButton
                label={`Sort: ${sortLabel}`}
                onClick={() => setSortOpen(true)}
                disabled={sort.disabled}
              >
                <ArrowDownUp className="size-4" />
              </IconButton>
            ) : null}
            {filters ? (
              <IconButton
                label={
                  filters.count > 0
                    ? `Filters (${filters.count} active)`
                    : "Filters"
                }
                active={filters.count > 0}
                onClick={filters.onOpen}
              >
                <SlidersHorizontal className="size-4" />
                {filters.count > 0 ? (
                  <span
                    aria-hidden="true"
                    className="bg-primary text-primary-foreground absolute -top-1 -right-1 grid h-4 min-w-4 place-items-center rounded-full px-1 text-[10px] leading-none font-semibold tabular-nums"
                  >
                    {filters.count}
                  </span>
                ) : null}
              </IconButton>
            ) : null}
            {primary}
            {menu ? (
              <IconButton
                label={menuTitle}
                onClick={() => setMenuOpen(true)}
                haspopup="dialog"
              >
                {menuIcon ?? <LayoutGrid className="size-4" />}
              </IconButton>
            ) : null}
          </div>
        </div>
      )}

      {sort ? (
        <Sheet open={sortOpen} onOpenChange={setSortOpen}>
          <SheetContent
            side="bottom"
            className="rounded-t-lg pb-[max(1.5rem,var(--safe-bottom))]"
          >
            <SheetHeader>
              <SheetTitle>Sort by</SheetTitle>
            </SheetHeader>
            <div role="radiogroup" aria-label="Sort by" className="-mx-2">
              {sort.options.map((o) => {
                const selected = o.value === sort.value;
                return (
                  <button
                    key={o.value}
                    type="button"
                    role="radio"
                    aria-checked={selected}
                    onClick={() => {
                      sort.onChange(o.value);
                      setSortOpen(false);
                    }}
                    className={cn(
                      "flex h-11 w-full items-center justify-between rounded-md px-2 text-sm",
                      selected ? "font-medium" : "text-foreground/90",
                      "hover:bg-muted active:bg-muted",
                    )}
                  >
                    {o.label}
                    {selected ? (
                      <Check className="size-4" aria-hidden="true" />
                    ) : null}
                  </button>
                );
              })}
            </div>
            {sort.order && sort.onOrder ? (
              <div
                role="group"
                aria-label="Order"
                className="bg-muted mt-2 grid grid-cols-2 gap-0.5 rounded-md p-0.5"
              >
                {(
                  [
                    { value: "asc", label: "Ascending" },
                    { value: "desc", label: "Descending" },
                  ] as const
                ).map((o) => (
                  <button
                    key={o.value}
                    type="button"
                    aria-pressed={sort.order === o.value}
                    onClick={() => sort.onOrder?.(o.value)}
                    className={cn(
                      "h-9 rounded text-sm transition-colors",
                      sort.order === o.value
                        ? "bg-background text-foreground shadow-sm"
                        : "text-muted-foreground",
                    )}
                  >
                    {o.label}
                  </button>
                ))}
              </div>
            ) : null}
          </SheetContent>
        </Sheet>
      ) : null}

      {menu ? (
        <Sheet open={menuOpen} onOpenChange={setMenuOpen}>
          <SheetContent
            side="bottom"
            className="rounded-t-lg pb-[max(1.5rem,var(--safe-bottom))]"
          >
            <SheetHeader>
              <SheetTitle>{menuTitle}</SheetTitle>
            </SheetHeader>
            <div className="space-y-4">{menu(() => setMenuOpen(false))}</div>
          </SheetContent>
        </Sheet>
      ) : null}
    </div>
  );
}

/** A full-width row button for the View options sheet, so callers' actions
 *  (Select, Save as view, Export…) line up as a tappable list. */
export function PhoneMenuAction({
  icon,
  children,
  onClick,
  disabled,
  asChild,
  className,
  ...rest
}: React.ComponentProps<typeof Button> & { icon?: React.ReactNode }) {
  return (
    <Button
      type="button"
      variant="outline"
      onClick={onClick}
      disabled={disabled}
      asChild={asChild}
      className={cn("h-11 w-full justify-start gap-2", className)}
      {...rest}
    >
      {asChild ? (
        children
      ) : (
        <>
          {icon}
          {children}
        </>
      )}
    </Button>
  );
}

function IconButton({
  label,
  active = false,
  onClick,
  disabled,
  haspopup,
  children,
}: {
  label: string;
  active?: boolean;
  onClick: () => void;
  disabled?: boolean;
  haspopup?: "dialog" | "menu";
  children: React.ReactNode;
}) {
  return (
    <Button
      type="button"
      variant={active ? "secondary" : "outline"}
      size="icon"
      aria-label={label}
      title={label}
      aria-haspopup={haspopup}
      aria-pressed={active || undefined}
      onClick={onClick}
      disabled={disabled}
      className="relative h-9 w-9 shrink-0"
    >
      {children}
    </Button>
  );
}
