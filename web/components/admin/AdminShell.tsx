"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { PanelLeftIcon } from "lucide-react";
import { useEffect, useState } from "react";

import { PullToRefresh } from "@/components/PullToRefresh";
import { TopbarSearchTrigger } from "@/components/TopbarSearchTrigger";
import {
  AppSidebar,
  AppSidebarProvider,
  SidebarInset,
  SidebarTrigger,
} from "@/components/folio/sidebar";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/folio/sheet";
import { Button } from "@/components/ui/button";
import { RouteFade } from "@/components/shell/RouteFade";
import { SkipToContent } from "@/components/shell/SkipToContent";
import { CHROME_BAND_CLASS } from "@/lib/ui/chrome-band";
import { useMediaQuery } from "@/lib/ui/use-media-query";
import { SIDEBAR_COOKIE } from "@/lib/sidebar-state";
import { cn } from "@/lib/utils";

import { AdminSidebar } from "./AdminSidebar";
import { RestartPendingBanner } from "./RestartPendingBanner";
import { ScanEventBeacon } from "./ScanEventBeacon";
import type { NavSection } from "./nav";

/** Tablet band for `tabletSheet`: Tailwind's `md` (48rem) up to `lg`
 *  (64rem), the same range as the `md:max-lg:` classes below. */
export const TABLET_NAV_QUERY = "(min-width: 48rem) and (max-width: 63.99rem)";

/** Classes that hide the docked rail in the tablet band; the rail keeps
 *  its `data-slot="sidebar"` outer box, the phone / tablet sheets are
 *  portaled out of the wrapper (and carry `data-mobile`). Exported for the
 *  layout test. */
export const TABLET_RAIL_HIDDEN_CLASS =
  "md:max-lg:[&_[data-slot=sidebar]:not([data-mobile])]:hidden";

/**
 * Admin / settings shell; see MainShell for the topbar-above-sidebar
 * composition. The one trigger in the topbar collapses the rail on md+ and
 * opens the nav sheet on phones (the upstream trigger does both).
 *
 * `tabletSheet` (the admin tree): between `md` and `lg` the docked rail
 * would take a quarter of the width the admin tables need, so the rail is
 * hidden there and the same nav opens as a left sheet from a "Menu"
 * button, the phone pattern. The vendored provider's phone breakpoint is
 * fixed at 768px, so the tablet sheet is the Folio sheet (the same Radix
 * dialog the phone sidebar uses, with its focus trap and focus return)
 * holding the same `AdminSidebar`. At `lg` and up nothing changes.
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
  tabletSheet = false,
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
  /** Collapse the rail into a sheet between `md` and `lg`. */
  tabletSheet?: boolean;
}) {
  const inTabletBand = useMediaQuery(TABLET_NAV_QUERY);
  const tablet = tabletSheet && inTabletBand;
  const [railOpen, setRailOpen] = useState(defaultSidebarOpen);
  const [tabletNavOpen, setTabletNavOpen] = useState(false);
  // Close the tablet sheet on navigation (a link inside it also closes it
  // directly; this covers history moves while it is open).
  const pathname = usePathname();
  const [prevPathname, setPrevPathname] = useState(pathname);
  if (pathname !== prevPathname) {
    setPrevPathname(pathname);
    setTabletNavOpen(false);
  }
  // Radix Dialog/Sheet sets `pointer-events: none` on <body> while open;
  // a close racing a navigation can leave it behind. Clear it on mount.
  useEffect(() => {
    if (typeof document !== "undefined") {
      document.body.style.pointerEvents = "";
    }
  }, []);
  return (
    <AppSidebarProvider
      // In the tablet band the rail is hidden, so the provider reports it
      // expanded (the sheet then shows full labels and no rail tooltips),
      // and the toggle (topbar, Mod+B, the rebindable keybind) opens the
      // sheet instead. The vendored setter also writes the rail cookie;
      // put the rail's real state back so a tablet toggle does not
      // collapse the desktop rail on the next visit.
      open={tablet ? true : railOpen}
      onOpenChange={(next) => {
        if (!tablet) {
          setRailOpen(next);
          return;
        }
        setTabletNavOpen((o) => !o);
        queueMicrotask(() => {
          document.cookie = `${SIDEBAR_COOKIE}=${railOpen}; path=/; max-age=${60 * 60 * 24 * 7}`;
        });
      }}
      className={cn(
        "bg-background text-foreground min-h-screen flex-col",
        tabletSheet && TABLET_RAIL_HIDDEN_CLASS,
      )}
    >
      <SkipToContent />
      <PullToRefresh />
      <header
        className={cn(
          CHROME_BAND_CLASS,
          "sticky top-0 z-30 flex h-(--topbar-h) items-center gap-3 border-b pt-(--safe-top) pr-[max(1rem,var(--safe-right))] pl-[max(1rem,var(--safe-left))] [view-transition-name:shell-topbar] md:pr-[max(1.5rem,var(--safe-right))] md:pl-[max(1.5rem,var(--safe-left))]",
        )}
      >
        <SidebarTrigger
          aria-label="Toggle navigation"
          className={tabletSheet ? "md:max-lg:hidden" : undefined}
        />
        {tabletSheet ? (
          // Visible text is the accessible name (no aria-label), so the
          // label-in-name rule holds. A shadcn button: touch gets the
          // invisible 44px tap area from the `data-slot="button"` rule.
          <Button
            variant="ghost"
            size="sm"
            className="hidden md:max-lg:inline-flex"
            aria-expanded={tabletNavOpen}
            aria-haspopup="dialog"
            onClick={() => setTabletNavOpen(true)}
            data-testid="admin-tablet-nav-trigger"
          >
            <PanelLeftIcon aria-hidden="true" />
            Menu
          </Button>
        ) : null}
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
        {tabletSheet ? (
          <Sheet open={tablet && tabletNavOpen} onOpenChange={setTabletNavOpen}>
            {/* Mirrors the vendored phone sidebar sheet (same data
                attributes, so the same safe-area rule and look). */}
            <SheetContent
              side="left"
              data-sidebar="sidebar"
              data-slot="sidebar"
              data-mobile="true"
              className="bg-sidebar text-sidebar-foreground w-(--sidebar-width) p-0 [&>button]:hidden"
              style={{ "--sidebar-width": "18rem" } as React.CSSProperties}
            >
              <SheetHeader className="sr-only">
                <SheetTitle>{title} navigation</SheetTitle>
                <SheetDescription>
                  Pages of the {title.toLowerCase()} area.
                </SheetDescription>
              </SheetHeader>
              {/* Catches bubbled clicks on the nav links so the sheet
                  closes after navigation; the links are the interactive
                  elements, hence `role="presentation"`. */}
              <div
                role="presentation"
                className="flex h-full min-h-0 w-full flex-col"
                onClick={(e) => {
                  if ((e.target as HTMLElement).closest("a")) {
                    setTabletNavOpen(false);
                  }
                }}
              >
                <AdminSidebar sections={sections} title={title} user={user} />
              </div>
            </SheetContent>
          </Sheet>
        ) : null}
        <SidebarInset
          id="main-content"
          tabIndex={-1}
          className="min-w-0 px-4 py-8 md:px-8"
        >
          {showRestartBanner ? <RestartPendingBanner /> : null}
          {/* Cross-fades the content column between settings / admin
              pages. */}
          <RouteFade>{children}</RouteFade>
        </SidebarInset>
      </div>
    </AppSidebarProvider>
  );
}
