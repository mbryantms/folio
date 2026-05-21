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
  const widgets = widgetsQuery.data?.widgets ?? [];

  return (
    <div className="mx-auto flex w-full max-w-7xl flex-col gap-6 px-4 py-6 lg:px-6">
      <LogHeader range={range} onRangeChange={setRange} widgets={widgets} />
      {widgetsQuery.isLoading ? (
        <div className="grid grid-cols-1 gap-6 md:grid-cols-2">
          <Skeleton className="h-64 w-full md:col-span-2" />
          <Skeleton className="h-48 w-full" />
          <Skeleton className="h-48 w-full" />
        </div>
      ) : widgetsQuery.data ? (
        <LogWidgetGrid widgets={widgets} scope={scope} />
      ) : (
        <p className="text-destructive text-sm">Failed to load widgets.</p>
      )}
    </div>
  );
}
