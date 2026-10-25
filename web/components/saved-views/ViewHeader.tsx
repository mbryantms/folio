"use client";

import * as React from "react";
import { Lock } from "lucide-react";

import { cn } from "@/lib/utils";
import type { SavedViewView } from "@/lib/api/types";

import { ViewActionsMenu } from "./ViewActionsMenu";

/** Detail-page header shared by both filter and CBL view types.
 *
 *  Layout:
 *    [title]                                      [extraActions] [⋯]
 *
 *  `extraActions` are the inline controls that stay visible at-a-glance
 *  (stats pills, CardSizeOptions, badges). Everything else — Edit,
 *  Sidebar toggle, Pin/Unpin, and any caller-supplied `extraMenuItems`
 *  (Export, Refresh, …) — collapses into the `⋯` overflow menu. Mobile
 *  used to overflow with seven inline buttons; collapsing keeps the
 *  same UI on both viewports.
 *
 *  System views (`is_system = true`) omit Edit and surface a small
 *  read-only `Built-in` chip inline so the constraint is visible
 *  instead of just dimmed.
 *
 *  The `⋯` itself is `ViewActionsMenu`; pass `actions="none"` to render
 *  it elsewhere (the CBL page puts it in its pinned control row).
 *
 *  Phones (< `sm`): the `extraActions` cluster is hidden — pages pass
 *  the same controls compacted into a `PhoneControlsBar` via
 *  `phoneControls`, which renders as a *sibling* of the `<header>`
 *  (not inside it) so its `position: sticky` is scoped to the page
 *  column rather than to the header's own box. Only the `Built-in`
 *  chip and the `⋯` trigger stay in the title row.
 */
export function ViewHeader({
  view,
  onEdit,
  titleSuffix,
  extraActions,
  extraMenuItems,
  phoneControls,
  phoneControlsPlacement = "row",
  actions = "inline",
  className,
}: {
  view: SavedViewView;
  onEdit?: () => void;
  /** Optional node rendered directly under the title and above the
   *  description. CBL views feed in their year range here so it
   *  sits prominently rather than living in the small-text info row
   *  below. Kept generic so other view kinds can use the slot too. */
  titleSuffix?: React.ReactNode;
  /** Inline controls placed to the right of the title and left of the
   *  overflow menu. Use for stats, density toggles, badges — anything
   *  the user wants visible at a glance. */
  extraActions?: React.ReactNode;
  /** Items appended to the overflow menu *between* Edit and the
   *  Sidebar/Pin actions. Each item should be a `<DropdownMenuItem>`
   *  (or `<DropdownMenuSeparator>`). Caller-specific actions like
   *  Export and Refresh live here. */
  extraMenuItems?: React.ReactNode;
  /** Phone-only (`< sm`) replacement for `extraActions`: normally a
   *  `<PhoneControlsBar>` (which carries `sm:hidden` and is sticky).
   *  Rendered directly under the title block as its own row. */
  phoneControls?: React.ReactNode;
  /** Where `phoneControls` goes. `"row"` (default): its own sticky row
   *  under the header — for bars that carry search or a leading slot
   *  (CBL). `"inline"`: inside the title row's right cluster, before
   *  the `⋯` actions menu — for bars that are a single icon (card size
   *  + Select); pass `sticky={false}` on the bar then. */
  phoneControlsPlacement?: "row" | "inline";
  /** Where the `⋯` More-actions menu renders. `"inline"` (default): at
   *  the end of the title row's right cluster. `"none"`: the header
   *  renders no menu and the page places `<ViewActionsMenu>` itself —
   *  CBL reading lists put it in their pinned control row at every
   *  width so the title row is title-only. */
  actions?: "inline" | "none";
  className?: string;
}) {
  return (
    <>
      <header className={cn("space-y-3", className)}>
        {/* Phones never wrap this row: the cluster is only the chip +
         *  `⋯` there (`extraActions` is hidden), so it stays pinned to
         *  the title's right while the title wraps. `sm:` and up keep
         *  `flex-wrap` so a wide desktop cluster can fold onto its own
         *  line instead of pushing `⋯` off-screen. */}
        <div className="flex items-start justify-between gap-3 sm:flex-wrap">
          <div className="min-w-0 space-y-1">
            {/* `md:truncate` only — on narrow viewports let the long CBL
             *  titles wrap onto multiple lines instead of clipping off
             *  the right edge. Desktop keeps the single-line truncate so
             *  pathological titles don't push the action cluster
             *  off-screen. */}
            {/* Title + suffix layout:
             *    Mobile  → stacked (`flex-col`). Title takes full width
             *              and wraps as needed; suffix appears on its
             *              own line directly below.
             *    Desktop → inline + baseline-aligned. `min-w-0` on the
             *              h1 keeps `md:truncate` working inside the
             *              flex line; `shrink-0` on the suffix keeps it
             *              visible when the title is long. */}
            <div className="flex min-w-0 flex-col gap-1 md:flex-row md:items-baseline md:gap-3">
              <h1
                className="min-w-0 text-2xl font-semibold tracking-tight max-sm:text-xl md:truncate"
                title={view.name}
              >
                {view.name}
              </h1>
              {titleSuffix ? (
                <span className="text-muted-foreground text-base font-medium max-sm:text-sm md:shrink-0">
                  {titleSuffix}
                </span>
              ) : null}
            </div>
            {view.description ? (
              <p className="text-muted-foreground text-sm max-sm:line-clamp-2 max-sm:text-xs">
                {view.description}
              </p>
            ) : null}
          </div>
          {/* `min-w-0 max-w-full` and no `shrink-0` from `sm` up: the
           *  cluster must be able to shrink to the row so its own
           *  `flex-wrap` can fold the controls onto more lines. With
           *  `md:shrink-0` an iPad with the sidebar open laid the cluster
           *  out at its full content width (wider than the page column) and
           *  the overflow menu's trigger sat past the right edge of the
           *  screen — the menu opened off-screen. Desktop still gets one
           *  row whenever the set fits. Phones hide `extraActions`, so the
           *  remaining chip + `⋯` are narrow enough to never shrink. */}
          <div className="flex max-w-full min-w-0 flex-wrap items-center justify-end gap-2 max-sm:shrink-0">
            {extraActions ? (
              <div
                data-view-header-actions=""
                className="flex max-w-full min-w-0 flex-wrap items-center justify-end gap-2 max-sm:hidden"
              >
                {extraActions}
              </div>
            ) : null}
            {phoneControls && phoneControlsPlacement === "inline"
              ? phoneControls
              : null}
            {view.is_system ? (
              <span
                className="text-muted-foreground bg-muted/40 inline-flex items-center rounded-md border px-2.5 py-1 text-xs font-medium"
                title="Built-in views can't be edited or deleted"
              >
                <Lock className="mr-1 h-3 w-3" /> Built-in
              </span>
            ) : null}
            {actions === "inline" ? (
              <ViewActionsMenu
                view={view}
                onEdit={onEdit}
                extraMenuItems={extraMenuItems}
              />
            ) : null}
          </div>
        </div>
      </header>
      {/* Sibling, not a child, of `<header>` — see the component doc:
       *  a sticky bar inside the header could only stick while the
       *  header itself is on screen. */}
      {phoneControlsPlacement === "row" ? phoneControls : null}
    </>
  );
}
