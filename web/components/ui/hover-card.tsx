"use client";

import * as React from "react";
import * as HoverCardPrimitive from "@radix-ui/react-hover-card";

import { cn } from "@/lib/utils";
import { safeAreaCollisionPadding } from "@/components/ui/collision-padding";

/**
 * Hover preview primitive (audit 3.7 discovery). Radix HoverCard is
 * pointer-hover-driven and inert on touch / keyboard-only flows by design,
 * so it layers a desktop "peek" on top of a card without changing the
 * card's click/long-press behavior. Portals to `<body>` so the preview
 * escapes the card grid's overflow + stacking context (same reasoning as
 * the tooltip primitive).
 */
const HoverCard = HoverCardPrimitive.Root;
const HoverCardTrigger = HoverCardPrimitive.Trigger;

const HoverCardContent = React.forwardRef<
  React.ComponentRef<typeof HoverCardPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof HoverCardPrimitive.Content>
>(
  (
    { className, align = "center", sideOffset = 8, collisionPadding, ...props },
    ref,
  ) => {
    const pad = React.useMemo(
      () => collisionPadding ?? safeAreaCollisionPadding(),
      [collisionPadding],
    );
    return (
      <HoverCardPrimitive.Portal>
        <HoverCardPrimitive.Content
          ref={ref}
          align={align}
          sideOffset={sideOffset}
          collisionPadding={pad}
          className={cn(
            "border-border bg-popover text-popover-foreground z-50 w-72 origin-[--radix-hover-card-content-transform-origin] rounded-md border p-3 shadow-md outline-none",
            "data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 motion-reduce:transition-none motion-reduce:data-[state=closed]:animate-none motion-reduce:data-[state=open]:animate-none",
            className,
          )}
          {...props}
        />
      </HoverCardPrimitive.Portal>
    );
  },
);
HoverCardContent.displayName = HoverCardPrimitive.Content.displayName;

export { HoverCard, HoverCardTrigger, HoverCardContent };
