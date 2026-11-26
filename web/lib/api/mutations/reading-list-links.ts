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
  DetectChaptersPreview,
  ImportOrderReq,
  ImportOrderResp,
  SavedReadingListRef,
  CopiedListRef,
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
    { successMessage: "List unlinked", onSuccess: invalidate },
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
  // `nameOnly`: the chapter stays (the first one, or the one after an
  // event) and only loses its name.
  return useApiMutation<null, { breakId: string; nameOnly?: boolean }>(
    ({ breakId }) => ({
      path: apiPath`/me/cbl-lists/${listId}/breaks/${breakId}`,
      method: "DELETE",
    }),
    {
      successMessage: (_d, input) =>
        input.nameOnly ? "Chapter name removed" : "Chapter break removed",
      onSuccess: invalidate,
    },
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
        input.skip ? "Event skipped" : "Event unskipped",
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

/** How many `import-order` calls one whole-order import makes at most
 *  (each imports as many parts as fit in its time budget). */
const IMPORT_ORDER_MAX_CALLS = 40;

/**
 * Import every part of a catalog reading order and chain them. One call
 * imports as many parts as fit in the server's time budget and says how
 * many remain, so this calls again until none do (or a call makes no
 * progress), reporting through `onProgress`. Resolves to the totals.
 */
/**
 * Save a collection as a reading list of the caller's: the same issues in
 * the same order (series expanded, reading-list entries in place). The
 * collection stays as it is. The toast links to the new list.
 */
export function useSaveCollectionAsReadingList(collectionId: string) {
  const qc = useQueryClient();
  return useApiMutation<SavedReadingListRef, void>(
    () => ({
      path: apiPath`/me/collections/${collectionId}/reading-list`,
      method: "POST",
    }),
    {
      successMessage: (r) =>
        r
          ? `Saved as a reading list of ${r.total} issue${r.total === 1 ? "" : "s"}`
          : "Saved as a reading list",
      successAction: (r) =>
        r?.view_id ? { label: "Open", href: `/views/${r.view_id}` } : undefined,
      onSuccess: () => invalidateSavedViews(qc),
    },
  );
}

/**
 * A personal copy of a list the caller can see (a shared one, usually):
 * its entries, chapters and links to events and the next part. The copy
 * is theirs to arrange; it does not follow the original afterwards.
 */
export function useCopyCblList(listId: string) {
  const qc = useQueryClient();
  return useApiMutation<CopiedListRef, void>(
    () => ({
      path: apiPath`/me/cbl-lists/${listId}/copy`,
      method: "POST",
    }),
    {
      successMessage: "Copied to your lists",
      successAction: (r) =>
        r?.view_id ? { label: "Open", href: `/views/${r.view_id}` } : undefined,
      onSuccess: () => {
        invalidateSavedViews(qc);
        qc.invalidateQueries({ queryKey: CBL_LINKS_PREFIX });
      },
    },
  );
}

export function useImportCblOrder() {
  const qc = useQueryClient();
  return useApiRunMutation<
    ImportOrderResp,
    ImportOrderReq & { onProgress?: (done: number, total: number) => void }
  >(
    async ({ onProgress, ...body }) => {
      let total: ImportOrderResp | null = null;
      for (let call = 0; call < IMPORT_ORDER_MAX_CALLS; call++) {
        const resp = await apiMutate<ImportOrderResp>({
          path: "/me/cbl-lists/import-order",
          method: "POST",
          body,
        });
        if (!resp) break;
        total = total
          ? {
              ...resp,
              imported: total.imported + resp.imported,
              linked: total.linked + resp.linked,
              failed: resp.failed,
            }
          : resp;
        onProgress?.(resp.total_parts - resp.remaining, resp.total_parts);
        // Done, or stuck (a call that imported nothing won't do better).
        if (resp.remaining === 0 || resp.imported === 0) break;
      }
      return total;
    },
    {
      successMessage: (r) => {
        if (!r) return "Nothing imported";
        const have = r.total_parts - r.remaining;
        const base =
          r.remaining === 0
            ? `All ${r.total_parts} parts are in your lists, linked in order`
            : `${have} of ${r.total_parts} parts imported`;
        const first = r.failed[0];
        return first ? `${base}. ${first.name}: ${first.message}` : base;
      },
      successVariant: (r) =>
        r && r.remaining === 0 && r.failed.length === 0 ? "success" : "info",
      onSettled: () => {
        qc.invalidateQueries({ queryKey: queryKeys.cblLists });
        qc.invalidateQueries({ queryKey: CBL_LINKS_PREFIX });
        invalidateSavedViews(qc);
      },
    },
  );
}

/** Find the events a list reads inline (a preview; it writes nothing). A
 *  mutation, not a query: it downloads from the catalog and is
 *  rate-limited. */
export function useDetectCblChapters(listId: string) {
  return useApiMutation<DetectChaptersPreview, void>(() => ({
    path: apiPath`/me/cbl-lists/${listId}/chapters/detect`,
    method: "POST",
  }));
}

/** One chapter break to create for a detected event. */
export type DetectedBreak = { entry_id: string; label: string | null };

/** Create the chapter breaks the reader kept, in order. A break that is
 *  refused (a chapter already starts there) doesn't stop the rest. */
export function useApplyDetectedChapters(listId: string) {
  const invalidate = useInvalidateLinks();
  return useApiRunMutation<
    { created: number; failed: number },
    DetectedBreak[]
  >(
    async (breaks) => {
      let created = 0;
      let failed = 0;
      for (const b of breaks) {
        try {
          await apiMutate<ReadingListLinksView>({
            path: apiPath`/me/cbl-lists/${listId}/breaks`,
            method: "POST",
            body: b,
          });
          created += 1;
        } catch {
          failed += 1;
        }
      }
      return { created, failed };
    },
    {
      successMessage: (r) => {
        const n = r?.created ?? 0;
        const base =
          n === 0
            ? "No chapters added"
            : `Added ${n} ${n === 1 ? "chapter" : "chapters"}`;
        return r && r.failed > 0
          ? `${base}; ${r.failed} couldn't be added`
          : base;
      },
      successVariant: (r) => (r && r.failed > 0 ? "info" : "success"),
      onSettled: invalidate,
    },
  );
}
