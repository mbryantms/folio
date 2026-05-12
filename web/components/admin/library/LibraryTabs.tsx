"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

import { cn } from "@/lib/utils";

const TABS = [
  { slug: "", label: "Overview" },
  { slug: "settings", label: "Settings" },
  { slug: "health", label: "Health" },
  { slug: "history", label: "History" },
  { slug: "removed", label: "Removed" },
  { slug: "scan", label: "Live scan" },
] as const;

export function LibraryTabs({ basePath }: { basePath: string }) {
  const pathname = usePathname() ?? "";
  return (
    <nav
      className="border-border -mb-px flex gap-1 overflow-x-auto border-b"
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
          </Link>
        );
      })}
    </nav>
  );
}
