"use client";

import { Suspense } from "react";

import { AddToHomeScreenBanner } from "@/components/AddToHomeScreenBanner";
import { PullToRefresh } from "@/components/PullToRefresh";
import { TopbarSearchInline } from "@/components/TopbarSearchInline";
import { ScanEventBeacon } from "@/components/admin/ScanEventBeacon";
import { OnboardingTour } from "@/components/onboarding/OnboardingTour";
import {
  AppSidebar,
  AppSidebarProvider,
  SidebarInset,
  SidebarTrigger,
  useSidebar,
} from "@/components/folio/sidebar";
import { RouteFade } from "@/components/shell/RouteFade";
import { SkipToContent } from "@/components/shell/SkipToContent";
import { Wordmark } from "@/components/shell/Wordmark";
import { CHROME_BAND_CLASS } from "@/lib/ui/chrome-band";
import { cn } from "@/lib/utils";

import { BottomTabBar } from "./BottomTabBar";
import { MainSidebar } from "./MainSidebar";
import type { MainNavSection } from "./main-nav";
import { CoverSizeSync } from "./use-cover-size";

/**
 * Library shell: Folio's full-width sticky topbar above the upstream shadcn
 * Sidebar (components/folio/sidebar.tsx positions the rail under the
 * topbar) and the page column in `SidebarInset`. Below `md` there is no
 * topbar (docs/dev/phone-layout.md, rule 0): the page's own control row
 * is the top bar, global search is the bottom tab bar's Search tab, and
 * the sidebar is the sheet the bar's "More" tab opens; the rail's open
 * state is the provider's `sidebar_state` cookie, read by the layout for
 * the first paint.
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
      // Phones and the sheet widths (< md) have no topbar: the page's own
      // control row is the top bar and the bottom tab bar carries global
      // search. globals.css reads this to collapse `--topbar-h` to the
      // status-bar inset, so every `top-(--topbar-h)` row pins at the top.
      data-phone-topbar="hidden"
      className="bg-background text-foreground min-h-screen flex-col"
    >
      <SkipToContent />
      <PullToRefresh />
      <AddToHomeScreenBanner />
      <header
        className={cn(
          CHROME_BAND_CLASS,
          "sticky top-0 z-30 hidden h-(--topbar-h) items-center gap-3 border-b pt-(--safe-top) pr-[max(1.5rem,var(--safe-right))] pl-[max(1.5rem,var(--safe-left))] [view-transition-name:shell-topbar] md:flex",
        )}
      >
        <BrandBlock homeHref={homeHref} />
        {/* Topbar search: a real `<input>` that types inline and opens a
            dropdown panel beneath. Categories + snippets + recents +
            commands render in the dropdown so the user never
            context-switches to a centered modal on desktop. Below md the
            whole header is gone and the bottom tab bar's Search tab
            reaches the search page instead. */}
        <div className="flex-1 sm:max-w-md" data-tour="search">
          <TopbarSearchInline />
        </div>
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
          className="min-w-0 px-4 py-6 pt-[calc(var(--safe-top)+1.5rem)] pb-[calc(var(--bottom-tab-h)+var(--safe-bottom)+1.5rem)] md:px-8 md:py-8"
        >
          {/* Measures this column: its width is the library row every
              cover size is derived from (use-cover-size.ts). */}
          <CoverSizeSync />
          {/* Cross-fades the content column between top-level pages. */}
          <RouteFade>{children}</RouteFade>
        </SidebarInset>
      </div>
      {/* Mobile bottom tab bar. Reads `?library=all` (Suspense-gated) to
          split the Home/Library tabs; "More" opens the nav sheet. */}
      <Suspense fallback={null}>
        <ShellBottomBar />
      </Suspense>
      {/* First-run tour on Home (and `/?tour=1` replays). */}
      <Suspense fallback={null}>
        <OnboardingTour />
      </Suspense>
    </AppSidebarProvider>
  );
}

/**
 * The sidebar toggle and the Folio lockup, sized to the sidebar column
 * while the sidebar is open, so the search field beside it starts at the
 * content column's left edge. The sum: the sidebar's width, plus the
 * content column's `md:px-8`, less the header's own left padding (1.5rem)
 * and the gap before the search field (`gap-3`). With the sidebar
 * collapsed the block takes its natural width. It is a minimum width, so
 * it can follow the sidebar's own 200ms slide (a width of `auto` cannot
 * be animated).
 */
function BrandBlock({ homeHref }: { homeHref: string }) {
  const { state } = useSidebar();
  return (
    <div
      data-brand-block=""
      className={cn(
        "flex shrink-0 items-center gap-3 transition-[min-width] duration-200 ease-linear motion-reduce:transition-none",
        state === "expanded"
          ? "min-w-[calc(var(--sidebar-width)+2rem-1.5rem-0.75rem)]"
          : "min-w-0",
      )}
    >
      {/* Desktop collapse toggle; on phones the bottom bar's "More" tab
          opens the nav sheet instead. */}
      <SidebarTrigger className="hidden md:inline-flex" />
      <Wordmark href={homeHref} />
    </div>
  );
}

function ShellBottomBar() {
  const { setOpenMobile } = useSidebar();
  return <BottomTabBar onMore={() => setOpenMobile(true)} />;
}
