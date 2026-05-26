"use client";

/**
 * Issue-page metadata panel (metadata-providers-1.0 M5.2).
 *
 * Mirrors `<SeriesMetadataPanel>` for the issue page. Mounts the
 * external-ids card + cover gallery + sources footer. The dialog
 * itself opens from the IssueSettingsMenu's "Fetch metadata…" item
 * (parent owns dialog state so the dropdown auto-closes on select).
 *
 * Issues don't carry the per-entity "Auto-sync pause" toggle —
 * `metadata_sync_paused` lives on the series row, not the issue —
 * so the SyncStatusCard isn't mounted here.
 */

import { CoverGallery } from "@/components/library/CoverGallery";
import { ExternalIdsCard } from "@/components/library/ExternalIdsCard";
import { SourcesFooter } from "@/components/library/SourcesFooter";
import { useExternalIdsIssue } from "@/lib/api/queries";

export function IssueMetadataPanel({
  seriesSlug,
  issueSlug,
  issueId,
}: {
  seriesSlug: string;
  issueSlug: string;
  issueId: string;
}) {
  const externalIds = useExternalIdsIssue(seriesSlug, issueSlug);
  return (
    <section className="grid gap-4 sm:grid-cols-2">
      <ExternalIdsCard
        entityType="issue"
        seriesSlug={seriesSlug}
        issueSlug={issueSlug}
      />
      <CoverGallery issueId={issueId} />
      <div className="sm:col-span-2">
        <SourcesFooter rows={externalIds.data?.rows} />
      </div>
    </section>
  );
}
