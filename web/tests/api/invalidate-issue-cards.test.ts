/**
 * `invalidateIssueCards`: the lists whose cards embed the viewer's
 * progress are marked stale by a write that names its issues (the card
 * reads the write from `queryKeys.issueProgress`), and refetched in place
 * only when the write names none (a whole-series mark).
 */
import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";

import { invalidateIssueCards } from "@/lib/api/mutations/rails";
import { queryKeys } from "@/lib/api/query-keys";

const CARD_KEYS = [
  queryKeys.seriesIssues("s1", {}),
  queryKeys.issuesCrossList({}),
  queryKeys.issueSearch({ q: "saga" }),
  queryKeys.entityItems("characters", "hazel", "issues", 24),
  queryKeys.entityInsights("characters", "hazel"),
  queryKeys.recentIssues,
  queryKeys.onThisDay(3, 15),
];
const OTHER_KEYS = [
  queryKeys.series("s1"),
  queryKeys.entityItems("characters", "hazel", "series", 24),
  queryKeys.userProgress,
];

function seeded() {
  const qc = new QueryClient();
  for (const key of [...CARD_KEYS, ...OTHER_KEYS]) qc.setQueryData(key, {});
  return qc;
}

describe("invalidateIssueCards", () => {
  it("marks every card list stale and leaves other queries alone", () => {
    const qc = seeded();
    invalidateIssueCards(qc);
    for (const key of CARD_KEYS)
      expect(qc.getQueryState(key)?.isInvalidated, String(key)).toBe(true);
    for (const key of OTHER_KEYS)
      expect(qc.getQueryState(key)?.isInvalidated, String(key)).toBe(false);
  });

  it("refetches a mounted list only when asked to", async () => {
    const qc = seeded();
    const queryFn = vi.fn(async () => ({}));
    const observer = new QueryObserver(qc, {
      queryKey: queryKeys.seriesIssues("s1", {}),
      queryFn,
      staleTime: Infinity,
    });
    const unsubscribe = observer.subscribe(() => {});
    invalidateIssueCards(qc);
    await Promise.resolve();
    expect(queryFn).not.toHaveBeenCalled();
    invalidateIssueCards(qc, { refetchActive: true });
    await vi.waitFor(() => expect(queryFn).toHaveBeenCalledTimes(1));
    unsubscribe();
  });
});
