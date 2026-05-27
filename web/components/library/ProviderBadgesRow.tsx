"use client";

/**
 * Thin client wrapper that fetches the entity's `external_ids` and
 * renders `<ProviderBadges>`. Mounted in the series + issue page
 * headers (RSC) so attribution pills sit next to the title without
 * pulling React-Query into the SSR path.
 *
 * Silent when the entity has no provider rows.
 */

import { ProviderBadges } from "@/components/library/ProviderBadges";
import {
  useExternalIdsIssue,
  useExternalIdsSeries,
} from "@/lib/api/queries";

type Props =
  | { scope: "series"; seriesSlug: string; className?: string }
  | {
      scope: "issue";
      seriesSlug: string;
      issueSlug: string;
      className?: string;
    };

export function ProviderBadgesRow(props: Props) {
  if (props.scope === "series") {
    return <SeriesBadges seriesSlug={props.seriesSlug} className={props.className} />;
  }
  return (
    <IssueBadges
      seriesSlug={props.seriesSlug}
      issueSlug={props.issueSlug}
      className={props.className}
    />
  );
}

function SeriesBadges({
  seriesSlug,
  className,
}: {
  seriesSlug: string;
  className?: string;
}) {
  const { data } = useExternalIdsSeries(seriesSlug);
  return <ProviderBadges rows={data?.rows} className={className} />;
}

function IssueBadges({
  seriesSlug,
  issueSlug,
  className,
}: {
  seriesSlug: string;
  issueSlug: string;
  className?: string;
}) {
  const { data } = useExternalIdsIssue(seriesSlug, issueSlug);
  return <ProviderBadges rows={data?.rows} className={className} />;
}
