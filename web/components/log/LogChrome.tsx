"use client";

import { Download } from "lucide-react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { useRef } from "react";

import { PageHeader } from "@/components/admin/PageHeader";
import { RouteTabPanel } from "@/components/folio/route-tab-panel";
import { Button } from "@/components/ui/button";
import {
  SLIDE_TRANSITION,
  useSlidingIndicator,
} from "@/lib/ui/use-sliding-indicator";
import { cn } from "@/lib/utils";

/** The reading log's tabs (folio-flair phase 4). Each is its own route so
 *  links, reloads and Back all land on the right tab. */
export const LOG_TABS = [
  { href: "/log", label: "Overview" },
  { href: "/log/activity", label: "Finished" },
  { href: "/log/stats", label: "Stats" },
  { href: "/log/sessions", label: "Sessions" },
] as const;

/** Which tab a path belongs to (`/log` matches only itself). */
export function activeLogTab(pathname: string): string | null {
  const path = pathname.replace(/\/+$/, "") || "/";
  const hit = [...LOG_TABS]
    .reverse()
    .find(
      (t) =>
        path === t.href || (t.href !== "/log" && path.startsWith(`${t.href}/`)),
    );
  return hit?.href ?? null;
}

const LOG_INDICATOR = {
  active: '[aria-current="page"]',
  items: "a",
  attributes: ["aria-current"],
};

/** The log's tab pages: a tab switch slides the new page in from the
 *  side its tab sits on (folio-flair). */
export function LogPanel({ children }: { children: React.ReactNode }) {
  const pathname = usePathname() ?? "/log";
  const active = activeLogTab(pathname);
  return (
    <RouteTabPanel index={LOG_TABS.findIndex((t) => t.href === active)}>
      {children}
    </RouteTabPanel>
  );
}

/**
 * Header + tab bar shared by every reading-log page. Hidden on the
 * full-screen year in review (`/log/year/…`), which draws its own frame.
 */
export function LogChrome() {
  const pathname = usePathname() ?? "/log";
  const strip = useRef<HTMLUListElement | null>(null);
  const year = /\/log\/year(\/|$)/.test(pathname);
  // One underline glides between the tabs (folio-flair), like the Radix
  // tab strips; each link's own `::after` stays hidden while it shows.
  const indicator = useSlidingIndicator(strip, {
    ...LOG_INDICATOR,
    enabled: !year,
  });
  if (year) return null;
  const active = activeLogTab(pathname);
  return (
    <div className="space-y-4">
      <PageHeader
        title="Reading log"
        description="What you've read, when, and how much — with your streaks, milestones and year in review."
        actions={
          // Data liberation (3.3): the full reading history as CSV. A
          // same-origin `<a download>` carries the session cookie.
          <Button
            variant="ghost"
            asChild
            title="Download your full reading log as a CSV"
          >
            <a href="/api/me/reading-log/export" download>
              <Download aria-hidden="true" className="mr-1 size-3.5" />
              Export CSV
            </a>
          </Button>
        }
      />
      <nav aria-label="Reading log sections" className="-mt-2 overflow-x-auto">
        <ul
          ref={strip}
          data-indicator={indicator ? "" : undefined}
          className="group/strip border-border relative flex min-w-max gap-1 border-b"
        >
          {LOG_TABS.map((t) => {
            const on = active === t.href;
            return (
              <li key={t.href}>
                <Link
                  href={t.href}
                  aria-current={on ? "page" : undefined}
                  className={cn(
                    "relative inline-flex h-10 items-center px-3 text-sm font-medium transition-colors",
                    "after:absolute after:inset-x-2 after:-bottom-px after:h-0.5 after:rounded-full after:transition-colors",
                    on
                      ? "text-foreground after:bg-primary"
                      : "text-muted-foreground hover:text-foreground after:bg-transparent",
                    "group-data-[indicator]/strip:after:opacity-0",
                  )}
                >
                  {t.label}
                </Link>
              </li>
            );
          })}
          {indicator ? (
            <li
              aria-hidden
              className={cn(
                "bg-primary pointer-events-none absolute top-0 left-0 h-0.5 rounded-full",
                indicator.animate && SLIDE_TRANSITION,
              )}
              style={{
                // Where the link's own underline sat: inset 8px each side,
                // over the strip's bottom border.
                width: Math.max(0, indicator.width - 16),
                translate: `${indicator.left + 8}px ${indicator.top + indicator.height - 1}px`,
              }}
            />
          ) : null}
        </ul>
      </nav>
    </div>
  );
}
