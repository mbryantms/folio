"use client";

import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";
import { ChevronDown, ChevronRight, Sparkles } from "lucide-react";

import {
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebarCollapsed,
} from "@/components/folio/sidebar";
import { UserFooter } from "@/components/shell/UserFooter";
import { useMarkerCount } from "@/lib/api/queries";
import { useSidebarSectionCollapse } from "@/lib/use-sidebar-section-collapse";

import { mainNavIcons } from "./main-nav-icons";
import type { MainNavSection } from "./main-nav";
import { railIconByKey } from "./rail-icons";

/**
 * Library navigation on the upstream shadcn Sidebar primitives (2026-10-26).
 * The icon-collapsed rail, the tooltips it shows, and the cookie-persisted
 * open state all come from the vendored component; this file only maps the
 * `/me/sidebar-layout` sections onto `SidebarGroup` / `SidebarMenu`.
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
  const markerCount = useMarkerCount({ enabled: showMarkerCount });
  const sectionCollapse = useSidebarSectionCollapse();

  return (
    <>
      <SidebarContent>
        <nav aria-label="Library navigation" className="flex flex-col gap-2">
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
                      const badge =
                        showMarkerCount &&
                        isBuiltin("bookmarks") &&
                        markerCount.data &&
                        markerCount.data.total > 0
                          ? markerCount.data.total > 99
                            ? "99+"
                            : String(markerCount.data.total)
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
                            >
                              <Icon />
                              <span>{item.label}</span>
                            </Link>
                          </SidebarMenuButton>
                          {badge ? (
                            <SidebarMenuBadge
                              aria-label={`${markerCount.data?.total} markers`}
                            >
                              {badge}
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
        <UserFooter user={user} collapsed={collapsed} />
      </SidebarFooter>
    </>
  );
}
