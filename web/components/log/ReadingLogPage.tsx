"use client";

import * as React from "react";

import { Skeleton } from "@/components/ui/skeleton";
import { useLogWidgets } from "@/lib/api/queries";
import type {
  LogWidgetKind,
  LogWidgetView,
  ReadingLogEventKind,
  ReadingStatsRange,
} from "@/lib/api/types";
import { cn } from "@/lib/utils";

import { LogHeader } from "./LogHeader";
import { WIDGET_REGISTRY } from "./widgets";
import type { LogScope } from "./widgets/types";

const ALL_KINDS: ReadingLogEventKind[] = [
  "issue_finished",
  "series_finished",
  "session_completed",
  "marker_created",
];

/** Top-level layout for `/log`. Holds the page-level scope (range +
 *  kind chips) and delegates rendering to the per-kind widgets in
 *  the registry, ordered by the server's `position` column.
 *
 *  M5 will wire the drag-and-drop reorder + add/configure dialogs on
 *  top of this; for M4 the grid is read-only and the kebab menu
 *  surfaces only Remove. */
export function ReadingLogPage() {
  const [range, setRange] = React.useState<ReadingStatsRange>("30d");
  const [kinds, setKinds] = React.useState<ReadingLogEventKind[]>(ALL_KINDS);
  const scope: LogScope = React.useMemo(
    () => ({ range, kinds }),
    [range, kinds],
  );
  const widgets = useLogWidgets();

  return (
    <div className="mx-auto flex w-full max-w-7xl flex-col gap-6 px-4 py-6 lg:px-6">
      <LogHeader
        range={range}
        onRangeChange={setRange}
        kinds={kinds}
        onKindsChange={setKinds}
      />
      {widgets.isLoading ? (
        <Grid>
          <Skeleton className="h-64 w-full md:col-span-2" />
          <Skeleton className="h-48 w-full" />
          <Skeleton className="h-48 w-full" />
        </Grid>
      ) : widgets.data ? (
        <Grid>
          {widgets.data.widgets.map((w) => (
            <WidgetSlot key={w.id} widget={w} scope={scope} />
          ))}
        </Grid>
      ) : (
        <p className="text-destructive text-sm">Failed to load widgets.</p>
      )}
    </div>
  );
}

function Grid({ children }: { children: React.ReactNode }) {
  // Two columns on md+. Widgets opt into spanning both via the
  // `size: "full"` flag the registry exposes (used to wrap each
  // slot in the right grid-column class below).
  return (
    <div className="grid grid-cols-1 gap-6 md:grid-cols-2">{children}</div>
  );
}

function WidgetSlot({
  widget,
  scope,
}: {
  widget: LogWidgetView;
  scope: LogScope;
}) {
  const def = WIDGET_REGISTRY[widget.kind as LogWidgetKind];
  if (!def) {
    // Unknown kind — the server lists every kind it accepts, so this
    // is only ever reachable on a temporary client/server drift. Render
    // a quiet placeholder rather than crashing the whole page.
    return (
      <div
        className={cn(
          "border-border/60 text-muted-foreground rounded-md border border-dashed p-4 text-sm",
        )}
      >
        Unknown widget kind: <code>{widget.kind}</code>
      </div>
    );
  }
  const { Component, size } = def;
  return (
    <div className={cn(size === "full" && "md:col-span-2")}>
      <Component
        widget={widget as LogWidgetView & { config: Record<string, unknown> }}
        scope={scope}
      />
    </div>
  );
}
