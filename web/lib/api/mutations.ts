/**
 * Typed TanStack mutation hooks. CSRF + toast wiring is centralised here so
 * later milestones (M2 dismiss/restore, M3 user updates, M4 preferences,
 * M5 password issuance) only need to declare the endpoint shape.
 */
import {
  useMutation,
  useQueryClient,
  type UseMutationOptions,
} from "@tanstack/react-query";
import { useRouter } from "next/navigation";
import { toast } from "sonner";

import { apiFetch } from "./auth-refresh";
import { queryKeys } from "./queries";
import type {
  AccountReq,
  AdminUserDetailView,
  AdminUserView,
  AddEntryReq,
  CblEntryView,
  CblListView,
  CollectionEntryView,
  CreateCblListReq,
  CreateCollectionReq,
  CreateLibraryReq,
  CreateMarkerReq,
  CreateSavedViewReq,
  MarkerView,
  DeleteAllResp,
  DeleteLibraryResp,
  ImportSummary,
  IssueDetailView,
  LibraryAccessReq,
  CreateRailDismissalReq,
  LibraryView,
  ManualMatchReq,
  MeView,
  PreferencesReq,
  QueueClearReq,
  QueueClearResp,
  ProgressView,
  RatingView,
  RegenerateResp,
  SavedViewListView,
  SavedViewView,
  ScanMode,
  ScanResp,
  SetRatingReq,
  ThumbnailsSettingsView,
  AppPasswordCreatedView,
  CreateAppPasswordReq,
  ReorderEntriesReq,
  RevokeAllSessionsResp,
  UpdateCblListReq,
  UpdateCollectionReq,
  UpdateIssueReq,
  UpdateLibraryReq,
  UpdateMarkerReq,
  UpdateSavedViewReq,
  UpdateSeriesReq,
  UpdateThumbnailsSettingsReq,
  UpdateUserReq,
  UpsertProgressReq,
  UpsertSeriesProgressReq,
  UpsertSeriesProgressResp,
} from "./types";

function getCsrfToken(): string | null {
  if (typeof document === "undefined") return null;
  const m = document.cookie.match(/(?:^|;\s*)(?:__Host-)?comic_csrf=([^;]+)/);
  return m ? decodeURIComponent(m[1]!) : null;
}

export type ApiMutationInput = {
  path: string;
  method: "POST" | "PATCH" | "PUT" | "DELETE";
  body?: unknown;
};

export async function apiMutate<T>({
  path,
  method,
  body,
}: ApiMutationInput): Promise<T | null> {
  const csrf = getCsrfToken();
  const res = await apiFetch(path, {
    method,
    headers: {
      "Content-Type": "application/json",
      Accept: "application/json",
      ...(csrf ? { "X-CSRF-Token": csrf } : {}),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) {
    let detail = "";
    try {
      detail = (await res.json()).error?.message ?? `${res.status}`;
    } catch {
      detail = `${res.status}`;
    }
    throw new Error(detail);
  }
  if (res.status === 204) return null;
  const text = await res.text();
  return text ? (JSON.parse(text) as T) : null;
}

export function useApiMutation<TData, TInput>(
  build: (input: TInput) => ApiMutationInput,
  options?: Omit<
    UseMutationOptions<TData | null, Error, TInput>,
    "mutationFn"
  > & {
    successMessage?: string | ((data: TData | null, input: TInput) => string);
  },
) {
  const { successMessage, onSuccess, onError, ...rest } = options ?? {};
  return useMutation<TData | null, Error, TInput>({
    mutationFn: (input) => apiMutate<TData>(build(input)),
    onSuccess: (data, input, ctx) => {
      if (successMessage) {
        const msg =
          typeof successMessage === "function"
            ? successMessage(data, input)
            : successMessage;
        toast.success(msg);
      }
      onSuccess?.(data, input, ctx);
    },
    onError: (err, input, ctx) => {
      toast.error(err.message);
      onError?.(err, input, ctx);
    },
    ...rest,
  });
}

// ---------- Library Scanner v1 mutations ----------

export function useCreateLibrary() {
  const qc = useQueryClient();
  return useApiMutation<LibraryView, CreateLibraryReq>(
    (body) => ({ path: "/libraries", method: "POST", body }),
    {
      successMessage: "Library created",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.libraries });
      },
    },
  );
}

export function useUpdateLibrary(id: string) {
  const qc = useQueryClient();
  return useApiMutation<LibraryView, UpdateLibraryReq>(
    (body) => ({ path: `/libraries/${id}`, method: "PATCH", body }),
    {
      successMessage: "Settings saved",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.library(id) });
        qc.invalidateQueries({ queryKey: queryKeys.libraries });
      },
    },
  );
}

/**
 * Hard-delete a library and everything that depends on it: series, issues
 * (active and removed), scan runs, health issues, on-disk thumbnails, and
 * Redis coalescing keys. The audit log row survives. Caller is expected to
 * route the user away from any per-library page after success.
 */
export function useDeleteLibrary(id: string) {
  const qc = useQueryClient();
  return useApiMutation<DeleteLibraryResp, void>(
    () => ({ path: `/libraries/${id}`, method: "DELETE" }),
    {
      successMessage: (data) =>
        `Deleted library — purged ${data?.deleted_issues ?? 0} issue${
          data?.deleted_issues === 1 ? "" : "s"
        }`,
      onSuccess: () => {
        // Drop every cache entry keyed by this library so a stale read
        // can't render a 404 placeholder.
        qc.removeQueries({ queryKey: queryKeys.library(id) });
        qc.removeQueries({ queryKey: queryKeys.health(id) });
        qc.removeQueries({ queryKey: queryKeys.scanRunsAll(id) });
        qc.removeQueries({ queryKey: queryKeys.removed(id) });
        qc.removeQueries({ queryKey: queryKeys.thumbnailsStatus(id) });
        qc.removeQueries({ queryKey: queryKeys.thumbnailsSettings(id) });
        qc.invalidateQueries({ queryKey: queryKeys.libraries });
      },
    },
  );
}

/**
 * Trigger a library scan. `mode` is preferred; `force` remains supported as
 * the legacy content-verify alias.
 */
export function useTriggerScan(libraryId: string) {
  const qc = useQueryClient();
  return useApiMutation<ScanResp, { force?: boolean; mode?: ScanMode } | void>(
    (input) => {
      const params = new URLSearchParams();
      if (input?.mode) params.set("mode", input.mode);
      if (input?.force) params.set("force", "true");
      const qs = params.toString();
      return {
        path: `/libraries/${libraryId}/scan${qs ? `?${qs}` : ""}`,
        method: "POST",
      };
    },
    {
      successMessage: (data, input) => {
        if (data?.coalesced) {
          return "Scan library already running — joined existing run";
        }
        if (data?.reason) return data.reason;
        const mode =
          input?.mode ?? (input?.force ? "content_verify" : "normal");
        return mode === "content_verify"
          ? "Content verification queued"
          : mode === "metadata_refresh"
            ? "Metadata refresh queued"
            : "Scan library queued";
      },
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.scanRunsAll(libraryId) });
        qc.invalidateQueries({ queryKey: queryKeys.scanPreview(libraryId) });
      },
    },
  );
}

export function useTriggerSeriesScan(seriesId: string, libraryId?: string) {
  const qc = useQueryClient();
  // Defaults to force=true; "Scan series" is an explicit user action and
  // skipping unchanged files is rarely what curators want when they click
  // the menu item. The endpoint accepts ?force=false for cron-style
  // callers that explicitly want the cheap path.
  return useApiMutation<ScanResp, void>(
    () => ({ path: `/series/${seriesId}/scan?force=true`, method: "POST" }),
    {
      successMessage: "Scan series queued",
      onSuccess: () => {
        if (libraryId) {
          qc.invalidateQueries({ queryKey: queryKeys.scanRunsAll(libraryId) });
        }
        qc.invalidateQueries({ queryKey: queryKeys.series(seriesId) });
      },
    },
  );
}

export function useDismissHealthIssue(libraryId: string) {
  const qc = useQueryClient();
  return useApiMutation<null, { issueId: string }>(
    ({ issueId }) => ({
      path: `/libraries/${libraryId}/health-issues/${issueId}/dismiss`,
      method: "POST",
    }),
    {
      successMessage: "Issue dismissed",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.health(libraryId) });
      },
    },
  );
}

export function useRestoreIssue(libraryId: string) {
  const qc = useQueryClient();
  return useApiMutation<null, { issueId: string }>(
    ({ issueId }) => ({ path: `/issues/${issueId}/restore`, method: "POST" }),
    {
      successMessage: "Issue restored",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.removed(libraryId) });
      },
    },
  );
}

/**
 * `POST /admin/queue/clear`. Clears pending Redis-backed background jobs by
 * queue scope. It cannot abort a job that a worker is already executing.
 */
export function useClearQueue() {
  const qc = useQueryClient();
  return useApiMutation<QueueClearResp, QueueClearReq>(
    (body) => ({ path: "/admin/queue/clear", method: "POST", body }),
    {
      successMessage: (data, input) => {
        const before = data?.before.total ?? 0;
        const after = data?.after.total ?? 0;
        const stopped = Math.max(0, before - after);
        const scope =
          input.target === "thumbnails"
            ? "thumbnail"
            : input.target === "scans"
              ? "scan"
              : "background";
        return stopped > 0
          ? `Cleared ${stopped} ${scope} job${stopped === 1 ? "" : "s"}`
          : `No pending ${scope} jobs to clear`;
      },
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.queueDepth });
        qc.invalidateQueries({ queryKey: ["admin", "libraries"] });
      },
    },
  );
}

export function useConfirmIssueRemoval(libraryId: string) {
  const qc = useQueryClient();
  return useApiMutation<null, { issueId: string }>(
    ({ issueId }) => ({
      path: `/issues/${issueId}/confirm-removal`,
      method: "POST",
    }),
    {
      successMessage: "Removal confirmed",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.removed(libraryId) });
      },
    },
  );
}

export function useUpdateSeries(seriesId: string) {
  const qc = useQueryClient();
  return useApiMutation<null, UpdateSeriesReq>(
    (body) => ({ path: `/series/${seriesId}`, method: "PATCH", body }),
    {
      successMessage: "Series updated",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.series(seriesId) });
      },
    },
  );
}

/**
 * `PUT /series/{slug}/rating` — set the calling user's rating for the
 * series. Pass `{ rating: null }` to clear. Half-star precision
 * (0/0.5/1/.../5) is enforced server-side.
 */
export function useSetSeriesRating(seriesSlug: string) {
  return useApiMutation<RatingView, SetRatingReq>(
    (body) => ({
      path: `/series/${encodeURIComponent(seriesSlug)}/rating`,
      method: "PUT",
      body,
    }),
    {
      // No toast on success — the rating control gives instant visual
      // feedback and a toast on every click would be noisy.
    },
  );
}

/**
 * `PUT /series/{slug}/issues/{slug}/rating` — same shape as the series
 * rating endpoint but scoped to a single issue.
 */
export function useSetIssueRating(seriesSlug: string, issueSlug: string) {
  return useApiMutation<RatingView, SetRatingReq>(
    (body) => ({
      path: `/series/${encodeURIComponent(seriesSlug)}/issues/${encodeURIComponent(issueSlug)}/rating`,
      method: "PUT",
      body,
    }),
    {},
  );
}

/**
 * `PATCH /series/{series_slug}/issues/{issue_slug}`. Used by the issue page
 * Edit drawer to override ComicInfo-derived fields. Server records the
 * touched fields in `user_edited` so the scanner skips them on rescans.
 */
export function useUpdateIssue(seriesSlug: string, issueSlug: string) {
  return useApiMutation<IssueDetailView, UpdateIssueReq>(
    (body) => ({
      path: `/series/${encodeURIComponent(seriesSlug)}/issues/${encodeURIComponent(issueSlug)}`,
      method: "PATCH",
      body,
    }),
    { successMessage: "Issue updated" },
  );
}

/**
 * `POST /series/{series_slug}/issues/{issue_slug}/scan` — narrow per-folder
 * rescan rooted at the issue's series. The user-facing affordance is "Scan
 * issue" (consistent with "Scan library" / "Scan series").
 */
export function useScanIssue(seriesSlug: string, issueSlug: string) {
  // Defaults to force=true on the server side, but make it explicit here so
  // a future server-default change doesn't silently regress this UX. The
  // user clicked "Scan issue" — they expect a fresh ingest.
  return useApiMutation<ScanResp, void>(
    () => ({
      path: `/series/${encodeURIComponent(seriesSlug)}/issues/${encodeURIComponent(issueSlug)}/scan?force=true`,
      method: "POST",
    }),
    { successMessage: "Scan issue queued" },
  );
}

// ---------- Reading progress ----------

/**
 * `POST /progress` — write or update the calling user's progress record for
 * a single issue. Used by the issue page settings menu's "Mark as
 * read/unread" actions.
 */
export function useUpsertIssueProgress() {
  return useApiMutation<ProgressView, UpsertProgressReq>((body) => ({
    path: "/progress",
    method: "POST",
    body,
  }));
}

/**
 * `POST /series/{id}/progress` — bulk mark-all-read or mark-all-unread for
 * a series. Server iterates the user's accessible issues so the UI doesn't
 * have to fan out per-issue requests for long series.
 */
export function useUpsertSeriesProgress(seriesId: string) {
  return useApiMutation<UpsertSeriesProgressResp, UpsertSeriesProgressReq>(
    (body) => ({ path: `/series/${seriesId}/progress`, method: "POST", body }),
    {
      successMessage: (data, input) => {
        if (!data) return "Progress updated";
        if (data.updated === 0) {
          return input.finished
            ? "Already marked as read"
            : "Already marked as unread";
        }
        const verb = input.finished ? "read" : "unread";
        return `Marked ${data.updated} issue${data.updated === 1 ? "" : "s"} as ${verb}`;
      },
    },
  );
}

// ---------- Saved-view per-user icon ----------

/** `POST /me/saved-views/{id}/icon` — pick (or reset) the icon shown
 *  on this rail's home header + sidebar entry. `icon = null` resets to
 *  the kind-based default. */
export function useSetSavedViewIcon() {
  const qc = useQueryClient();
  return useApiMutation<null, { id: string; icon: string | null }>(
    (vars) => ({
      path: `/me/saved-views/${encodeURIComponent(vars.id)}/icon`,
      method: "POST",
      body: { icon: vars.icon },
    }),
    {
      onSuccess: () => {
        // Invalidate every saved-view query — both the home rail and
        // the sidebar consume the same `is_pinned` list.
        qc.invalidateQueries({ queryKey: ["saved-views"], exact: false });
      },
    },
  );
}

// ---------- Home rails (Continue Reading / On Deck) ----------

/** Invalidate everything the home rails depend on — progress delta, both
 *  rail queries, and the saved-views listing (system rails appear there). */
function invalidateRails(qc: ReturnType<typeof useQueryClient>) {
  qc.invalidateQueries({ queryKey: queryKeys.continueReading });
  qc.invalidateQueries({ queryKey: queryKeys.onDeck });
  qc.invalidateQueries({ queryKey: ["saved-views"], exact: false });
}

/** `POST /me/rail-dismissals` — hide an issue / series / CBL from the home
 *  rails. Auto-restores when the underlying target sees new progress past
 *  `dismissed_at`. */
export function useDismissRailItem() {
  const qc = useQueryClient();
  return useApiMutation<null, CreateRailDismissalReq>(
    (body) => ({ path: "/me/rail-dismissals", method: "POST", body }),
    {
      successMessage: "Hidden from rail",
      onSuccess: () => invalidateRails(qc),
    },
  );
}

/** `DELETE /me/rail-dismissals/{kind}/{target_id}` — explicit restore from
 *  a settings UI or undo toast. */
export function useRestoreRailItem() {
  const qc = useQueryClient();
  return useApiMutation<null, { target_kind: string; target_id: string }>(
    (vars) => ({
      path: `/me/rail-dismissals/${encodeURIComponent(vars.target_kind)}/${encodeURIComponent(vars.target_id)}`,
      method: "DELETE",
    }),
    {
      successMessage: "Restored to rail",
      onSuccess: () => invalidateRails(qc),
    },
  );
}

// ---------- Admin user management (M3) ----------

function invalidateUser(qc: ReturnType<typeof useQueryClient>, id: string) {
  qc.invalidateQueries({ queryKey: queryKeys.user(id) });
  qc.invalidateQueries({ queryKey: ["admin", "users"], exact: false });
  qc.invalidateQueries({ queryKey: ["admin", "audit"], exact: false });
}

export function useUpdateUser(id: string) {
  const qc = useQueryClient();
  return useApiMutation<AdminUserDetailView, UpdateUserReq>(
    (body) => ({ path: `/admin/users/${id}`, method: "PATCH", body }),
    {
      successMessage: "User updated",
      onSuccess: () => invalidateUser(qc, id),
    },
  );
}

export function useDisableUser(id: string) {
  const qc = useQueryClient();
  return useApiMutation<AdminUserView, void>(
    () => ({ path: `/admin/users/${id}/disable`, method: "POST" }),
    {
      successMessage: "User disabled",
      onSuccess: () => invalidateUser(qc, id),
    },
  );
}

export function useEnableUser(id: string) {
  const qc = useQueryClient();
  return useApiMutation<AdminUserView, void>(
    () => ({ path: `/admin/users/${id}/enable`, method: "POST" }),
    {
      successMessage: "User enabled",
      onSuccess: () => invalidateUser(qc, id),
    },
  );
}

export function useUpdateLibraryAccess(id: string) {
  const qc = useQueryClient();
  return useApiMutation<AdminUserDetailView, LibraryAccessReq>(
    (body) => ({
      path: `/admin/users/${id}/library-access`,
      method: "POST",
      body,
    }),
    {
      successMessage: "Library access updated",
      onSuccess: () => invalidateUser(qc, id),
    },
  );
}

// ---------- Per-user settings (M4) ----------

/**
 * `PATCH /me/preferences`. Updates the calling user's reader/theme/keybind
 * preferences and returns the refreshed `MeView`. The cache is updated in
 * place so the new prefs are immediately visible to every consumer of
 * `useMe()` without an extra round-trip.
 */
export function useUpdatePreferences(opts?: { silent?: boolean }) {
  const qc = useQueryClient();
  return useApiMutation<MeView, PreferencesReq>(
    (body) => ({ path: "/me/preferences", method: "PATCH", body }),
    {
      ...(opts?.silent ? {} : { successMessage: "Preferences saved" }),
      onSuccess: (data) => {
        if (data) qc.setQueryData(queryKeys.me, data);
      },
    },
  );
}

/**
 * `POST /me/reading-sessions/clear`. Destructive — deletes ALL of the
 * caller's reading_sessions rows. Audited server-side as
 * `me.activity.history.clear`. The success toast confirms how many rows
 * were removed.
 */
export function useClearReadingHistory() {
  const qc = useQueryClient();
  return useApiMutation<{ deleted: number }, void>(
    () => ({ path: "/me/reading-sessions/clear", method: "POST", body: {} }),
    {
      successMessage: "Reading history cleared",
      onSuccess: () => {
        // Invalidate every stats / sessions query so the UI redraws empty.
        qc.invalidateQueries({ queryKey: ["reading"] });
      },
    },
  );
}

/**
 * `PATCH /me/account`. Used by /settings/account for display name, email,
 * and password change. Bumps `token_version` server-side when the password
 * changes — clients should re-fetch /auth/me after a successful call.
 */
export function useUpdateAccount() {
  const qc = useQueryClient();
  return useApiMutation<MeView, AccountReq>(
    (body) => ({ path: "/me/account", method: "PATCH", body }),
    {
      successMessage: "Account updated",
      onSuccess: (data) => {
        if (data) qc.setQueryData(queryKeys.me, data);
      },
    },
  );
}

// ---------- Sessions (M5) ----------

export function useRevokeSession() {
  const qc = useQueryClient();
  return useApiMutation<null, string>(
    (id) => ({ path: `/me/sessions/${id}`, method: "DELETE" }),
    {
      successMessage: "Session signed out",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.sessions });
      },
    },
  );
}

/** Revokes every session for the calling user and bumps `token_version`,
 *  so the access token in the calling browser stops working immediately. */
export function useRevokeAllSessions() {
  const qc = useQueryClient();
  return useApiMutation<RevokeAllSessionsResp, void>(
    () => ({ path: "/me/sessions/revoke-all", method: "POST" }),
    {
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.sessions });
        qc.invalidateQueries({ queryKey: queryKeys.me });
      },
    },
  );
}

// ---------- App passwords (M7) ----------

export function useCreateAppPassword() {
  const qc = useQueryClient();
  return useApiMutation<AppPasswordCreatedView, CreateAppPasswordReq>(
    (body) => ({ path: "/me/app-passwords", method: "POST", body }),
    {
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.appPasswords });
      },
    },
  );
}

export function useRevokeAppPassword() {
  const qc = useQueryClient();
  return useApiMutation<null, string>(
    (id) => ({ path: `/me/app-passwords/${id}`, method: "DELETE" }),
    {
      successMessage: "App password revoked",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.appPasswords });
      },
    },
  );
}

// ---------- Thumbnail pipeline ----------

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

// ---------- CBL lists (saved-views M6) ----------

/** POST /me/cbl-lists — create from JSON body (URL or catalog source). */
export function useCreateCblList() {
  const qc = useQueryClient();
  return useApiMutation<CblListView, CreateCblListReq>(
    (body) => ({ path: "/me/cbl-lists", method: "POST", body }),
    {
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.cblLists });
      },
    },
  );
}

/** Multipart upload — `apiMutate` is JSON-only, so use a bespoke helper
 *  that wires `FormData` + the CSRF header by hand. */
export async function uploadCblFile(
  file: File,
  opts?: { name?: string; description?: string },
): Promise<CblListView> {
  const csrf = getCsrfToken();
  const fd = new FormData();
  fd.append("file", file);
  if (opts?.name) fd.append("name", opts.name);
  if (opts?.description) fd.append("description", opts.description);
  const res = await apiFetch("/me/cbl-lists/upload", {
    method: "POST",
    headers: csrf ? { "X-CSRF-Token": csrf } : undefined,
    body: fd,
  });
  if (!res.ok) {
    let detail = `${res.status}`;
    try {
      const body = await res.json();
      detail = body?.error?.message ?? detail;
    } catch {
      /* ignore */
    }
    throw new Error(detail);
  }
  return (await res.json()) as CblListView;
}

export function useUpdateCblList(id: string) {
  const qc = useQueryClient();
  return useApiMutation<CblListView, UpdateCblListReq>(
    (body) => ({ path: `/me/cbl-lists/${id}`, method: "PATCH", body }),
    {
      successMessage: "Saved",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.cblList(id) });
        qc.invalidateQueries({ queryKey: queryKeys.cblLists });
      },
    },
  );
}

export function useDeleteCblList(id: string) {
  const qc = useQueryClient();
  return useApiMutation<unknown, void>(
    () => ({ path: `/me/cbl-lists/${id}`, method: "DELETE" }),
    {
      successMessage: "List deleted",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.cblLists });
        qc.invalidateQueries({ queryKey: queryKeys.cblList(id) });
      },
    },
  );
}

export function useRefreshCblList(id: string) {
  const qc = useQueryClient();
  return useApiMutation<ImportSummary, { force?: boolean }>(
    (input) => ({
      path: `/me/cbl-lists/${id}/refresh${input?.force ? "?force=true" : ""}`,
      method: "POST",
    }),
    {
      successMessage: (data) => {
        if (!data) return "Refreshed";
        // The server still re-runs the matcher when upstream is
        // unchanged (304 / same blob SHA), so `rematched > 0` can
        // happen on an "up to date" response — newly-scanned issues
        // resolved previously-missing entries. Surface both facts.
        const parts: string[] = [];
        if (data.added) parts.push(`+${data.added} added`);
        if (data.removed) parts.push(`-${data.removed} removed`);
        if (data.reordered) parts.push(`${data.reordered} reordered`);
        if (data.rematched) parts.push(`${data.rematched} newly matched`);
        if (!data.upstream_changed) {
          return parts.length
            ? `Up to date · ${parts.join(", ")}`
            : "Already up to date";
        }
        return parts.length ? parts.join(", ") : "Refreshed";
      },
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.cblList(id) });
        qc.invalidateQueries({ queryKey: queryKeys.cblRefreshLog(id) });
      },
    },
  );
}

/** Manual issue match for a CBL entry. `entryId` is the cbl_entries row id. */
export function useManualMatchEntry(listId: string) {
  const qc = useQueryClient();
  return useApiMutation<CblEntryView, { entryId: string; req: ManualMatchReq }>(
    ({ entryId, req }) => ({
      path: `/me/cbl-lists/${listId}/entries/${entryId}/match`,
      method: "POST",
      body: req,
    }),
    {
      successMessage: "Match saved",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.cblList(listId) });
      },
    },
  );
}

export function useClearMatchEntry(listId: string) {
  const qc = useQueryClient();
  return useApiMutation<CblEntryView, string>(
    (entryId) => ({
      path: `/me/cbl-lists/${listId}/entries/${entryId}/clear-match`,
      method: "POST",
    }),
    {
      successMessage: "Match cleared",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.cblList(listId) });
      },
    },
  );
}

// ---------- Saved views (M6 needs create + delete + pin from import flow) ----------

export function useCreateSavedView() {
  const qc = useQueryClient();
  return useApiMutation<SavedViewView, CreateSavedViewReq>(
    (body) => ({ path: "/me/saved-views", method: "POST", body }),
    {
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: ["saved-views"] });
      },
    },
  );
}

export function useUpdateSavedView(id: string) {
  const qc = useQueryClient();
  return useApiMutation<SavedViewView, UpdateSavedViewReq>(
    (body) => ({ path: `/me/saved-views/${id}`, method: "PATCH", body }),
    {
      successMessage: "Saved",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: ["saved-views"] });
      },
    },
  );
}

export function useDeleteSavedView(id: string) {
  const qc = useQueryClient();
  return useApiMutation<unknown, void>(
    () => ({ path: `/me/saved-views/${id}`, method: "DELETE" }),
    {
      successMessage: "View deleted",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: ["saved-views"] });
      },
    },
  );
}

export function usePinSavedView() {
  const qc = useQueryClient();
  const router = useRouter();
  return useApiMutation<unknown, { id: string; pinned: boolean }>(
    ({ id, pinned }) => ({
      path: `/me/saved-views/${id}/${pinned ? "pin" : "unpin"}`,
      method: "POST",
    }),
    {
      // Surface a toast so the user gets explicit confirmation that the
      // action took effect. The optimistic UI flips the button label
      // immediately, but on dense pages the change is easy to miss
      // without the toast.
      successMessage: (_data, input) =>
        input.pinned ? "Pinned to home" : "Unpinned from home",
      // Optimistic: flip the `pinned` flag in every cached saved-view
      // list so the rail-menu / manager / detail-page button labels
      // update on click instead of waiting for the refetch round-trip.
      onMutate: async ({ id, pinned }) => {
        await qc.cancelQueries({ queryKey: ["saved-views"] });
        const snapshot = qc.getQueriesData<SavedViewListView>({
          queryKey: ["saved-views", "list"],
        });
        for (const [key, data] of snapshot) {
          if (!data) continue;
          qc.setQueryData<SavedViewListView>(key, {
            ...data,
            items: data.items.map((v) =>
              v.id === id
                ? {
                    ...v,
                    pinned,
                    pinned_position: pinned ? v.pinned_position : null,
                  }
                : v,
            ),
          });
        }
        return { snapshot };
      },
      onError: (_err, _vars, ctx) => {
        const snap = (
          ctx as
            | {
                snapshot?: ReadonlyArray<
                  readonly [readonly unknown[], SavedViewListView | undefined]
                >;
              }
            | undefined
        )?.snapshot;
        if (snap) {
          for (const [key, data] of snap) {
            qc.setQueryData(key, data);
          }
        }
      },
      onSettled: () => {
        qc.invalidateQueries({ queryKey: ["saved-views"] });
        // The saved-view detail page (`/views/[id]`) and the home page
        // both fetch their view data SERVER-SIDE — the page component
        // receives `view` as a prop, not from a TanStack query — so
        // invalidating the cache alone won't refresh the rendered
        // button label. `router.refresh()` re-runs the server
        // component and the new `pinned` value flows back into the
        // ViewHeader, so the button correctly switches between
        // "Pin" ↔ "Unpin" after each toggle.
        router.refresh();
      },
    },
  );
}

/** POST /me/saved-views/{id}/sidebar?show=true|false. Mirror of
 *  `usePinSavedView` but for the left-nav "Saved views" section.
 *  Optimistic flip with rollback on error so the toggle feels instant.
 *  Also invalidates the layout-level cache so the sidebar nav redraws
 *  on the next render. */
export function useSidebarSavedView() {
  const qc = useQueryClient();
  const router = useRouter();
  return useApiMutation<unknown, { id: string; show: boolean }>(
    ({ id, show }) => ({
      path: `/me/saved-views/${id}/sidebar?show=${show ? "true" : "false"}`,
      method: "POST",
    }),
    {
      successMessage: (_data, input) =>
        input.show ? "Added to sidebar" : "Removed from sidebar",
      onMutate: async ({ id, show }) => {
        await qc.cancelQueries({ queryKey: ["saved-views"] });
        const snapshot = qc.getQueriesData<SavedViewListView>({
          queryKey: ["saved-views", "list"],
        });
        for (const [key, data] of snapshot) {
          if (!data) continue;
          qc.setQueryData<SavedViewListView>(key, {
            ...data,
            items: data.items.map((v) =>
              v.id === id ? { ...v, show_in_sidebar: show } : v,
            ),
          });
        }
        return { snapshot };
      },
      onError: (_err, _vars, ctx) => {
        const snap = (
          ctx as
            | {
                snapshot?: ReadonlyArray<
                  readonly [readonly unknown[], SavedViewListView | undefined]
                >;
              }
            | undefined
        )?.snapshot;
        if (snap) {
          for (const [key, data] of snap) {
            qc.setQueryData(key, data);
          }
        }
      },
      onSettled: () => {
        qc.invalidateQueries({ queryKey: ["saved-views"] });
        // The library layout fetches sidebar views server-side; refresh
        // so the nav reflects the change.
        router.refresh();
      },
    },
  );
}

/** POST /me/saved-views/reorder. Server expects view ids in the desired
 *  pin order; views not currently pinned are rejected. */
export function useReorderSavedViews() {
  const qc = useQueryClient();
  return useApiMutation<unknown, { view_ids: string[] }>(
    (body) => ({ path: "/me/saved-views/reorder", method: "POST", body }),
    {
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: ["saved-views"] });
      },
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

// ---------- Collections (markers + collections M3) ----------

/** Create a new user collection. The cover-menu "Create new… " flow
 *  chains this with `useAddCollectionEntry` to insert the just-selected
 *  series/issue. */
export function useCreateCollection() {
  const qc = useQueryClient();
  return useApiMutation<SavedViewView, CreateCollectionReq>(
    (body) => ({ path: "/me/collections", method: "POST", body }),
    {
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.collections });
        // Sidebar reads from `/me/saved-views`; new collections appear
        // there too once the user pins or sidebar-toggles them.
        qc.invalidateQueries({ queryKey: ["saved-views"] });
      },
    },
  );
}

/** Rename / redescribe a collection. Send `description: ""` to clear. */
export function useUpdateCollection(id: string) {
  const qc = useQueryClient();
  return useApiMutation<SavedViewView, UpdateCollectionReq>(
    (body) => ({ path: `/me/collections/${id}`, method: "PATCH", body }),
    {
      successMessage: "Saved",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.collections });
        qc.invalidateQueries({ queryKey: queryKeys.collection(id) });
        qc.invalidateQueries({ queryKey: ["saved-views"] });
      },
    },
  );
}

/** Delete a collection. The server rejects deletion of the per-user
 *  Want to Read row (`system_key='want_to_read'`) with 409. */
export function useDeleteCollection(id: string) {
  const qc = useQueryClient();
  return useApiMutation<unknown, void>(
    () => ({ path: `/me/collections/${id}`, method: "DELETE" }),
    {
      successMessage: "Collection deleted",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.collections });
        qc.invalidateQueries({ queryKey: ["saved-views"] });
      },
    },
  );
}

/** Add a series or issue ref to a collection. The server is
 *  idempotent — a duplicate add returns `409 already_in_collection`,
 *  which propagates as a toast message. */
export function useAddCollectionEntry(collectionId: string) {
  const qc = useQueryClient();
  return useApiMutation<CollectionEntryView, AddEntryReq>(
    (body) => ({
      path: `/me/collections/${collectionId}/entries`,
      method: "POST",
      body,
    }),
    {
      onSuccess: () => {
        qc.invalidateQueries({
          queryKey: ["collections", "entries", collectionId],
        });
        qc.invalidateQueries({ queryKey: queryKeys.collections });
      },
    },
  );
}

/** Remove a single entry from a collection. */
export function useRemoveCollectionEntry(collectionId: string) {
  const qc = useQueryClient();
  return useApiMutation<unknown, { entryId: string }>(
    ({ entryId }) => ({
      path: `/me/collections/${collectionId}/entries/${entryId}`,
      method: "DELETE",
    }),
    {
      successMessage: "Removed",
      onSuccess: () => {
        qc.invalidateQueries({
          queryKey: ["collections", "entries", collectionId],
        });
        qc.invalidateQueries({ queryKey: queryKeys.collections });
      },
    },
  );
}

// ---------- Markers (markers + collections M5) ----------

/** Create a marker — bookmark / note / favorite / highlight. The
 *  reader cover-menu and `b` / `n` / `h` keybinds chain here. On
 *  success the per-issue + global feed caches are invalidated so the
 *  overlay and `/bookmarks` page refresh together. */
export function useCreateMarker() {
  const qc = useQueryClient();
  return useApiMutation<MarkerView, CreateMarkerReq>(
    (body) => ({ path: "/me/markers", method: "POST", body }),
    {
      onSuccess: (_data, input) => {
        qc.invalidateQueries({
          queryKey: ["markers", "issue", input.issue_id],
        });
        qc.invalidateQueries({ queryKey: ["markers", "list"] });
        // Sidebar badge — only create + delete change the total, so we
        // skip the count invalidation in `useUpdateMarker`.
        qc.invalidateQueries({ queryKey: ["markers", "count"] });
        qc.invalidateQueries({ queryKey: ["markers", "tags"] });
      },
    },
  );
}

/** Edit a marker's body / color / region / selection. Per-kind
 *  invariants are enforced server-side (e.g. a note body can't be
 *  cleared). `issueId` keys the per-issue invalidation. */
export function useUpdateMarker(id: string, issueId: string) {
  const qc = useQueryClient();
  return useApiMutation<MarkerView, UpdateMarkerReq>(
    (body) => ({ path: `/me/markers/${id}`, method: "PATCH", body }),
    {
      // Tailor the toast for the most common single-field toggles so
      // the action is unambiguous. Falls back to "Saved" for editor
      // submits (body / region / tags / multiple-field updates).
      successMessage: (_data, input) => {
        const keys = Object.keys(input);
        if (keys.length === 1 && keys[0] === "is_favorite") {
          return input.is_favorite
            ? "Added to favorites"
            : "Removed from favorites";
        }
        return "Saved";
      },
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: ["markers", "issue", issueId] });
        qc.invalidateQueries({ queryKey: ["markers", "list"] });
        // Tag edits change the rollup but not the count — invalidate
        // tags specifically (count stays stable so we skip that key).
        qc.invalidateQueries({ queryKey: ["markers", "tags"] });
      },
    },
  );
}

export function useDeleteMarker(id: string, issueId: string) {
  const qc = useQueryClient();
  return useApiMutation<unknown, void>(
    () => ({ path: `/me/markers/${id}`, method: "DELETE" }),
    {
      successMessage: "Removed",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: ["markers", "issue", issueId] });
        qc.invalidateQueries({ queryKey: ["markers", "list"] });
        qc.invalidateQueries({ queryKey: ["markers", "count"] });
        qc.invalidateQueries({ queryKey: ["markers", "tags"] });
      },
    },
  );
}

/** Apply a full reorder of entries in one transaction. The server
 *  rejects partial reorders — every current entry id must be present. */
export function useReorderCollectionEntries(collectionId: string) {
  const qc = useQueryClient();
  return useApiMutation<unknown, ReorderEntriesReq>(
    (body) => ({
      path: `/me/collections/${collectionId}/entries/reorder`,
      method: "POST",
      body,
    }),
    {
      onSuccess: () => {
        qc.invalidateQueries({
          queryKey: ["collections", "entries", collectionId],
        });
      },
    },
  );
}

// ---------- Runtime settings + email (runtime-config-admin M1/M2) ----------

import type { TestEmailResp, UpdateSettingsReq } from "./types";

/** Apply a batch of setting updates. Unknown keys reject the whole batch.
 *  On success the server re-runs `Config::overlay_db`, swaps the live
 *  snapshot, and (when an `smtp.*` key changed) rebuilds the email sender. */
export function useUpdateSettings() {
  const qc = useQueryClient();
  return useApiMutation<unknown, UpdateSettingsReq>(
    (body) => ({ path: "/admin/settings", method: "PATCH", body }),
    {
      successMessage: "Settings saved",
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.adminSettings });
        qc.invalidateQueries({ queryKey: queryKeys.adminAuthConfig });
        qc.invalidateQueries({ queryKey: queryKeys.adminEmailStatus });
      },
    },
  );
}

/** Fire a probe email to the calling admin's address. Successful responses
 *  include `{ delivered, duration_ms, to }`. Errors surface as the
 *  underlying lettre message so operators can diagnose. */
export function useSendTestEmail() {
  const qc = useQueryClient();
  return useApiMutation<TestEmailResp, void>(
    () => ({ path: "/admin/email/test", method: "POST" }),
    {
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: queryKeys.adminEmailStatus });
      },
    },
  );
}

import type { OidcDiscoverReq, OidcDiscoverResp } from "./types";

/** Probe an OIDC issuer's discovery doc before committing it. Returns the
 *  parsed endpoints + scopes_supported. Used by the "Test discovery"
 *  button on /admin/auth so an admin can verify reachability without
 *  saving a half-baked config. */
export function useProbeOidcDiscovery() {
  return useApiMutation<OidcDiscoverResp, OidcDiscoverReq>(
    (body) => ({ path: "/admin/auth/oidc/discover", method: "POST", body }),
    {
      onError: () => {
        /* surfaced by the form inline; default toast still fires */
      },
    },
  );
}
