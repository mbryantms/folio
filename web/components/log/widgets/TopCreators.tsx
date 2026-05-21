"use client";

import * as React from "react";

import { Skeleton } from "@/components/ui/skeleton";
import { useReadingStats } from "@/lib/api/queries";
import { formatDurationMs } from "@/lib/activity";

import { WidgetCard } from "../WidgetCard";
import type { LogWidgetProps, TopCreatorsConfig } from "./types";

const ROLE_LABEL: Record<string, string> = {
  writer: "Writers",
  penciller: "Pencillers",
  inker: "Inkers",
  colorist: "Colorists",
  letterer: "Letterers",
  cover_artist: "Cover artists",
  editor: "Editors",
  translator: "Translators",
};

/** Top creators in the configured role, ranked by `active_ms` over
 *  the page's selected range. Backed by `/me/reading-stats.top_creators`. */
export function TopCreators({
  widget,
  scope,
}: LogWidgetProps<TopCreatorsConfig>) {
  const range = widget.config.range ?? scope.range;
  const role = widget.config.role ?? "writer";
  const limit = widget.config.limit ?? 5;
  const stats = useReadingStats({ type: "all" }, range);

  const rows = React.useMemo(() => {
    const all = stats.data?.top_creators ?? [];
    return all.filter((c) => c.role === role).slice(0, limit);
  }, [stats.data, role, limit]);

  return (
    <WidgetCard
      widget={widget}
      title={`Top ${ROLE_LABEL[role]?.toLowerCase() ?? role}`}
      subtitle={`Last ${range}`}
    >
      {stats.isLoading ? (
        <div className="space-y-2">
          <Skeleton className="h-3 w-3/4" />
          <Skeleton className="h-3 w-2/3" />
          <Skeleton className="h-3 w-1/2" />
        </div>
      ) : rows.length === 0 ? (
        <p className="text-muted-foreground text-xs">
          No {ROLE_LABEL[role]?.toLowerCase() ?? role} credits yet in this
          range.
        </p>
      ) : (
        <ol className="flex flex-col gap-1.5">
          {rows.map((c, i) => (
            <li
              key={`${role}-${c.person}`}
              className="flex items-center gap-2 text-sm"
            >
              <span className="text-muted-foreground/70 w-4 text-xs tabular-nums">
                {i + 1}
              </span>
              <span className="truncate" title={c.person}>
                {c.person}
              </span>
              <span className="text-muted-foreground/80 ml-auto text-xs tabular-nums">
                {formatDurationMs(c.active_ms)}
              </span>
            </li>
          ))}
        </ol>
      )}
    </WidgetCard>
  );
}
