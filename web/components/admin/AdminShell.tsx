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
import { SkipToContent } from "@/components/shell/SkipToContent";
import type { SidebarState } from "@/lib/sidebar-state";
import { useSidebarState } from "@/lib/use-sidebar-state";
import { cn } from "@/lib/utils";

import { AdminSidebar } from "./AdminSidebar";
import { ScanEventBeacon } from "./ScanEventBeacon";
import type { NavSection } from "./nav";

export function AdminShell({
  children,
  user,
  sections,
  title,
  homeHref,
  showScanBeacon = false,
  defaultSidebar = "expanded",
}: {
  children: React.ReactNode;
  user: { display_name: string; email: string | null; role: string };
  sections: NavSection[];
  title: string;
  homeHref: string;
  /** Show the WebSocket scan-event beacon — admin tree only. */
  showScanBeacon?: boolean;
  /** SSR-resolved initial sidebar state (read from cookie by the layout)
   *  so the first paint matches the user's preference — no expand-then-
   *  collapse flash on reload. */
  defaultSidebar?: SidebarState;
}) {
  const [mobileOpen, setMobileOpen] = useState(false);
  const sidebar = useSidebarState(defaultSidebar);
  return (
    <div className="bg-background text-foreground min-h-screen">
      <SkipToContent />
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
          <div className="sticky top-14 h-[calc(100vh-3.5rem)]">
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
          {children}
        </main>
      </div>
    </div>
  );
}
