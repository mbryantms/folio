"use client";

import * as React from "react";
import { ChevronDown, MoreHorizontal, X } from "lucide-react";
import type { LucideIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
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
 * **Responsive overflow:** `primary` and grouped actions render
 * inline at all widths. Utility actions and `overflow` actions
 * render inline only at `2xl+` and collapse into a `MoreHorizontal`
 * dropdown below that so the toolbar stays usable on narrow screens.
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

export type SelectionActionGroup = {
  id: string;
  label: string;
  icon?: LucideIcon;
  actions: SelectionAction[];
  disabled?: boolean;
};

export type SelectionToolbarProps = {
  count: number;
  total: number;
  primary: SelectionAction[];
  overflow?: SelectionAction[];
  actionGroups?: SelectionActionGroup[];
  onDone: () => void;
  onClear: () => void;
  onSelectAll?: () => void;
  onSelectAllMatching?: () => void;
  matchingTotal?: number | null;
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
      // Keep a real mounted copy after the first open frame. The render
      // path below already paints the closed wrapper immediately when
      // `open` flips true, avoiding the one-frame gap where the trigger
      // disappeared but the toolbar was still null.
      // eslint-disable-next-line react-hooks/set-state-in-effect
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

  if (!shouldRender && !open) return null;

  // Outer wrapper handles the height collapse via the modern
  // `grid-template-rows: 0fr ↔ 1fr` trick so content below the
  // toolbar slides down/up smoothly instead of jumping when the
  // toolbar mounts/unmounts. The inner body still owns the
  // fade + translate via its keyframes; together they read as a
  // single coordinated motion.
  return (
    <div data-state={phase} className="selection-toolbar-wrap">
      <div className="selection-toolbar-wrap-inner">
        <SelectionToolbarBody {...body} dataState={phase} />
      </div>
    </div>
  );
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
  actionGroups,
  onDone,
  onClear,
  onSelectAll,
  onSelectAllMatching,
  matchingTotal,
  isPending,
  dataState = "open",
}: SelectionToolbarProps & { dataState?: "open" | "closed" }) {
  const allSelected = count === total && total > 0;
  const overflowItems = overflow ?? [];
  const groupedItems = actionGroups ?? [];
  const canSelectAll = !!onSelectAll && !allSelected;
  const canSelectAllMatching = !!onSelectAllMatching;
  const canClear = count > 0;
  const hasOverflowMenu =
    overflowItems.length > 0 ||
    !!onSelectAll ||
    canSelectAllMatching ||
    canClear;
  const actionDisabled = isPending || count === 0;
  const matchingLabel =
    matchingTotal != null
      ? `Select all matching (${matchingTotal})`
      : "Select all matching";

  return (
    <div
      role="toolbar"
      data-state={dataState}
      aria-label={`${count} item${count === 1 ? "" : "s"} selected`}
      aria-live="polite"
      className={cn(
        "bg-background/95 border-border sticky top-0 z-20 grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2 border-b py-2 backdrop-blur",
        "sm:flex sm:flex-wrap 2xl:flex-nowrap",
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

      <span className="min-w-[5.75rem] text-sm font-medium tabular-nums">
        {count} selected
      </span>

      <div className="hidden min-w-[13rem] items-center gap-1 2xl:flex">
        {canSelectAll && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={onSelectAll}
            className="text-muted-foreground hover:text-foreground"
          >
            Select loaded ({total})
          </Button>
        )}

        {canSelectAllMatching && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={onSelectAllMatching}
            className="text-muted-foreground hover:text-foreground"
          >
            {matchingLabel}
          </Button>
        )}

        {canClear && (
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
      </div>

      {hasOverflowMenu && (
        <div className="flex justify-end 2xl:hidden">
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                type="button"
                variant="outline"
                size="icon"
                aria-label="More actions"
                aria-haspopup="menu"
                disabled={
                  isPending &&
                  overflowItems.length > 0 &&
                  !canSelectAll &&
                  !canSelectAllMatching &&
                  !canClear
                }
                className="h-9 w-9"
              >
                <MoreHorizontal className="h-4 w-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {canSelectAll && (
                <DropdownMenuItem onClick={onSelectAll}>
                  Select loaded ({total})
                </DropdownMenuItem>
              )}
              {canSelectAllMatching && (
                <DropdownMenuItem onClick={onSelectAllMatching}>
                  {matchingLabel}
                </DropdownMenuItem>
              )}
              {canClear && (
                <DropdownMenuItem onClick={onClear}>Clear</DropdownMenuItem>
              )}
              {(canSelectAll || canSelectAllMatching || canClear) &&
                overflowItems.length > 0 && <DropdownMenuSeparator />}
              {overflowItems.map((a, i) => {
                const Icon = a.icon;
                return (
                  <React.Fragment key={a.id}>
                    {a.destructive && i > 0 && <DropdownMenuSeparator />}
                    <DropdownMenuItem
                      onClick={a.onClick}
                      disabled={a.disabled || actionDisabled}
                      className={a.destructive ? "text-destructive" : undefined}
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
      )}

      <div className="sm:col-span-auto col-span-3 flex min-w-0 items-center gap-1 sm:ml-auto">
        {primary.map((a) => {
          const Icon = a.icon;
          return (
            <Button
              key={a.id}
              type="button"
              variant={a.destructive ? "destructive" : "default"}
              size="sm"
              onClick={a.onClick}
              disabled={a.disabled || actionDisabled}
              className="min-w-0 flex-1 sm:flex-none"
            >
              {Icon && <Icon className="mr-1.5 h-4 w-4" />}
              {a.label}
            </Button>
          );
        })}

        {groupedItems.map((group) => {
          const GroupIcon = group.icon;
          const triggerDisabled =
            group.disabled ||
            actionDisabled ||
            group.actions.every((a) => a.disabled);
          return (
            <DropdownMenu key={group.id}>
              <DropdownMenuTrigger asChild>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={triggerDisabled}
                  className="min-w-0 flex-1 sm:flex-none"
                >
                  {GroupIcon && <GroupIcon className="mr-1.5 h-4 w-4" />}
                  {group.label}
                  <ChevronDown className="ml-1.5 h-3.5 w-3.5" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuLabel>{group.label}</DropdownMenuLabel>
                {group.actions.map((a, i) => {
                  const Icon = a.icon;
                  return (
                    <React.Fragment key={a.id}>
                      {a.destructive && i > 0 && <DropdownMenuSeparator />}
                      <DropdownMenuItem
                        onClick={a.onClick}
                        disabled={
                          group.disabled || a.disabled || actionDisabled
                        }
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
          );
        })}

        {/* Utility + overflow actions render inline only on very wide
         *  screens. Below that, the menu above owns them so the primary
         *  actions never collide with long "Select all matching" labels. */}
        {overflowItems.length > 0 && (
          <div className="hidden items-center gap-1 2xl:flex">
            {overflowItems.map((a) => {
              const Icon = a.icon;
              return (
                <Button
                  key={a.id}
                  type="button"
                  variant={a.destructive ? "destructive" : "outline"}
                  size="sm"
                  onClick={a.onClick}
                  disabled={a.disabled || actionDisabled}
                >
                  {Icon && <Icon className="mr-1.5 h-4 w-4" />}
                  {a.label}
                </Button>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
