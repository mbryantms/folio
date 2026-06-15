"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { Menu } from "lucide-react";

import { PullToRefresh } from "@/components/PullToRefresh";
import { Button } from "@/components/ui/button";
import {
  Sheet,
  SheetContent,
  SheetTitle,
  SheetTrigger,
} from "@/components/ui/sheet";
import { SidebarTrigger } from "@/components/shell/SidebarTrigger";
import { SkipToContent } from "@/components/shell/SkipToContent";
import type { SidebarState } from "@/lib/sidebar-state";
import { useSidebarState } from "@/lib/use-sidebar-state";
import { cn } from "@/lib/utils";

import { AdminSidebar } from "./AdminSidebar";
import { RestartPendingBanner } from "./RestartPendingBanner";
import { ScanEventBeacon } from "./ScanEventBeacon";
import type { NavSection } from "./nav";

export function AdminShell({
  children,
  user,
  sections,
  title,
  homeHref,
  showScanBeacon = false,
  showRestartBanner = false,
  defaultSidebar = "expanded",
}: {
  children: React.ReactNode;
  user: { display_name: string; email?: string | null; role: string };
  sections: NavSection[];
  title: string;
  homeHref: string;
  /** Show the WebSocket scan-event beacon — admin tree only. */
  showScanBeacon?: boolean;
  /** Show the restart-pending banner — admin tree only (the query is
   *  admin-gated; the settings tree shares this shell for non-admins). */
  showRestartBanner?: boolean;
  /** SSR-resolved initial sidebar state (read from cookie by the layout)
   *  so the first paint matches the user's preference — no expand-then-
   *  collapse flash on reload. */
  defaultSidebar?: SidebarState;
}) {
  const [mobileOpen, setMobileOpen] = useState(false);
  const sidebar = useSidebarState(defaultSidebar);
  // Radix Dialog/Sheet sets `pointer-events: none` on <body> while
  // open. When the mobile sheet closes simultaneously with a cross
  // layout-group navigation, the previous shell can unmount before
  // Radix's exit animation completes, leaving the body lock stuck.
  // Clearing it on mount restores click handling on the freshly-
  // routed page.
  useEffect(() => {
    if (typeof document !== "undefined") {
      document.body.style.pointerEvents = "";
    }
  }, []);
  return (
    <div className="bg-background text-foreground min-h-screen">
      <SkipToContent />
      <PullToRefresh />
      <header className="border-border bg-background/80 sticky top-0 z-30 flex h-(--topbar-h) items-center gap-3 border-b pt-(--safe-top) pl-[max(1rem,var(--safe-left))] pr-[max(1rem,var(--safe-right))] backdrop-blur md:pl-[max(1.5rem,var(--safe-left))] md:pr-[max(1.5rem,var(--safe-right))]">
        <Sheet open={mobileOpen} onOpenChange={setMobileOpen}>
          <SheetTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              className="md:hidden"
              aria-label="Open navigation"
            >
              <Menu className="h-5 w-5" />
            </Button>
          </SheetTrigger>
          <SheetContent
            side="left"
            // Safe-area insets — mirrors `MainShell`. Without these
            // the iPhone clock / Dynamic Island sits on top of the
            // drawer's first nav row in PWA standalone mode.
            className="w-72 p-0 pt-(--safe-top) pb-(--safe-bottom) pl-(--safe-left)"
            onClick={(e) => {
              // Mobile UX: clicking a link inside the drawer should close
              // it along with navigating. Buttons stay click-through.
              if ((e.target as HTMLElement).closest("a")) {
                setMobileOpen(false);
              }
            }}
          >
            <SheetTitle className="sr-only">{title} navigation</SheetTitle>
            <AdminSidebar sections={sections} title={title} user={user} />
          </SheetContent>
        </Sheet>
        <SidebarTrigger
          collapsed={sidebar.collapsed}
          onToggle={sidebar.toggle}
        />
        <Link href={homeHref} className="font-semibold tracking-tight">
          Folio
        </Link>
        <span className="text-muted-foreground ml-2 hidden text-xs font-medium tracking-widest uppercase sm:inline">
          {title}
        </span>
        {showScanBeacon ? (
          <div className="text-muted-foreground ml-auto flex items-center gap-3 text-sm">
            <ScanEventBeacon />
          </div>
        ) : null}
      </header>
      <div className="flex">
        <aside
          className={cn(
            "border-border hidden shrink-0 border-r transition-[width] duration-200 ease-out motion-reduce:transition-none md:block",
            sidebar.collapsed ? "w-14" : "w-64",
          )}
          data-collapsed={sidebar.collapsed ? "true" : "false"}
          aria-label={`${title} sidebar`}
        >
          {/* v0.3.46: see the matching note in MainShell.tsx — `dvh`
           * keeps the sidebar height tied to the actually-visible
           * viewport so the UserFooter stays on-screen in iOS PWA
           * standalone mode. */}
          <div className="sticky top-(--topbar-h) h-[calc(100dvh-var(--topbar-h))]">
            <AdminSidebar
              sections={sections}
              title={title}
              user={user}
              collapsed={sidebar.collapsed}
            />
          </div>
        </aside>
        <main
          id="main-content"
          tabIndex={-1}
          className="min-w-0 flex-1 px-4 py-8 md:px-8"
        >
          {showRestartBanner ? <RestartPendingBanner /> : null}
          {children}
        </main>
      </div>
    </div>
  );
}
