"use client";

/**
 * Client wrapper that mounts the metadata-providers M5 surfaces
 * (sync-status card + external-ids card + sources footer) on the
 * series page. Kept separate so the server-rendered `page.tsx` can
 * stay an `async function` without pulling React-Query hooks into
 * the SSR path.
 */

import { ExternalIdsCard } from "@/components/library/ExternalIdsCard";
import { MetadataSyncStatusCard } from "@/components/library/MetadataSyncStatusCard";
import { SourcesFooter } from "@/components/library/SourcesFooter";
import { useExternalIdsSeries } from "@/lib/api/queries";

export function SeriesMetadataPanel({ seriesSlug }: { seriesSlug: string }) {
  const externalIds = useExternalIdsSeries(seriesSlug);
  return (
    <section className="grid gap-4 sm:grid-cols-2">
      <MetadataSyncStatusCard seriesSlug={seriesSlug} />
      <ExternalIdsCard entityType="series" seriesSlug={seriesSlug} />
      <div className="sm:col-span-2">
        <SourcesFooter rows={externalIds.data?.rows} />
      </div>
    </section>
  );
}
