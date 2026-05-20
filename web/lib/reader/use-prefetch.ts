import { useEffect } from "react";
import type { ViewMode } from "@/lib/reader/detect";
import type { SpreadGroup } from "@/lib/reader/spreads";

const PREFETCH_AHEAD = 2;

/**
 * Prefetch upcoming page bytes into the browser HTTP cache so the
 * reader's next page flip is instant. Double-page mode walks by
 * spread group (so we don't waste requests on the back of a pair
 * we just rendered); single-page walks by page index. Webtoon
 * renders the whole stack and skips prefetch.
 */
export function useReaderPrefetch(opts: {
  issueId: string;
  totalPages: number;
  currentPage: number;
  currentGroupIdx: number;
  groups: ReadonlyArray<SpreadGroup>;
  viewMode: ViewMode;
}): void {
  const {
    issueId,
    totalPages,
    currentPage,
    currentGroupIdx,
    groups,
    viewMode,
  } = opts;
  useEffect(() => {
    if (viewMode === "webtoon") return;
    if (viewMode === "double" && groups.length > 0) {
      for (let g = 1; g <= PREFETCH_AHEAD; g += 1) {
        const grp = groups[currentGroupIdx + g];
        if (!grp) break;
        for (const p of grp) {
          const img = new Image();
          img.src = `/issues/${issueId}/pages/${p}`;
        }
      }
      return;
    }
    for (let i = 1; i <= PREFETCH_AHEAD; i += 1) {
      const next = currentPage + i;
      if (next >= totalPages) break;
      const img = new Image();
      img.src = `/issues/${issueId}/pages/${next}`;
    }
  }, [currentPage, currentGroupIdx, groups, issueId, totalPages, viewMode]);
}
