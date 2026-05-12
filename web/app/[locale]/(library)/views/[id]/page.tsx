import { notFound } from "next/navigation";

import { apiGet, ApiError } from "@/lib/api/fetch";
import type { SavedViewListView, SavedViewView } from "@/lib/api/types";

import { ViewClient } from "./ViewClient";

/** Saved-view detail page. Three kinds dispatch:
 *
 *   - `filter_series` — DSL-driven series grid via `<FilterViewDetail>`
 *   - `cbl`           — reading-list detail via `<CblViewDetail>`
 *   - `system`        — built-in rails (Continue reading / On deck) via
 *                       `<SystemViewDetail>`, rendered as a full-page grid
 *                       of the same cards used in the home rail.
 *
 *  The route accepts both UUID paths (`/views/{uuid}`) for user/admin
 *  views and kebab-case system aliases (`/views/continue-reading`,
 *  `/views/on-deck`) that map to the row's `system_key`. The aliases keep
 *  the URL human-readable when the user clicks a system-rail header on
 *  the home page. */
export default async function ViewPage({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = await params;
  let list: SavedViewListView;
  try {
    list = await apiGet<SavedViewListView>("/me/saved-views");
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) notFound();
    throw e;
  }
  let found: SavedViewView | undefined = list.items.find((v) => v.id === id);
  if (!found) {
    // Kebab-case URL alias for a system rail (e.g. `continue-reading` →
    // system_key `continue_reading`). Keeps URLs human-readable while the
    // canonical row id stays a UUID.
    const systemKey = id.replace(/-/g, "_");
    found = list.items.find((v) => v.system_key === systemKey);
  }
  if (!found) notFound();
  return <ViewClient view={found} />;
}
