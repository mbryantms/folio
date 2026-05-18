"use client";

import { Keyboard } from "lucide-react";

import { useShortcutsSheet } from "@/components/GlobalShortcutsSheet";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";

/**
 * Sidebar-bottom keyboard-shortcuts hint. The user-footer dropdown
 * still holds the primary entry; this is a low-profile discoverability
 * nudge — a small muted "Shortcuts ?" line in expanded mode, and an
 * icon-only tooltip button when the sidebar is collapsed.
 */
export function ShortcutsHelpButton({
  collapsed = false,
}: {
  collapsed?: boolean;
}) {
  const shortcuts = useShortcutsSheet();
  if (collapsed) {
    return (
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            onClick={() => shortcuts.open()}
            aria-label="Keyboard shortcuts"
            className="text-muted-foreground/70 hover:text-foreground mx-auto flex size-7 items-center justify-center rounded-md transition-colors"
          >
            <Keyboard className="h-3.5 w-3.5" aria-hidden="true" />
          </button>
        </TooltipTrigger>
        <TooltipContent side="right" sideOffset={8}>
          Keyboard shortcuts
        </TooltipContent>
      </Tooltip>
    );
  }
  return (
    <button
      type="button"
      onClick={() => shortcuts.open()}
      aria-label="Keyboard shortcuts"
      className="text-muted-foreground/60 hover:text-foreground ml-auto flex items-center gap-1.5 px-1 py-0.5 text-[11px] transition-colors"
    >
      <span>Shortcuts</span>
      <kbd className="border-border/40 inline-flex h-4 min-w-4 items-center justify-center rounded border px-1 font-mono text-[10px] leading-none">
        ?
      </kbd>
    </button>
  );
}
