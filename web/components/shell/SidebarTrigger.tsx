"use client";

import { PanelLeft, PanelLeftClose } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

/**
 * Desktop-only sidebar toggle. The icon flips between
 * `PanelLeftClose` (will collapse) and `PanelLeft` (will expand) so the
 * affordance previews the next state — matches Cursor / GitHub / Linear.
 *
 * Mobile menu is the shell's existing `<Sheet>` hamburger; this trigger is
 * hidden below `md` so the two affordances don't both render at once.
 */
export function SidebarTrigger({
  collapsed,
  onToggle,
}: {
  collapsed: boolean;
  onToggle: () => void;
}) {
  const Icon = collapsed ? PanelLeft : PanelLeftClose;
  const label = collapsed ? "Expand sidebar" : "Collapse sidebar";
  return (
    <TooltipProvider delayDuration={200}>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            onClick={onToggle}
            aria-label={label}
            aria-expanded={!collapsed}
            className="hidden md:inline-flex"
          >
            <Icon className="h-5 w-5" />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="bottom">
          {label}
          <span className="text-muted-foreground ml-2 text-[10px] tracking-wider uppercase">
            ⌘B
          </span>
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
