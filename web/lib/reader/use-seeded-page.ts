"use client";

import { useState } from "react";

import { useReaderStore } from "./store";

/**
 * The page a `Reader` shows, safe to use from its first render.
 *
 * The store is a module singleton seeded by `init` in an effect, so the
 * first render of a new issue — a fresh mount after reading another issue in
 * the same app lifetime, or an in-place end-of-issue navigation — finds the
 * previous issue's `currentPage` there (0 on a cold start). Until `init` has
 * run this returns the page the open lands on instead. Otherwise a resume
 * mounts that stale page, downloads it at high priority ahead of the real
 * one, and plays a page turn from it.
 *
 * `storeReady` also tells the two hooks that persist a page (progress write,
 * reading session) to wait for this issue's seed; without that the old page
 * was written against the new issue, and because both the writer and the
 * server keep the furthest page it stuck until the PWA reloaded and "resumed"
 * there.
 *
 * Readiness is "`init` has run since this instance last saw a new `issueId`":
 * the store's `initSeq` must have moved past the value captured when the
 * issue changed (adjust-state-during-render, so a re-render with a new issue
 * starts unready in that same render).
 */
export function useSeededPage({
  issueId,
  initialPage,
  totalPages,
}: {
  issueId: string;
  initialPage: number;
  totalPages: number;
}): { currentPage: number; storeReady: boolean } {
  const initSeq = useReaderStore((s) => s.initSeq);
  const storePage = useReaderStore((s) => s.currentPage);
  const [seen, setSeen] = useState({ issueId, initSeq });
  if (seen.issueId !== issueId) setSeen({ issueId, initSeq });
  const storeReady = seen.issueId === issueId && initSeq > seen.initSeq;
  return {
    currentPage: storeReady
      ? storePage
      : Math.max(0, Math.min(totalPages - 1, initialPage)),
    storeReady,
  };
}
