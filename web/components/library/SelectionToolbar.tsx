"use client";

import * as React from "react";
import { MoreHorizontal, X } from "lucide-react";
import type { LucideIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";

/**
 * Per-page bulk-action toolbar. Renders sticky at the top of the
 * list page once the page is in select mode. Container-agnostic:
 * each list page passes its own `primary` / `overflow` actions
 * derived from the per-container action matrix.
 *
 * **Done vs. Clear semantics:**
 *   - **Clear** empties `selected` but stays in select mode. Useful
 *     for "I picked the wrong ones, start over without leaving
 *     select mode."
 *   - **Done** exits select mode entirely. Toolbar disappears;
 *     cards revert to navigate-on-click. On desktop, `Esc` maps to
 *     this. On mobile, the explicit "Done" button is the only way
 *     out.
 *
 * **Responsive overflow:** `primary` actions render inline at all
 * widths. `overflow` actions render inline at `sm+` and collapse
 * into a `MoreHorizontal` dropdown at `sm-` so the toolbar still
 * fits a 375 px viewport.
 *
 * **Mount/unmount animation (v0.3.19+):** the public `SelectionToolbar`
 * owns an `open` prop; when `open` flips false the toolbar plays an
 * exit animation, *then* unmounts. The pure markup lives in
 * `<SelectionToolbarBody>` (exported for unit tests) and the
 * presence-aware wrapper drives `data-state="open" | "closed"` for
 * the keyframes in `web/styles/globals.css`.
 *
 * Plan: `~/.claude/plans/multi-select-bulk-actions-1.0.md` (M1).
 */
export type SelectionAction = {
  id: string;
  label: string;
  icon?: LucideIcon;
  onClick: () => void;
  disabled?: boolean;
  destructive?: boolean;
};

export type SelectionToolbarProps = {
  count: number;
  total: number;
  primary: SelectionAction[];
  overflow?: SelectionAction[];
  onDone: () => void;
  onClear: () => void;
  onSelectAll?: () => void;
  /** When true, primary action buttons are disabled (mid-mutation).
   *  Lets the toolbar prevent double-firing while a bulk request
   *  is in-flight. */
  isPending?: boolean;
};

/** Length of the exit keyframe in `globals.css`. Bump in lockstep
 *  with the `selection-toolbar-out` duration there — we wait this
 *  long before unmounting so the slide-up plays in full. */
const EXIT_ANIMATION_MS = 220;

/**
 * Presence wrapper: keeps the toolbar mounted just long enough to
 * play the slide-out keyframe after `open` flips to `false`.
 *
 * Pass `open={selection.selectMode}` from each list page — the
 * surrounding `&& <Toolbar />` guards that v0.3.18 used are no
 * longer necessary (and break the exit animation since they unmount
 * before the keyframe can run).
 */
export function SelectionToolbar({
  open = true,
  ...body
}: SelectionToolbarProps & { open?: boolean }) {
  const [shouldRender, setShouldRender] = React.useState(open);
  const [phase, setPhase] = React.useState<"open" | "closed">(
    open ? "open" : "closed",
  );

  React.useEffect(() => {
    if (open) {
      setShouldRender(true);
      // Two RAFs so the browser paints the closed state first; without
      // this, mounting with `data-state="open"` from a clean tree
      // skips the in-animation because the keyframe's `from` frame
      // never gets a layout pass.
      let inner = 0;
      const outer = requestAnimationFrame(() => {
        inner = requestAnimationFrame(() => setPhase("open"));
      });
      return () => {
        cancelAnimationFrame(outer);
        if (inner) cancelAnimationFrame(inner);
      };
    }
    setPhase("closed");
    const t = setTimeout(() => setShouldRender(false), EXIT_ANIMATION_MS);
    return () => clearTimeout(t);
  }, [open]);

  if (!shouldRender) return null;

  return <SelectionToolbarBody {...body} dataState={phase} />;
}

/**
 * Pure-render body. Exported separately so unit tests can call it
 * as a plain function (vitest node-env can't call hooks). The
 * `dataState` prop is wired by `<SelectionToolbar>`; callers should
 * generally use the presence wrapper, not this directly.
 */
export function SelectionToolbarBody({
  count,
  total,
  primary,
  overflow,
  onDone,
  onClear,
  onSelectAll,
  isPending,
  dataState = "open",
}: SelectionToolbarProps & { dataState?: "open" | "closed" }) {
  const allSelected = count === total && total > 0;
  const overflowItems = overflow ?? [];

  return (
    <div
      role="toolbar"
      data-state={dataState}
      aria-label={`${count} item${count === 1 ? "" : "s"} selected`}
      aria-live="polite"
      className={cn(
        "bg-background/95 border-border sticky top-0 z-20 flex flex-wrap items-center gap-2 border-b py-2 backdrop-blur",
        "sm:flex-nowrap",
        // Mount + unmount animation: keyframes selection-toolbar-in
        // / -out in `web/styles/globals.css`. Toggled via the
        // `data-state` attribute the presence wrapper sets.
        "selection-toolbar-anim",
      )}
    >
      <Button
        type="button"
        variant="ghost"
        size="icon"
        onClick={onDone}
        aria-label="Done — exit select mode"
        className="h-9 w-9"
      >
        <X className="h-4 w-4" />
      </Button>

      <span className="text-sm font-medium tabular-nums">
        {count} selected
      </span>

      {onSelectAll && !allSelected && (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={onSelectAll}
          className="text-muted-foreground hover:text-foreground"
        >
          Select all ({total})
        </Button>
      )}

      {count > 0 && (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={onClear}
          className="text-muted-foreground hover:text-foreground"
        >
          Clear
        </Button>
      )}

      <div className="ml-auto flex items-center gap-1">
        {primary.map((a) => {
          const Icon = a.icon;
          return (
            <Button
              key={a.id}
              type="button"
              variant={a.destructive ? "destructive" : "default"}
              size="sm"
              onClick={a.onClick}
              disabled={a.disabled || isPending || count === 0}
            >
              {Icon && <Icon className="mr-1.5 h-4 w-4" />}
              {a.label}
            </Button>
          );
        })}

        {/* Overflow actions: inline at sm+, dropdown at sm-. The
         *  hidden/flex toggle is done via Tailwind responsive
         *  classes rather than JS-driven media queries to keep
         *  SSR + hydration consistent. */}
        {overflowItems.length > 0 && (
          <>
            <div className="hidden items-center gap-1 sm:flex">
              {overflowItems.map((a) => {
                const Icon = a.icon;
                return (
                  <Button
                    key={a.id}
                    type="button"
                    variant={a.destructive ? "destructive" : "outline"}
                    size="sm"
                    onClick={a.onClick}
                    disabled={a.disabled || isPending || count === 0}
                  >
                    {Icon && <Icon className="mr-1.5 h-4 w-4" />}
                    {a.label}
                  </Button>
                );
              })}
            </div>
            <div className="flex sm:hidden">
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    type="button"
                    variant="outline"
                    size="icon"
                    aria-label="More actions"
                    aria-haspopup="menu"
                    disabled={isPending || count === 0}
                    className="h-9 w-9"
                  >
                    <MoreHorizontal className="h-4 w-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  {overflowItems.map((a, i) => {
                    const Icon = a.icon;
                    return (
                      <React.Fragment key={a.id}>
                        {a.destructive && i > 0 && <DropdownMenuSeparator />}
                        <DropdownMenuItem
                          onClick={a.onClick}
                          disabled={a.disabled}
                          className={
                            a.destructive ? "text-destructive" : undefined
                          }
                        >
                          {Icon && <Icon className="mr-2 h-4 w-4" />}
                          {a.label}
                        </DropdownMenuItem>
                      </React.Fragment>
                    );
                  })}
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
