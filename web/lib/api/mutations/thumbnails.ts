/**
 * Thumbnail pipeline mutations.
 *
 * Extracted from `mutations/index.ts` during code-quality-cleanup M5
 * to slim the main file (~300 LOC removed). Surface kept stable —
 * `index.ts` re-exports `*` from this module, so callers continue to
 * `import { useGenerateMissingThumbnails } from "@/lib/api/mutations"`.
 */
import { useQueryClient } from "@tanstack/react-query";

import { queryKeys } from "../queries";
import type {
  DeleteAllResp,
  RegenerateResp,
  ThumbnailsSettingsView,
  UpdateThumbnailsSettingsReq,
} from "../types";
import { useApiMutation } from "./_core";

/** Common cache-invalidation pair for any thumbnail-touching mutation:
 * the per-library readiness status + the queue-depth poll the admin
 * dashboard uses. */
function invalidateThumbs(
  qc: ReturnType<typeof useQueryClient>,
  libraryId: string,
) {
  qc.invalidateQueries({ queryKey: queryKeys.thumbnailsStatus(libraryId) });
  qc.invalidateQueries({ queryKey: queryKeys.queueDepth });
}

/**
 * `PATCH /admin/libraries/{id}/thumbnails-settings`. Updates `enabled`,
 * `format`, and encoder quality. Format/quality changes do not
 * auto-regenerate; the admin runs force-recreate when ready.
 */
export function useUpdateThumbnailsSettings(libraryId: string) {
  const qc = useQueryClient();
  return useApiMutation<ThumbnailsSettingsView, UpdateThumbnailsSettingsReq>(
    (body) => ({
      path: `/admin/libraries/${libraryId}/thumbnails-settings`,
      method: "PATCH",
      body,
    }),
    {
      successMessage: "Thumbnail settings saved",
      onSuccess: (data) => {
        if (data) {
          qc.setQueryData(queryKeys.thumbnailsSettings(libraryId), data);
        } else {
          qc.invalidateQueries({
            queryKey: queryKeys.thumbnailsSettings(libraryId),
          });
        }
      },
    },
  );
}

/**
 * Enqueue thumbnail jobs only for issues currently missing or stamped at
 * an older `thumbnail_version`. Does not wipe any on-disk files. Honors
 * the per-library `enabled` flag — disabled libraries return 409.
 */
export function useGenerateMissingThumbnails(libraryId: string) {
  const qc = useQueryClient();
  return useApiMutation<RegenerateResp, void>(
    () => ({
      path: `/admin/libraries/${libraryId}/thumbnails/generate-missing`,
      method: "POST",
    }),
    {
      successMessage: (data) =>
        data && data.enqueued > 0
          ? `Enqueued ${data.enqueued} thumbnail job${
              data.enqueued === 1 ? "" : "s"
            }`
          : "Nothing to generate — all thumbnails are up to date",
      onSuccess: () => invalidateThumbs(qc, libraryId),
    },
  );
}

/**
 * Enqueue page-map strip thumbnail jobs for every active issue. This is
 * intentionally separate from cover generation: it lets admins warm the reader
 * page strip in the background without putting that cost on scans or library
 * page loads.
 */
export function useGeneratePageMapThumbnails(libraryId: string) {
  const qc = useQueryClient();
  return useApiMutation<RegenerateResp, void>(
    () => ({
      path: `/admin/libraries/${libraryId}/thumbnails/generate-page-map`,
      method: "POST",
    }),
    {
      successMessage: (data) =>
        data && data.enqueued > 0
          ? `Enqueued ${data.enqueued} page-map thumbnail job${
              data.enqueued === 1 ? "" : "s"
            }`
          : "No page-map thumbnail jobs were queued",
      onSuccess: () => invalidateThumbs(qc, libraryId),
    },
  );
}

/**
 * Wipe every thumbnail for the library and re-enqueue. The only path that
 * picks up a format change. Confirms destructively in the UI.
 */
export function useForceRecreateThumbnails(libraryId: string) {
  const qc = useQueryClient();
  return useApiMutation<RegenerateResp, void>(
    () => ({
      path: `/admin/libraries/${libraryId}/thumbnails/force-recreate`,
      method: "POST",
    }),
    {
      successMessage: (data) =>
        `Enqueued ${data?.enqueued ?? 0} thumbnail job${
          data?.enqueued === 1 ? "" : "s"
        }`,
      onSuccess: () => invalidateThumbs(qc, libraryId),
    },
  );
}

// ---------- Series-scoped thumbnail regen ----------
//
// Each pair of hooks (cover, fill, force) targets one series so the admin
// can rebuild a single book without rerunning the library-wide jobs. They
// reuse `invalidateThumbs(qc, libraryId)` so the per-library readiness
// status repolls after the queue depth ticks up.

export function useRegenerateSeriesCover(
  seriesSlug: string,
  libraryId: string,
) {
  const qc = useQueryClient();
  return useApiMutation<RegenerateResp, void>(
    () => ({
      path: `/admin/series/${seriesSlug}/thumbnails/regenerate-cover`,
      method: "POST",
    }),
    {
      successMessage: (data) =>
        `Enqueued ${data?.enqueued ?? 0} cover job${
          data?.enqueued === 1 ? "" : "s"
        }`,
      onSuccess: () => invalidateThumbs(qc, libraryId),
    },
  );
}

export function useGenerateSeriesPageMap(
  seriesSlug: string,
  libraryId: string,
) {
  const qc = useQueryClient();
  return useApiMutation<RegenerateResp, void>(
    () => ({
      path: `/admin/series/${seriesSlug}/thumbnails/generate-page-map`,
      method: "POST",
    }),
    {
      successMessage: (data) =>
        data && data.enqueued > 0
          ? `Enqueued ${data.enqueued} page-thumbnail job${
              data.enqueued === 1 ? "" : "s"
            }`
          : "No page-thumbnail jobs were queued",
      onSuccess: () => invalidateThumbs(qc, libraryId),
    },
  );
}

export function useForceRecreateSeriesPageMap(
  seriesSlug: string,
  libraryId: string,
) {
  const qc = useQueryClient();
  return useApiMutation<RegenerateResp, void>(
    () => ({
      path: `/admin/series/${seriesSlug}/thumbnails/force-recreate-page-map`,
      method: "POST",
    }),
    {
      successMessage: (data) =>
        `Enqueued ${data?.enqueued ?? 0} page-thumbnail job${
          data?.enqueued === 1 ? "" : "s"
        }`,
      onSuccess: () => invalidateThumbs(qc, libraryId),
    },
  );
}

// ---------- Issue-scoped thumbnail regen ----------

export function useRegenerateIssueCover(
  seriesSlug: string,
  issueSlug: string,
  libraryId: string,
) {
  const qc = useQueryClient();
  return useApiMutation<RegenerateResp, void>(
    () => ({
      path: `/admin/series/${seriesSlug}/issues/${issueSlug}/thumbnails/regenerate-cover`,
      method: "POST",
    }),
    {
      successMessage: () => "Cover regeneration queued",
      onSuccess: () => invalidateThumbs(qc, libraryId),
    },
  );
}

export function useGenerateIssuePageMap(
  seriesSlug: string,
  issueSlug: string,
  libraryId: string,
) {
  const qc = useQueryClient();
  return useApiMutation<RegenerateResp, void>(
    () => ({
      path: `/admin/series/${seriesSlug}/issues/${issueSlug}/thumbnails/generate-page-map`,
      method: "POST",
    }),
    {
      successMessage: (data) =>
        data && data.enqueued > 0
          ? "Page-thumbnail job queued"
          : "Page thumbnails are already up to date",
      onSuccess: () => invalidateThumbs(qc, libraryId),
    },
  );
}

export function useForceRecreateIssuePageMap(
  seriesSlug: string,
  issueSlug: string,
  libraryId: string,
) {
  const qc = useQueryClient();
  return useApiMutation<RegenerateResp, void>(
    () => ({
      path: `/admin/series/${seriesSlug}/issues/${issueSlug}/thumbnails/force-recreate-page-map`,
      method: "POST",
    }),
    {
      successMessage: () => "Page thumbnails wiped and re-queued",
      onSuccess: () => invalidateThumbs(qc, libraryId),
    },
  );
}

/**
 * `DELETE /admin/libraries/{id}/thumbnails`. Wipe every on-disk thumbnail
 * for the library and clear DB state. No re-enqueue.
 */
export function useDeleteAllThumbnails(libraryId: string) {
  const qc = useQueryClient();
  return useApiMutation<DeleteAllResp, void>(
    () => ({
      path: `/admin/libraries/${libraryId}/thumbnails`,
      method: "DELETE",
    }),
    {
      successMessage: (data) =>
        `Deleted thumbnails for ${data?.deleted ?? 0} issue${
          data?.deleted === 1 ? "" : "s"
        }`,
      onSuccess: () => invalidateThumbs(qc, libraryId),
    },
  );
}
