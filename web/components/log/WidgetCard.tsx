"use client";

import * as React from "react";
import { GripVertical, MoreVertical, Settings2, Trash2 } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
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
import { cn } from "@/lib/utils";

/** Uniform shell every reading-log widget renders inside. Provides
 *  the title row, the kebab menu (Configure → M5 dialog stub,
 *  Remove → confirm + mutation), and a drag-handle slot the M5 grid
 *  hooks DnD into. The body renders whatever the kind-specific
 *  component passes as `children`. */
export function WidgetCard({
  widgetId,
  title,
  subtitle,
  Icon,
  children,
  /** When false, suppresses the kebab menu entirely. Useful for the
   *  `note` widget where Remove/Configure live inside the body. Off
   *  by default — every other widget surfaces the menu. */
  showMenu = true,
  /** Drag-handle render prop. M5 supplies @dnd-kit listeners /
   *  attributes here; M4 leaves it `undefined` so the handle is a
   *  visual placeholder without behavior. */
  dragHandleProps,
  className,
}: {
  widgetId: string;
  title: string;
  subtitle?: string;
  Icon?: React.ComponentType<{ className?: string }>;
  children: React.ReactNode;
  showMenu?: boolean;
  dragHandleProps?: React.HTMLAttributes<HTMLButtonElement>;
  className?: string;
}) {
  const remove = useRemoveLogWidget(widgetId);
  const [confirmOpen, setConfirmOpen] = React.useState(false);

  return (
    <Card className={cn("group/widget relative", className)}>
      <CardHeader className="flex flex-row items-center gap-2 pb-3">
        <button
          type="button"
          aria-label="Reorder widget"
          {...dragHandleProps}
          className={cn(
            "text-muted-foreground/40 hover:text-muted-foreground -ml-1 hidden h-6 w-6 cursor-grab items-center justify-center rounded transition-colors group-hover/widget:flex active:cursor-grabbing",
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
              {title}
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
              <DropdownMenuItem disabled title="Configuration UI lands in M5">
                <Settings2 aria-hidden="true" className="mr-2 h-4 w-4" />
                Configure…
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                className="text-destructive focus:text-destructive"
                onSelect={(e) => {
                  // shadcn DropdownMenuItem closes on select; we want
                  // to keep focus and open the confirm dialog instead.
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
          <AlertDialogTrigger className="sr-only" tabIndex={-1} />
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Remove this widget?</AlertDialogTitle>
              <AlertDialogDescription>
                You can add it back any time from the “Add widget” menu, or use
                “Reset to defaults” to restore the original layout.
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
      </CardHeader>
      <CardContent>{children}</CardContent>
    </Card>
  );
}
