"use client";

import { useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import { queryKeys } from "@/lib/api/queries";
import type { MeView, SidebarLayoutView } from "@/lib/api/types";

/**
 * Seeds the TanStack Query cache with the `me` (+ sidebar-layout) payloads
 * the server layout already fetched, so `useMe` / `useSidebarLayout` (called
 * by shell descendants) read them from cache on first mount instead of
 * immediately refetching what was just loaded server-side (audit G7 — the
 * SSR→client double fetch).
 *
 * Rendered as the first child of a layout, *before* the shell, so the seed
 * runs before any consumer subscribes — on the initial render no observer
 * exists yet, so this can't notify a mid-render component. The seed lives in
 * a `useState` initializer: it runs exactly once on mount (the React-blessed
 * run-once-during-render hook), so re-renders don't re-stamp `dataUpdatedAt`
 * and slide the staleness window forward forever.
 */
export function HydrateAuthCache({
  me,
  sidebar,
}: {
  me?: MeView;
  sidebar?: SidebarLayoutView;
}) {
  const qc = useQueryClient();
  React.useState(() => {
    if (me) qc.setQueryData(queryKeys.me, me);
    if (sidebar) qc.setQueryData(queryKeys.sidebarLayout, sidebar);
  });
  return null;
}
