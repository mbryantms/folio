/**
 * TanStack Query key registry — the single source of truth for every
 * query key in the app. Extracted from `queries.ts` (audit H1a) so the
 * `check-query-keys` gate can whitelist exactly one file: query
 * *definitions* (`useQuery` / `useInfiniteQuery` / `useSuspenseQuery`)
 * must reference `queryKeys.*` rather than inlining a tuple, which keeps
 * a hook's cache key identical to the keys mutations invalidate against.
 *
 * Filter-shape types live in `./queries` (and `./types`) and are
 * imported type-only here — the import is erased at runtime, so there is
 * no module cycle. New hooks: add a key factory here, then reference it.
 */
import type { ReadingLogFilters, ReadingStatsRange } from "./types";
import type {
  AdminActivityFilters,
  AdminLogFilters,
  AuditFilters,
  CblEntriesFilters,
  IssueListFilters,
  IssuesCrossListFilters,
  IssueSearchFilters,
  CreatorsListFilters,
  MarkerListFilters,
  MarkerSearchFilters,
  PeopleSearchFilters,
  ReadingSessionsFilters,
  ReadingStatsScope,
  SavedViewListFilters,
  SeriesListFilters,
  UserListFilters,
} from "./queries";

export const queryKeys = {
  me: ["auth", "me"] as const,
  libraries: ["libraries"] as const,
  library: (id: string) => ["libraries", id] as const,
  /** Prefix for every per-library health query — invalidating this matches
   *  both the single-page summary and the table's infinite query. */
  health: (libraryId: string) => ["libraries", libraryId, "health"] as const,
  /** Paginated/faceted health table. Status/severity/kind are server params,
   *  so each combination is its own infinite query. */
  healthInfinite: (
    libraryId: string,
    filters: { status: string; severity: string; kind: string | null },
  ) => ["libraries", libraryId, "health", "infinite", filters] as const,
  backupStorage: (libraryId: string) =>
    ["libraries", libraryId, "backup-storage"] as const,
  archivePageCount: (issueId: string) =>
    ["issues", issueId, "archive", "page-count"] as const,
  /** Cross-library findings — admin findings page + dashboard cards. */
  adminHealthIssues: (filters: {
    library_id?: string;
    kind?: string;
    severity?: string;
    include_resolved?: boolean;
    include_dismissed?: boolean;
    limit?: number;
  }) => ["admin", "health-issues", filters] as const,
  adminHealthIssuesInfinite: (filters: {
    library_id?: string;
    kind?: string;
    severity?: string;
    include_resolved?: boolean;
    include_dismissed?: boolean;
    limit?: number;
  }) => ["admin", "health-issues-infinite", filters] as const,
  adminScanRuns: (filters: {
    library_id?: string;
    kind?: string;
    state?: string;
    since?: string;
    limit?: number;
  }) => ["admin", "scan-runs", filters] as const,
  adminScanRunsInfinite: (filters: {
    library_id?: string;
    kind?: string;
    state?: string;
    since?: string;
    limit?: number;
  }) => ["admin", "scan-runs-infinite", filters] as const,
  adminLatestScanPerLibrary: [
    "admin",
    "scan-runs",
    "latest-per-library",
  ] as const,
  /** Scan-all batches — observability-split M9 dashboard. */
  adminScanBatches: (state?: string) =>
    ["admin", "scan-batches", state ?? "all"] as const,
  adminScanBatch: (id: string) => ["admin", "scan-batches", id] as const,
  /** Durable library-event manifest (observability-split M10/M11). */
  adminLibraryEventsInfinite: (filters: {
    library_id?: string;
    batch_id?: string;
    scan_run_id?: string;
    category?: string;
    action?: string;
    severity?: string;
  }) => ["admin", "library-events-infinite", filters] as const,
  issueHealth: (seriesSlug: string, issueSlug: string) =>
    ["issues", seriesSlug, issueSlug, "health"] as const,
  issueMetadataOverview: (seriesSlug: string, issueSlug: string) =>
    ["issues", seriesSlug, issueSlug, "metadata-overview"] as const,
  scanRuns: (libraryId: string, kind?: string) =>
    ["libraries", libraryId, "scan-runs", kind ?? "all"] as const,
  /** Cursor-paginated variant (load-more history table). */
  scanRunsInfinite: (libraryId: string, kind?: string) =>
    ["libraries", libraryId, "scan-runs", "infinite", kind ?? "all"] as const,
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
  /** Infinite cross-library issue listing (`/issues`) — metadata-suite
   *  filters at the issue level. */
  issuesCrossList: (filters: IssuesCrossListFilters) =>
    ["issues", "cross-list", filters] as const,
  /** Cross-library issue search (`/issues/search`). Backs the global
   *  search modal + page's Issues section. */
  issueSearch: (filters: IssueSearchFilters) =>
    ["issues", "search", filters] as const,
  /** Series typeahead in the CBL manual-match popover. */
  seriesManualMatchSearch: (q: string) =>
    ["series", "manual-match-search", q] as const,
  /** Issue list for a chosen series in the CBL manual-match popover. */
  seriesManualMatchIssues: (seriesSlug: string, q: string) =>
    ["series", "manual-match-issues", seriesSlug, q] as const,
  /** Cross-credits people search (`/people`). Global-search M4. */
  peopleSearch: (filters: PeopleSearchFilters) =>
    ["people", "search", filters] as const,
  /** Alphabetical creator browse index (`/creators`). Audit A11. */
  creatorsList: (filters: CreatorsListFilters) =>
    ["creators", "list", filters] as const,
  /** Marker search (`/me/markers/search`). 4th global-search category. */
  markerSearch: (filters: MarkerSearchFilters) =>
    ["markers", "search", filters] as const,
  queueDepth: ["admin", "queue-depth"] as const,
  /** Per-queue dead-letter counts (`/admin/queue/dead-letters`). */
  deadLetters: ["admin", "queue", "dead-letters"] as const,
  /** Paginated dead-job list for one queue (`/admin/queue/dead-jobs`). */
  deadJobs: (queue: string, page: number) =>
    ["admin", "queue", "dead-jobs", queue, page] as const,
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
  /** Reading log feed — cursor-paginated event union. */
  readingLog: (filters: ReadingLogFilters) =>
    ["reading", "log", filters] as const,
  /** Per-user log-widget grid (auto-seeded on first GET). */
  logWidgets: ["reading", "log", "widgets"] as const,
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
  /** Boot-only settings changed since startup (need a restart). */
  restartPending: ["admin", "restart-pending"] as const,
  latestRelease: ["admin", "latest-release"] as const,
  /** OCR model download / on-disk state (text-detection-1.0 M5). */
  ocrModels: ["admin", "ocr-models"] as const,
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
  /** Metadata-providers (M5): candidate-list polling for the dialog. */
  metadataCandidatesSeries: (slug: string, runId: string | null) =>
    ["series", slug, "metadata", "candidates", runId ?? "latest"] as const,
  metadataCandidatesIssue: (
    slug: string,
    issueSlug: string,
    runId: string | null,
  ) =>
    [
      "series",
      slug,
      "issues",
      issueSlug,
      "metadata",
      "candidates",
      runId ?? "latest",
    ] as const,
  /** Proposed-diff (M5 preview pane). Keyed by run+ordinal+mode+
   *  override so flipping any of these arguments triggers a refetch. */
  metadataDiffSeries: (
    slug: string,
    runId: string,
    ordinal: number,
    mode: string,
    override: boolean,
  ) =>
    [
      "series",
      slug,
      "metadata",
      "diff",
      runId,
      ordinal,
      mode,
      override,
    ] as const,
  metadataDiffIssue: (
    slug: string,
    issueSlug: string,
    runId: string,
    ordinal: number,
    mode: string,
    override: boolean,
  ) =>
    [
      "series",
      slug,
      "issues",
      issueSlug,
      "metadata",
      "diff",
      runId,
      ordinal,
      mode,
      override,
    ] as const,
  /** Composite (multi-provider) compare view. Keyed by run + mode +
   *  override + the included candidate-ordinal set so any change
   *  refetches. */
  metadataCompositeDiffSeries: (
    slug: string,
    runId: string,
    mode: string,
    override: boolean,
    include: number[],
  ) =>
    [
      "series",
      slug,
      "metadata",
      "composite-diff",
      runId,
      mode,
      override,
      include.join(","),
    ] as const,
  metadataCompositeDiffIssue: (
    slug: string,
    issueSlug: string,
    runId: string,
    mode: string,
    override: boolean,
    include: number[],
  ) =>
    [
      "series",
      slug,
      "issues",
      issueSlug,
      "metadata",
      "composite-diff",
      runId,
      mode,
      override,
      include.join(","),
    ] as const,
  /** Sync-status card (last_metadata_sync_at + paused). */
  metadataSyncStatusSeries: (slug: string) =>
    ["series", slug, "metadata", "status"] as const,
  /** Collection-completeness report (owned vs. expected + missing issues). */
  seriesCollection: (slug: string) => ["series", slug, "collection"] as const,
  /** Bulk-metadata batch status (live progress + child list). */
  metadataBatch: (batchId: string) => ["metadata", "batch", batchId] as const,
  /** Recent bulk-metadata batches (Review tab picker). */
  metadataBatches: ["metadata", "batches"] as const,
  /** External-IDs card listing. */
  externalIdsSeries: (slug: string) =>
    ["series", slug, "external-ids"] as const,
  externalIdsIssue: (slug: string, issueSlug: string) =>
    ["series", slug, "issues", issueSlug, "external-ids"] as const,
  /** Cover gallery for an issue (M5.2). */
  issueCovers: (issueId: string) => ["issues", issueId, "covers"] as const,
  // ── M6 admin surface ──
  adminMetadataDashboard: ["admin", "metadata", "dashboard"] as const,
  adminMetadataMatchQuality: ["admin", "metadata", "match-quality"] as const,
  adminMetadataProviders: ["admin", "metadata", "providers"] as const,
  adminMetadataRuns: (filters: {
    library_id?: string;
    scope?: string;
    status?: string;
    before?: string;
  }) => ["admin", "metadata", "runs", filters] as const,
  adminMetadataRun: (id: string) => ["admin", "metadata", "runs", id] as const,
  adminMetadataRecentApplies: (limit: number) =>
    ["admin", "metadata", "recent-applies", limit] as const,
  adminMetadataAutoSynced: ["admin", "metadata", "auto-synced"] as const,
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
  /** Multi-page rails: the user's full page list (system + custom). */
  mePages: ["me", "pages"] as const,
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
  /** Cursor-paginated CBL entries, optionally narrowed by status. Use
   *  `["cbl-lists", "entries", id]` (no filters) as a prefix to
   *  invalidate every status-variant in one go. */
  cblListEntries: (id: string, filters?: CblEntriesFilters) =>
    ["cbl-lists", "entries", id, filters ?? {}] as const,
  cblListIssues: (id: string, opts?: { limit?: number; offset?: number }) =>
    ["cbl-lists", "issues", id, opts ?? {}] as const,
  /** Reading-window slice — anchored on the user's next unread matched
   *  entry. Powers the home rail's progress-centered view. */
  cblListWindow: (id: string, opts?: { before?: number; after?: number }) =>
    ["cbl-lists", "window", id, opts ?? {}] as const,
  /** Bidirectional infinite-scroll window — same anchor band, but
   *  with cursor-paginated before/after extensions. Distinct from
   *  `cblListWindow` so the two hooks' caches don't collide. */
  cblListWindowInfinite: (
    id: string,
    opts?: { before?: number; after?: number; limit?: number },
  ) => ["cbl-lists", "window-infinite", id, opts ?? {}] as const,
  cblRefreshLog: (id: string) => ["cbl-lists", "refresh-log", id] as const,
  /** Markers + Collections M2 — user collections (kind='collection') with
   *  Want to Read auto-seeded on first GET. */
  collections: ["collections", "list"] as const,
  collection: (id: string) => ["collections", "detail", id] as const,
  collectionEntries: (id: string, opts?: { cursor?: string; limit?: number }) =>
    ["collections", "entries", id, opts ?? {}] as const,
  /** Infinite-scroll variant of `collectionEntries`. Distinct key
   *  because TanStack's prefix-match doesn't traverse from `entries`
   *  to `entries-infinite`. Mutations that change collection
   *  membership must invalidate BOTH so paginated + infinite
   *  consumers stay in sync. */
  collectionEntriesInfinite: (id: string) =>
    ["collections", "entries-infinite", id] as const,
  /** Markers + Collections M5 — global feed for `/bookmarks` index. */
  markers: (filters: MarkerListFilters = {}) =>
    ["markers", "list", filters] as const,
  /** Reader overlay's per-issue fetch (one round-trip, all kinds). */
  issueMarkers: (issueId: string) => ["markers", "issue", issueId] as const,
  /** Detected speech-bubble outlines for one page (OCR rework 1.0).
   *  Fetched when the reader enters text-capture mode. */
  issuePageTextRegions: (issueId: string, page: number) =>
    ["ocr", "text-regions", issueId, page] as const,
  /** Cheap COUNT — drives the Bookmarks sidebar badge. Cached 60s. */
  markerCount: ["markers", "count"] as const,
  /** Distinct tag rollup — drives the /bookmarks tag filter chips. */
  markerTags: ["markers", "tags"] as const,
  /** Continue-reading rail (`/me/continue-reading`). Invalidated by any
   *  progress mutation + rail dismissal mutation. */
  continueReading: ["rails", "continue-reading"] as const,
  /** On-deck rail (`/me/on-deck`). Same invalidation set as continueReading. */
  onDeck: ["rails", "on-deck"] as const,
  /** Reader's single-issue "what's next?" resolver
   *  (`/issues/{id}/next-up`). Separate cache entry per (issue, cbl)
   *  pairing so a CBL-context read and a series-context read don't
   *  trample each other's results. */
  nextUp: (issueId: string, cblSavedViewId?: string | null) =>
    ["reader", "next-up", issueId, cblSavedViewId ?? ""] as const,
  /** Reader's single-issue "what came before?" resolver
   *  (`/issues/{id}/prev-up`). Symmetric with nextUp; pure sequence
   *  nav, doesn't filter by finished state. */
  prevUp: (issueId: string, cblSavedViewId?: string | null) =>
    ["reader", "prev-up", issueId, cblSavedViewId ?? ""] as const,
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
