"use client";

import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";
import { ChevronDown, ChevronRight, Sparkles } from "lucide-react";
import dynamic from "next/dynamic";
import { useRef, useState } from "react";

import { ScanEventBeacon } from "@/components/admin/ScanEventBeacon";
import {
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarHeader,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
  useSidebarCollapsed,
} from "@/components/folio/sidebar";
import { SlidingNavHighlight } from "@/components/shell/SlidingNavHighlight";
import { UserFooter } from "@/components/shell/UserFooter";
import { useMarkerCount, useUpcomingSummary } from "@/lib/api/queries";
import { useLocalToday } from "@/lib/upcoming/today";
import { useSidebarSectionCollapse } from "@/lib/use-sidebar-section-collapse";

import { mainNavIcons } from "./main-nav-icons";

// The in-place editor (drag and drop, menus) loads only when "Edit
// sidebar" is pressed, so it adds nothing to every page's bundle.
const SidebarEditMode = dynamic(
  () => import("./SidebarEditMode").then((m) => m.SidebarEditMode),
  { ssr: false },
);
import type { MainNavSection } from "./main-nav";
import { railIconByKey } from "./rail-icons";

/**
 * Library navigation on the upstream shadcn Sidebar primitives (2026-10-26).
 * The icon-collapsed rail, the tooltips it shows, and the cookie-persisted
 * open state all come from the vendored component; this file only maps the
 * `/me/sidebar-layout` sections onto `SidebarGroup` / `SidebarMenu`.
 *
 * "Edit sidebar" in the account menu at the foot of the sidebar swaps the
 * links for the in-place editor (`SidebarEditMode`): reorder, rename, hide
 * and add headers right where the sidebar is read. It lives in that menu,
 * not in the nav, so it never reads as one more destination.
 */
export function MainSidebar({
  sections,
  user,
  showMarkerCount = false,
}: {
  sections: MainNavSection[];
  user: { display_name: string; email?: string | null; role: string };
  showMarkerCount?: boolean;
}) {
  const pathname = usePathname() ?? "";
  const search = useSearchParams();
  const activeLibrary = search?.get("library") ?? null;
  const collapsed = useSidebarCollapsed();
  const { isMobile, setOpen } = useSidebar();
  const [editing, setEditing] = useState(false);
  // The editor needs the full-width column; in the icon rail the links
  // show as usual until the rail is expanded again.
  const showEditor = editing && !collapsed;
  const markerCount = useMarkerCount({ enabled: showMarkerCount });
  // The Upcoming entry exists only while the feature is on, so its
  // presence is what turns the count on: this week's releases in the
  // reader's series.
  const hasUpcoming = sections.some((s) =>
    s.items.some((i) => i.kind === "builtin" && i.refId === "upcoming"),
  );
  const today = useLocalToday();
  const upcoming = useUpcomingSummary(today ?? "", {
    enabled: hasUpcoming && today !== null,
  });
  const upcomingThisWeek = upcoming.data?.this_week ?? 0;
  const sectionCollapse = useSidebarSectionCollapse();
  const navRef = useRef<HTMLElement | null>(null);

  return (
    <>
      {/* The scan beacon lives in the topbar from md up; below md the
          library shell has no topbar (phone-layout rule 0), so the nav
          sheet carries it for admins. Gated on `isMobile`, not a class:
          each beacon opens its own socket, and the desktop rail must not
          add a second one. The header pads only while a pill is showing
          (the beacon renders an empty row at idle). */}
      {isMobile && user.role === "admin" ? (
        <SidebarHeader className="p-0 [&:has(div>*)]:p-2">
          <ScanEventBeacon />
        </SidebarHeader>
      ) : null}
      <SidebarContent>
        {showEditor ? (
          <SidebarEditMode onDone={() => setEditing(false)} />
        ) : null}
        {/* `relative isolate`: the sliding highlight is positioned in the
            nav's box and paints under the items (`-z-10` inside this
            stacking context). Once it is placed, the active item drops
            its own background so only the pill shows. */}
        <nav
          ref={navRef}
          aria-label="Library navigation"
          hidden={showEditor}
          className="relative isolate flex flex-col gap-2 data-[nav-pill=ready]:[&_[data-sidebar=menu-button][data-active=true]]:bg-transparent"
        >
          <SlidingNavHighlight
            containerRef={navRef}
            className="bg-sidebar-accent rounded-md"
          />
          {sections.map((section, sectionIdx) => {
            if (section.isSpacer) {
              return (
                <div
                  key={`spacer-${sectionIdx}`}
                  className="h-0.5"
                  aria-hidden
                />
              );
            }
            const collapsible =
              !collapsed && !!section.headerRefId && !!section.label;
            const sectionClosed = collapsible
              ? sectionCollapse.isCollapsed(section.headerRefId!)
              : false;
            const itemsId = collapsible
              ? `sidebar-section-${section.headerRefId}`
              : undefined;
            return (
              <SidebarGroup
                key={`${section.label ?? "untitled"}-${sectionIdx}`}
                // First-run tour anchor: the section holding the libraries.
                data-tour={
                  section.items.some((i) => i.kind === "library")
                    ? "nav-libraries"
                    : undefined
                }
              >
                {section.label ? (
                  collapsible ? (
                    <SidebarGroupLabel asChild>
                      <button
                        type="button"
                        aria-expanded={!sectionClosed}
                        aria-controls={itemsId}
                        onClick={() =>
                          sectionCollapse.toggle(section.headerRefId!)
                        }
                        className="hover:text-foreground/80 w-full gap-1.5 text-left tracking-widest uppercase"
                      >
                        {sectionClosed ? (
                          <ChevronRight className="size-3 shrink-0" />
                        ) : (
                          <ChevronDown className="size-3 shrink-0" />
                        )}
                        <span className="truncate">{section.label}</span>
                      </button>
                    </SidebarGroupLabel>
                  ) : (
                    <SidebarGroupLabel className="tracking-widest uppercase">
                      {section.label}
                    </SidebarGroupLabel>
                  )
                ) : null}
                <SidebarGroupContent id={itemsId} hidden={sectionClosed}>
                  <SidebarMenu>
                    {section.items.map((item) => {
                      const Icon =
                        mainNavIcons[item.icon as keyof typeof mainNavIcons] ??
                        railIconByKey(item.icon)?.Icon ??
                        Sparkles;
                      const itemSearch = item.href.includes("?library=")
                        ? (item.href.split("?library=")[1] ?? null)
                        : null;
                      const itemPath = item.href.split("?")[0]!;
                      const isBuiltin = (key: string) =>
                        item.kind === "builtin" && item.refId === key;
                      let active = false;
                      if (itemSearch) {
                        active =
                          pathname === itemPath && activeLibrary === itemSearch;
                      } else if (isBuiltin("all_libraries")) {
                        active =
                          pathname === itemPath &&
                          activeLibrary != null &&
                          pathname === "/";
                      } else if (isBuiltin("home")) {
                        active = pathname === itemPath && activeLibrary == null;
                      } else {
                        active =
                          pathname === itemPath ||
                          (itemPath !== "" &&
                            pathname.startsWith(itemPath + "/"));
                      }
                      const capped = (n: number) =>
                        n > 99 ? "99+" : String(n);
                      const badge =
                        showMarkerCount &&
                        isBuiltin("bookmarks") &&
                        markerCount.data &&
                        markerCount.data.total > 0
                          ? {
                              text: capped(markerCount.data.total),
                              label: `${markerCount.data.total} markers`,
                            }
                          : isBuiltin("upcoming") && upcomingThisWeek > 0
                            ? {
                                text: capped(upcomingThisWeek),
                                label: `${upcomingThisWeek} out this week`,
                              }
                            : null;
                      return (
                        <SidebarMenuItem key={`${item.href}-${item.label}`}>
                          <SidebarMenuButton
                            asChild
                            isActive={active}
                            tooltip={item.label}
                          >
                            <Link
                              href={item.href}
                              aria-current={active ? "page" : undefined}
                              // First-run tour anchors (`nav-collections`, …).
                              data-tour={
                                item.kind === "builtin"
                                  ? `nav-${item.refId}`
                                  : undefined
                              }
                            >
                              <Icon />
                              <span>{item.label}</span>
                            </Link>
                          </SidebarMenuButton>
                          {badge ? (
                            <SidebarMenuBadge aria-label={badge.label}>
                              {badge.text}
                            </SidebarMenuBadge>
                          ) : null}
                        </SidebarMenuItem>
                      );
                    })}
                  </SidebarMenu>
                </SidebarGroupContent>
              </SidebarGroup>
            );
          })}
        </nav>
      </SidebarContent>
      <SidebarFooter className="p-0">
        <UserFooter
          user={user}
          collapsed={collapsed}
          onEditSidebar={() => {
            // The editor needs the full-width column.
            if (collapsed) setOpen(true);
            setEditing(true);
          }}
        />
      </SidebarFooter>
    </>
  );
}
