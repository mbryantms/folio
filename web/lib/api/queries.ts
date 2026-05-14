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
import type {
  ActivityKind,
  ActivityListView,
  CollectionEntriesView,
  IssueMarkersView,
  MarkerKind,
  MarkerCountView,
  MarkerListView,
  MarkerTagMatch,
  MarkerTagsView,
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
  CblListListView,
  CblWindowView,
  ContinueReadingView,
  HealthIssueView,
  OnDeckView,
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
  ServerInfoView,
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

export const queryKeys = {
  me: ["auth", "me"] as const,
  libraries: ["libraries"] as const,
  library: (id: string) => ["libraries", id] as const,
  health: (libraryId: string) => ["libraries", libraryId, "health"] as const,
  scanRuns: (libraryId: string, kind?: string) =>
    ["libraries", libraryId, "scan-runs", kind ?? "all"] as const,
  /** Prefix that matches every `scanRuns(libraryId, *)` variant — used by
   *  cache invalidations that don't care which kind filter is active. */
  scanRunsAll: (libraryId: string) =>
    ["libraries", libraryId, "scan-runs"] as const,
  scanPreview: (libraryId: string) =>
    ["libraries", libraryId, "scan-preview"] as const,
  removed: (libraryId: string) => ["libraries", libraryId, "removed"] as const,
  series: (id: string) => ["series", id] as const,
  seriesList: (filters: SeriesListFilters) =>
    ["series", "list", filters] as const,
  seriesIssues: (seriesId: string, filters: IssueListFilters) =>
    ["series", seriesId, "issues", filters] as const,
  /** Cross-library issue search (`/issues/search`). Backs the global
   *  search modal + page's Issues section. */
  issueSearch: (filters: IssueSearchFilters) =>
    ["issues", "search", filters] as const,
  /** Cross-credits people search (`/people`). Global-search M4. */
  peopleSearch: (filters: PeopleSearchFilters) =>
    ["people", "search", filters] as const,
  queueDepth: ["admin", "queue-depth"] as const,
  thumbnailsStatus: (libraryId: string) =>
    ["admin", "libraries", libraryId, "thumbnails-status"] as const,
  thumbnailsSettings: (libraryId: string) =>
    ["admin", "libraries", libraryId, "thumbnails-settings"] as const,
  users: (filters: UserListFilters) => ["admin", "users", filters] as const,
  user: (id: string) => ["admin", "users", id] as const,
  audit: (filters: AuditFilters) => ["admin", "audit", filters] as const,
  readingSessions: (filters: ReadingSessionsFilters) =>
    ["reading", "sessions", filters] as const,
  readingStats: (scope: ReadingStatsScope, range: ReadingStatsRange) =>
    ["reading", "stats", scope, range] as const,
  /** Admin dashboard overview — aggregate, never per-user. */
  adminOverview: ["admin", "stats", "overview"] as const,
  /** Stats v2: per-user aggregates list. */
  adminUsersStats: ["admin", "stats", "users"] as const,
  /** Stats v2: DAU/WAU/MAU + device breakdown. */
  adminEngagement: ["admin", "stats", "engagement"] as const,
  /** Stats v2: dead-stock + abandoned + funnel. */
  adminContent: ["admin", "stats", "content"] as const,
  /** Stats v2: orphan / long / metadata diagnostics. */
  adminQuality: ["admin", "stats", "quality"] as const,
  /** Per-user reading stats viewed by an admin. Each fetch writes an
   *  `admin.user.activity.view` audit row server-side. */
  adminUserReadingStats: (userId: string, range: ReadingStatsRange) =>
    ["admin", "users", userId, "reading-stats", range] as const,
  /** Server info — version, uptime, redis/postgres pings. */
  serverInfo: ["admin", "server-info"] as const,
  /** In-process log ring buffer. Tail filter is part of the key so the
   *  follow-tail toggle never collides with a paused snapshot. */
  adminLogs: (filters: AdminLogFilters) => ["admin", "logs", filters] as const,
  /** Directory listing for the New Library picker. `path` is part of
   *  the key so each drill-in is a separate cache entry. */
  adminFsList: (path: string | undefined) =>
    ["admin", "fs-list", path ?? ""] as const,
  /** Combined activity feed (audit / scan / health / reading volume). */
  adminActivity: (filters: AdminActivityFilters) =>
    ["admin", "activity", filters] as const,
  /** Read-only auth-config view. Cheap; cached for the session. */
  adminAuthConfig: ["admin", "auth-config"] as const,
  /** Runtime-editable settings (M1 of runtime-config-admin). Registry
   *  + resolved values; mutated via PATCH /admin/settings. */
  adminSettings: ["admin", "settings"] as const,
  /** Last-result probe for the outbound email pipeline (M2). */
  adminEmailStatus: ["admin", "email", "status"] as const,
  /** Resolved sidebar layout (built-ins + libraries + saved views in
   *  the user's chosen order, with visibility flags). Mutating drag-
   *  reorder + hide-toggle goes through `useUpdateSidebarLayout`. */
  sidebarLayout: ["sidebar-layout"] as const,
  /** Saved views (filter + CBL). `pinned` may be undefined for the
   *  full visible list. */
  savedViews: (filters: SavedViewListFilters = {}) =>
    ["saved-views", "list", filters] as const,
  savedView: (id: string) => ["saved-views", "detail", id] as const,
  savedViewResults: (id: string, cursor?: string) =>
    ["saved-views", "results", id, cursor ?? null] as const,
  savedViewResultsInfinite: (id: string) =>
    ["saved-views", "results-infinite", id] as const,
  /** Filter-builder option lookups (genres / tags / credits/<role>).
   *  `kind` is the path suffix, e.g. `'genres'` or `'credits/writer'`. */
  filterOptions: (
    kind: string,
    filters: { library?: string; q?: string } = {},
  ) => ["filter-options", kind, filters] as const,
  cblLists: ["cbl-lists", "list"] as const,
  cblList: (id: string) => ["cbl-lists", "detail", id] as const,
  cblEntries: (id: string, cursor?: string) =>
    ["cbl-lists", "entries", id, cursor ?? null] as const,
  cblListIssues: (id: string, opts?: { limit?: number; offset?: number }) =>
    ["cbl-lists", "issues", id, opts ?? {}] as const,
  /** Reading-window slice — anchored on the user's next unread matched
   *  entry. Powers the home rail's progress-centered view. */
  cblListWindow: (id: string, opts?: { before?: number; after?: number }) =>
    ["cbl-lists", "window", id, opts ?? {}] as const,
  cblRefreshLog: (id: string) => ["cbl-lists", "refresh-log", id] as const,
  /** Markers + Collections M2 — user collections (kind='collection') with
   *  Want to Read auto-seeded on first GET. */
  collections: ["collections", "list"] as const,
  collection: (id: string) => ["collections", "detail", id] as const,
  collectionEntries: (id: string, opts?: { cursor?: string; limit?: number }) =>
    ["collections", "entries", id, opts ?? {}] as const,
  /** Markers + Collections M5 — global feed for `/bookmarks` index. */
  markers: (filters: MarkerListFilters = {}) =>
    ["markers", "list", filters] as const,
  /** Reader overlay's per-issue fetch (one round-trip, all kinds). */
  issueMarkers: (issueId: string) => ["markers", "issue", issueId] as const,
  /** Cheap COUNT — drives the Bookmarks sidebar badge. Cached 60s. */
  markerCount: ["markers", "count"] as const,
  /** Distinct tag rollup — drives the /bookmarks tag filter chips. */
  markerTags: ["markers", "tags"] as const,
  /** Continue-reading rail (`/me/continue-reading`). Invalidated by any
   *  progress mutation + rail dismissal mutation. */
  continueReading: ["rails", "continue-reading"] as const,
  /** On-deck rail (`/me/on-deck`). Same invalidation set as continueReading. */
  onDeck: ["rails", "on-deck"] as const,
  /** Catalog source list (admin-managed, public read). */
  catalogSources: ["catalog", "sources"] as const,
  /** Cached `.cbl` listing inside a configured catalog source. */
  catalogEntries: (sourceId: string) =>
    ["catalog", "sources", sourceId, "entries"] as const,
  /** /me/sessions — self-managed refresh-session list. */
  sessions: ["auth", "sessions"] as const,
  /** /me/app-passwords — self-managed Bearer credentials (M7). */
  appPasswords: ["auth", "app-passwords"] as const,
  /** `GET /progress` — full per-user progress list, used by issue cards
   *  to render finished / in-progress badges. Invalidated by progress
   *  mutations. */
  userProgress: ["progress", "list"] as const,
};

export type SavedViewListFilters = {
  pinned?: boolean;
};

export type AdminLogFilters = {
  level?: LogLevel;
  q?: string;
  since?: number;
  limit?: number;
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

export function useMe() {
  return useQuery({
    queryKey: queryKeys.me,
    queryFn: () => jsonFetch<MeView>("/auth/me"),
    staleTime: 60_000,
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

export function useHealthIssues(libraryId: string) {
  return useQuery({
    queryKey: queryKeys.health(libraryId),
    queryFn: () =>
      jsonFetch<HealthIssueView[]>(`/libraries/${libraryId}/health-issues`),
    enabled: !!libraryId,
  });
}

/**
 * Library scan history. `kind` filters by trigger: `'library' | 'series' |
 * 'issue'`, or omitted for all. Server caps at 500 rows; default is 50.
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
      jsonFetch<ScanRunView[]>(`/libraries/${libraryId}/scan-runs${qs}`),
    enabled: !!libraryId,
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
    queryKey: ["issues", "cross-list", filters] as const,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      jsonFetch<IssueListView>(
        `/issues${buildQuery({ ...rest, cursor: pageParam })}`,
      ),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: options.enabled ?? true,
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
  return useQuery({
    queryKey: queryKeys.savedViews(filters),
    queryFn: () =>
      jsonFetch<SavedViewListView>(`/me/saved-views${buildQuery(params)}`),
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
 *  kind, issue, and free-text search against `body` + `selection.text`. */
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
 *  position order. Pages via opaque cursor; `total` is first-page-only. */
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
