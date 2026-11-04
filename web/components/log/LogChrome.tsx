"use client";

import { Download } from "lucide-react";
import Link from "next/link";
import { usePathname } from "next/navigation";

import { PageHeader } from "@/components/admin/PageHeader";
import { Button } from "@/components/ui/button";
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

/**
 * Header + tab bar shared by every reading-log page. Hidden on the
 * full-screen year in review (`/log/year/…`), which draws its own frame.
 */
export function LogChrome() {
  const pathname = usePathname() ?? "/log";
  if (/\/log\/year(\/|$)/.test(pathname)) return null;
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
        <ul className="border-border flex min-w-max gap-1 border-b">
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
                  )}
                >
                  {t.label}
                </Link>
              </li>
            );
          })}
        </ul>
      </nav>
    </div>
  );
}
