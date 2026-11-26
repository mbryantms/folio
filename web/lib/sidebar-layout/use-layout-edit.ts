"use client";

import * as React from "react";

import { useUpdateSidebarLayout } from "@/lib/api/mutations";
import { useSidebarLayout } from "@/lib/api/queries";
import type { SidebarEntryView } from "@/lib/api/types";
import { showUndoToast } from "@/lib/ui/undo-toast";

import {
  insertAbove,
  isCustomDivider,
  keyOf,
  moveBy,
  newDivider,
  rename,
  reorder,
  showAgain,
  splitLayout,
  takeOut,
  toPayload,
  type LayoutState,
} from "./edit";

function newRefId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return "00000000-0000-4000-8000-" + Math.random().toString(16).slice(2, 14);
}

/**
 * The sidebar layout plus the edits the in-place editor offers. Every
 * edit saves at once (the server replaces the whole list); the edited
 * state shows until the save settles and the query takes over again.
 */
export function useSidebarLayoutEdit() {
  const layoutQ = useSidebarLayout();
  const update = useUpdateSidebarLayout();
  const [optimistic, setOptimistic] = React.useState<LayoutState | null>(null);
  const server = React.useMemo(
    () => splitLayout(layoutQ.data?.entries ?? []),
    [layoutQ.data],
  );
  const state = optimistic ?? server;

  const commit = (
    next: LayoutState,
    undo?: { message: string; failedMessage: string },
  ) => {
    if (next === state) return;
    const before = toPayload(state);
    setOptimistic(next);
    update.mutate(
      { entries: toPayload(next) },
      {
        onSuccess: undo
          ? () =>
              showUndoToast({
                ...undo,
                onUndo: () => update.mutateAsync({ entries: before }),
              })
          : undefined,
        onSettled: () => setOptimistic(null),
      },
    );
  };

  return {
    visible: state.visible,
    hidden: state.hidden,
    isLoading: layoutQ.isLoading,
    isError: layoutQ.isError,
    reorder: (fromKey: string, toKey: string) =>
      commit(reorder(state, fromKey, toKey)),
    moveBy: (entry: SidebarEntryView, delta: number) =>
      commit(moveBy(state, keyOf(entry), delta)),
    /** Hide a row, or delete a custom header / spacer (with Undo). */
    takeOut: (entry: SidebarEntryView) =>
      commit(
        takeOut(state, keyOf(entry)),
        isCustomDivider(entry)
          ? {
              message:
                entry.kind === "header"
                  ? `Removed header "${entry.label}"`
                  : "Removed spacer",
              failedMessage: "Couldn't restore the row",
            }
          : undefined,
      ),
    showAgain: (entry: SidebarEntryView) =>
      commit(showAgain(state, keyOf(entry))),
    rename: (entry: SidebarEntryView, label: string) =>
      commit(rename(state, keyOf(entry), label)),
    /** Add a header or spacer above `entry` (at the end when `null`). */
    addDivider: (kind: "header" | "spacer", above: SidebarEntryView | null) =>
      commit(
        insertAbove(
          state,
          above ? keyOf(above) : null,
          newDivider(kind, newRefId()),
        ),
      ),
  };
}

export type SidebarLayoutEdit = ReturnType<typeof useSidebarLayoutEdit>;
