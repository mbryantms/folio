/**
 * Linked reading lists (PR 2b): mutations on a list's event / next-part
 * links, the reader's own "skip this event", and "mark read up to here".
 *
 * Every write returns or changes the links view, so each one invalidates
 * `queryKeys.cblListLinks(listId)`; progress writes also refresh the
 * entries pages, the list's stats and every progress surface.
 */
import { useQueryClient } from "@tanstack/react-query";

import { apiPath } from "../path";
import { queryKeys } from "../queries";
import type {
  CreateLinkReq,
  MarkThroughResp,
  ReadingListLinksView,
  UpdateLinkReq,
} from "../types";
import { apiMutate, useApiMutation } from "./_core";
import {
  invalidateIssueCards,
  invalidateRails,
  invalidateSeriesProgress,
} from "./rails";

function useInvalidateLinks(listId: string) {
  const qc = useQueryClient();
  return () => {
    qc.invalidateQueries({ queryKey: queryKeys.cblListLinks(listId) });
  };
}

/** Link a list into `listId` as an event (after an entry) or as its next
 *  part. */
export function useCreateCblLink(listId: string) {
  const invalidate = useInvalidateLinks(listId);
  return useApiMutation<ReadingListLinksView, CreateLinkReq>(
    (body) => ({
      path: apiPath`/me/cbl-lists/${listId}/links`,
      method: "POST",
      body,
    }),
    {
      successMessage: (_d, input) =>
        input.kind === "continues" ? "Next part linked" : "Event linked",
      onSuccess: invalidate,
    },
  );
}

/** Move an event to after another entry, or reorder it at its spot. */
export function useUpdateCblLink(listId: string) {
  const invalidate = useInvalidateLinks(listId);
  return useApiMutation<
    ReadingListLinksView,
    { linkId: string } & UpdateLinkReq
  >(
    ({ linkId, ...body }) => ({
      path: apiPath`/me/cbl-lists/${listId}/links/${linkId}`,
      method: "PATCH",
      body,
    }),
    { successMessage: "Event moved", onSuccess: invalidate },
  );
}

export function useDeleteCblLink(listId: string) {
  const invalidate = useInvalidateLinks(listId);
  return useApiMutation<null, { linkId: string }>(
    ({ linkId }) => ({
      path: apiPath`/me/cbl-lists/${listId}/links/${linkId}`,
      method: "DELETE",
    }),
    { successMessage: "Link removed", onSuccess: invalidate },
  );
}

/** The reader's own skip / un-skip of an event. Symmetric toggle →
 *  `toast.message`-style neutral wording via the success message. */
export function useSkipCblLink(listId: string) {
  const invalidate = useInvalidateLinks(listId);
  return useApiMutation<null, { linkId: string; skip: boolean }>(
    ({ linkId, skip }) => ({
      path: apiPath`/me/cbl-lists/${listId}/links/${linkId}/skip`,
      method: skip ? "PUT" : "DELETE",
    }),
    {
      successMessage: (_d, input) =>
        input.skip ? "Event skipped" : "Event back in your reading",
      onSuccess: invalidate,
    },
  );
}

/** How many issues "mark read up to here" would mark, without writing. */
export function countCblReadThrough(
  listId: string,
  position: number,
): Promise<MarkThroughResp | null> {
  return apiMutate<MarkThroughResp>({
    path: apiPath`/me/cbl-lists/${listId}/progress/through`,
    method: "POST",
    body: { position, dry_run: true },
  });
}

/** Mark every unfinished issue at or before `position` read. */
export function useMarkCblReadThrough(listId: string) {
  const qc = useQueryClient();
  return useApiMutation<MarkThroughResp, { position: number }>(
    ({ position }) => ({
      path: apiPath`/me/cbl-lists/${listId}/progress/through`,
      method: "POST",
      body: { position },
    }),
    {
      successMessage: (data) =>
        data?.failed
          ? `Marked ${data.marked} read · ${data.failed} couldn't be saved`
          : `Marked ${data?.marked ?? 0} read`,
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.userProgress });
        qc.invalidateQueries({ queryKey: queryKeys.cblList(listId) });
        qc.invalidateQueries({ queryKey: queryKeys.cblListLinks(listId) });
        qc.invalidateQueries({
          queryKey: ["cbl-lists", "entries", listId],
          exact: false,
        });
        qc.invalidateQueries({
          queryKey: ["cbl-lists", "window", listId],
          exact: false,
        });
        invalidateRails(qc);
        invalidateSeriesProgress(qc);
        invalidateIssueCards(qc, { refetchActive: true });
      },
    },
  );
}
