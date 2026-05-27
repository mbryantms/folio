"use client";

/**
 * Issue-page sources footer — the CV/Metron TOS attribution surface.
 *
 * Originally a wider "Issue metadata panel" with `<ExternalIdsCard>`,
 * `<CoverGallery>`, and `<SourcesFooter>` stacked in a 2-col grid.
 * The first two moved into the page's `<Tabs>` row (so the issue
 * page has a tab-per-section model end-to-end); only the attribution
 * footer remains here because it must stay visible regardless of
 * which tab the user has open.
 *
 * Kept as a separate client component so the server-rendered
 * `page.tsx` can stay an `async function` without pulling
 * `useExternalIdsIssue` into the SSR path.
 */

import { SourcesFooter } from "@/components/library/SourcesFooter";
import { useExternalIdsIssue } from "@/lib/api/queries";

export function IssueSourcesFooter({
  seriesSlug,
  issueSlug,
}: {
  seriesSlug: string;
  issueSlug: string;
}) {
  const externalIds = useExternalIdsIssue(seriesSlug, issueSlug);
  return <SourcesFooter rows={externalIds.data?.rows} />;
}
