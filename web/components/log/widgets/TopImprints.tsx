"use client";

import { useReadingStats } from "@/lib/api/queries";

import { WidgetCard } from "../WidgetCard";
import { RankingBody } from "./TopPublishers";
import type { LogWidgetProps, RankingConfig } from "./types";

/** Top imprints by `active_ms`. Shares the same render shell as the
 *  publishers widget — imprints come back as `TopNameEntry[]` from
 *  the same stats endpoint. */
export function TopImprints({ widget, scope }: LogWidgetProps<RankingConfig>) {
  const range = widget.config.range ?? scope.range;
  const limit = widget.config.limit ?? 5;
  const stats = useReadingStats({ type: "all" }, range);
  const rows = (stats.data?.top_imprints ?? []).slice(0, limit);
  return (
    <WidgetCard widget={widget} title="Top imprints" subtitle={`Last ${range}`}>
      <RankingBody
        loading={stats.isLoading}
        rows={rows}
        emptyHint="No imprint activity yet."
      />
    </WidgetCard>
  );
}
