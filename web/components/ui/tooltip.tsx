"use client";

import * as React from "react";
import * as TooltipPrimitive from "@radix-ui/react-tooltip";

import { cn } from "@/lib/utils";
import { safeAreaCollisionPadding } from "@/components/ui/collision-padding";

const TooltipProvider = TooltipPrimitive.Provider;
const Tooltip = TooltipPrimitive.Root;
const TooltipTrigger = TooltipPrimitive.Trigger;

const TooltipContent = React.forwardRef<
  React.ComponentRef<typeof TooltipPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof TooltipPrimitive.Content>
>(({ className, sideOffset = 4, collisionPadding, ...props }, ref) => {
  const pad = React.useMemo(
    () => collisionPadding ?? safeAreaCollisionPadding(),
    [collisionPadding],
  );
  return (
    // Portal to <body> so the tooltip escapes its trigger's stacking context
    // and overflow. Without it, a collapsed-sidebar icon's `side="right"`
    // tooltip rendered inside the sidebar's context — below the content
    // area, so cover cards painted over it (and the sidebar's overflow could
    // clip it). `z-50` only competes within a context; the portal lifts it
    // to the top level where it wins.
    <TooltipPrimitive.Portal>
      <TooltipPrimitive.Content
        ref={ref}
        sideOffset={sideOffset}
        collisionPadding={pad}
        className={cn(
          "border-border bg-popover text-popover-foreground z-50 origin-[--radix-tooltip-content-transform-origin] overflow-hidden rounded-md border px-3 py-1.5 text-xs shadow-md transition-[opacity,transform] duration-150 ease-out data-[state=closed]:scale-95 data-[state=closed]:opacity-0 motion-reduce:transition-none",
          className,
        )}
        {...props}
      />
    </TooltipPrimitive.Portal>
  );
});
TooltipContent.displayName = TooltipPrimitive.Content.displayName;

export { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider };
