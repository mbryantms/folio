"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

import { useRemovedPending } from "@/lib/api/queries";
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

export function LibraryTabs({
  basePath,
  librarySlug,
}: {
  basePath: string;
  librarySlug: string;
}) {
  const pathname = usePathname() ?? "";
  // Removals the last scans left waiting for an operator. Surfaced on the
  // tab so a vanished folder is noticed without opening it.
  const pending = useRemovedPending(librarySlug);
  const pendingCount =
    (pending.data?.issues ?? 0) > 0
      ? (pending.data?.issues ?? 0)
      : (pending.data?.series ?? 0);
  return (
    <nav
      className="border-border -mb-px flex gap-1 overflow-x-auto overflow-y-hidden border-b"
      aria-label="Library sections"
    >
      {TABS.map((tab) => {
        const href = tab.slug ? `${basePath}/${tab.slug}` : basePath;
        const active =
          tab.slug === "" ? pathname === basePath : pathname.startsWith(href);
        return (
          <Link
            key={tab.slug || "overview"}
            href={href}
            className={cn(
              "border-b-2 px-3 py-2 text-sm font-medium transition-colors",
              active
                ? "border-primary text-foreground"
                : "text-muted-foreground hover:text-foreground border-transparent",
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
    </nav>
  );
}
