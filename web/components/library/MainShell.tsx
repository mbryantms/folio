"use client";

import Link from "next/link";
import { useState } from "react";
import { Menu } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Sheet,
  SheetContent,
  SheetTitle,
  SheetTrigger,
} from "@/components/ui/sheet";
import { SidebarTrigger } from "@/components/shell/SidebarTrigger";
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
  return (
    <div className="bg-background text-foreground min-h-screen">
      <header className="border-border bg-background/80 sticky top-0 z-30 flex h-14 items-center gap-3 border-b px-4 backdrop-blur md:px-6">
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
          <SheetContent side="left" className="w-72 p-0">
            <SheetTitle className="sr-only">Library navigation</SheetTitle>
            <MainSidebar
              sections={sections}
              title="Folio"
              user={user}
              showMarkerCount={showMarkerCount}
            />
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
          Library
        </span>
        {user.role === "admin" ? (
          <div className="ml-auto flex items-center">
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
          <div className="sticky top-14 h-[calc(100vh-3.5rem)]">
            <MainSidebar
              sections={sections}
              title="Folio"
              user={user}
              collapsed={sidebar.collapsed}
              showMarkerCount={showMarkerCount}
            />
          </div>
        </aside>
        <main className="min-w-0 flex-1 px-4 py-6 md:px-8 md:py-8">
          {children}
        </main>
      </div>
    </div>
  );
}
