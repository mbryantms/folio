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
import { useExternalIdsIssue, useExternalIdsSeries } from "@/lib/api/queries";

type Variant = "chips" | "text";
type Props =
  | {
      scope: "series";
      seriesSlug: string;
      className?: string;
      variant?: Variant;
    }
  | {
      scope: "issue";
      seriesSlug: string;
      issueSlug: string;
      className?: string;
      variant?: Variant;
    };

export function ProviderBadgesRow(props: Props) {
  if (props.scope === "series") {
    return (
      <SeriesBadges
        seriesSlug={props.seriesSlug}
        className={props.className}
        variant={props.variant}
      />
    );
  }
  return (
    <IssueBadges
      seriesSlug={props.seriesSlug}
      issueSlug={props.issueSlug}
      className={props.className}
      variant={props.variant}
    />
  );
}

function SeriesBadges({
  seriesSlug,
  className,
  variant,
}: {
  seriesSlug: string;
  className?: string;
  variant?: Variant;
}) {
  const { data } = useExternalIdsSeries(seriesSlug);
  return (
    <ProviderBadges rows={data?.rows} className={className} variant={variant} />
  );
}

function IssueBadges({
  seriesSlug,
  issueSlug,
  className,
  variant,
}: {
  seriesSlug: string;
  issueSlug: string;
  className?: string;
  variant?: Variant;
}) {
  const { data } = useExternalIdsIssue(seriesSlug, issueSlug);
  return (
    <ProviderBadges rows={data?.rows} className={className} variant={variant} />
  );
}
