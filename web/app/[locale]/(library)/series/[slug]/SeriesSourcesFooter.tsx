"use client";

/**
 * Series-page sources footer — mirrors `IssueSourcesFooter`.
 *
 * The CV / Metron TOS attribution row that used to live in
 * `<SeriesMetadataPanel>` alongside the sync card + ExternalIds card.
 * Those two now have their own tabs in the page's Tabs row, but the
 * attribution must stay visible regardless of which tab is open — so
 * it lives at the page bottom in its own dedicated row.
 *
 * Kept as a client component so the server-rendered `page.tsx` can stay
 * an `async function` without pulling `useExternalIdsSeries` into the
 * SSR path.
 */

import { SourcesFooter } from "@/components/library/SourcesFooter";
import { useExternalIdsSeries } from "@/lib/api/queries";

export function SeriesSourcesFooter({ seriesSlug }: { seriesSlug: string }) {
  const externalIds = useExternalIdsSeries(seriesSlug);
  return <SourcesFooter rows={externalIds.data?.rows} />;
}
