"use client";

import * as React from "react";

import { Skeleton } from "@/components/ui/skeleton";
import { useLogWidgets } from "@/lib/api/queries";
import type { ReadingStatsRange } from "@/lib/api/types";

import { LogHeader } from "./LogHeader";
import { LogWidgetGrid } from "./LogWidgetGrid";
import type { LogScope } from "./widgets/types";

/** Top-level layout for `/log`. Holds the page-level range and
 *  delegates the widget grid to `<LogWidgetGrid>`, which owns the
 *  DnD reorder + the sortable plumbing. Per-widget Configure /
 *  Remove dialogs live inside each widget's `<WidgetCard>` shell. */
export function ReadingLogPage() {
  const [range, setRange] = React.useState<ReadingStatsRange>("30d");
  const scope: LogScope = React.useMemo(() => ({ range }), [range]);
  const widgetsQuery = useLogWidgets();
  const widgets = widgetsQuery.data?.items ?? [];

  return (
    <div className="space-y-6">
      <LogHeader range={range} onRangeChange={setRange} widgets={widgets} />
      {widgetsQuery.isLoading ? (
        // Matches the LogWidgetGrid's multicolumn flow so the
        // loading state doesn't visibly reflow on data arrival.
        <div className="columns-1 gap-x-6 md:columns-2">
          <Skeleton className="mb-6 inline-block h-64 w-full break-inside-avoid" />
          <Skeleton className="mb-6 inline-block h-48 w-full break-inside-avoid" />
          <Skeleton className="mb-6 inline-block h-32 w-full break-inside-avoid" />
        </div>
      ) : widgetsQuery.data ? (
        <LogWidgetGrid widgets={widgets} scope={scope} />
      ) : (
        <p className="text-destructive text-sm">Failed to load widgets.</p>
      )}
    </div>
  );
}
