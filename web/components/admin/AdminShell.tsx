"use client";

import Link from "next/link";
import { useEffect } from "react";

import { PullToRefresh } from "@/components/PullToRefresh";
import { TopbarSearchTrigger } from "@/components/TopbarSearchTrigger";
import {
  AppSidebar,
  AppSidebarProvider,
  SidebarInset,
  SidebarTrigger,
} from "@/components/folio/sidebar";
import { SkipToContent } from "@/components/shell/SkipToContent";
import { CHROME_BAND_CLASS } from "@/lib/ui/chrome-band";
import { cn } from "@/lib/utils";

import { AdminSidebar } from "./AdminSidebar";
import { RestartPendingBanner } from "./RestartPendingBanner";
import { ScanEventBeacon } from "./ScanEventBeacon";
import type { NavSection } from "./nav";

/**
 * Admin / settings shell; see MainShell for the topbar-above-sidebar
 * composition. The one trigger in the topbar collapses the rail on md+ and
 * opens the nav sheet on phones (the upstream trigger does both).
 */
export function AdminShell({
  children,
  user,
  sections,
  title,
  homeHref,
  showScanBeacon = false,
  showRestartBanner = false,
  defaultSidebarOpen = true,
}: {
  children: React.ReactNode;
  user: { display_name: string; email?: string | null; role: string };
  sections: NavSection[];
  title: string;
  homeHref: string;
  showScanBeacon?: boolean;
  showRestartBanner?: boolean;
  /** SSR-resolved initial sidebar state (the `sidebar_state` cookie). */
  defaultSidebarOpen?: boolean;
}) {
  // Radix Dialog/Sheet sets `pointer-events: none` on <body> while open;
  // a close racing a navigation can leave it behind. Clear it on mount.
  useEffect(() => {
    if (typeof document !== "undefined") {
      document.body.style.pointerEvents = "";
    }
  }, []);
  return (
    <AppSidebarProvider
      defaultOpen={defaultSidebarOpen}
      className="bg-background text-foreground min-h-screen flex-col"
    >
      <SkipToContent />
      <PullToRefresh />
      <header
        className={cn(
          CHROME_BAND_CLASS,
          "sticky top-0 z-30 flex h-(--topbar-h) items-center gap-3 border-b pt-(--safe-top) pr-[max(1rem,var(--safe-right))] pl-[max(1rem,var(--safe-left))] md:pr-[max(1.5rem,var(--safe-right))] md:pl-[max(1.5rem,var(--safe-left))]",
        )}
      >
        <SidebarTrigger aria-label="Toggle navigation" />
        <Link href={homeHref} className="font-semibold tracking-tight">
          Folio
        </Link>
        <span className="text-muted-foreground ml-2 hidden text-xs font-medium tracking-widest uppercase sm:inline">
          {title}
        </span>
        {/* Global search — the same `<SearchModal>` surface as the
            app-wide `Mod+K` / `/` hotkeys, surfaced here so the admin +
            settings trees advertise search instead of being a visible
            dead zone (A4). Input-shaped and grows to fill at sm+; an
            icon button on phones. */}
        <div className="ml-2 hidden min-w-0 flex-1 sm:block sm:max-w-md">
          <TopbarSearchTrigger />
        </div>
        <TopbarSearchTrigger className="ml-auto sm:hidden" />
        {showScanBeacon ? (
          <div className="text-muted-foreground flex shrink-0 items-center gap-3 text-sm">
            <ScanEventBeacon />
          </div>
        ) : null}
      </header>
      <div className="flex flex-1">
        <AppSidebar aria-label={`${title} sidebar`}>
          <AdminSidebar sections={sections} title={title} user={user} />
        </AppSidebar>
        <SidebarInset
          id="main-content"
          tabIndex={-1}
          className="min-w-0 px-4 py-8 md:px-8"
        >
          {showRestartBanner ? <RestartPendingBanner /> : null}
          {children}
        </SidebarInset>
      </div>
    </AppSidebarProvider>
  );
}
