"use client";

/* Folio wrapper over the vendored shadcn Sidebar (components/ui is
 * pristine). What Folio adds, and nothing else:
 *  - `AppSidebarProvider`: the upstream provider plus the user-rebindable
 *    `toggleSidebar` keybind. The vendored provider already handles
 *    Mod+B itself, so that exact combination is left to it (a second
 *    listener would toggle twice and cancel out).
 *  - `AppSidebar`: icon-collapsible, positioned under Folio's full-width
 *    sticky topbar (the upstream "sidebar below the header" pattern:
 *    `top-(--topbar-h)` + a height that subtracts it), and the mobile
 *    sheet closes when a link inside it is followed. The rail is
 *    `sticky`, not upstream's `fixed`: iOS pins a fixed box to the
 *    viewport during the rubber-band bounce, so the rail stood still
 *    while the sticky topbar and the page slid down over it (iPad,
 *    2026-10-30). A sticky rail is part of the scrolling document and
 *    bounces with them, as the pre-#1346 `<aside>` did.
 * App code imports from here, never from components/ui (ESLint enforces
 * it). */
import * as React from "react";

import {
  Sidebar as UiSidebar,
  SidebarProvider as UiSidebarProvider,
  useSidebar,
} from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useMe } from "@/lib/api/queries";
import {
  actionForKey,
  readMeKeybinds,
  resolveKeybinds,
  shouldSkipHotkey,
} from "@/lib/reader/keybinds";
import { cn } from "@/lib/utils";

export * from "@/components/ui/sidebar";

function SidebarKeybind() {
  const { toggleSidebar } = useSidebar();
  const me = useMe();
  const meKeybinds = readMeKeybinds(me);
  const bindings = React.useMemo(
    () => resolveKeybinds(meKeybinds),
    [meKeybinds],
  );
  React.useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (shouldSkipHotkey(e)) return;
      // The vendored provider owns Mod+B.
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "b") return;
      if (actionForKey(e, bindings) !== "toggleSidebar") return;
      e.preventDefault();
      toggleSidebar();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [bindings, toggleSidebar]);
  return null;
}

export function AppSidebarProvider({
  children,
  ...props
}: React.ComponentProps<typeof UiSidebarProvider>) {
  return (
    // The vendored SidebarMenuButton renders the vendored Tooltip, which
    // needs a provider above it; the shells carry it here rather than
    // the root layout so routes without a sidebar (the reader) never load
    // the tooltip package.
    <TooltipProvider delayDuration={0}>
      <UiSidebarProvider {...props}>
        <SidebarKeybind />
        {children}
      </UiSidebarProvider>
    </TooltipProvider>
  );
}

export function AppSidebar({
  className,
  children,
  ...props
}: React.ComponentProps<typeof UiSidebar>) {
  const { isMobile, setOpenMobile } = useSidebar();
  return (
    <UiSidebar
      collapsible="icon"
      className={cn(
        // `sticky` outranks upstream's `fixed` through tailwind-merge; the
        // gap div before the container has no height, so the rail sits at
        // the wrapper's top and the row's stretched height is its track.
        "sticky top-(--topbar-h) h-[calc(100svh-var(--topbar-h))]",
        className,
      )}
      {...props}
    >
      {/* Catches bubbled clicks on the nav links so the phone sheet
          closes after navigation; the links are the interactive
          elements, hence `role="presentation"`. */}
      <div
        role="presentation"
        className="flex h-full min-h-0 flex-col"
        onClick={(e) => {
          if (isMobile && (e.target as HTMLElement).closest("a")) {
            setOpenMobile(false);
          }
        }}
      >
        {children}
      </div>
    </UiSidebar>
  );
}

/** `true` while the desktop rail is in its icon-only state. On phones the
 *  sheet always shows the full labels. */
export function useSidebarCollapsed(): boolean {
  const { state, isMobile } = useSidebar();
  return !isMobile && state === "collapsed";
}
