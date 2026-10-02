"use client";

import * as React from "react";

import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useSeriesRelationships } from "@/lib/api/queries";

/** Every tab the series page can show, in strip order. */
export const SERIES_TABS = [
  "credits",
  "cast",
  "details",
  "collection",
  "related",
  "appearances",
  "activity",
  "markers",
] as const;
export type SeriesTab = (typeof SERIES_TABS)[number];
export const DEFAULT_SERIES_TAB: SeriesTab = "credits";

/** `?tab=` → a tab this page actually renders (else the default). */
export function resolveSeriesTab(
  raw: string | null | undefined,
  available: ReadonlySet<SeriesTab>,
): SeriesTab {
  const t = (raw ?? "").toLowerCase() as SeriesTab;
  return available.has(t) ? t : DEFAULT_SERIES_TAB;
}

/** The URL for `tab`: `?tab=` set (the default tab drops it), every
 *  other param kept. */
export function urlWithTab(href: string, tab: SeriesTab): string {
  const url = new URL(href);
  if (tab === DEFAULT_SERIES_TAB) url.searchParams.delete("tab");
  else url.searchParams.set("tab", tab);
  return url.toString();
}

/**
 * Series page tab strip (WP-7.7). Controlled so the active tab mirrors
 * `?tab=` — a deep link (`?tab=related`) opens that tab and switching
 * tabs `replaceState`s the URL (no history entry, no RSC round-trip; the
 * same pattern the Issues panel uses for `?q=`). Panels are the server-
 * rendered `children`; the Related panel unmounts while inactive, so its
 * queries are lazy.
 *
 * The Related label shows the direct-relationship count: the server's
 * `relationship_count` until the tab has loaded the relationships, then
 * the live count from the query cache (read with `enabled: false`, so the
 * label never fetches on its own).
 */
export function SeriesTabs({
  seriesSlug,
  initialTab,
  relationshipCount,
  hasAppearances,
  hasNotes,
  children,
}: {
  seriesSlug: string;
  initialTab?: string | null;
  relationshipCount: number | null;
  hasAppearances: boolean;
  hasNotes: boolean;
  children: React.ReactNode;
}) {
  const available = React.useMemo(() => {
    const s = new Set<SeriesTab>(SERIES_TABS);
    if (!hasAppearances) s.delete("appearances");
    if (!hasNotes) s.delete("markers");
    return s;
  }, [hasAppearances, hasNotes]);
  const resolved = resolveSeriesTab(initialTab, available);
  const [tab, setTab] = React.useState<SeriesTab>(resolved);
  // A soft navigation to a different `?tab=` re-renders the server page
  // with a new `initialTab`; follow it (derive-from-props, no effect).
  const [prevResolved, setPrevResolved] = React.useState(resolved);
  if (prevResolved !== resolved) {
    setPrevResolved(resolved);
    setTab(resolved);
  }

  const cached = useSeriesRelationships(seriesSlug, { enabled: false });
  const liveCount = cached.data
    ? cached.data.relationships.length + cached.data.arcs.length
    : null;
  const count = liveCount ?? relationshipCount;

  const onChange = (v: string) => {
    const next = resolveSeriesTab(v, available);
    setTab(next);
    if (typeof window !== "undefined") {
      window.history.replaceState(
        window.history.state,
        "",
        urlWithTab(window.location.href, next),
      );
    }
  };

  return (
    <Tabs value={tab} onValueChange={onChange}>
      <TabsList>
        <TabsTrigger value="credits">Credits</TabsTrigger>
        <TabsTrigger value="cast">Cast &amp; Setting</TabsTrigger>
        <TabsTrigger value="details">Details</TabsTrigger>
        <TabsTrigger value="collection">Collection</TabsTrigger>
        <TabsTrigger value="related">
          Related
          {count != null && count > 0 ? (
            <span
              className="text-muted-foreground ml-1.5 text-xs tabular-nums"
              aria-label={`${count} ${count === 1 ? "relationship" : "relationships"}`}
            >
              {count}
            </span>
          ) : null}
        </TabsTrigger>
        {hasAppearances && (
          <TabsTrigger value="appearances">Appears in</TabsTrigger>
        )}
        <TabsTrigger value="activity">Activity</TabsTrigger>
        {hasNotes && <TabsTrigger value="markers">Your notes</TabsTrigger>}
      </TabsList>
      {children}
    </Tabs>
  );
}
