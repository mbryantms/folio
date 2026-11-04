"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useRef } from "react";

import { RouteTabPanel } from "@/components/folio/route-tab-panel";
import { useRemovedPending } from "@/lib/api/queries";
import {
  SLIDE_TRANSITION,
  useSlidingIndicator,
} from "@/lib/ui/use-sliding-indicator";
import { cn } from "@/lib/utils";

const TABS = [
  { slug: "", label: "Overview" },
  { slug: "settings", label: "Settings" },
  { slug: "health", label: "Health" },
  { slug: "history", label: "History" },
  { slug: "duplicates", label: "Duplicates" },
  { slug: "removed", label: "Removed" },
  { slug: "scan", label: "Live scan" },
] as const;

/** Index of the tab `pathname` belongs to under `basePath` (-1: none). */
export function activeLibraryTab(pathname: string, basePath: string): number {
  return TABS.findIndex((tab) => {
    const href = tab.slug ? `${basePath}/${tab.slug}` : basePath;
    return tab.slug === "" ? pathname === basePath : pathname.startsWith(href);
  });
}

const NAV_INDICATOR = {
  active: '[aria-current="page"]',
  items: "a",
  attributes: ["aria-current"],
};

/** The section pages under the tabs: a tab switch slides the new page in
 *  from the side its tab sits on (folio-flair). */
export function LibraryTabPanel({
  basePath,
  className,
  children,
}: {
  basePath: string;
  className?: string;
  children: React.ReactNode;
}) {
  const pathname = usePathname() ?? "";
  return (
    <RouteTabPanel
      index={activeLibraryTab(pathname, basePath)}
      className={className}
    >
      {children}
    </RouteTabPanel>
  );
}

export function LibraryTabs({
  basePath,
  librarySlug,
}: {
  basePath: string;
  librarySlug: string;
}) {
  const pathname = usePathname() ?? "";
  const strip = useRef<HTMLElement | null>(null);
  // One underline glides between the sections (folio-flair) in place of
  // each link's own bottom border.
  const indicator = useSlidingIndicator(strip, NAV_INDICATOR);
  const at = activeLibraryTab(pathname, basePath);
  // Removals the last scans left waiting for an operator. Surfaced on the
  // tab so a vanished folder is noticed without opening it.
  const pending = useRemovedPending(librarySlug);
  const pendingCount =
    (pending.data?.issues ?? 0) > 0
      ? (pending.data?.issues ?? 0)
      : (pending.data?.series ?? 0);
  return (
    <nav
      ref={strip}
      className="border-border relative -mb-px flex gap-1 overflow-x-auto overflow-y-hidden border-b"
      aria-label="Library sections"
    >
      {TABS.map((tab, i) => {
        const href = tab.slug ? `${basePath}/${tab.slug}` : basePath;
        const active = i === at;
        return (
          <Link
            key={tab.slug || "overview"}
            href={href}
            className={cn(
              "border-b-2 px-3 py-2 text-sm font-medium transition-colors",
              active
                ? "text-foreground"
                : "text-muted-foreground hover:text-foreground",
              active && !indicator ? "border-primary" : "border-transparent",
            )}
            aria-current={active ? "page" : undefined}
          >
            {tab.label}
            {tab.slug === "removed" && pendingCount > 0 ? (
              <span
                className="bg-muted text-foreground ml-1.5 inline-flex min-w-5 items-center justify-center rounded-full px-1.5 text-xs tabular-nums"
                aria-label={`${pendingCount} pending`}
              >
                {pendingCount.toLocaleString()}
              </span>
            ) : null}
          </Link>
        );
      })}
      {indicator ? (
        <span
          aria-hidden
          className={cn(
            "bg-primary pointer-events-none absolute top-0 left-0 h-0.5",
            indicator.animate && SLIDE_TRANSITION,
          )}
          style={{
            width: indicator.width,
            // Over the link's own (transparent) 2px bottom border.
            translate: `${indicator.left}px ${indicator.top + indicator.height - 2}px`,
          }}
        />
      ) : null}
    </nav>
  );
}
