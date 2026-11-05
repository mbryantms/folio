"use client";

import { useParams, usePathname } from "next/navigation";
import type * as React from "react";

import { PageRailsSkeleton } from "@/components/skeletons/LoadingShapes";
import { cardGridColumns, CARD_SIZE_KEYS } from "@/lib/library/card-size";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** The card-size key of the cover grid a `(library)` URL will show. A
 *  `loading.tsx` can't see the URL, so the grid fallback asks here:
 *  `/views/<uuid>` and `/views/want-to-read` are saved views (filter,
 *  CBL, collection), any other `/views/<alias>` a system rail
 *  (`continue-reading`, `on-deck`, …), everything else the library grid. */
export function cardSizeKeyForPath(pathname: string): string {
  const m = /^\/views\/([^/]+)/.exec(pathname);
  if (!m) return CARD_SIZE_KEYS.library;
  const id = m[1]!;
  return UUID.test(id) || id === "want-to-read"
    ? CARD_SIZE_KEYS.savedView
    : CARD_SIZE_KEYS.systemView;
}

/** A cover grid sized by the card-size preference of the route it
 *  stands in for. */
export function RouteCardGrid({
  className,
  children,
  ...rest
}: React.HTMLAttributes<HTMLDivElement>) {
  const key = cardSizeKeyForPath(usePathname() ?? "");
  return (
    <div
      {...rest}
      className={className}
      style={{ gridTemplateColumns: cardGridColumns(key) }}
    >
      {children}
    </div>
  );
}

/** `/pages/<slug>` fallback: series rails at that page's card size,
 *  looked up by slug (`PageRails` records slug → page id). */
export function CustomPageRailsSkeleton() {
  const slug = useParams<{ slug?: string }>()?.slug;
  return (
    <PageRailsSkeleton
      rails={3}
      card="series"
      cardSizeKey={slug ? CARD_SIZE_KEYS.pageSlug(slug) : undefined}
      label="Loading page…"
    />
  );
}
