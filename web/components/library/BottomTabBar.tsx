"use client";

import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";
import {
  Bookmark,
  House,
  Library,
  Menu,
  Search,
  type LucideIcon,
} from "lucide-react";

import { useEffect, useRef } from "react";

import { SlidingNavHighlight } from "@/components/shell/SlidingNavHighlight";
import { useSearchModal } from "@/lib/search/use-search-modal";
import { cn } from "@/lib/utils";
import { CHROME_BAND_CLASS } from "@/lib/ui/chrome-band";
import { useHideOnScroll } from "@/lib/ui/use-hide-on-scroll";

const TAB_CLASS =
  "flex flex-1 flex-col items-center justify-center gap-0.5 text-[10px] font-medium transition-colors active:bg-secondary focus-visible:outline-2 focus-visible:outline-ring";

/**
 * Mobile-only bottom tab bar (`md:hidden`). Thumb-reachable shortcuts to the
 * core library destinations; the **More** tab opens the shell's existing nav
 * sheet (the full sidebar). **Search** opens the global `SearchModal`
 * (the same surface as desktop's ⌘K and topbar search): below `md` the
 * library shell has no topbar (phone-chrome-1.0), so this is the phone's
 * search entry, and it lands in a focused input with live results and
 * recents instead of the empty `/search` page, which stays the full
 * results destination (the sheet's footer). Desktop keeps the left
 * sidebar and the topbar search — this bar never renders at `md+`.
 * Rendered only inside `MainShell` (the library/reader-app shell), so it
 * doesn't appear in admin/settings or the reader.
 *
 * Reads `?library=all` to distinguish the Home and Library tabs, which share
 * the `/` path — so it must be mounted under a `Suspense` boundary.
 *
 * Motion (folio-flair, standard): the active tab's pill slides between
 * tabs on navigation (`SlidingNavHighlight`), and the bar slides away
 * while scrolling down and returns on scrolling up, near the top, at the
 * page end, on navigation, when an overlay opens, or when keyboard focus
 * enters it (`useHideOnScroll`). It only translates: `MainShell` keeps
 * its `--bottom-tab-h` bottom padding either way, so content never
 * reflows when the bar moves.
 */
export function BottomTabBar({ onMore }: { onMore: () => void }) {
  const pathname = usePathname() ?? "";
  const searchParams = useSearchParams();
  const libraryAll = searchParams.get("library") === "all";
  const onBookmarks =
    pathname === "/bookmarks" || pathname.startsWith("/bookmarks/");
  const onSearch = pathname === "/search" || pathname.startsWith("/search/");
  const { open: searchOpen, setOpen: setSearchOpen } = useSearchModal();
  const navRef = useRef<HTMLElement | null>(null);
  const { hidden, reveal } = useHideOnScroll({ blocked: searchOpen });
  // Warm the lazy modal chunk on phones so the first tap opens at once
  // and the input's focus lands inside the tap gesture, which is when
  // iOS shows the keyboard. Desktop loads it on the first ⌘K as before.
  useEffect(() => {
    if (!window.matchMedia("(width < 48rem)").matches) return;
    void import("@/components/SearchModal");
  }, []);

  return (
    <nav
      ref={navRef}
      aria-label="Primary"
      data-hidden={hidden ? "true" : undefined}
      onFocus={reveal}
      // Floating content (menus, popovers, selects) keeps clear of the bar:
      // `safeAreaCollisionPadding` reads this marker's height.
      data-bottom-inset=""
      className={cn(
        CHROME_BAND_CLASS,
        "fixed inset-x-0 bottom-0 isolate z-30 flex h-[calc(var(--bottom-tab-h)+var(--safe-bottom))] items-stretch border-t pr-(--safe-right) pb-(--safe-bottom) pl-(--safe-left) transition-transform duration-200 ease-out [view-transition-name:shell-tabbar] data-hidden:translate-y-full md:hidden",
      )}
    >
      <SlidingNavHighlight
        containerRef={navRef}
        inset={{ x: 10, y: 5 }}
        className="bg-secondary rounded-lg"
      />
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
        tour="tab-library"
      />
      <button
        type="button"
        onClick={() => setSearchOpen(true)}
        aria-haspopup="dialog"
        aria-expanded={searchOpen}
        aria-current={onSearch ? "page" : undefined}
        data-tour="tab-search"
        className={cn(
          TAB_CLASS,
          searchOpen || onSearch
            ? "text-foreground"
            : "text-muted-foreground hover:text-foreground",
        )}
      >
        <Search className="size-5" aria-hidden="true" />
        <span>Search</span>
      </button>
      <TabLink
        href="/bookmarks"
        icon={Bookmark}
        label="Bookmarks"
        active={onBookmarks}
        tour="tab-bookmarks"
      />
      <button
        type="button"
        onClick={onMore}
        aria-label="More navigation"
        aria-haspopup="dialog"
        data-tour="tab-more"
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
  tour,
}: {
  href: string;
  icon: LucideIcon;
  label: string;
  active: boolean;
  /** First-run tour anchor (phone fallback for the sidebar steps). */
  tour?: string;
}) {
  return (
    <Link
      href={href}
      data-tour={tour}
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
