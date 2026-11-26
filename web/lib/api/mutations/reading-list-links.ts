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
  ApplyDeriveReq,
  ApplyDeriveResp,
  CreateBreakReq,
  DerivePreview,
  CreateLinkReq,
  ListViewRef,
  MarkThroughResp,
  ReadingListLinksView,
  UpdateBreakReq,
  UpdateLinkReq,
} from "../types";
import { apiMutate, useApiMutation, useApiRunMutation } from "./_core";
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

/** Prefixes of `queryKeys.cblListLinks(id)` and `queryKeys.nextUp(…)`. */
const CBL_LINKS_PREFIX = ["cbl-lists", "links"] as const;
const NEXT_UP_PREFIX = ["reader", "next-up"] as const;

function useInvalidateLinks() {
  const qc = useQueryClient();
  return () => {
    // Every list's links, not only this one's: the linked list's own page
    // shows "Part of …" from the other side of the same link. And Up
    // Next, which offers events as detours.
    qc.invalidateQueries({ queryKey: CBL_LINKS_PREFIX });
    qc.invalidateQueries({ queryKey: NEXT_UP_PREFIX });
  };
}

/** Link a list into `listId` as an event (after an entry) or as its next
 *  part. */
export function useCreateCblLink(listId: string) {
  const qc = useQueryClient();
  const invalidate = useInvalidateLinks();
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
  const qc = useQueryClient();
  const invalidate = useInvalidateLinks();
  return useApiMutation<
    ReadingListLinksView,
    { linkId: string } & UpdateLinkReq
  >(
    ({ linkId, ...body }) => ({
      path: apiPath`/me/cbl-lists/${listId}/links/${linkId}`,
      method: "PATCH",
      body,
    }),
    {
      successMessage: "Event moved",
      onSuccess: (data) => {
        // The response is the list's new layout: show it at once, so the
        // page (and a scroll to the event's new place) doesn't act on the
        // old one while the refetch is in flight.
        if (data) qc.setQueryData(queryKeys.cblListLinks(listId), data);
        invalidate();
      },
    },
  );
}

/** Change the order of the events read after one issue: one write per
 *  event whose slot changes (`reorderWrites`), one toast for the lot. */
export function useReorderCblLinks(listId: string) {
  const invalidate = useInvalidateLinks();
  return useApiRunMutation<
    null,
    { writes: { linkId: string; sort_within_anchor: number }[] }
  >(
    async ({ writes }) => {
      for (const w of writes) {
        await apiMutate<ReadingListLinksView>({
          path: apiPath`/me/cbl-lists/${listId}/links/${w.linkId}`,
          method: "PATCH",
          body: { sort_within_anchor: w.sort_within_anchor },
        });
      }
      return null;
    },
    { successMessage: "Event order changed", onSettled: invalidate },
  );
}

export function useDeleteCblLink(listId: string) {
  const invalidate = useInvalidateLinks();
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
  const invalidate = useInvalidateLinks();
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
  const invalidate = useInvalidateLinks();
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
  const invalidate = useInvalidateLinks();
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
  const invalidate = useInvalidateLinks();
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

/**
 * PR 3b: ask the server which events (CBRO with-events twin) and parts
 * this catalog list has. A POST because it reads the catalog; nothing is
 * written.
 */
export function useDeriveCblLinksPreview(listId: string) {
  return useApiMutation<DerivePreview, void>(() => ({
    path: apiPath`/me/cbl-lists/${listId}/links/derive`,
    method: "POST",
  }));
}

/** Import + link the proposals the person kept. */
export function useApplyCblDerive(listId: string) {
  const qc = useQueryClient();
  return useApiMutation<ApplyDeriveResp, ApplyDeriveReq>(
    (body) => ({
      path: apiPath`/me/cbl-lists/${listId}/links/derive/apply`,
      method: "POST",
      body,
    }),
    {
      successMessage: (resp) => {
        const n = resp?.linked ?? 0;
        const linked =
          n === 0
            ? "Nothing new linked"
            : `Linked ${n} ${n === 1 ? "list" : "lists"}`;
        const failed = resp?.failed ?? [];
        if (failed.length === 0) return linked;
        const first = failed[0]!;
        const more = failed.length > 1 ? ` (+${failed.length - 1} more)` : "";
        return `${linked}. ${first.name}: ${first.message}${more}`;
      },
      successVariant: (resp) =>
        (resp?.failed.length ?? 0) > 0 ? "info" : "success",
      onSuccess: (resp) => {
        if (resp) qc.setQueryData(queryKeys.cblListLinks(listId), resp.links);
        qc.invalidateQueries({ queryKey: queryKeys.cblListLinks(listId) });
        qc.invalidateQueries({ queryKey: queryKeys.cblLists });
        invalidateSavedViews(qc);
      },
    },
  );
}
