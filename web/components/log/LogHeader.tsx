"use client";

import { Download, RotateCcw } from "lucide-react";

import { ActivityRangeSelector } from "@/components/activity/ActivityRangeSelector";
import { PageHeader } from "@/components/admin/PageHeader";
import { Button } from "@/components/ui/button";
import { useQueryClient } from "@tanstack/react-query";

import { apiMutate, useApiMutation } from "@/lib/api/mutations/_core";
import { queryKeys } from "@/lib/api/queries";
import type {
  LogWidgetListView,
  LogWidgetView,
  ReadingStatsRange,
} from "@/lib/api/types";
import { showUndoToast } from "@/lib/ui/undo-toast";

import { AddWidgetMenu } from "./AddWidgetMenu";

/** Reading-log page header — title + the range selector reused
 *  from `/settings/activity`, plus customization actions (Add
 *  widget + Reset to defaults).
 *
 *  The per-event-kind filter chips that originally lived here were
 *  removed: the chrono_feed widget now owns kind selection in its
 *  Configure dialog, which is the surface that actually consumes
 *  the filter. Other widgets either hard-code their kinds
 *  (SeriesFinishes, RecentBookmarks) or ignore the page-level
 *  setting, so the page chips were duplicating work that nothing
 *  observed. */
export function LogHeader({
  range,
  onRangeChange,
  widgets,
}: {
  range: ReadingStatsRange;
  onRangeChange: (next: ReadingStatsRange) => void;
  /** Current widget list — drives the Add-widget menu's filtering
   *  so each kind appears once (except `note`, which is multi-
   *  instance-permitted). */
  widgets: LogWidgetView[];
}) {
  const qc = useQueryClient();
  // Undo instead of confirm (folio-flair-1.0): a reset only rebuilds the
  // widget grid — reading history is untouched — and the old grid is
  // fully described by `widgets` (kind + config, in order), so Undo can
  // put it back. Silent mutation: the Undo toast is the only toast.
  const reset = useApiMutation<LogWidgetListView, void>(
    () => ({ path: "/me/log/widgets/reset", method: "POST" }),
    {
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.logWidgets });
      },
    },
  );

  function resetLayout() {
    const before = widgets.map(({ kind, config }) => ({ kind, config }));
    reset.mutate(undefined, {
      onSuccess: (seeded) =>
        showUndoToast({
          message: "Reading log reset to defaults",
          restoredMessage: "Restored your reading-log layout",
          failedMessage: "Couldn't restore your layout",
          onUndo: async () => {
            try {
              await restoreLogLayout(before, seeded?.items ?? []);
            } finally {
              qc.invalidateQueries({ queryKey: queryKeys.logWidgets });
            }
          },
        }),
    });
  }

  return (
    <>
      <PageHeader
        title="Reading log"
        description="Every issue read, series finished, session, and bookmark — in order, with everything Folio knows about each one."
        actions={
          <>
            <ActivityRangeSelector value={range} onChange={onRangeChange} />
            <AddWidgetMenu current={widgets} />
            {/* Data liberation (3.3): download the full reading history as
                CSV. A plain same-origin `<a download>` carries the session
                cookie, so no client fetch/auth plumbing is needed. */}
            <Button
              variant="ghost"

              asChild
              title="Download your full reading log as a CSV"
            >
              <a href="/api/me/reading-log/export" download>
                <Download aria-hidden="true" className="mr-1 size-3.5" />
                Export CSV
              </a>
            </Button>
            <Button
              variant="ghost"

              onClick={resetLayout}
              disabled={reset.isPending}
              title="Reset to default layout"
            >
              <RotateCcw aria-hidden="true" className="mr-1 size-3.5" />
              Reset
            </Button>
          </>
        }
      />
    </>
  );
}

/** Undo for "Reset": re-add the previous widgets in order (each POST
 *  appends, so order is preserved), then remove the freshly seeded
 *  defaults. Adding first means a failure part-way leaves extra widgets
 *  rather than an empty grid. */
async function restoreLogLayout(
  before: readonly Pick<LogWidgetView, "kind" | "config">[],
  seeded: readonly Pick<LogWidgetView, "id">[],
): Promise<void> {
  for (const w of before) {
    await apiMutate({
      path: "/me/log/widgets",
      method: "POST",
      body: { kind: w.kind, config: w.config ?? {} },
    });
  }
  for (const w of seeded) {
    await apiMutate({ path: `/me/log/widgets/${w.id}`, method: "DELETE" });
  }
}
