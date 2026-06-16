"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { ArrowLeft } from "lucide-react";

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { UserFooter } from "@/components/shell/UserFooter";
import { cn } from "@/lib/utils";

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

export function AdminSidebar({
  sections,
  title,
  user,
  collapsed = false,
}: {
  sections: NavSection[];
  title: string;
  user: { display_name: string; email?: string | null; role: string };
  /** When true, the sidebar shrinks to icon-only mode with hover tooltips. */
  collapsed?: boolean;
}) {
  const pathname = usePathname() ?? "";
  return (
    <div className="flex h-full flex-col">
      <TooltipProvider delayDuration={200}>
        <nav
          aria-label={`${title} navigation`}
          className={cn(
            "flex flex-1 flex-col gap-6 overflow-y-auto py-6 text-sm",
            collapsed ? "px-2" : "px-3",
          )}
        >
          {/* Wayfinding back to the main library app — admin/settings are
              secondary surfaces reached from the library, so the return
              trip is a first-class row at the top of the nav. */}
          {collapsed ? (
            <Tooltip>
              <TooltipTrigger asChild>
                <Link
                  href="/"
                  aria-label="Back to library"
                  className="text-muted-foreground hover:bg-secondary/50 hover:text-foreground mx-auto flex size-9 items-center justify-center rounded-md transition-colors"
                >
                  <ArrowLeft className="h-4 w-4" />
                </Link>
              </TooltipTrigger>
              <TooltipContent side="right" sideOffset={8}>
                Back to library
              </TooltipContent>
            </Tooltip>
          ) : (
            <Link
              href="/"
              className="text-muted-foreground hover:bg-secondary/50 hover:text-foreground flex items-center gap-2.5 rounded-md px-3 py-1.5 transition-colors"
            >
              <ArrowLeft className="h-4 w-4 shrink-0" />
              <span className="truncate">Back to library</span>
            </Link>
          )}
          {!collapsed && (
            <div className="px-3">
              <p className="text-muted-foreground text-xs font-semibold tracking-widest uppercase">
                {title}
              </p>
            </div>
          )}
          {sections.map((section) => (
            <div key={section.label} className="flex flex-col gap-1">
              {!collapsed && (
                <p className="text-muted-foreground/70 px-3 text-[11px] font-medium tracking-widest uppercase">
                  {section.label}
                </p>
              )}
              <ul className="flex flex-col gap-0.5">
                {section.items.map((item) => {
                  // Exact-match nav entries (typically the section
                  // index, e.g. Dashboard whose href is `/admin`)
                  // light up ONLY on their own path. Without this
                  // flag the descendant-match below would light
                  // Dashboard on every admin sub-page since they're
                  // all `/admin/…`.
                  const active = item.exact
                    ? pathname === item.href
                    : pathname === item.href ||
                      (item.href !== "" &&
                        pathname.startsWith(item.href + "/"));
                  const Icon = navIcons[item.icon];
                  const link = (
                    <Link
                      href={item.href}
                      className={cn(
                        "relative flex items-center rounded-md transition-colors",
                        collapsed
                          ? "size-9 justify-center"
                          : "gap-2.5 px-3 py-1.5",
                        active
                          ? "bg-secondary text-foreground"
                          : "text-muted-foreground hover:bg-secondary/50 hover:text-foreground",
                      )}
                      aria-current={active ? "page" : undefined}
                      aria-label={collapsed ? item.label : undefined}
                    >
                      <Icon className="h-4 w-4 shrink-0" />
                      {!collapsed && (
                        <span className="truncate">{item.label}</span>
                      )}
                      {item.dynamicBadge ? (
                        <DynamicBadgeFor
                          kind={item.dynamicBadge}
                          collapsed={collapsed}
                        />
                      ) : null}
                    </Link>
                  );
                  return (
                    <li key={item.href}>
                      {collapsed ? (
                        <Tooltip>
                          <TooltipTrigger asChild>{link}</TooltipTrigger>
                          <TooltipContent side="right" sideOffset={8}>
                            {item.label}
                          </TooltipContent>
                        </Tooltip>
                      ) : (
                        link
                      )}
                    </li>
                  );
                })}
              </ul>
            </div>
          ))}
        </nav>
      </TooltipProvider>
      <UserFooter user={user} collapsed={collapsed} libraryHref="/" />
    </div>
  );
}
