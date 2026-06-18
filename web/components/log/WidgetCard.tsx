"use client";

import * as React from "react";
import Link from "next/link";
import {
  ChevronRight,
  GripVertical,
  MoreVertical,
  Settings2,
  Trash2,
} from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useRemoveLogWidget } from "@/lib/api/mutations";
import type { LogWidgetView } from "@/lib/api/types";
import { cn } from "@/lib/utils";

import { ConfigureWidgetDialog } from "./ConfigureWidgetDialog";
import { useDragInfo } from "./LogWidgetGrid";

/** Uniform shell every reading-log widget renders inside.
 *
 *  Owns three dialog/menu surfaces:
 *    - Kebab dropdown: `Configure…` / `Remove`
 *    - Configure dialog: opens a kind-specific form (M5)
 *    - Remove confirm: AlertDialog → `useRemoveLogWidget`
 *
 *  The drag handle (left edge, hover-visible) accepts `dragHandleProps`
 *  from the parent grid — the grid spreads `@dnd-kit` listeners +
 *  attributes onto it so the entire card can move from one spot.
 *  M4 left the slot empty; M5 wires it. */
export function WidgetCard({
  widget,
  title,
  /** Optional href the title links to. When set, the title text
   *  renders as a `<Link>` with a trailing chevron — used by the
   *  chrono_feed widget to link out to `/log/activity`, the rich
   *  activity report. */
  titleHref,
  subtitle,
  Icon,
  children,
  showMenu = true,
  extraMenuItems,
  className,
}: {
  widget: LogWidgetView;
  title: string;
  titleHref?: string;
  subtitle?: string;
  Icon?: React.ComponentType<{ className?: string }>;
  children: React.ReactNode;
  showMenu?: boolean;
  /** Optional renderer for widget-specific menu items. Rendered above
   *  the standard `Configure… / Remove` block, with a separator in
   *  between when present. Use this for toggles like "Show hidden"
   *  on the chrono feed so per-widget preferences live alongside
   *  the widget's other actions instead of cluttering the card body. */
  extraMenuItems?: React.ReactNode;
  className?: string;
}) {
  const remove = useRemoveLogWidget(widget.id);
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const [configureOpen, setConfigureOpen] = React.useState(false);
  // Drag handle + state come from the enclosing SortableContext via
  // `useDragInfo`. Null when the card renders outside a grid (e.g.,
  // a future read-only embed or test harness).
  const drag = useDragInfo();
  const dragHandleProps = drag?.dragHandleProps;
  const isDragging = drag?.isDragging ?? false;

  return (
    <Card
      className={cn(
        "group/widget relative",
        isDragging && "opacity-50",
        className,
      )}
    >
      <CardHeader className="flex flex-row items-center gap-2 pb-3">
        <button
          type="button"
          aria-label="Reorder widget"
          {...dragHandleProps}
          className={cn(
            // Opacity (not `display`) reveal so the handle stays in the tab
            // order and a keyboard user can reach it — `display:none` made the
            // configured KeyboardSensor dead code (audit E8). 44px hit area
            // for touch; the grip glyph stays small.
            "text-muted-foreground/40 hover:text-muted-foreground -ml-1.5 flex h-11 w-8 cursor-grab touch-none items-center justify-center rounded transition-opacity active:cursor-grabbing",
            "focus-visible:ring-ring opacity-0 group-hover/widget:opacity-100 focus-visible:opacity-100 focus-visible:ring-2 focus-visible:outline-none",
            // Always visible on mobile (no hover) so reorder isn't gated.
            "max-md:opacity-100",
          )}
        >
          <GripVertical aria-hidden="true" className="h-4 w-4" />
        </button>
        <div className="flex min-w-0 flex-1 items-center gap-2">
          {Icon ? (
            <Icon className="text-muted-foreground h-4 w-4 shrink-0" />
          ) : null}
          <div className="min-w-0">
            <CardTitle className="truncate text-base" title={title}>
              {titleHref ? (
                <Link
                  href={titleHref}
                  className="hover:text-primary inline-flex items-center gap-1 transition-colors"
                >
                  {title}
                  <ChevronRight aria-hidden="true" className="h-4 w-4" />
                </Link>
              ) : (
                title
              )}
            </CardTitle>
            {subtitle ? (
              <p
                className="text-muted-foreground truncate text-xs"
                title={subtitle}
              >
                {subtitle}
              </p>
            ) : null}
          </div>
        </div>
        {showMenu ? (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                aria-label="Widget options"
                className="text-muted-foreground/60 hover:text-foreground hover:bg-muted/50 inline-flex h-7 w-7 items-center justify-center rounded transition-colors"
              >
                <MoreVertical aria-hidden="true" className="h-4 w-4" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {extraMenuItems ? (
                <>
                  {extraMenuItems}
                  <DropdownMenuSeparator />
                </>
              ) : null}
              <DropdownMenuItem
                onSelect={(e) => {
                  e.preventDefault();
                  setConfigureOpen(true);
                }}
              >
                <Settings2 aria-hidden="true" className="mr-2 h-4 w-4" />
                Configure…
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                className="text-destructive focus:text-destructive"
                onSelect={(e) => {
                  // shadcn DropdownMenuItem closes on select; keep
                  // focus and open the confirm dialog instead.
                  e.preventDefault();
                  setConfirmOpen(true);
                }}
              >
                <Trash2 aria-hidden="true" className="mr-2 h-4 w-4" />
                Remove
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        ) : null}
        <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Remove this widget?</AlertDialogTitle>
              <AlertDialogDescription>
                You can add it back any time from the &ldquo;Add widget&rdquo;
                menu, or use &ldquo;Reset to defaults&rdquo; to restore the
                original layout.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction
                onClick={() => remove.mutate()}
                className="bg-destructive hover:bg-destructive/90 text-destructive-foreground"
              >
                Remove
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
        <ConfigureWidgetDialog
          widget={widget}
          open={configureOpen}
          onOpenChange={setConfigureOpen}
        />
      </CardHeader>
      <CardContent>{children}</CardContent>
    </Card>
  );
}
