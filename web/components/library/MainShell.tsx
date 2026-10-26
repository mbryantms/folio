"use client";

import Link from "next/link";
import { Suspense } from "react";

import { AddToHomeScreenBanner } from "@/components/AddToHomeScreenBanner";
import { PullToRefresh } from "@/components/PullToRefresh";
import { TopbarSearchInline } from "@/components/TopbarSearchInline";
import { TopbarSearchTrigger } from "@/components/TopbarSearchTrigger";
import { ScanEventBeacon } from "@/components/admin/ScanEventBeacon";
import {
  AppSidebar,
  AppSidebarProvider,
  SidebarInset,
  SidebarTrigger,
  useSidebar,
} from "@/components/folio/sidebar";
import { SkipToContent } from "@/components/shell/SkipToContent";
import { CHROME_BAND_CLASS } from "@/lib/ui/chrome-band";
import { cn } from "@/lib/utils";

import { BottomTabBar } from "./BottomTabBar";
import { MainSidebar } from "./MainSidebar";
import type { MainNavSection } from "./main-nav";

/**
 * Library shell: Folio's full-width sticky topbar above the upstream shadcn
 * Sidebar (components/folio/sidebar.tsx positions the rail under the
 * topbar) and the page column in `SidebarInset`. On phones the sidebar is
 * the sheet the bottom tab bar's "More" tab opens; the rail's open state
 * is the provider's `sidebar_state` cookie, read by the layout for the
 * first paint.
 */
export function MainShell({
  children,
  sections,
  user,
  homeHref,
  defaultSidebarOpen = true,
  showMarkerCount = false,
}: {
  children: React.ReactNode;
  sections: MainNavSection[];
  user: { display_name: string; email?: string | null; role: string };
  homeHref: string;
  /** SSR-resolved initial sidebar state (the `sidebar_state` cookie). */
  defaultSidebarOpen?: boolean;
  /** SSR-resolved per-user toggle for the Bookmarks sidebar count badge. */
  showMarkerCount?: boolean;
}) {
  return (
    <AppSidebarProvider
      defaultOpen={defaultSidebarOpen}
      data-library-shell
      className="bg-background text-foreground min-h-screen flex-col"
    >
      <SkipToContent />
      <PullToRefresh />
      <AddToHomeScreenBanner />
      <header
        className={cn(
          CHROME_BAND_CLASS,
          "sticky top-0 z-30 flex h-(--topbar-h) items-center gap-3 border-b pt-(--safe-top) pr-[max(1rem,var(--safe-right))] pl-[max(1rem,var(--safe-left))] md:pr-[max(1.5rem,var(--safe-right))] md:pl-[max(1.5rem,var(--safe-left))]",
        )}
      >
        {/* Desktop collapse toggle; on phones the bottom bar's "More" tab
            opens the nav sheet instead. */}
        <SidebarTrigger className="hidden md:inline-flex" />
        <Link href={homeHref} className="font-semibold tracking-tight">
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
        <TopbarSearchTrigger className="ml-auto sm:hidden" />
        {user.role === "admin" ? (
          <div className="flex shrink-0 items-center">
            <ScanEventBeacon />
          </div>
        ) : null}
      </header>
      <div className="flex flex-1">
        <AppSidebar aria-label="Library sidebar">
          <MainSidebar
            sections={sections}
            user={user}
            showMarkerCount={showMarkerCount}
          />
        </AppSidebar>
        <SidebarInset
          id="main-content"
          tabIndex={-1}
          className="min-w-0 px-4 py-6 pb-[calc(var(--bottom-tab-h)+var(--safe-bottom)+1.5rem)] md:px-8 md:py-8"
        >
          {children}
        </SidebarInset>
      </div>
      {/* Mobile bottom tab bar. Reads `?library=all` (Suspense-gated) to
          split the Home/Library tabs; "More" opens the nav sheet. */}
      <Suspense fallback={null}>
        <ShellBottomBar />
      </Suspense>
    </AppSidebarProvider>
  );
}

function ShellBottomBar() {
  const { setOpenMobile } = useSidebar();
  return <BottomTabBar onMore={() => setOpenMobile(true)} />;
}
