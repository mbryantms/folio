"use client";

import * as React from "react";

import { CHROME_BAND_CLASS } from "@/lib/ui/chrome-band";
import { useStickyToolbar } from "@/lib/ui/use-sticky-toolbar";
import { cn } from "@/lib/utils";

/**
 * The one pinned control row a list page may have from `sm` up — the
 * desktop counterpart of `PhoneControlsBar`'s sticky band.
 *
 * Rule (docs/dev/phone-layout.md, rule 3): a row pins under the topbar
 * at every width if and only if it holds tools you need while
 * scrolling — search, sort, filters, bulk selection. Titles, tab
 * strips, chip rows and one-time settings never pin. This component is
 * how a page states that its row qualifies; the library grid,
 * bookmarks, CBL reading lists and the series Issues panel use it.
 *
 * Behaviour comes from `useStickyToolbar`: the row pins only while it
 * fits on one line (a wrapped toolbar on an iPad with the sidebar open
 * stays in flow rather than pinning 100px of chrome) and only while
 * `enabled` — pass `!selectMode` so the sticky `SelectionToolbar` owns
 * the band while selecting. The chrome-band border / backdrop appear
 * only once the row is actually held, so the page is unchanged at rest.
 *
 * Edge to edge inside the shell column (`-mx-4` / `md:-mx-8` mirror the
 * shell's `px-4` / `md:px-8`). Extra `data-*` attributes pass through
 * to the row for tests and styling hooks.
 */
export function StickyToolbarRow({
  enabled = true,
  className,
  children,
  ...rest
}: {
  enabled?: boolean;
  className?: string;
  children: React.ReactNode;
} & Omit<React.HTMLAttributes<HTMLDivElement>, "className" | "children">) {
  const { ref, sentinelRef, sticky, stuck } = useStickyToolbar({ enabled });
  return (
    <>
      <div ref={sentinelRef} aria-hidden="true" className="max-sm:hidden" />
      <div
        ref={ref}
        data-toolbar-row=""
        data-sticky={sticky ? "" : undefined}
        data-stuck={stuck ? "" : undefined}
        className={cn(
          "-mx-4 flex flex-wrap items-center gap-2 px-4 py-2 max-sm:hidden md:-mx-8 md:px-8",
          sticky && "sticky top-(--topbar-h) z-20",
          stuck && cn(CHROME_BAND_CLASS, "border-b"),
          className,
        )}
        {...rest}
      >
        {children}
      </div>
    </>
  );
}
