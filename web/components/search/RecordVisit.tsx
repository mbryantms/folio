"use client";

import { useEffect } from "react";

import type { RecentVisit } from "@/lib/search/recent-visits";
import { recordVisit } from "@/lib/search/use-recent-visits";

/** Renders nothing; records the page it is mounted on into the
 *  "Recently visited" list the search palette shows. Server pages build
 *  the entry with the pure builders in `lib/search/recent-visits.ts`
 *  (`<RecordVisit visit={seriesVisit(series)} />`); `null` skips. */
export function RecordVisit({ visit }: { visit: RecentVisit | null }) {
  // Primitive deps: a server page hands a fresh object every render.
  const kind = visit?.kind;
  const id = visit?.id;
  const title = visit?.title;
  const subtitle = visit?.subtitle ?? null;
  const href = visit?.href;
  const thumbUrl = visit?.thumbUrl ?? null;
  useEffect(() => {
    if (!kind || !id || !title || !href) return;
    recordVisit({ kind, id, title, subtitle, href, thumbUrl });
  }, [kind, id, title, subtitle, href, thumbUrl]);
  return null;
}
