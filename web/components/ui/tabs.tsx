"use client";

import * as React from "react";
import * as TabsPrimitive from "@radix-ui/react-tabs";

import { cn } from "@/lib/utils";

const Tabs = TabsPrimitive.Root;

const TabsList = React.forwardRef<
  React.ComponentRef<typeof TabsPrimitive.List>,
  React.ComponentPropsWithoutRef<typeof TabsPrimitive.List>
>(({ className, ...props }, ref) => (
  <TabsPrimitive.List
    ref={ref}
    className={cn(
      // Mobile: full-width row that scrolls horizontally rather than
      //   overflowing the viewport (the prior `inline-flex` blew out the
      //   layout on phones with >3 tabs — see screenshot in the
      //   runtime-config-admin work). md+: content-width like before.
      // `justify-[safe_center]` is the CSS "safe center" keyword pair —
      //   center the children when they fit, fall back to start when
      //   they overflow. Without `safe`, an overflowing strip centers
      //   the row in place, clipping both ends equally and burying the
      //   first tab; with `safe`, overflow scrolls from the leading
      //   edge as users expect. A plain `justify-start` would always
      //   left-align, which broke the centered 2-tab sign-in form.
      // `[&>*]:shrink-0` keeps each trigger at its intrinsic width so
      //   long labels ("Cast & Setting") stay readable inside the
      //   scrollable strip.
      "bg-muted text-muted-foreground flex h-9 w-full items-center justify-[safe_center] overflow-x-auto rounded-md p-1 md:inline-flex md:w-fit md:overflow-visible",
      "[&::-webkit-scrollbar]:hidden [&>*]:shrink-0 [scrollbar-width:none]",
      className,
    )}
    {...props}
  />
));
TabsList.displayName = TabsPrimitive.List.displayName;

const TabsTrigger = React.forwardRef<
  React.ComponentRef<typeof TabsPrimitive.Trigger>,
  React.ComponentPropsWithoutRef<typeof TabsPrimitive.Trigger>
>(({ className, ...props }, ref) => (
  <TabsPrimitive.Trigger
    ref={ref}
    className={cn(
      "ring-offset-background focus-visible:ring-ring data-[state=active]:bg-background data-[state=active]:text-foreground inline-flex items-center justify-center rounded-sm px-3 py-1 text-sm font-medium whitespace-nowrap transition-all focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:outline-none disabled:pointer-events-none disabled:opacity-50 data-[state=active]:shadow",
      className,
    )}
    {...props}
  />
));
TabsTrigger.displayName = TabsPrimitive.Trigger.displayName;

const TabsContent = React.forwardRef<
  React.ComponentRef<typeof TabsPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof TabsPrimitive.Content>
>(({ className, ...props }, ref) => (
  <TabsPrimitive.Content
    ref={ref}
    className={cn(
      "ring-offset-background focus-visible:ring-ring mt-4 focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:outline-none",
      className,
    )}
    {...props}
  />
));
TabsContent.displayName = TabsPrimitive.Content.displayName;

export { Tabs, TabsList, TabsTrigger, TabsContent };
