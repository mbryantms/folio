"use client";

import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";
import { Bookmark, House, Library, Menu, type LucideIcon } from "lucide-react";

import { cn } from "@/lib/utils";

const TAB_CLASS =
  "flex flex-1 flex-col items-center justify-center gap-0.5 text-[10px] font-medium transition-colors";

/**
 * Mobile-only bottom tab bar (`md:hidden`). Thumb-reachable shortcuts to the
 * core library destinations; the **More** tab opens the shell's existing nav
 * sheet (the full sidebar). Desktop keeps the left sidebar — this bar never
 * renders at `md+`. Rendered only inside `MainShell` (the library/reader-app
 * shell), so it doesn't appear in admin/settings or the reader.
 *
 * Reads `?library=all` to distinguish the Home and Library tabs, which share
 * the `/` path — so it must be mounted under a `Suspense` boundary.
 */
export function BottomTabBar({ onMore }: { onMore: () => void }) {
  const pathname = usePathname() ?? "";
  const searchParams = useSearchParams();
  const libraryAll = searchParams.get("library") === "all";
  const onBookmarks =
    pathname === "/bookmarks" || pathname.startsWith("/bookmarks/");

  return (
    <nav
      aria-label="Primary"
      className="border-border bg-background/95 fixed inset-x-0 bottom-0 z-30 flex h-[calc(var(--bottom-tab-h)+var(--safe-bottom))] items-stretch border-t pb-(--safe-bottom) backdrop-blur md:hidden"
    >
      <TabLink
        href="/"
        icon={House}
        label="Home"
        active={pathname === "/" && !libraryAll}
      />
      <TabLink
        href="/?library=all"
        icon={Library}
        label="Library"
        active={libraryAll}
      />
      <TabLink
        href="/bookmarks"
        icon={Bookmark}
        label="Bookmarks"
        active={onBookmarks}
      />
      <button
        type="button"
        onClick={onMore}
        aria-label="More navigation"
        aria-haspopup="dialog"
        className={cn(TAB_CLASS, "text-muted-foreground hover:text-foreground")}
      >
        <Menu className="size-5" aria-hidden="true" />
        <span>More</span>
      </button>
    </nav>
  );
}

function TabLink({
  href,
  icon: Icon,
  label,
  active,
}: {
  href: string;
  icon: LucideIcon;
  label: string;
  active: boolean;
}) {
  return (
    <Link
      href={href}
      aria-current={active ? "page" : undefined}
      className={cn(
        TAB_CLASS,
        active
          ? "text-foreground"
          : "text-muted-foreground hover:text-foreground",
      )}
    >
      <Icon className="size-5" aria-hidden="true" />
      <span>{label}</span>
    </Link>
  );
}
