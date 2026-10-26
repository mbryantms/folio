"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { ArrowLeft } from "lucide-react";

import {
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebarCollapsed,
} from "@/components/folio/sidebar";
import { UserFooter } from "@/components/shell/UserFooter";

import { MetadataUnmatchedBadge } from "./MetadataUnmatchedBadge";
import type { DynamicBadge, NavSection } from "./nav";
import { navIcons } from "./nav-icons";

function DynamicBadgeFor({
  kind,
  collapsed,
}: {
  kind: DynamicBadge;
  collapsed: boolean;
}) {
  switch (kind) {
    case "metadata-unmatched":
      return <MetadataUnmatchedBadge collapsed={collapsed} />;
  }
}

/**
 * Admin / settings navigation on the upstream shadcn Sidebar primitives
 * (2026-10-26); see MainSidebar for the split of responsibilities.
 */
export function AdminSidebar({
  sections,
  title,
  user,
}: {
  sections: NavSection[];
  title: string;
  user: { display_name: string; email?: string | null; role: string };
}) {
  const pathname = usePathname() ?? "";
  const collapsed = useSidebarCollapsed();
  return (
    <>
      <SidebarContent>
        <nav aria-label={`${title} navigation`} className="flex flex-col gap-2">
          {/* Wayfinding back to the main library app — admin/settings are
              secondary surfaces reached from the library, so the return
              trip is a first-class row at the top of the nav. */}
          <SidebarGroup>
            <SidebarGroupContent>
              <SidebarMenu>
                <SidebarMenuItem>
                  <SidebarMenuButton asChild tooltip="Back to library">
                    <Link href="/">
                      <ArrowLeft />
                      <span>Back to library</span>
                    </Link>
                  </SidebarMenuButton>
                </SidebarMenuItem>
              </SidebarMenu>
            </SidebarGroupContent>
            <SidebarGroupLabel className="font-semibold tracking-widest uppercase">
              {title}
            </SidebarGroupLabel>
          </SidebarGroup>
          {sections.map((section) => (
            <SidebarGroup key={section.label}>
              <SidebarGroupLabel className="tracking-widest uppercase">
                {section.label}
              </SidebarGroupLabel>
              <SidebarGroupContent>
                <SidebarMenu>
                  {section.items.map((item) => {
                    const active = item.exact
                      ? pathname === item.href
                      : pathname === item.href ||
                        (item.href !== "" &&
                          pathname.startsWith(item.href + "/"));
                    const Icon = navIcons[item.icon];
                    return (
                      <SidebarMenuItem key={item.href}>
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
                            {item.dynamicBadge ? (
                              <DynamicBadgeFor
                                kind={item.dynamicBadge}
                                collapsed={collapsed}
                              />
                            ) : null}
                          </Link>
                        </SidebarMenuButton>
                      </SidebarMenuItem>
                    );
                  })}
                </SidebarMenu>
              </SidebarGroupContent>
            </SidebarGroup>
          ))}
        </nav>
      </SidebarContent>
      <SidebarFooter className="p-0">
        <UserFooter user={user} collapsed={collapsed} libraryHref="/" />
      </SidebarFooter>
    </>
  );
}
