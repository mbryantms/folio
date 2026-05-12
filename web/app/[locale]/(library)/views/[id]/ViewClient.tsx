"use client";

import { CblViewDetail } from "@/components/saved-views/CblViewDetail";
import { CollectionViewDetail } from "@/components/saved-views/CollectionViewDetail";
import { FilterViewDetail } from "@/components/saved-views/FilterViewDetail";
import { SystemViewDetail } from "@/components/saved-views/SystemViewDetail";
import type { SavedViewView } from "@/lib/api/types";

/** The (library) layout's `<main>` already provides page-level
 *  padding (`px-4 py-6 md:px-8 md:py-8`) — render directly without
 *  an extra container so spacing matches the home page and series
 *  detail. */
export function ViewClient({ view }: { view: SavedViewView }) {
  if (view.kind === "system") return <SystemViewDetail view={view} />;
  if (view.kind === "cbl") return <CblViewDetail savedView={view} />;
  if (view.kind === "collection")
    return <CollectionViewDetail savedView={view} />;
  return <FilterViewDetail view={view} />;
}
