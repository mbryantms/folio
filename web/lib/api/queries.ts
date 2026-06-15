/**
 * Typed TanStack Query hooks. Populated per-resource as later milestones land:
 * - M2: libraries, scan runs, health issues, removed items, series identity
 * - M3: users, audit
 * - M4: preferences (extended)
 * - M5: app passwords, api tokens
 * - M6: stats, server info, logs, activity
 *
 * The shared `useMe` hook lives here so any client tree can read the current
 * user without re-fetching across components.
 */
import {
  useQuery,
  useQueries,
  useInfiniteQuery,
  keepPreviousData,
} from "@tanstack/react-query";

import { apiFetch, getCsrfToken } from "./auth-refresh";
// Query-key registry extracted to ./query-keys (audit H1a) so the
// check-query-keys gate whitelists exactly one file. Imported for the
// hooks below; re-exported (where the object used to live) so the
// long-standing `import { queryKeys } from "@/lib/api/queries"` is
// unchanged for every consumer.
import { queryKeys } from "./query-keys";
import type {
  ActivityKind,
  ActivityListView,
  CollectionEntriesView,
  IssueMarkersView,
  MarkerKind,
  MarkerCountView,
  MarkerListView,
  MarkerSearchView,
  MarkerTagMatch,
  MarkerTagsView,
  TextRegionsView,
  AdminOverviewView,
  AdminUserDetailView,
  AdminUserListView,
  AdminUserStatsListView,
  ContentInsightsView,
  DataQualityView,
  EngagementView,
  AuditListView,
  AuthConfigView,
  EmailStatusView,
  SettingsView,
  CatalogEntriesView,
  CatalogSourceListView,
  CblDetailView,
  CblEntryListView,
  CblListListView,
  CblWindowPageView,
  CblWindowView,
  ContinueReadingView,
  BackupStorageView,
  CrossLibHealthIssueView,
  CrossLibScanRunView,
  ScanBatchView,
  ScanBatchDetailView,
  LibraryEventView,
  HealthIssueView,
  HealthIssuesPage,
  MetadataOverviewView,
  NextUpView,
  PageCountResponse,
  OnDeckView,
  PageListView,
  PreviewReq,
  RefreshLogListView,
  IssueListView,
  IssueSearchView,
  IssueSort,
  PeopleListView,
  LibraryView,
  FsListResp,
  LogLevel,
  LogsResp,
  MeView,
  QueueDepthView,
  LogWidgetListView,
  ReadingLogFilters,
  ReadingLogPageView,
  ReadingSessionListView,
  ReadingStatsRange,
  ReadingStatsView,
  RemovedListView,
  SavedViewListView,
  SavedViewView,
  SidebarLayoutView,
  ScanPreviewView,
  AppPasswordListView,
  ProgressView,
  ScanRunView,
  SeriesListView,
  SeriesSort,
  SeriesView,
  LatestReleaseView,
  OcrModelsView,
  ServerInfoView,
  RestartPendingView,
  SessionListView,
  SortOrder,
  ThumbnailsSettingsView,
  ThumbnailsStatusView,
} from "./types";

export type UserListFilters = {
  role?: "admin" | "user";
  state?: "pending_verification" | "active" | "disabled";
  q?: string;
  limit?: number;
  cursor?: string;
};

export type AuditFilters = {
  actor_id?: string;
  action?: string;
  target_type?: string;
  since?: string;
  limit?: number;
  cursor?: string;
};

export type ReadingSessionsFilters = {
  issue_id?: string;
  series_id?: string;
  limit?: number;
  cursor?: string;
};

export type ReadingStatsScope =
  | { type: "all" }
  | { type: "series"; id: string }
  | { type: "issue"; id: string };

export type SeriesListFilters = {
  library?: string;
  q?: string;
  sort?: SeriesSort;
  order?: SortOrder;
  limit?: number;
  cursor?: string;
  /** Single status value: continuing | ended | cancelled | hiatus. */
  status?: string;
  /** Inclusive year-of-first-publication bounds. NULL years are
   *  excluded server-side when either bound is set. */
  year_from?: number;
  year_to?: number;
  /** Comma-separated CSVs. Series-direct columns (publisher,
   *  language, age_rating) are IN-set; genres/tags and the credit
   *  roles are includes-any against their junction tables. */
  publisher?: string;
  genres?: string;
  tags?: string;
  language?: string;
  age_rating?: string;
  writers?: string;
  pencillers?: string;
  inkers?: string;
  colorists?: string;
  letterers?: string;
  cover_artists?: string;
  editors?: string;
  translators?: string;
  /** Any-role credit filter — CSV of person names. Matches series
   *  where the person holds *any* credit role. Use this rather than
   *  stacking per-role facets when surfacing a single creator's full
   *  body of work (per-role facets AND-combine and drop creators with
   *  mixed roles). */
  credits?: string;
  /** Cast / setting facets — CSV, includes-any against the
   *  same-named CSV columns on the issues table. Server-side
   *  matching is case-insensitive and splits on `[,;]`. */
  characters?: string;
  teams?: string;
  locations?: string;
  /** Inclusive bounds (0..=5, half-star steps) on the calling user's
   *  series rating. Series the caller hasn't rated are excluded when
   *  either bound is set. */
  user_rating_min?: number;
  user_rating_max?: number;
  /** CSV of the caller's per-series read state — any of `unread`,
   *  `in_progress`, `read` (OR-combined). Matches the saved-views
   *  three-state rollup; a never-touched series reads as `unread`. */
  read_status?: string;
};

export type IssueListFilters = {
  q?: string;
  sort?: IssueSort;
  order?: SortOrder;
  limit?: number;
  cursor?: string;
};

/** Cross-library issue search shape. Server caps `q` length and clamps
 *  `limit` server-side; `series_id` constrains hits to one series. */
export type IssueSearchFilters = {
  q?: string;
  series_id?: string;
  limit?: number;
};

/** People search shape (global-search M4). */
export type PeopleSearchFilters = {
  q?: string;
  limit?: number;
};

/** Marker search shape (global-search M2 of the search-improvements
 *  plan). Backed by `/me/markers/search`. Per-caller scoped so only
 *  the user's own bookmarks / notes / highlights surface. */
export type MarkerSearchFilters = {
  q?: string;
  limit?: number;
};

/** Filter shape for the cross-library `/issues` listing. Mirrors the
 *  series-level `SeriesListFilters` minus `status` (series-only) and
 *  with all the same metadata-suite CSV facets, plus issue-specific
 *  sort options (`year`, `page_count`, `user_rating`). */
export type IssuesCrossListFilters = {
  library?: string;
  q?: string;
  sort?: IssueSort;
  order?: SortOrder;
  limit?: number;
  cursor?: string;
  year_from?: number;
  year_to?: number;
  publisher?: string;
  language?: string;
  age_rating?: string;
  genres?: string;
  tags?: string;
  writers?: string;
  pencillers?: string;
  inkers?: string;
  colorists?: string;
  letterers?: string;
  cover_artists?: string;
  editors?: string;
  translators?: string;
  characters?: string;
  teams?: string;
  locations?: string;
  user_rating_min?: number;
  user_rating_max?: number;
};

// `queryKeys` now lives in ./query-keys (imported above). Re-export it
// here so existing `from "@/lib/api/queries"` imports keep resolving.
export { queryKeys };

/** Filter shape for `useCblListEntriesInfinite`. `status` is a
 *  comma-separated subset of `matched,ambiguous,missing,manual`;
 *  omit (or pass empty) for "all". `limit` overrides the server
 *  default of 100, clamped to [1, 200]. */
export type CblEntriesFilters = {
  status?: string;
  limit?: number;
  q?: string;
};

export type SavedViewListFilters = {
  pinned?: boolean;
  /** Multi-page rails M3: restrict the pinned filter to a single page.
   *  When set, the result is the views pinned on that page only and
   *  implies `pinned=true` server-side. Omit + pair with `pinned: true`
   *  for the legacy "system Home" alias still supported in M3-M5. */
  pinnedOn?: string;
};

export type AdminLogFilters = {
  level?: LogLevel;
  q?: string;
  since?: number;
  limit?: number;
  /** UUID of a single library to scope to, or `"all"` / omit for
   *  cross-library. Backed by the scanner's instrumented spans —
   *  scan-emitted events carry the library_id via the RingLayer's
   *  parent-span walk. */
  library_id?: string;
  /** Stream filter (observability-split M12): `server` (app runtime, the
   *  Server-log default) | `library` (scanner/worker) | `all`. */
  domain?: "server" | "library";
};

export type AdminActivityFilters = {
  kinds?: ActivityKind[];
  limit?: number;
  cursor?: string;
};

/**
 * Typed error thrown by every client-side query in this module so callers can
 * inspect the HTTP status without parsing message strings. The server-side
 * `ApiError` in `lib/api/fetch.ts` cannot be reused here because that module
 * imports `next/headers` (server-only).
 */
export class HttpError extends Error {
  constructor(
    public readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = "HttpError";
  }
}

export async function jsonFetch<T>(path: string): Promise<T> {
  const res = await apiFetch(path, {
    headers: { Accept: "application/json" },
  });
  if (!res.ok) {
    let detail = `${path} → ${res.status}`;
    try {
      const body = await res.json();
      detail = body?.error?.message ?? detail;
    } catch {
      /* ignore */
    }
    throw new HttpError(res.status, detail);
  }
  return (await res.json()) as T;
}

export function useMe({ enabled = true }: { enabled?: boolean } = {}) {
  return useQuery({
    queryKey: queryKeys.me,
    queryFn: () => jsonFetch<MeView>("/auth/me"),
    staleTime: 60_000,
    // `enabled: false` lets always-mounted callers (root-layout
    // listeners) skip the probe on surfaces that are anonymous by
    // construction — an unauthed /auth/me triggers the refresh dance
    // and used to spray ~8 doomed 401/403s across the sign-in page.
    enabled,
  });
}

export function useSessions() {
  return useQuery({
    queryKey: queryKeys.sessions,
    queryFn: () => jsonFetch<SessionListView>("/me/sessions"),
    staleTime: 30_000,
  });
}

export function useAppPasswords() {
  return useQuery({
    queryKey: queryKeys.appPasswords,
    queryFn: () => jsonFetch<AppPasswordListView>("/me/app-passwords"),
    staleTime: 30_000,
  });
}

export function useLibraryList() {
  return useQuery({
    queryKey: queryKeys.libraries,
    queryFn: () => jsonFetch<LibraryView[]>("/libraries"),
  });
}

export function useLibrary(id: string) {
  return useQuery({
    queryKey: queryKeys.library(id),
    queryFn: () => jsonFetch<LibraryView>(`/libraries/${id}`),
    enabled: !!id,
  });
}

/**
 * Single first-page health summary for the lightweight consumers (library
 * overview stat card, live-scan seed). Returns the full envelope: `items` is a
 * recent sample across all statuses, `counts` carries the library-wide
 * open/resolved/dismissed tallies. The paginated/faceted table uses
 * {@link useHealthIssuesInfinite} instead.
 */
export function useHealthIssues(libraryId: string) {
  return useQuery({
    queryKey: queryKeys.health(libraryId),
    queryFn: () =>
      jsonFetch<HealthIssuesPage>(
        `/libraries/${libraryId}/health-issues?status=all&limit=50`,
      ),
    enabled: !!libraryId,
  });
}

/** `getNextPageParam` for {@link useHealthIssuesInfinite}. Exported + tested so
 *  a refactor can't swallow `next_cursor` and silently truncate the table —
 *  see web/tests/api/health-issues-next-page.test.ts. */
export function healthIssuesNextPage(
  page: HealthIssuesPage,
): string | undefined {
  return page.next_cursor ?? undefined;
}

/**
 * Paginated + server-faceted per-library health-issue table. Status, severity,
 * and kind are server query params (never client `.filter()` over a truncated
 * page); the first page carries a `counts` summary that drives the filter pills.
 */
export function useHealthIssuesInfinite(
  libraryId: string,
  filters: { status: string; severity: string; kind: string | null },
) {
  return useInfiniteQuery({
    queryKey: queryKeys.healthInfinite(libraryId, filters),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam }) => {
      const params = new URLSearchParams();
      params.set("status", filters.status);
      if (filters.severity !== "all") params.set("severity", filters.severity);
      if (filters.kind) params.set("kind", filters.kind);
      if (pageParam) params.set("cursor", pageParam);
      return jsonFetch<HealthIssuesPage>(
        `/libraries/${libraryId}/health-issues?${params.toString()}`,
      );
    },
    getNextPageParam: healthIssuesNextPage,
    enabled: !!libraryId,
  });
}

/**
 * Live archive page count for an issue — the *actual* count read from the
 * file, authoritative over the DB's `issue.page_count` (which can drift). The
 * page editor uses this to build its tiles so it never shows a phantom page.
 * `enabled` gates it to when the editor is open.
 */
export function useArchivePageCount(issueId: string, enabled: boolean) {
  return useQuery({
    queryKey: queryKeys.archivePageCount(issueId),
    enabled: enabled && !!issueId,
    queryFn: () =>
      jsonFetch<PageCountResponse>(
        `/issues/${encodeURIComponent(issueId)}/archive/page-count`,
      ),
    staleTime: 0,
  });
}

/** Rolled-up `.bak` backup-file footprint for a library (archive-rewrite M7). */
export function useBackupStorage(libraryId: string) {
  return useQuery({
    queryKey: queryKeys.backupStorage(libraryId),
    queryFn: () =>
      jsonFetch<BackupStorageView>(`/libraries/${libraryId}/backup-storage`),
    enabled: !!libraryId,
  });
}

/**
 * Per-issue health-issue list — open rows whose file_path matches this
 * issue's file. Tranche B of recovery-visibility surfaces these as a
 * badge on the issue detail page and as a one-time toast on reader
 * open. Returns `[]` when the file is clean; consumers can `.length`
 * to gate the UI.
 */
export function useIssueHealth(seriesSlug: string, issueSlug: string) {
  return useQuery({
    queryKey: queryKeys.issueHealth(seriesSlug, issueSlug),
    queryFn: () =>
      jsonFetch<HealthIssueView[]>(
        `/series/${seriesSlug}/issues/${issueSlug}/health-issues`,
      ),
    enabled: !!seriesSlug && !!issueSlug,
  });
}

/** Total metadata overview for an issue: completeness, source files,
 *  freshness, per-field provenance, external IDs, and pinned fields. Powers
 *  the issue page's Metadata tab. */
export function useIssueMetadataOverview(
  seriesSlug: string,
  issueSlug: string,
) {
  return useQuery({
    queryKey: queryKeys.issueMetadataOverview(seriesSlug, issueSlug),
    queryFn: () =>
      jsonFetch<MetadataOverviewView>(
        `/series/${encodeURIComponent(seriesSlug)}/issues/${encodeURIComponent(issueSlug)}/metadata-overview`,
      ),
    enabled: !!seriesSlug && !!issueSlug,
    staleTime: 30_000,
  });
}

/**
 * Library scan history. `kind` filters by trigger: `'library' | 'series' |
 * 'issue'`, or omitted for all. Server caps at 500 rows; default is 50.
 */
/**
 * First page of a library's scan runs as a flat array — for summary
 * consumers (overview card, live-scan progress) that only read the most
 * recent run (`data[0]`). The endpoint is now cursor-paginated; the full
 * scrollable history uses {@link useScanRunsInfinite}.
 */
export function useScanRuns(
  libraryId: string,
  opts?: { kind?: "library" | "series" | "issue" | "all" },
) {
  const kind = opts?.kind && opts.kind !== "all" ? opts.kind : undefined;
  const qs = kind ? `?kind=${encodeURIComponent(kind)}` : "";
  return useQuery({
    queryKey: queryKeys.scanRuns(libraryId, kind ?? "all"),
    queryFn: () =>
      jsonFetch<{ items: ScanRunView[]; next_cursor: string | null }>(
        `/libraries/${libraryId}/scan-runs${qs}`,
      ).then((p) => p.items),
    enabled: !!libraryId,
  });
}

/**
 * Cursor-paginated scan history for a single library — backs the
 * `ScanRunsTable` with a Load-more button (audit D5). Mirrors the
 * cross-library `useAdminScanRunsInfinite` shape.
 */
export function useScanRunsInfinite(
  libraryId: string,
  opts?: { kind?: "library" | "series" | "issue" | "all"; limit?: number },
) {
  const kind = opts?.kind && opts.kind !== "all" ? opts.kind : undefined;
  return useInfiniteQuery({
    queryKey: queryKeys.scanRunsInfinite(libraryId, kind ?? "all"),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam }) => {
      const params = new URLSearchParams();
      if (kind) params.set("kind", kind);
      if (opts?.limit) params.set("limit", String(opts.limit));
      if (pageParam) params.set("cursor", pageParam);
      const qs = params.toString();
      return jsonFetch<{ items: ScanRunView[]; next_cursor: string | null }>(
        `/libraries/${libraryId}/scan-runs${qs ? `?${qs}` : ""}`,
      );
    },
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    enabled: !!libraryId,
  });
}

/**
 * Cross-library health-issue feed. Backs both the dashboard "Open
 * health issues" card (limit:5 truncated view) and the /admin/findings
 * page (paginated table with filters).
 *
 * Cursor pagination is the caller's responsibility — pass `cursor` for
 * subsequent pages.
 */
export function useAdminHealthIssues(filters: {
  library_id?: string;
  kind?: string;
  severity?: string;
  include_resolved?: boolean;
  include_dismissed?: boolean;
  limit?: number;
  cursor?: string;
}) {
  const params = new URLSearchParams();
  if (filters.library_id) params.set("library_id", filters.library_id);
  if (filters.kind) params.set("kind", filters.kind);
  if (filters.severity) params.set("severity", filters.severity);
  if (filters.include_resolved) params.set("include_resolved", "true");
  if (filters.include_dismissed) params.set("include_dismissed", "true");
  if (filters.limit != null) params.set("limit", String(filters.limit));
  if (filters.cursor) params.set("cursor", filters.cursor);
  const qs = params.toString();
  // Strip cursor from the cache key so successive pages share a cache
  // bucket and `useInfiniteQuery` can stitch them together if we later
  // migrate to that pattern.
  const { cursor: _drop, ...keyFilters } = filters;
  return useQuery({
    queryKey: queryKeys.adminHealthIssues(keyFilters),
    queryFn: () =>
      jsonFetch<{
        items: CrossLibHealthIssueView[];
        next_cursor: string | null;
      }>(`/admin/health-issues${qs ? `?${qs}` : ""}`),
  });
}

export function useAdminHealthIssuesInfinite(filters: {
  library_id?: string;
  kind?: string;
  severity?: string;
  include_resolved?: boolean;
  include_dismissed?: boolean;
  limit?: number;
}) {
  return useInfiniteQuery({
    queryKey: queryKeys.adminHealthIssuesInfinite(filters),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam }) => {
      const params = new URLSearchParams();
      if (filters.library_id) params.set("library_id", filters.library_id);
      if (filters.kind) params.set("kind", filters.kind);
      if (filters.severity) params.set("severity", filters.severity);
      if (filters.include_resolved) params.set("include_resolved", "true");
      if (filters.include_dismissed) {
        params.set("include_dismissed", "true");
      }
      if (filters.limit != null) params.set("limit", String(filters.limit));
      if (pageParam) params.set("cursor", pageParam);
      const qs = params.toString();
      return jsonFetch<{
        items: CrossLibHealthIssueView[];
        next_cursor: string | null;
      }>(`/admin/health-issues${qs ? `?${qs}` : ""}`);
    },
    getNextPageParam: (page) => page.next_cursor ?? undefined,
  });
}

/**
 * Cross-library scan-run history. Used by the dashboard's "Recent
 * scan failures" card (filter `state=failed&since=<7d-ago>`), the
 * findings page's Scan-runs rail, and any "what just ran" surface.
 */
export function useAdminScanRuns(filters: {
  library_id?: string;
  kind?: string;
  state?: string;
  since?: string;
  limit?: number;
  cursor?: string;
}) {
  const params = new URLSearchParams();
  if (filters.library_id) params.set("library_id", filters.library_id);
  if (filters.kind) params.set("kind", filters.kind);
  if (filters.state) params.set("state", filters.state);
  if (filters.since) params.set("since", filters.since);
  if (filters.limit != null) params.set("limit", String(filters.limit));
  if (filters.cursor) params.set("cursor", filters.cursor);
  const qs = params.toString();
  const { cursor: _drop, ...keyFilters } = filters;
  return useQuery({
    queryKey: queryKeys.adminScanRuns(keyFilters),
    queryFn: () =>
      jsonFetch<{
        items: CrossLibScanRunView[];
        next_cursor: string | null;
      }>(`/admin/scan-runs${qs ? `?${qs}` : ""}`),
  });
}

export function useAdminScanRunsInfinite(filters: {
  library_id?: string;
  kind?: string;
  state?: string;
  since?: string;
  limit?: number;
}) {
  return useInfiniteQuery({
    queryKey: queryKeys.adminScanRunsInfinite(filters),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam }) => {
      const params = new URLSearchParams();
      if (filters.library_id) params.set("library_id", filters.library_id);
      if (filters.kind) params.set("kind", filters.kind);
      if (filters.state) params.set("state", filters.state);
      if (filters.since) params.set("since", filters.since);
      if (filters.limit != null) params.set("limit", String(filters.limit));
      if (pageParam) params.set("cursor", pageParam);
      const qs = params.toString();
      return jsonFetch<{
        items: CrossLibScanRunView[];
        next_cursor: string | null;
      }>(`/admin/scan-runs${qs ? `?${qs}` : ""}`);
    },
    getNextPageParam: (page) => page.next_cursor ?? undefined,
  });
}

/**
 * Most-recent scan_run row per library, oldest-scanned-first. Backs
 * the dashboard's "Latest scan per library" card so operators see at
 * a glance which libraries haven't been touched in a while.
 */
export function useAdminLatestScanPerLibrary() {
  return useQuery({
    queryKey: queryKeys.adminLatestScanPerLibrary,
    queryFn: () =>
      jsonFetch<CrossLibScanRunView[]>(`/admin/scan-runs/latest-per-library`),
  });
}

/**
 * Recent "Scan all" batches for the dashboard's batch rail
 * (observability-split M9). Polls while any batch is still running so the
 * list reflects newly-finished batches without a WS round-trip.
 */
export function useScanBatches(state?: string) {
  return useQuery({
    queryKey: queryKeys.adminScanBatches(state),
    queryFn: () => {
      const qs = state ? `?state=${encodeURIComponent(state)}` : "";
      return jsonFetch<{ items: ScanBatchView[]; next_cursor: string | null }>(
        `/admin/scan-batches${qs}`,
      );
    },
    refetchInterval: (q) =>
      q.state.data?.items.some((b) => b.state === "running") ? 5_000 : false,
  });
}

/**
 * Single scan-all batch detail: member runs, aggregated totals, event count.
 * `enabled` is false when no batch is selected. Polls while the batch is
 * running as a backstop to the live WS reducer.
 */
export function useScanBatch(id: string | null) {
  return useQuery({
    queryKey: queryKeys.adminScanBatch(id ?? ""),
    enabled: !!id,
    queryFn: () => jsonFetch<ScanBatchDetailView>(`/admin/scan-batches/${id}`),
    refetchInterval: (q) => (q.state.data?.state === "running" ? 5_000 : false),
  });
}

/**
 * Infinite list over the durable `library_events` manifest
 * (observability-split M10). Filters are server-side query params; the caller
 * drives an IntersectionObserver sentinel off `fetchNextPage` /
 * `hasNextPage` (no silent truncation).
 */
export function useLibraryEventsInfinite(filters: {
  library_id?: string;
  batch_id?: string;
  scan_run_id?: string;
  category?: string;
  action?: string;
  severity?: string;
  limit?: number;
}) {
  const { limit, ...keyFilters } = filters;
  return useInfiniteQuery({
    queryKey: queryKeys.adminLibraryEventsInfinite(keyFilters),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam }) => {
      const params = new URLSearchParams();
      if (filters.library_id) params.set("library_id", filters.library_id);
      if (filters.batch_id) params.set("batch_id", filters.batch_id);
      if (filters.scan_run_id) params.set("scan_run_id", filters.scan_run_id);
      if (filters.category) params.set("category", filters.category);
      if (filters.action) params.set("action", filters.action);
      if (filters.severity) params.set("severity", filters.severity);
      if (limit != null) params.set("limit", String(limit));
      if (pageParam) params.set("cursor", pageParam);
      const qs = params.toString();
      return jsonFetch<{
        items: LibraryEventView[];
        next_cursor: string | null;
      }>(`/admin/library-events${qs ? `?${qs}` : ""}`);
    },
    getNextPageParam: (page) => page.next_cursor ?? undefined,
  });
}

export function useScanPreview(libraryId: string) {
  return useQuery({
    queryKey: queryKeys.scanPreview(libraryId),
    queryFn: () =>
      jsonFetch<ScanPreviewView>(`/libraries/${libraryId}/scan-preview`),
    enabled: !!libraryId,
  });
}

export function useRemovedItems(libraryId: string) {
  return useQuery({
    queryKey: queryKeys.removed(libraryId),
    queryFn: () =>
      jsonFetch<RemovedListView>(`/libraries/${libraryId}/removed`),
    enabled: !!libraryId,
  });
}

export function useSeries(id: string) {
  return useQuery({
    queryKey: queryKeys.series(id),
    queryFn: () => jsonFetch<SeriesView>(`/series/${id}`),
    enabled: !!id,
  });
}

/**
 * Batched series lookup. Shares the per-id cache with `useSeries(id)` so a
 * page that calls both stays consistent. 404s are not retried — orphan
 * localStorage entries (deleted series) shouldn't trigger four requests.
 */
export function useSeriesMany(ids: readonly string[]) {
  return useQueries({
    queries: ids.map((id) => ({
      queryKey: queryKeys.series(id),
      queryFn: () => jsonFetch<SeriesView>(`/series/${id}`),
      enabled: !!id,
      retry: (failureCount: number, error: unknown) => {
        if (error instanceof HttpError && error.status === 404) return false;
        return failureCount < 3;
      },
    })),
  });
}

/**
 * Apalis queue depth — polled at a steady cadence so the admin shell can show
 * a visible "draining N jobs" indicator without depending on the WebSocket.
 * The endpoint is admin-only; callers gate on `me.role === "admin"`.
 */
export function useQueueDepth(opts?: {
  enabled?: boolean;
  intervalMs?: number;
}) {
  const { enabled = true, intervalMs = 5_000 } = opts ?? {};
  return useQuery({
    queryKey: queryKeys.queueDepth,
    queryFn: () => jsonFetch<QueueDepthView>("/admin/queue-depth"),
    enabled,
    refetchInterval: intervalMs,
    staleTime: intervalMs,
  });
}

/**
 * Library thumbnail-pipeline status (M3). Polled at 5s while the queue is
 * draining (i.e. `in_flight > 0`); otherwise refreshes on tab focus or
 * manual revalidation. Called from the library overview card.
 */
export function useThumbnailsStatus(
  libraryId: string,
  opts?: { intervalMs?: number; enabled?: boolean },
) {
  const { intervalMs = 5_000, enabled = true } = opts ?? {};
  return useQuery({
    queryKey: queryKeys.thumbnailsStatus(libraryId),
    queryFn: () =>
      jsonFetch<ThumbnailsStatusView>(
        `/admin/libraries/${libraryId}/thumbnails-status`,
      ),
    enabled: enabled && !!libraryId,
    refetchInterval: (query) => {
      const data = query.state.data as ThumbnailsStatusView | undefined;
      return data && data.in_flight > 0 ? intervalMs : false;
    },
  });
}

/** Per-library thumbnail settings (enabled + format). Fetched once on
 *  mount; the PATCH mutation invalidates this key. */
export function useThumbnailsSettings(libraryId: string) {
  return useQuery({
    queryKey: queryKeys.thumbnailsSettings(libraryId),
    queryFn: () =>
      jsonFetch<ThumbnailsSettingsView>(
        `/admin/libraries/${libraryId}/thumbnails-settings`,
      ),
    enabled: !!libraryId,
  });
}

// ---------- Admin users + audit (M3) ----------

function buildQuery(
  params: Record<string, string | number | undefined>,
): string {
  const sp = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === "" || v === null) continue;
    sp.set(k, String(v));
  }
  const s = sp.toString();
  return s ? `?${s}` : "";
}

export function useUserList(filters: UserListFilters = {}) {
  return useQuery({
    queryKey: queryKeys.users(filters),
    queryFn: () =>
      jsonFetch<AdminUserListView>(`/admin/users${buildQuery(filters)}`),
    placeholderData: keepPreviousData,
  });
}

export function useUser(id: string) {
  return useQuery({
    queryKey: queryKeys.user(id),
    queryFn: () => jsonFetch<AdminUserDetailView>(`/admin/users/${id}`),
    enabled: !!id,
  });
}

export function useAuditLog(filters: AuditFilters = {}) {
  return useQuery({
    queryKey: queryKeys.audit(filters),
    queryFn: () =>
      jsonFetch<AuditListView>(`/admin/audit${buildQuery(filters)}`),
    placeholderData: keepPreviousData,
  });
}

// ---------- Reading sessions / stats (M6a) ----------

/**
 * List the current user's reading sessions, optionally filtered by issue or
 * series. The settings/activity page calls this with no filter; the series
 * and issue pages (M6b) will call it scoped.
 */
export function useReadingSessions(filters: ReadingSessionsFilters = {}) {
  return useInfiniteQuery({
    queryKey: queryKeys.readingSessions(filters),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      jsonFetch<ReadingSessionListView>(
        `/me/reading-sessions${buildQuery({ ...filters, cursor: pageParam })}`,
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

/**
 * Aggregated reading stats — totals + per-day buckets in the user's
 * timezone + global streaks. M6a uses this on /settings/activity for the
 * top card and sparkline.
 */
export function useReadingStats(
  scope: ReadingStatsScope,
  range: ReadingStatsRange = "30d",
) {
  const params: Record<string, string> = { range };
  if (scope.type === "series") params.series_id = scope.id;
  else if (scope.type === "issue") params.issue_id = scope.id;
  return useQuery({
    queryKey: queryKeys.readingStats(scope, range),
    queryFn: () =>
      jsonFetch<ReadingStatsView>(`/me/reading-stats${buildQuery(params)}`),
  });
}

/** The user's pinned log-widget grid in `position` order. The server
 *  auto-seeds the M2 default layout on the first GET — meaning this
 *  hook is never going to return an empty array for a freshly
 *  registered account. */
export function useLogWidgets() {
  return useQuery({
    queryKey: queryKeys.logWidgets,
    queryFn: () => jsonFetch<LogWidgetListView>("/me/log/widgets"),
    // `LogWidgetListView` is now the uniform `CursorPage<LogWidgetView>`
    // envelope (audit-remediation M4). Widgets are bounded per-user;
    // `next_cursor` is always null. The single consumer reads
    // `data?.items`.
  });
}

/** Cursor-paginated walk over the reverse-chronological reading-log
 *  event feed. Backs the `/log` page's main column. Filters drive the
 *  server query — never client-side `.filter()` on the (cursor-bound)
 *  returned set, per CLAUDE.md's list-pagination convention. */
export function useReadingLogInfinite(filters: ReadingLogFilters = {}) {
  const params: Record<string, string | number> = {};
  if (filters.kinds && filters.kinds.length > 0) {
    params.kind = filters.kinds.join(",");
  }
  if (filters.from) params.from = filters.from;
  if (filters.to) params.to = filters.to;
  if (filters.library_id) params.library_id = filters.library_id;
  if (filters.series_id) params.series_id = filters.series_id;
  if (filters.limit != null) params.limit = filters.limit;
  if (filters.include_hidden) params.include_hidden = "true";
  return useInfiniteQuery({
    queryKey: queryKeys.readingLog(filters),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      jsonFetch<ReadingLogPageView>(
        `/me/reading-log${buildQuery({ ...params, cursor: pageParam })}`,
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

// ---------- Admin observability (M6c) ----------

/**
 * Admin dashboard overview — totals, open-health-by-severity, scans in
 * flight, sessions-today, active-readers-now (5m heartbeat), and a 14-day
 * reads-volume sparkline. All aggregate; never per-user. Polled every 15s
 * so the "currently scanning / reading" tiles feel live.
 */
export function useAdminOverview(opts?: {
  intervalMs?: number;
  enabled?: boolean;
}) {
  const { intervalMs = 15_000, enabled = true } = opts ?? {};
  return useQuery({
    queryKey: queryKeys.adminOverview,
    queryFn: () => jsonFetch<AdminOverviewView>("/admin/stats/overview"),
    enabled,
    refetchInterval: intervalMs,
    staleTime: intervalMs,
  });
}

/**
 * Per-user reading stats from the admin perspective. The server emits an
 * `admin.user.activity.view` audit row on every successful fetch — so the
 * caller should expect this query to fire only when the user is actively
 * looking at the Reading tab on a user-detail page.
 */
export function useAdminUserReadingStats(
  userId: string,
  range: ReadingStatsRange = "30d",
  opts?: { enabled?: boolean },
) {
  const { enabled = true } = opts ?? {};
  return useQuery({
    queryKey: queryKeys.adminUserReadingStats(userId, range),
    queryFn: () =>
      jsonFetch<ReadingStatsView>(
        `/admin/users/${userId}/reading-stats?range=${range}`,
      ),
    enabled: enabled && !!userId,
  });
}

/** Stats v2 — per-user aggregates (admin-only). One row per user. */
export function useAdminUsersStats(opts?: { enabled?: boolean }) {
  const { enabled = true } = opts ?? {};
  return useQuery({
    queryKey: queryKeys.adminUsersStats,
    queryFn: () => jsonFetch<AdminUserStatsListView>("/admin/stats/users"),
    enabled,
  });
}

/** Stats v2 — DAU/WAU/MAU rolling counts (90d) + device-30d. */
export function useAdminEngagement(opts?: { enabled?: boolean }) {
  const { enabled = true } = opts ?? {};
  return useQuery({
    queryKey: queryKeys.adminEngagement,
    queryFn: () => jsonFetch<EngagementView>("/admin/stats/engagement"),
    enabled,
  });
}

/** Stats v2 — content insights (dead-stock, abandoned, completion funnel). */
export function useAdminContent(opts?: { enabled?: boolean }) {
  const { enabled = true } = opts ?? {};
  return useQuery({
    queryKey: queryKeys.adminContent,
    queryFn: () => jsonFetch<ContentInsightsView>("/admin/stats/content"),
    enabled,
  });
}

/** Stats v2 — orphan/long sessions + metadata coverage. */
export function useAdminQuality(opts?: { enabled?: boolean }) {
  const { enabled = true } = opts ?? {};
  return useQuery({
    queryKey: queryKeys.adminQuality,
    queryFn: () => jsonFetch<DataQualityView>("/admin/stats/quality"),
    enabled,
  });
}

/** Server health/version info. Polled every 30s on the dashboard. */
export function useServerInfo(opts?: {
  intervalMs?: number;
  enabled?: boolean;
}) {
  const { intervalMs = 30_000, enabled = true } = opts ?? {};
  return useQuery({
    queryKey: queryKeys.serverInfo,
    queryFn: () => jsonFetch<ServerInfoView>("/admin/server/info"),
    enabled,
    refetchInterval: intervalMs,
    staleTime: intervalMs,
  });
}

/** Boot-only settings (worker pools, ZIP LRU, metadata cron) whose
 *  persisted value changed since startup and need a restart to apply.
 *  Admin-only; gate with `enabled` so it never fires on the shared shell
 *  for a non-admin user. Polled lightly — restart state only changes on a
 *  settings save. */
export function useRestartPending(opts?: { enabled?: boolean }) {
  const { enabled = true } = opts ?? {};
  return useQuery({
    queryKey: queryKeys.restartPending,
    queryFn: () =>
      jsonFetch<RestartPendingView>("/admin/server/restart-pending"),
    enabled,
    staleTime: 30_000,
  });
}

/** OCR model cache state (text-detection-1.0 plan, M5).
 *  Read-only; the response shape is stable across requests so we
 *  poll lightly — operators look at this on demand, not in
 *  real-time. */
export function useOcrModels(opts?: { enabled?: boolean }) {
  const { enabled = true } = opts ?? {};
  return useQuery({
    queryKey: queryKeys.ocrModels,
    queryFn: () => jsonFetch<OcrModelsView>("/admin/ocr/models"),
    enabled,
    // Models almost never change after first download; keep stale
    // for 30 s so an operator polling the page sees fresh-enough
    // data without polling.
    staleTime: 30_000,
  });
}

/** Latest GitHub release for the repo this server was built from.
 *  Server-side cache TTL is 1 hour, so polling more often than that
 *  yields the same value. Returns `null` (mapped from HTTP 204) when
 *  the update check is disabled, the repo isn't on GitHub, or the
 *  last fetch errored. UI hides its "update available" line on null. */
export function useLatestRelease(opts?: { enabled?: boolean }) {
  const { enabled = true } = opts ?? {};
  return useQuery({
    queryKey: queryKeys.latestRelease,
    queryFn: async (): Promise<LatestReleaseView | null> => {
      const res = await apiFetch("/admin/server/latest-release", {
        headers: { Accept: "application/json" },
      });
      if (res.status === 204) return null;
      if (!res.ok) {
        // Non-2xx other than 204 — treat as no-data rather than
        // surfacing an error toast for a non-essential probe.
        return null;
      }
      return (await res.json()) as LatestReleaseView;
    },
    enabled,
    // Match the server's 1-hour TTL so we don't spin extra requests
    // when the answer can't change anyway.
    staleTime: 3_600_000,
    // No refetch interval — only fetched on mount of the build card.
  });
}

/**
 * In-process log ring buffer. The follow-tail toggle is implemented at the
 * page level by polling with `?since=<watermark>` — pass `intervalMs` to
 * enable that.
 */
export function useAdminLogs(
  filters: AdminLogFilters = {},
  opts?: { intervalMs?: number; enabled?: boolean },
) {
  const { intervalMs, enabled = true } = opts ?? {};
  return useQuery({
    queryKey: queryKeys.adminLogs(filters),
    queryFn: () => jsonFetch<LogsResp>(`/admin/logs${buildQuery(filters)}`),
    enabled,
    refetchInterval: intervalMs ?? false,
    staleTime: intervalMs ?? 5_000,
    placeholderData: keepPreviousData,
  });
}

/**
 * Lists immediate-child directories under a path inside the configured
 * library root. Backs the New Library dialog's path picker. Pass
 * `undefined` (or omit) on the first call to land at the root.
 */
export function useAdminFsList(path: string | undefined, enabled = true) {
  return useQuery({
    queryKey: queryKeys.adminFsList(path),
    queryFn: () =>
      jsonFetch<FsListResp>(
        `/admin/fs/list${path ? `?path=${encodeURIComponent(path)}` : ""}`,
      ),
    enabled,
    // Folder structure changes rarely during a single dialog session.
    staleTime: 30_000,
  });
}

/**
 * Combined activity feed. Infinite-paginated via the opaque cursor; pass
 * filter chips via `kinds`. Reading entries are always aggregated — never
 * per-user.
 */
export function useAdminActivity(filters: AdminActivityFilters = {}) {
  const { kinds, limit, ...rest } = filters;
  void rest;
  const baseQs: Record<string, string | number> = {};
  if (kinds && kinds.length > 0) baseQs.kinds = kinds.join(",");
  if (limit) baseQs.limit = limit;
  return useInfiniteQuery({
    queryKey: queryKeys.adminActivity(filters),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      jsonFetch<ActivityListView>(
        `/admin/activity${buildQuery({ ...baseQs, cursor: pageParam })}`,
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

/** Read-only auth config (M6e). Cached for the page session. */
export function useAuthConfig(opts?: { enabled?: boolean }) {
  const { enabled = true } = opts ?? {};
  return useQuery({
    queryKey: queryKeys.adminAuthConfig,
    queryFn: () => jsonFetch<AuthConfigView>("/admin/auth/config"),
    enabled,
    staleTime: 60_000,
  });
}

/** Runtime-editable settings list (M1+ of runtime-config-admin). The
 *  registry grows milestone-by-milestone; secret values are returned as
 *  `"<set>"` strings. */
export function useAdminSettings(opts?: { enabled?: boolean }) {
  const { enabled = true } = opts ?? {};
  return useQuery({
    queryKey: queryKeys.adminSettings,
    queryFn: () => jsonFetch<SettingsView>("/admin/settings"),
    enabled,
    staleTime: 5_000,
  });
}

/** Outbound-email pipeline probe (M2). Refetches every 15s so the
 *  /admin/email page reflects the result of a "Send test email" click
 *  without manual reload. */
export function useEmailStatus(opts?: { enabled?: boolean }) {
  const { enabled = true } = opts ?? {};
  return useQuery({
    queryKey: queryKeys.adminEmailStatus,
    queryFn: () => jsonFetch<EmailStatusView>("/admin/email/status"),
    enabled,
    refetchInterval: 15_000,
    staleTime: 0,
  });
}

// ---------- Series + issues lists ----------

function stripCursor<T extends { cursor?: string }>(
  filters: T,
): Omit<T, "cursor"> {
  const { cursor, ...rest } = filters;
  void cursor;
  return rest;
}

/** One-shot series list fetch — used for the per-library "recently
 *  added/updated" rails on the home page. For paginated discovery use
 *  `useSeriesListInfinite`. */
export function useSeriesList(filters: SeriesListFilters = {}) {
  return useQuery({
    queryKey: queryKeys.seriesList(filters),
    queryFn: () => jsonFetch<SeriesListView>(`/series${buildQuery(filters)}`),
    placeholderData: keepPreviousData,
  });
}

/** Infinite-scroll variant of `useSeriesList`. The `cursor` field on
 *  filters is overridden per page; pass everything else (sort, library,
 *  q) at the call site. `enabled: false` keeps the hook mounted but
 *  idle — used by the library grid to switch modes without tearing
 *  down a hook (would violate the rules of hooks). */
export function useSeriesListInfinite(
  filters: SeriesListFilters = {},
  options: { enabled?: boolean } = {},
) {
  // Strip the caller-passed `cursor` (if any); the infinite-query owns it.
  const rest = stripCursor(filters);
  return useInfiniteQuery({
    queryKey: queryKeys.seriesList(filters),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      jsonFetch<SeriesListView>(
        `/series${buildQuery({ ...rest, cursor: pageParam })}`,
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: options.enabled ?? true,
    // Keep loaded pages cached longer than the 5-min default so a
    // browse → open → back round-trip restores the full windowed grid
    // (and its scroll position) from cache instead of refetching page 1
    // and collapsing the content height (audit B15 / G1).
    gcTime: 30 * 60_000,
  });
}

/** Infinite-scroll cross-library issues listing. Same shape as
 *  `useSeriesListInfinite` but hits `/issues`, which exposes the
 *  metadata-suite filters at the issue level (no `status`, plus
 *  `year`/`page_count`/`user_rating` sorts). */
export function useIssuesCrossListInfinite(
  filters: IssuesCrossListFilters = {},
  options: { enabled?: boolean } = {},
) {
  const rest = stripCursor(filters);
  return useInfiniteQuery({
    queryKey: queryKeys.issuesCrossList(filters),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      jsonFetch<IssueListView>(
        `/issues${buildQuery({ ...rest, cursor: pageParam })}`,
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: options.enabled ?? true,
    // See `useSeriesListInfinite` — keep pages cached for back-nav grid
    // + scroll restoration (audit B15 / G1).
    gcTime: 30 * 60_000,
  });
}

/** Cross-library issue search backed by `GET /issues/search`. Used by
 *  the global search modal + `/search` page. Returns up to `limit`
 *  hits ranked by tsvector similarity. */
export function useIssueSearch(filters: IssueSearchFilters = {}) {
  const enabled = !!filters.q && filters.q.length >= 2;
  return useQuery({
    queryKey: queryKeys.issueSearch(filters),
    queryFn: () =>
      jsonFetch<IssueSearchView>(`/issues/search${buildQuery(filters)}`),
    enabled,
    placeholderData: keepPreviousData,
  });
}

/** Cross-credits people search backed by `GET /people`. */
export function usePeopleSearch(filters: PeopleSearchFilters = {}) {
  const enabled = !!filters.q && filters.q.length >= 2;
  return useQuery({
    queryKey: queryKeys.peopleSearch(filters),
    queryFn: () => jsonFetch<PeopleListView>(`/people${buildQuery(filters)}`),
    enabled,
    placeholderData: keepPreviousData,
  });
}

/** Global-search marker hook. Same 2-char min as the other category
 *  searches so the modal doesn't fire a request after a single
 *  keystroke. Per-caller scoped via the `/me/...` route prefix. */
export function useMarkerSearch(filters: MarkerSearchFilters = {}) {
  const enabled = !!filters.q && filters.q.length >= 2;
  return useQuery({
    queryKey: queryKeys.markerSearch(filters),
    queryFn: () =>
      jsonFetch<MarkerSearchView>(`/me/markers/search${buildQuery(filters)}`),
    enabled,
    placeholderData: keepPreviousData,
  });
}

// ---------- Sidebar layout (navigation customization M1) ----------

/** Client-side fetch of the resolved sidebar layout. The
 *  `/settings/navigation` page uses this so drag-reorder + hide-toggle
 *  can update the cache optimistically; the main app shell still reads
 *  the layout server-side in `[locale]/(library)/layout.tsx`. */
export function useSidebarLayout() {
  return useQuery({
    queryKey: queryKeys.sidebarLayout,
    queryFn: () => jsonFetch<SidebarLayoutView>("/me/sidebar-layout"),
  });
}

// ---------- Saved views (M5) ----------

export function useSavedViews(filters: SavedViewListFilters = {}) {
  const params: Record<string, string> = {};
  if (typeof filters.pinned === "boolean") {
    params.pinned = String(filters.pinned);
  }
  if (filters.pinnedOn) {
    params.pinned_on = filters.pinnedOn;
  }
  return useQuery({
    queryKey: queryKeys.savedViews(filters),
    queryFn: () =>
      jsonFetch<SavedViewListView>(`/me/saved-views${buildQuery(params)}`),
    placeholderData: keepPreviousData,
    // pinnedOn is gated on resolving the system page id; skip the
    // request until the caller has it.
    enabled: filters.pinnedOn === undefined || filters.pinnedOn.length > 0,
  });
}

/** Multi-page rails: list the user's pages (system Home + custom).
 *  Drives the `/pages/[slug]` route resolver and the upcoming multi-pin
 *  picker. Stable cache — `useMePages()` is safe to call from any tree
 *  without coordination. */
export function useMePages() {
  return useQuery({
    queryKey: queryKeys.mePages,
    // Server returns `CursorPage<PageView>` (audit-remediation M4 uniform
    // envelope); pages are bounded per-user so `next_cursor` is always
    // null. Unwrap to `PageView[]` here so call sites keep their
    // `.map(...)` shape. When the cap eventually lifts and pagination
    // kicks in, swap to `useInfiniteQuery`.
    queryFn: async () => {
      const page = await jsonFetch<PageListView>("/me/pages");
      return page.items;
    },
    placeholderData: keepPreviousData,
  });
}

export function useSavedView(id: string) {
  return useQuery({
    queryKey: queryKeys.savedView(id),
    queryFn: async () => {
      const list = await jsonFetch<SavedViewListView>("/me/saved-views");
      const found = list.items.find((v) => v.id === id);
      if (!found) throw new HttpError(404, `saved view ${id} not found`);
      return found;
    },
    enabled: !!id,
  });
}

/** Filter views always return a series list. CBL views currently stub
 *  to an empty series list; M6 will move CBL results to a dedicated
 *  endpoint. */
export function useSavedViewResults(id: string, cursor?: string) {
  const qs = buildQuery(cursor ? { cursor } : {});
  return useQuery({
    queryKey: queryKeys.savedViewResults(id, cursor),
    queryFn: () =>
      jsonFetch<SeriesListView>(`/me/saved-views/${id}/results${qs}`),
    enabled: !!id,
  });
}

/** Cursor-paginated filter-view results — used by the detail page's
 *  series grid. The "saved-views" prefix is shared with the
 *  single-fetch hook so blanket invalidations from the edit/pin
 *  mutations refresh both. */
export function useSavedViewResultsInfinite(id: string) {
  return useInfiniteQuery({
    queryKey: queryKeys.savedViewResultsInfinite(id),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      jsonFetch<SeriesListView>(
        `/me/saved-views/${id}/results${buildQuery({ cursor: pageParam })}`,
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: !!id,
  });
}

/** POST /me/saved-views/preview — stateless filter-DSL preview. */
export async function previewSavedView(
  req: PreviewReq,
): Promise<SeriesListView> {
  const csrf = getCsrfToken();
  const res = await apiFetch("/me/saved-views/preview", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "application/json",
      ...(csrf ? { "X-CSRF-Token": csrf } : {}),
    },
    body: JSON.stringify(req),
  });
  if (!res.ok) {
    let detail = `/me/saved-views/preview → ${res.status}`;
    try {
      const body = await res.json();
      detail = body?.error?.message ?? detail;
    } catch {
      /* ignore */
    }
    throw new HttpError(res.status, detail);
  }
  return (await res.json()) as SeriesListView;
}

// ---------- CBL lists (M5 read-only hooks; M6 builds on these) ----------

export function useCblLists() {
  return useQuery({
    queryKey: queryKeys.cblLists,
    queryFn: () => jsonFetch<CblListListView>("/me/cbl-lists"),
  });
}

export function useCblList(id: string) {
  return useQuery({
    queryKey: queryKeys.cblList(id),
    queryFn: () => jsonFetch<CblDetailView>(`/me/cbl-lists/${id}`),
    enabled: !!id,
  });
}

/** Cursor-paginated walk over a CBL list's entries. Each item carries
 *  the entry **and** its hydrated `IssueSummaryView` (for matched
 *  rows), so the consumption grid + management sheet don't need a
 *  second `/issues` round-trip. Status filter is server-side, so the
 *  Resolution tab can stream only bad matches.
 *
 *  Replaces the old "embed `entries[]` in `/me/cbl-lists/{id}` and
 *  filter client-side" pattern, which silently capped lists at 500. */
export function useCblListEntriesInfinite(
  id: string,
  filters: CblEntriesFilters = {},
) {
  const params: Record<string, string | number> = {};
  if (filters.status) params.status = filters.status;
  if (filters.limit != null) params.limit = filters.limit;
  if (filters.q) params.q = filters.q;
  return useInfiniteQuery({
    queryKey: queryKeys.cblListEntries(id, filters),
    enabled: !!id,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      jsonFetch<CblEntryListView>(
        `/me/cbl-lists/${id}/entries${buildQuery({ ...params, cursor: pageParam })}`,
      ),
    getNextPageParam: cblEntriesNextPage,
  });
}

/** Pure `getNextPageParam` callback for `useCblListEntriesInfinite`.
 *  Extracted so the cursor contract is unit-testable — a refactor
 *  that accidentally drops the null-check (the original
 *  list-pagination-completeness bug-class) is caught by
 *  `web/tests/api/cbl-entries-next-page.test.ts` rather than
 *  shipping. The shape returned here flows directly into TanStack's
 *  page-walking loop: `undefined` ⇒ "no more pages, stop". */
export function cblEntriesNextPage(last: CblEntryListView): string | undefined {
  return last.next_cursor ?? undefined;
}

/** Matched issues from a CBL list in `position` order. Backed by
 *  `/me/cbl-lists/{id}/issues`; pages with `limit` + `offset`. */
export function useCblListIssues(
  id: string,
  opts?: { limit?: number; offset?: number },
) {
  const params: Record<string, number> = {};
  if (opts?.limit) params.limit = opts.limit;
  if (opts?.offset) params.offset = opts.offset;
  return useQuery({
    queryKey: queryKeys.cblListIssues(id, opts),
    queryFn: () =>
      jsonFetch<IssueListView>(
        `/me/cbl-lists/${id}/issues${buildQuery(params)}`,
      ),
    enabled: !!id,
  });
}

/** Progress-centered window of a CBL — slice of matched entries anchored
 *  on the user's next-unfinished one. `before` defaults to 3 (already-
 *  read context), `after` to 8 (upcoming). */
export function useCblListWindow(
  id: string,
  opts?: { before?: number; after?: number },
) {
  const params: Record<string, number> = {};
  if (opts?.before != null) params.before = opts.before;
  if (opts?.after != null) params.after = opts.after;
  return useQuery({
    queryKey: queryKeys.cblListWindow(id, opts),
    queryFn: () =>
      jsonFetch<CblWindowView>(
        `/me/cbl-lists/${id}/window${buildQuery(params)}`,
      ),
    enabled: !!id,
  });
}

/** Page param shape for `useCblListWindowInfinite`. The initial fetch
 *  carries no cursor; subsequent forward/backward pages carry the
 *  position cursor returned by the previous edge page. */
type CblWindowPageParam =
  | { direction: "initial" }
  | { direction: "after"; cursor: number }
  | { direction: "before"; cursor: number };

/** Bidirectional infinite-scroll variant of `useCblListWindow`. The
 *  initial page anchors on the user's next-unfinished entry (same
 *  `before` / `after` defaults as `useCblListWindow`); subsequent
 *  pages walk forward or backward from the cursor edges so a rail can
 *  load surrounding entries asynchronously as the user scrolls
 *  without disturbing the already-rendered anchor band. */
export function useCblListWindowInfinite(
  id: string,
  opts?: { before?: number; after?: number; limit?: number },
) {
  const initialParams: Record<string, number> = {};
  if (opts?.before != null) initialParams.before = opts.before;
  if (opts?.after != null) initialParams.after = opts.after;
  const pageLimit = opts?.limit ?? 24;
  return useInfiniteQuery({
    queryKey: queryKeys.cblListWindowInfinite(id, opts),
    enabled: !!id,
    initialPageParam: { direction: "initial" } as CblWindowPageParam,
    queryFn: ({ pageParam }) => {
      const params: Record<string, string | number> = {};
      if (pageParam.direction === "initial") {
        Object.assign(params, initialParams);
      } else {
        params.direction = pageParam.direction;
        params.cursor = pageParam.cursor;
        params.limit = pageLimit;
      }
      return jsonFetch<CblWindowPageView>(
        `/me/cbl-lists/${id}/window-paginated${buildQuery(params)}`,
      );
    },
    getNextPageParam: (last) =>
      last.has_more_after && last.max_position != null
        ? ({ direction: "after", cursor: last.max_position } as const)
        : undefined,
    getPreviousPageParam: (first) =>
      first.has_more_before && first.min_position != null
        ? ({ direction: "before", cursor: first.min_position } as const)
        : undefined,
  });
}

export function useCblRefreshLog(id: string, opts?: { limit?: number }) {
  const qs = buildQuery(opts?.limit ? { limit: opts.limit } : {});
  return useQuery({
    queryKey: queryKeys.cblRefreshLog(id),
    queryFn: () =>
      jsonFetch<RefreshLogListView>(`/me/cbl-lists/${id}/refresh-log${qs}`),
    enabled: !!id,
  });
}

// ---------- Markers (markers + collections M5) ----------

export type MarkerListFilters = {
  kind?: MarkerKind;
  issue_id?: string;
  q?: string;
  is_favorite?: boolean;
  /** Comma-separated tag list. Empty / undefined skips the filter. */
  tags?: string;
  /** `"all"` (default) — markers must have every selected tag.
   *  `"any"` — markers need at least one. */
  tag_match?: MarkerTagMatch;
  cursor?: string;
  limit?: number;
};

/** Cursor-paginated global feed of the caller's markers. Filters by
 *  kind, issue, and free-text search against `body` + `selection.text`.
 *  Single-page variant — kept for surfaces that need a one-shot fetch
 *  with a known finite count. The Bookmarks index uses
 *  [`useMarkersInfinite`](#useMarkersInfinite) instead so no marker
 *  is ever hidden behind a hard cap. */
export function useMarkers(filters: MarkerListFilters = {}) {
  const params: Record<string, string> = {};
  if (filters.kind) params.kind = filters.kind;
  if (filters.issue_id) params.issue_id = filters.issue_id;
  if (filters.q) params.q = filters.q;
  if (filters.is_favorite === true) params.is_favorite = "true";
  if (filters.tags) params.tags = filters.tags;
  if (filters.tag_match) params.tag_match = filters.tag_match;
  if (filters.cursor) params.cursor = filters.cursor;
  if (filters.limit) params.limit = String(filters.limit);
  return useQuery({
    queryKey: queryKeys.markers(filters),
    queryFn: () =>
      jsonFetch<MarkerListView>(`/me/markers${buildQuery(params)}`),
    placeholderData: keepPreviousData,
  });
}

/** Infinite-scroll variant of `useMarkers`. Strip caller `cursor` (if
 *  any) so the query owns paging; everything else flows through as
 *  server-side filters. */
export function useMarkersInfinite(filters: MarkerListFilters = {}) {
  const rest = stripCursor(filters);
  const params: Record<string, string> = {};
  if (rest.kind) params.kind = rest.kind;
  if (rest.issue_id) params.issue_id = rest.issue_id;
  if (rest.q) params.q = rest.q;
  if (rest.is_favorite === true) params.is_favorite = "true";
  if (rest.tags) params.tags = rest.tags;
  if (rest.tag_match) params.tag_match = rest.tag_match;
  if (rest.limit) params.limit = String(rest.limit);
  return useInfiniteQuery({
    queryKey: queryKeys.markers(filters),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      jsonFetch<MarkerListView>(
        `/me/markers${buildQuery({ ...params, cursor: pageParam })}`,
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

/** Distinct tag rollup driving the bookmarks tag-chip filter. Cached
 *  60s; create/update/delete marker mutations invalidate it. */
export function useMarkerTags() {
  return useQuery({
    queryKey: queryKeys.markerTags,
    queryFn: () => jsonFetch<MarkerTagsView>("/me/markers/tags"),
    staleTime: 60_000,
  });
}

/** Reader overlay's one-shot fetch — every marker the caller has on
 *  this issue, across all pages, no pagination. */
export function useIssueMarkers(issueId: string) {
  return useQuery({
    queryKey: queryKeys.issueMarkers(issueId),
    queryFn: () => jsonFetch<IssueMarkersView>(`/me/issues/${issueId}/markers`),
    enabled: !!issueId,
  });
}

/** Detected speech-bubble regions for one page, percent coords
 *  (OCR rework 1.0). Drives the tappable bubble outlines the reader
 *  shows in text-capture mode.
 *
 *  - `staleTime: Infinity` — the server caches detections per
 *    content-hash; within a session a page's regions never change,
 *    and each fetch warms the server-side cache that makes every
 *    subsequent bubble OCR on the page recognize-only.
 *  - `retry: false` — a cache miss costs a full detector inference
 *    server-side (seconds, tens of seconds on weak hosts);
 *    auto-retry would stack more runs onto a struggling box. The
 *    overlay treats errors as "no outlines" and drag still works. */
export function useIssuePageTextRegions(
  issueId: string,
  page: number,
  enabled: boolean,
) {
  return useQuery({
    queryKey: queryKeys.issuePageTextRegions(issueId, page),
    queryFn: () =>
      jsonFetch<TextRegionsView>(
        `/me/issues/${issueId}/pages/${page}/text-regions`,
      ),
    staleTime: Infinity,
    retry: false,
    enabled: enabled && !!issueId,
  });
}

/** Cheap `SELECT COUNT(*)` for the Bookmarks sidebar badge. Cached
 *  60s so navigation hover doesn't refetch. The mutate hooks invalidate
 *  this key on create/delete so the badge nudges in real time. The
 *  `enabled` opt lets callers gate the query off when the user has the
 *  badge disabled — saves the round-trip on every page load. */
export function useMarkerCount({ enabled = true }: { enabled?: boolean } = {}) {
  return useQuery({
    queryKey: queryKeys.markerCount,
    queryFn: () => jsonFetch<MarkerCountView>("/me/markers/count"),
    staleTime: 60_000,
    enabled,
  });
}

// ---------- Collections (markers + collections M2) ----------

/** List all user collections. The server lazy-seeds Want to Read on
 *  first call, so a fresh user will always see at least one row. */
export function useCollections() {
  return useQuery({
    queryKey: queryKeys.collections,
    queryFn: () => jsonFetch<SavedViewView[]>("/me/collections"),
  });
}

/** Single-collection detail. Reads from the cached list (avoids a
 *  dedicated endpoint round-trip — `/me/collections` is cheap). */
export function useCollection(id: string) {
  return useQuery({
    queryKey: queryKeys.collection(id),
    queryFn: async () => {
      const list = await jsonFetch<SavedViewView[]>("/me/collections");
      const found = list.find((c) => c.id === id);
      if (!found) throw new HttpError(404, `collection ${id} not found`);
      return found;
    },
    enabled: !!id,
  });
}

/** Hydrated mixed-series-and-issue entries for a collection, in
 *  position order. Pages via opaque cursor; `total` is first-page-only.
 *  Single-shot variant — keep for surfaces (e.g. rail previews) that
 *  want only the first page. The collection detail page uses
 *  [`useCollectionEntriesInfinite`](#useCollectionEntriesInfinite) so
 *  reorder semantics see the full list. */
export function useCollectionEntries(
  id: string,
  opts?: { cursor?: string; limit?: number },
) {
  const params: Record<string, string> = {};
  if (opts?.cursor) params.cursor = opts.cursor;
  if (opts?.limit) params.limit = String(opts.limit);
  return useQuery({
    queryKey: queryKeys.collectionEntries(id, opts),
    queryFn: () =>
      jsonFetch<CollectionEntriesView>(
        `/me/collections/${id}/entries${buildQuery(params)}`,
      ),
    enabled: !!id,
  });
}

/** Cursor-paginated walk over every collection entry. The detail page
 *  uses this with an auto-walk effect so reorder semantics see the
 *  full list — sending `entry_ids` from a half-loaded view would wipe
 *  the tail. */
export function useCollectionEntriesInfinite(id: string) {
  return useInfiniteQuery({
    queryKey: queryKeys.collectionEntriesInfinite(id),
    enabled: !!id,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      jsonFetch<CollectionEntriesView>(
        `/me/collections/${id}/entries${buildQuery({
          cursor: pageParam,
          limit: 200,
        })}`,
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

// ---------- Catalog (M6) ----------

export function useCatalogSources() {
  return useQuery({
    queryKey: queryKeys.catalogSources,
    queryFn: () => jsonFetch<CatalogSourceListView>("/catalog/sources"),
  });
}

export function useCatalogEntries(sourceId: string) {
  return useQuery({
    queryKey: queryKeys.catalogEntries(sourceId),
    queryFn: () =>
      jsonFetch<CatalogEntriesView>(`/catalog/sources/${sourceId}/lists`),
    enabled: !!sourceId,
    staleTime: 60_000,
  });
}

export function useSeriesIssuesInfinite(
  seriesId: string,
  filters: IssueListFilters = {},
) {
  // Strip the caller-passed `cursor` (if any); the infinite-query owns it.
  const rest = stripCursor(filters);
  return useInfiniteQuery({
    queryKey: queryKeys.seriesIssues(seriesId, filters),
    enabled: !!seriesId,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      jsonFetch<IssueListView>(
        `/series/${seriesId}/issues${buildQuery({ ...rest, cursor: pageParam })}`,
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
  });
}

// ---------- Home rails (Continue Reading / On Deck) ----------

/** Issues the user has started but not finished, most-recent-activity first.
 *  Drives the Continue Reading rail on the home page. */
export function useContinueReading() {
  return useQuery({
    queryKey: queryKeys.continueReading,
    queryFn: () => jsonFetch<ContinueReadingView>("/me/continue-reading"),
  });
}

/** "What's next" suggestions across series + CBLs the user is working
 *  through. Drives the On Deck rail on the home page. */
export function useOnDeck() {
  return useQuery({
    queryKey: queryKeys.onDeck,
    queryFn: () => jsonFetch<OnDeckView>("/me/on-deck"),
  });
}

/** Per-issue "what should I read next?" resolver — drives the reader's
 *  `Shift+N` keybind and the end-of-issue card (M4). Picks CBL > series
 *  > none. Pass the saved-view id of the CBL the user is reading through
 *  via `cblSavedViewId`; omit it for series-only resolution.
 *
 *  Cached for 5 minutes — progress flips that move the resolved target
 *  show up via the existing progress-mutation invalidation chain on the
 *  *next* read; not worth a tighter loop since the reader is mostly
 *  read-only during a single session. */
export function useNextUp(
  issueId: string,
  cblSavedViewId?: string | null,
  opts?: { enabled?: boolean },
) {
  const enabled = (opts?.enabled ?? true) && issueId.length > 0;
  return useQuery({
    queryKey: queryKeys.nextUp(issueId, cblSavedViewId),
    queryFn: () => {
      const qs = cblSavedViewId
        ? `?cbl=${encodeURIComponent(cblSavedViewId)}`
        : "";
      return jsonFetch<NextUpView>(
        `/issues/${encodeURIComponent(issueId)}/next-up${qs}`,
      );
    },
    enabled,
    staleTime: 5 * 60 * 1000,
  });
}

/** Per-issue "what came before?" resolver — sibling to `useNextUp` for
 *  the reader's `Shift+P` keybind. Picks CBL > series > none. Pure
 *  sequential navigation; doesn't filter by finished state (a user
 *  pressing `Shift+P` is asking to back up one step, not to find an
 *  unread issue). `fallback_suggestion` is never populated for prev. */
export function usePrevUp(
  issueId: string,
  cblSavedViewId?: string | null,
  opts?: { enabled?: boolean },
) {
  const enabled = (opts?.enabled ?? true) && issueId.length > 0;
  return useQuery({
    queryKey: queryKeys.prevUp(issueId, cblSavedViewId),
    queryFn: () => {
      const qs = cblSavedViewId
        ? `?cbl=${encodeURIComponent(cblSavedViewId)}`
        : "";
      return jsonFetch<NextUpView>(
        `/issues/${encodeURIComponent(issueId)}/prev-up${qs}`,
      );
    },
    enabled,
    staleTime: 5 * 60 * 1000,
  });
}

/** Full per-user progress list. Powers the finished/in-progress badges
 *  on issue covers across library/series/saved-view surfaces. One
 *  shared query — every IssueCard subscribes by the same key, so the
 *  network call fans out across the page. Mutations in
 *  [mutations.ts](./mutations.ts) invalidate this. */
export function useUserProgress() {
  return useQuery({
    queryKey: queryKeys.userProgress,
    queryFn: async () => {
      const resp = await jsonFetch<{ records: ProgressView[] }>("/progress");
      return new Map(resp.records.map((r) => [r.issue_id, r]));
    },
    staleTime: 30_000,
  });
}

// ───────── metadata-providers-1.0 ─────────

import type {
  BatchListResp,
  BatchStatusResp,
  CandidatesResp,
  CollectionReportView,
  CompositeDiffResp,
  DiffResp,
  ExternalIdsListResp,
  IssueCoversResp,
  SyncStatusResp,
} from "./types";

/** Polls a single search run until it finalizes. `enabled=false`
 *  while no `runId` exists; `refetchInterval` short-circuits to
 *  `false` once status hits `completed`/`failed`/`awaiting_quota` so
 *  the polling loop stops on its own. */
export function useMetadataCandidatesSeries(
  seriesSlug: string,
  runId: string | null,
) {
  return useQuery({
    queryKey: queryKeys.metadataCandidatesSeries(seriesSlug, runId),
    queryFn: () =>
      jsonFetch<CandidatesResp>(
        runId
          ? `/series/${encodeURIComponent(seriesSlug)}/metadata/candidates?run_id=${encodeURIComponent(runId)}`
          : `/series/${encodeURIComponent(seriesSlug)}/metadata/candidates`,
      ),
    enabled: !!seriesSlug && !!runId,
    refetchInterval: (q) => {
      const d = q.state.data;
      if (!d) return 1500;
      return d.status === "completed" ||
        d.status === "failed" ||
        d.status === "awaiting_quota"
        ? false
        : 1500;
    },
  });
}

export function useMetadataCandidatesIssue(
  seriesSlug: string,
  issueSlug: string,
  runId: string | null,
) {
  return useQuery({
    queryKey: queryKeys.metadataCandidatesIssue(seriesSlug, issueSlug, runId),
    queryFn: () =>
      jsonFetch<CandidatesResp>(
        runId
          ? `/series/${encodeURIComponent(seriesSlug)}/issues/${encodeURIComponent(issueSlug)}/metadata/candidates?run_id=${encodeURIComponent(runId)}`
          : `/series/${encodeURIComponent(seriesSlug)}/issues/${encodeURIComponent(issueSlug)}/metadata/candidates`,
      ),
    enabled: !!seriesSlug && !!issueSlug && !!runId,
    refetchInterval: (q) => {
      const d = q.state.data;
      if (!d) return 1500;
      return d.status === "completed" ||
        d.status === "failed" ||
        d.status === "awaiting_quota"
        ? false
        : 1500;
    },
  });
}

/** M5 preview pane — fetches the per-field diff for one candidate so
 *  the dialog can render a "what will change" preview before the
 *  user commits. `staleTime` is short (10s) so toggling the mode
 *  picker / override toggle re-runs the server-side classifier on
 *  the same cached provider detail (the apply pipeline's cache layer
 *  carries the cost). */
export function useMetadataProposedDiffSeries(
  seriesSlug: string,
  runId: string | null,
  ordinal: number | null,
  mode: "fill_missing" | "replace_all",
  overrideUserEdits: boolean,
) {
  return useQuery({
    queryKey: queryKeys.metadataDiffSeries(
      seriesSlug,
      runId ?? "",
      ordinal ?? -1,
      mode,
      overrideUserEdits,
    ),
    queryFn: () => {
      const qs = new URLSearchParams({
        run_id: runId!,
        ordinal: String(ordinal!),
        mode,
        override_user_edits: String(overrideUserEdits),
      });
      return jsonFetch<DiffResp>(
        `/series/${encodeURIComponent(seriesSlug)}/metadata/proposed-diff?${qs.toString()}`,
      );
    },
    enabled: !!seriesSlug && !!runId && ordinal != null && ordinal >= 0,
    staleTime: 10_000,
  });
}

export function useMetadataProposedDiffIssue(
  seriesSlug: string,
  issueSlug: string,
  runId: string | null,
  ordinal: number | null,
  mode: "fill_missing" | "replace_all",
  overrideUserEdits: boolean,
) {
  return useQuery({
    queryKey: queryKeys.metadataDiffIssue(
      seriesSlug,
      issueSlug,
      runId ?? "",
      ordinal ?? -1,
      mode,
      overrideUserEdits,
    ),
    queryFn: () => {
      const qs = new URLSearchParams({
        run_id: runId!,
        ordinal: String(ordinal!),
        mode,
        override_user_edits: String(overrideUserEdits),
      });
      return jsonFetch<DiffResp>(
        `/series/${encodeURIComponent(seriesSlug)}/issues/${encodeURIComponent(issueSlug)}/metadata/proposed-diff?${qs.toString()}`,
      );
    },
    enabled:
      !!seriesSlug && !!issueSlug && !!runId && ordinal != null && ordinal >= 0,
    staleTime: 10_000,
  });
}

function compositeDiffQs(
  runId: string,
  mode: string,
  overrideUserEdits: boolean,
  include: number[],
): string {
  const qs = new URLSearchParams({
    run_id: runId,
    mode,
    override_user_edits: String(overrideUserEdits),
  });
  // serde_urlencoded (axum Query) can't decode repeated keys into a Vec,
  // so the ordinals go as one comma-separated value.
  if (include.length) qs.set("include", include.join(","));
  return qs.toString();
}

export function useMetadataCompositeDiffSeries(
  seriesSlug: string,
  runId: string | null,
  mode: "fill_missing" | "replace_all",
  overrideUserEdits: boolean,
  include: number[],
) {
  return useQuery({
    queryKey: queryKeys.metadataCompositeDiffSeries(
      seriesSlug,
      runId ?? "",
      mode,
      overrideUserEdits,
      include,
    ),
    queryFn: () =>
      jsonFetch<CompositeDiffResp>(
        `/series/${encodeURIComponent(seriesSlug)}/metadata/composite-diff?${compositeDiffQs(runId!, mode, overrideUserEdits, include)}`,
      ),
    enabled: !!seriesSlug && !!runId,
    staleTime: 10_000,
  });
}

export function useMetadataCompositeDiffIssue(
  seriesSlug: string,
  issueSlug: string,
  runId: string | null,
  mode: "fill_missing" | "replace_all",
  overrideUserEdits: boolean,
  include: number[],
) {
  return useQuery({
    queryKey: queryKeys.metadataCompositeDiffIssue(
      seriesSlug,
      issueSlug,
      runId ?? "",
      mode,
      overrideUserEdits,
      include,
    ),
    queryFn: () =>
      jsonFetch<CompositeDiffResp>(
        `/series/${encodeURIComponent(seriesSlug)}/issues/${encodeURIComponent(issueSlug)}/metadata/composite-diff?${compositeDiffQs(runId!, mode, overrideUserEdits, include)}`,
      ),
    enabled: !!seriesSlug && !!issueSlug && !!runId,
    staleTime: 10_000,
  });
}

export function useMetadataSyncStatus(seriesSlug: string) {
  return useQuery({
    queryKey: queryKeys.metadataSyncStatusSeries(seriesSlug),
    queryFn: () =>
      jsonFetch<SyncStatusResp>(
        `/series/${encodeURIComponent(seriesSlug)}/metadata/status`,
      ),
    enabled: !!seriesSlug,
    staleTime: 30_000,
  });
}

/** Collection-completeness report: owned vs. expected issue counts plus the
 *  inferred missing main-run numbers. Single-shot (not a cursor list), so a
 *  plain `useQuery` is correct. */
export function useSeriesCollection(seriesSlug: string) {
  return useQuery({
    queryKey: queryKeys.seriesCollection(seriesSlug),
    queryFn: () =>
      jsonFetch<CollectionReportView>(
        `/series/${encodeURIComponent(seriesSlug)}/collection`,
      ),
    enabled: !!seriesSlug,
    staleTime: 30_000,
  });
}

/** Bulk-metadata batch status — polls every 2s while children are still in
 *  flight or parked, then settles. Powers the Review tab progress + queue. */
export function useMetadataBatch(batchId: string | null) {
  return useQuery({
    queryKey: queryKeys.metadataBatch(batchId ?? ""),
    queryFn: () =>
      jsonFetch<BatchStatusResp>(
        `/metadata/batch/${encodeURIComponent(batchId ?? "")}`,
      ),
    enabled: !!batchId,
    refetchInterval: (q) => {
      const s = q.state.data?.status;
      return s === "running" || s === "awaiting_quota" ? 2000 : false;
    },
  });
}

/** Recent bulk-metadata batches for the Review tab picker. */
export function useMetadataBatches() {
  return useQuery({
    queryKey: queryKeys.metadataBatches,
    queryFn: () => jsonFetch<BatchListResp>(`/metadata/batches`),
    staleTime: 10_000,
  });
}

export function useExternalIdsSeries(seriesSlug: string) {
  return useQuery({
    queryKey: queryKeys.externalIdsSeries(seriesSlug),
    queryFn: () =>
      jsonFetch<ExternalIdsListResp>(
        `/series/${encodeURIComponent(seriesSlug)}/external-ids`,
      ),
    enabled: !!seriesSlug,
    staleTime: 30_000,
  });
}

export function useExternalIdsIssue(seriesSlug: string, issueSlug: string) {
  return useQuery({
    queryKey: queryKeys.externalIdsIssue(seriesSlug, issueSlug),
    queryFn: () =>
      jsonFetch<ExternalIdsListResp>(
        `/series/${encodeURIComponent(seriesSlug)}/issues/${encodeURIComponent(issueSlug)}/external-ids`,
      ),
    enabled: !!seriesSlug && !!issueSlug,
    staleTime: 30_000,
  });
}

export function useIssueCovers(issueId: string) {
  return useQuery({
    queryKey: queryKeys.issueCovers(issueId),
    queryFn: () =>
      jsonFetch<IssueCoversResp>(
        `/issues/${encodeURIComponent(issueId)}/covers`,
      ),
    enabled: !!issueId,
    staleTime: 60_000,
  });
}

// ───────── M6 admin surface ─────────

import type {
  DashboardResp,
  MatchQualityResp,
  ProvidersListResp,
  RunDetailResp,
  RunsListResp,
  AutoSyncedResp,
} from "./types";

export function useAdminMetadataDashboard() {
  return useQuery({
    queryKey: queryKeys.adminMetadataDashboard,
    queryFn: () => jsonFetch<DashboardResp>(`/admin/metadata/dashboard`),
    staleTime: 30_000,
    // No `refetchInterval`: the scan-events WS invalidates this on every
    // `metadata.applied` (and `lagged` recovery), so the 60s poll was
    // redundant (audit G5 / G8).
  });
}

export function useAdminMetadataMatchQuality() {
  return useQuery({
    queryKey: queryKeys.adminMetadataMatchQuality,
    queryFn: () => jsonFetch<MatchQualityResp>(`/admin/metadata/match-quality`),
    staleTime: 30_000,
    // No `refetchInterval`: invalidated by the metadata.applied WS event.
  });
}

export function useAdminMetadataAutoSynced() {
  return useQuery({
    queryKey: queryKeys.adminMetadataAutoSynced,
    queryFn: () => jsonFetch<AutoSyncedResp>(`/admin/metadata/auto-synced`),
    staleTime: 30_000,
  });
}

export function useAdminMetadataProviders() {
  return useQuery({
    queryKey: queryKeys.adminMetadataProviders,
    queryFn: () => jsonFetch<ProvidersListResp>(`/admin/metadata/providers`),
    staleTime: 30_000,
  });
}

export function useAdminMetadataRuns(filters: {
  library_id?: string;
  scope?: string;
  status?: string;
  before?: string;
}) {
  return useQuery({
    queryKey: queryKeys.adminMetadataRuns(filters),
    queryFn: () => {
      const params = new URLSearchParams();
      if (filters.library_id) params.set("library_id", filters.library_id);
      if (filters.scope) params.set("scope", filters.scope);
      if (filters.status) params.set("status", filters.status);
      if (filters.before) params.set("before", filters.before);
      const qs = params.toString();
      return jsonFetch<RunsListResp>(
        `/admin/metadata/runs${qs ? `?${qs}` : ""}`,
      );
    },
    staleTime: 15_000,
  });
}

export function useAdminMetadataRun(id: string) {
  return useQuery({
    queryKey: queryKeys.adminMetadataRun(id),
    queryFn: () =>
      jsonFetch<RunDetailResp>(
        `/admin/metadata/runs/${encodeURIComponent(id)}`,
      ),
    enabled: !!id,
    staleTime: 15_000,
  });
}
