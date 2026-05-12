"use client";

import * as React from "react";
import * as PopoverPrimitive from "@radix-ui/react-popover";

import { cn } from "@/lib/utils";

const Popover = PopoverPrimitive.Root;
const PopoverTrigger = PopoverPrimitive.Trigger;

/** Default-null portal-target context. Wrap a parent (e.g. a Dialog
 *  body) with `<PopoverPortalContainer value={dialogContentEl}>` to
 *  re-anchor every descendant Popover into that subtree. Without this,
 *  the popover portals to `document.body` and Radix Dialog's modal
 *  aria-hides it — the symptom is "popover opens but the search input
 *  inside it doesn't accept focus". */
const PopoverPortalContext = React.createContext<HTMLElement | null>(null);

export const PopoverPortalContainer = PopoverPortalContext.Provider;

const PopoverContent = React.forwardRef<
  React.ComponentRef<typeof PopoverPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof PopoverPrimitive.Content>
>(({ className, align = "center", sideOffset = 4, ...props }, ref) => {
  const container = React.useContext(PopoverPortalContext);
  return (
    <PopoverPrimitive.Portal container={container ?? undefined}>
      <PopoverPrimitive.Content
        ref={ref}
        align={align}
        sideOffset={sideOffset}
        className={cn(
          "border-border bg-popover text-popover-foreground data-[state=open]:animate-in data-[state=closed]:animate-out z-50 w-72 rounded-md border p-4 shadow-md outline-none",
          className,
        )}
        {...props}
      />
    </PopoverPrimitive.Portal>
  );
});
PopoverContent.displayName = PopoverPrimitive.Content.displayName;

export { Popover, PopoverTrigger, PopoverContent };
