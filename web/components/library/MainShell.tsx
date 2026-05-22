"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { Menu } from "lucide-react";

import { AddToHomeScreenBanner } from "@/components/AddToHomeScreenBanner";
import { PullToRefresh } from "@/components/PullToRefresh";
import { TopbarSearchInline } from "@/components/TopbarSearchInline";
import { TopbarSearchTrigger } from "@/components/TopbarSearchTrigger";
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

import { ScanEventBeacon } from "@/components/admin/ScanEventBeacon";

import { MainSidebar } from "./MainSidebar";
import type { MainNavSection } from "./main-nav";

/**
 * Library/reader-app shell. Mirrors the structure of `AdminShell` so the two
 * trees feel cohesive — sticky topbar, fixed sidebar on md+, off-canvas
 * sheet on mobile. Pages render their own page-level chrome (title,
 * breadcrumbs) inside `children`; the shell only handles the persistent
 * navigation surface.
 */
export function MainShell({
  children,
  sections,
  user,
  homeHref,
  defaultSidebar = "expanded",
  showMarkerCount = false,
}: {
  children: React.ReactNode;
  sections: MainNavSection[];
  user: { display_name: string; email: string | null; role: string };
  homeHref: string;
  /** SSR-resolved initial sidebar state (read from cookie by the layout). */
  defaultSidebar?: SidebarState;
  /** SSR-resolved per-user toggle for the Bookmarks sidebar count badge. */
  showMarkerCount?: boolean;
}) {
  const [mobileOpen, setMobileOpen] = useState(false);
  const sidebar = useSidebarState(defaultSidebar);
  // Radix Dialog/Sheet sets `pointer-events: none` on <body> while
  // open. When the mobile sheet closes simultaneously with a cross
  // layout-group navigation (e.g. `/` → `/admin` → `/settings`), the
  // previous shell can unmount before Radix's exit animation
  // completes, leaving the body lock stuck. Clearing it on mount
  // restores click handling on the freshly-routed page.
  useEffect(() => {
    if (typeof document !== "undefined") {
      document.body.style.pointerEvents = "";
    }
  }, []);
  return (
    <div className="bg-background text-foreground min-h-screen">
      <SkipToContent />
      <PullToRefresh />
      <AddToHomeScreenBanner />
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
            className="w-72 p-0"
            onClick={(e) => {
              // Mobile UX: clicking a link inside the drawer should close
              // the drawer along with navigating. Buttons (theme toggle,
              // help, user menu trigger) intentionally don't match — they
              // open submenus that the user is still interacting with.
              if ((e.target as HTMLElement).closest("a")) {
                setMobileOpen(false);
              }
            }}
          >
            <SheetTitle className="sr-only">Library navigation</SheetTitle>
            <MainSidebar
              sections={sections}
              user={user}
              showMarkerCount={showMarkerCount}
            />
          </SheetContent>
        </Sheet>
        <SidebarTrigger
          collapsed={sidebar.collapsed}
          onToggle={sidebar.toggle}
        />
        <Link
          href={homeHref}
          // Hide on the smallest viewports so the search trigger can
          // claim the row width; the hamburger to the left is already
          // a strong anchor.
          className="hidden font-semibold tracking-tight sm:inline"
        >
          Folio
        </Link>
        {/* Topbar search.
            - sm+: real `<input>` that types inline and opens a
              dropdown panel beneath. Categories + snippets +
              recents + commands render in the dropdown so the user
              never context-switches to a centered modal on
              desktop.
            - <sm: icon button that opens the rich `<SearchModal>`.
              The dropdown is too cramped at phone widths; the
              fullscreen Dialog is the right shape there. */}
        <div className="ml-1 hidden flex-1 sm:block sm:max-w-md">
          <TopbarSearchInline />
        </div>
        <TopbarSearchTrigger className="ml-1 sm:hidden" />
        {user.role === "admin" ? (
          <div className="flex shrink-0 items-center">
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
          aria-label="Library sidebar"
        >
          {/* v0.3.46: use `dvh` (dynamic viewport height) instead of
           * `vh`. On iOS Safari — especially when the app is launched
           * from a saved home-screen icon in standalone PWA mode —
           * `100vh` resolves to the "large" viewport (which includes
           * the area currently hidden behind the browser UI / safe-
           * area inset), so the sidebar height overshoots and the
           * UserFooter at its bottom lands below the visible area
           * until the user scrolls. `100dvh` resizes with the actual
           * available viewport and keeps the footer on-screen. */}
          <div className="sticky top-(--topbar-h) h-[calc(100dvh-var(--topbar-h))]">
            <MainSidebar
              sections={sections}
              user={user}
              collapsed={sidebar.collapsed}
              showMarkerCount={showMarkerCount}
            />
          </div>
        </aside>
        <main
          id="main-content"
          tabIndex={-1}
          className="min-w-0 flex-1 px-4 py-6 md:px-8 md:py-8"
        >
          {children}
        </main>
      </div>
    </div>
  );
}
