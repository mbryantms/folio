"use client";

import * as React from "react";

import { SeriesCard } from "@/components/library/SeriesCard";
import { useDismissRailItem } from "@/lib/api/mutations";
import type { SimilarSeriesItem } from "@/lib/api/types";
import { formatBecause, reasonLabel } from "@/lib/similar";

/**
 * Series card for the WP-7.4 "Similar series" rails: the regular
 * `<SeriesCard>` plus a one-line "Because …" caption naming the shared
 * entities that earned the match (full list in the tooltip). The cover
 * menu gains "Hide from suggestions", which records a series rail
 * dismissal — the same per-user hide the On Deck rail uses — so the
 * series drops out of every similar list until the user reads it again.
 */
export function SimilarSeriesCard({ item }: { item: SimilarSeriesItem }) {
  const dismiss = useDismissRailItem();
  const because = formatBecause(item.because);
  const full = item.because.map(reasonLabel).join(", ");
  return (
    <div className="flex flex-col">
      <SeriesCard
        series={item.series}
        size="md"
        extraActions={[
          {
            label: "Hide from suggestions",
            onSelect: () =>
              dismiss.mutate({
                target_kind: "series",
                target_id: item.series.id,
              }),
          },
        ]}
      />
      {because ? (
        <p
          className="text-muted-foreground line-clamp-2 px-2 text-xs"
          title={`Because: ${full}`}
        >
          <span className="text-foreground/80 font-medium">Because:</span>{" "}
          {because}
        </p>
      ) : null}
    </div>
  );
}
