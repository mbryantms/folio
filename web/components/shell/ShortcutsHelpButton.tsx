"use client";

import { Keyboard } from "lucide-react";

import { useShortcutsSheet } from "@/components/GlobalShortcutsSheet";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

/**
 * Sidebar-bottom affordance for the global keyboard-shortcuts sheet.
 * Mirrors the user-footer dropdown entry but exposes the sheet without
 * requiring a menu click — improves discoverability for new users. The
 * collapsed variant renders icon-only with a hover tooltip, matching
 * the rest of the sidebar's collapsed look.
 */
export function ShortcutsHelpButton({
  collapsed = false,
}: {
  collapsed?: boolean;
}) {
  const shortcuts = useShortcutsSheet();
  const trigger = (
    <button
      type="button"
      onClick={() => shortcuts.open()}
      aria-label="Keyboard shortcuts"
      className={cn(
        "text-muted-foreground hover:bg-secondary/50 hover:text-foreground flex w-full items-center rounded-md transition-colors",
        collapsed
          ? "mx-auto size-9 justify-center"
          : "gap-2.5 px-3 py-1.5 text-sm",
      )}
    >
      <Keyboard className="h-4 w-4 shrink-0" aria-hidden="true" />
      {!collapsed && (
        <>
          <span className="truncate">Keyboard shortcuts</span>
          <kbd className="border-border/60 text-muted-foreground ml-auto inline-flex h-5 min-w-5 items-center justify-center rounded border px-1.5 font-mono text-[10px]">
            ?
          </kbd>
        </>
      )}
    </button>
  );
  if (collapsed) {
    return (
      <Tooltip>
        <TooltipTrigger asChild>{trigger}</TooltipTrigger>
        <TooltipContent side="right" sideOffset={8}>
          Keyboard shortcuts
        </TooltipContent>
      </Tooltip>
    );
  }
  return trigger;
}
