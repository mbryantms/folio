"use client";

import * as React from "react";
import { Tabs as TabsPrimitive } from "radix-ui";

import { cn } from "@/lib/utils";

const Tabs = TabsPrimitive.Root;

type EdgeFade = "" | "left" | "right" | "both";

/** Which edges of a horizontally scrolling strip hide more content. iOS
 *  draws no scrollbar until a swipe starts, so a clipped last tab read
 *  as a layout bug; a short gradient mask on the overflowing edge says
 *  "more this way" and goes away once scrolled to that end. */
function useEdgeFade<T extends HTMLElement>(): [
  React.RefObject<T | null>,
  EdgeFade,
] {
  const ref = React.useRef<T | null>(null);
  const [fade, setFade] = React.useState<EdgeFade>("");
  React.useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const update = () => {
      const left = el.scrollLeft > 1;
      const right = el.scrollLeft + el.clientWidth < el.scrollWidth - 1;
      setFade(left && right ? "both" : left ? "left" : right ? "right" : "");
    };
    update();
    el.addEventListener("scroll", update, { passive: true });
    window.addEventListener("resize", update);
    const ro =
      typeof ResizeObserver !== "undefined" ? new ResizeObserver(update) : null;
    ro?.observe(el);
    return () => {
      el.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
      ro?.disconnect();
    };
  }, []);
  return [ref, fade];
}

const TabsList = React.forwardRef<
  React.ComponentRef<typeof TabsPrimitive.List>,
  React.ComponentPropsWithoutRef<typeof TabsPrimitive.List>
>(({ className, ...props }, ref) => {
  const [inner, fade] = useEdgeFade<HTMLDivElement>();
  React.useImperativeHandle(ref, () => inner.current as HTMLDivElement);
  return (
    <TabsPrimitive.List
      ref={inner}
      data-fade={fade || undefined}
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
        // md+: content-width, but capped at the parent and still scrollable
        //   — a long strip (the series page's 7-8 tabs beside the sidebar
        //   at 768px) otherwise pushed the whole page into horizontal
        //   scroll. The `p-1` leaves room for the triggers' focus ring
        //   (ring-2 + offset-2) inside the scroller.
        // `overflow-y-hidden`: `overflow-x-auto` alone makes the strip a
        //   scroll container on both axes, and on touch each trigger's
        //   invisible 44px tap area (`.folio-tap::after`, globals.css)
        //   pokes 4px past the 36px strip — iOS then lets a finger drag
        //   the labels up and down inside the bar. Hidden keeps the strip
        //   horizontal-only and hands vertical swipes to the page.
        "bg-muted text-muted-foreground justify-[safe_center] flex h-9 w-full max-w-full items-center overflow-x-auto overflow-y-hidden rounded-md p-1 md:inline-flex md:w-fit",
        "[scrollbar-width:none] [&::-webkit-scrollbar]:hidden [&>*]:shrink-0",
        // Edge fade (see `useEdgeFade`): a 2rem gradient mask on whichever
        //   side still hides tabs. Never a scrollbar.
        "data-[fade=both]:mask-x-from-[calc(100%-2rem)] data-[fade=left]:mask-l-from-[calc(100%-2rem)] data-[fade=right]:mask-r-from-[calc(100%-2rem)]",
        className,
      )}
      {...props}
    />
  );
});
TabsList.displayName = TabsPrimitive.List.displayName;

const TabsTrigger = React.forwardRef<
  React.ComponentRef<typeof TabsPrimitive.Trigger>,
  React.ComponentPropsWithoutRef<typeof TabsPrimitive.Trigger>
>(({ className, ...props }, ref) => (
  <TabsPrimitive.Trigger
    ref={ref}
    className={cn(
      "folio-tap ring-offset-background focus-visible:ring-ring data-[state=active]:bg-background data-[state=active]:text-foreground inline-flex items-center justify-center rounded-sm px-3 py-1 text-sm font-medium whitespace-nowrap transition-all focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:outline-none disabled:pointer-events-none disabled:opacity-50 data-[state=active]:shadow",
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
