/**
 * The caller's per-issue rating and closing note. Sharded out of the
 * mutations barrel (re-exported there) so the reader, which writes both
 * from its end-of-issue Wrap up, imports these two hooks without pulling
 * the whole barrel into its first-load JS (`scripts/check-bundle-size.mjs`).
 */
import { useQueryClient } from "@tanstack/react-query";

import type {
  IssueNoteView,
  RatingView,
  SetIssueNoteReq,
  SetRatingReq,
} from "../types";
import { apiPath } from "../path";
import { useApiMutation } from "./_core";

/**
 * `PUT /series/{slug}/issues/{slug}/rating` — same shape as the series
 * rating endpoint but scoped to a single issue.
 *
 * No toast on success — the star control fills in inline; a per-click
 * toast would be noisy. Mirrors `useSetSeriesRating`.
 */
export function useSetIssueRating(seriesSlug: string, issueSlug: string) {
  return useApiMutation<RatingView, SetRatingReq>(
    (body) => ({
      path: apiPath`/series/${seriesSlug}/issues/${issueSlug}/rating`,
      method: "PUT",
      body,
    }),
    {},
  );
}

/**
 * `PUT /series/{series_slug}/issues/{issue_slug}/note` — the caller's
 * issue-level closing note (not a page note). `body: null` or blank
 * clears it. Written from the reader's end-of-issue Wrap up pane and
 * the issue page; the response echoes the stored body.
 */
export function useSetIssueNote(
  seriesSlug: string,
  issueSlug: string,
  opts?: { silent?: boolean },
) {
  const qc = useQueryClient();
  return useApiMutation<IssueNoteView, SetIssueNoteReq>(
    (body) => ({
      path: apiPath`/series/${seriesSlug}/issues/${issueSlug}/note`,
      method: "PUT",
      body,
    }),
    {
      // `silent`: the caller raises its own (Undo) toast.
      ...(opts?.silent
        ? {}
        : {
            successMessage: (view: IssueNoteView | null) =>
              view?.body ? "Closing note saved" : "Closing note cleared",
          }),
      // The /bookmarks feed, its counts and series list carry closing
      // notes (`["markers", "feed…"]` keys).
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: ["markers", "feed"] });
        qc.invalidateQueries({ queryKey: ["markers", "feed-counts"] });
        qc.invalidateQueries({ queryKey: ["markers", "feed-series"] });
      },
    },
  );
}
