/**
 * Linked reading lists (PR 2b): mutations on a list's event / next-part
 * links, the reader's own "skip this event", "mark read up to here", and
 * (PR 3a) the list's chapter breaks.
 *
 * Every write returns or changes the links view, so each one invalidates
 * `queryKeys.cblListLinks(listId)`; progress writes also refresh the
 * entries pages, the list's stats and every progress surface.
 */
import { useQueryClient } from "@tanstack/react-query";

import { apiPath } from "../path";
import { queryKeys } from "../queries";
import type {
  CreateBreakReq,
  CreateLinkReq,
  ListViewRef,
  MarkThroughResp,
  ReadingListLinksView,
  UpdateBreakReq,
  UpdateLinkReq,
} from "../types";
import { apiMutate, useApiMutation } from "./_core";
import {
  invalidateIssueCards,
  invalidateRails,
  invalidateSeriesProgress,
} from "./rails";

/** A new saved view shows up in the reader's lists and sidebar. */
function invalidateSavedViews(qc: ReturnType<typeof useQueryClient>) {
  qc.invalidateQueries({ queryKey: ["saved-views"], exact: false });
  qc.invalidateQueries({ queryKey: queryKeys.sidebarLayout });
}

function useInvalidateLinks(listId: string) {
  const qc = useQueryClient();
  return () => {
    qc.invalidateQueries({ queryKey: queryKeys.cblListLinks(listId) });
  };
}

/** Link a list into `listId` as an event (after an entry) or as its next
 *  part. */
export function useCreateCblLink(listId: string) {
  const qc = useQueryClient();
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
      onSuccess: (_d, input) => {
        invalidate();
        // Linking an event gives the linker a saved view of it.
        if (input.kind === "event") invalidateSavedViews(qc);
      },
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

/** Start a new chapter at an entry, optionally named. */
export function useCreateCblBreak(listId: string) {
  const invalidate = useInvalidateLinks(listId);
  return useApiMutation<ReadingListLinksView, CreateBreakReq>(
    (body) => ({
      path: apiPath`/me/cbl-lists/${listId}/breaks`,
      method: "POST",
      body,
    }),
    { successMessage: "Chapter started", onSuccess: invalidate },
  );
}

/** Rename a chapter break; a blank label names it after its first issue. */
export function useUpdateCblBreak(listId: string) {
  const invalidate = useInvalidateLinks(listId);
  return useApiMutation<
    ReadingListLinksView,
    { breakId: string } & UpdateBreakReq
  >(
    ({ breakId, ...body }) => ({
      path: apiPath`/me/cbl-lists/${listId}/breaks/${breakId}`,
      method: "PATCH",
      body,
    }),
    { successMessage: "Chapter renamed", onSuccess: invalidate },
  );
}

export function useDeleteCblBreak(listId: string) {
  const invalidate = useInvalidateLinks(listId);
  return useApiMutation<null, { breakId: string }>(
    ({ breakId }) => ({
      path: apiPath`/me/cbl-lists/${listId}/breaks/${breakId}`,
      method: "DELETE",
    }),
    { successMessage: "Chapter break removed", onSuccess: invalidate },
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

/** The caller's saved view of list `listId`, created when they have none
 *  (an event band opening a list the reader hasn't saved). Refreshes the
 *  parent's links view so the band picks up the id, and the reader's
 *  saved views so the list appears among theirs. */
export function useOpenCblListView(parentListId: string) {
  const qc = useQueryClient();
  return useApiMutation<ListViewRef, { listId: string }>(
    ({ listId }) => ({
      path: apiPath`/me/cbl-lists/${listId}/view`,
      method: "POST",
    }),
    {
      onSuccess: (data) => {
        qc.invalidateQueries({
          queryKey: queryKeys.cblListLinks(parentListId),
        });
        if (data?.created) invalidateSavedViews(qc);
      },
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
