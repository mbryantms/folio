"use client";

import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { ShortcutsHelpButton } from "@/components/shell/ShortcutsHelpButton";
import { UserFooter } from "@/components/shell/UserFooter";
import { useMarkerCount } from "@/lib/api/queries";
import { cn } from "@/lib/utils";

import { mainNavIcons } from "./main-nav-icons";
import type { MainNavSection } from "./main-nav";
import { railIconByKey } from "./rail-icons";
import { Sparkles } from "lucide-react";

/**
 * Library sidebar — counterpart to AdminSidebar but for the main reader app.
 * Highlights the active item by both pathname and (for "All Libraries" /
 * per-library entries) the `?library=…` query param so per-library views
 * pick the right row even though they share `/` as their base path.
 */
export function MainSidebar({
  sections,
  title,
  user,
  collapsed = false,
  showMarkerCount = false,
}: {
  sections: MainNavSection[];
  title: string;
  user: { display_name: string; email: string | null; role: string };
  /** When true, the sidebar shrinks to icon-only mode with hover tooltips. */
  collapsed?: boolean;
  /** Mirrors `me.show_marker_count`. When false (default), the Bookmarks
   *  row stays uncluttered; users can flip it in /settings/account. */
  showMarkerCount?: boolean;
}) {
  const pathname = usePathname() ?? "";
  const search = useSearchParams();
  const activeLibrary = search?.get("library") ?? null;
  // M8 polish: surface the marker total on the Bookmarks row. Cached
  // 60s server-side; create/delete invalidations refresh it eagerly.
  // Skipped when the user has the badge disabled — keeps a quiet
  // sidebar by default + saves the network round-trip.
  const markerCount = useMarkerCount({ enabled: showMarkerCount });

  return (
    <div className="flex h-full flex-col">
      <TooltipProvider delayDuration={200}>
        <nav
          aria-label={`${title} navigation`}
          className={cn(
            // Section-to-section gap kept compact — the uppercase
            // header rows give visual separation on their own, and a
            // bigger gap stacks up too much white space when the user
            // splits a kind across two groups (e.g. "All Libraries" in
            // one section, the named libraries in another).
            "flex flex-1 flex-col gap-3 overflow-y-auto py-6 text-sm",
            collapsed ? "px-2" : "px-3",
          )}
        >
          {!collapsed && (
            <div className="px-3">
              <p className="text-muted-foreground text-xs font-semibold tracking-widest uppercase">
                {title}
              </p>
            </div>
          )}
          {sections.map((section, sectionIdx) => {
            // Spacer rows: small visual gap, no header, no items.
            if (section.isSpacer) {
              return (
                <div
                  key={`spacer-${sectionIdx}`}
                  className="h-0.5"
                  aria-hidden
                />
              );
            }
            return (
            <div
              key={`${section.label ?? "untitled"}-${sectionIdx}`}
              className="flex flex-col gap-1"
            >
              {!collapsed && section.label && (
                <p className="text-muted-foreground/70 px-3 text-[11px] font-medium tracking-widest uppercase">
                  {section.label}
                </p>
              )}
              <ul className="flex flex-col gap-0.5">
                {section.items.map((item) => {
                  // Icon resolution order:
                  //   1. The fixed main-nav registry (Home / Library /
                  //      Bookmark / etc. — keyed by PascalCase).
                  //   2. The rail-icon registry shared with the home
                  //      header (kebab-case keys like "sparkles",
                  //      "shield"). Saved-view sidebar entries emit
                  //      this form.
                  //   3. Sparkles fallback so the row never renders
                  //      iconless.
                  const Icon =
                    mainNavIcons[item.icon as keyof typeof mainNavIcons] ??
                    railIconByKey(item.icon)?.Icon ??
                    Sparkles;
                  const itemSearch = item.href.includes("?library=")
                    ? (item.href.split("?library=")[1] ?? null)
                    : null;
                  const itemPath = item.href.split("?")[0]!;
                  // "All Libraries" and "Home" both point at "/", so pick the
                  // active row off the query param: present → match a per-library
                  // entry, absent → match plain Home/All Libraries.
                  let active = false;
                  if (itemSearch) {
                    // per-library entry
                    active =
                      pathname === itemPath && activeLibrary === itemSearch;
                  } else if (item.label === "All Libraries") {
                    active =
                      pathname === itemPath &&
                      activeLibrary != null &&
                      pathname === "/";
                    // "All Libraries" lights up only when a library is selected
                    // (treat it as "you're inside the libraries section"). The
                    // "Home" item handles the no-filter case.
                  } else if (item.label === "Home") {
                    active = pathname === itemPath && activeLibrary == null;
                  } else {
                    active =
                      pathname === itemPath ||
                      (itemPath !== "" && pathname.startsWith(itemPath + "/"));
                  }
                  // Bookmarks gets a count badge sourced from
                  // useMarkerCount when the user opts in. Hidden when
                  // collapsed (icon-only), when 0 (avoid a "0" chip on
                  // fresh accounts), and when the placeholder pill is
                  // showing (soon-tag wins).
                  const badge =
                    showMarkerCount &&
                    !collapsed &&
                    !item.placeholder &&
                    item.label === "Bookmarks" &&
                    markerCount.data &&
                    markerCount.data.total > 0
                      ? markerCount.data.total > 99
                        ? "99+"
                        : String(markerCount.data.total)
                      : null;
                  const link = (
                    <Link
                      href={item.href}
                      className={cn(
                        "flex items-center rounded-md transition-colors",
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
                      {!collapsed && item.placeholder ? (
                        <span className="text-muted-foreground/60 ml-auto text-[10px] tracking-wider uppercase">
                          soon
                        </span>
                      ) : badge ? (
                        <span
                          className="bg-muted text-muted-foreground ml-auto rounded-full px-1.5 py-0.5 text-[10px] font-medium tabular-nums"
                          aria-label={`${markerCount.data?.total} markers`}
                        >
                          {badge}
                        </span>
                      ) : null}
                    </Link>
                  );
                  return (
                    <li key={`${item.href}-${item.label}`}>
                      {collapsed ? (
                        <Tooltip>
                          <TooltipTrigger asChild>{link}</TooltipTrigger>
                          <TooltipContent side="right" sideOffset={8}>
                            {item.label}
                            {item.placeholder ? (
                              <span className="text-muted-foreground ml-2 text-[10px] tracking-wider uppercase">
                                soon
                              </span>
                            ) : null}
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
            );
          })}
        </nav>
        <div className={cn("px-2 pb-1", collapsed && "px-2")}>
          <ShortcutsHelpButton collapsed={collapsed} />
        </div>
      </TooltipProvider>
      <UserFooter user={user} collapsed={collapsed} />
    </div>
  );
}
