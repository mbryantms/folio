/**
 * **Hybrid alias shim (audit-remediation M1c, shipped 2026-05-23).** This
 * file used to be a 2392-line hand-curated mirror of the Rust API surface.
 * It's now a thin shim: 177 of 219 named types are one-line aliases over
 * `components["schemas"]["X"]` from [./types.generated.ts](./types.generated.ts);
 * the remaining ~40 entries are inline because they either don't have a
 * codegen equivalent or carry narrower types than what codegen emits.
 *
 * **Source of truth:**
 *   - For aliased types: the Rust DTO (codegen wins). Hand-edits to the
 *     aliased block are silently overridden by the next `just openapi`.
 *   - For inline types: hand-maintained. Each inline entry falls into one
 *     of these buckets:
 *       1. **Frontend-only computed types** (union/intersection helpers,
 *          `OnDeckCard`'s variant shape, etc.) — by design.
 *       2. **WS payload shapes** (`ScanEvent`) — the OpenAPI surface
 *          excludes WebSocket events.
 *       3. **Typed enums where the Rust source uses bare `String`** —
 *          `MarkerKind`, `MarkerShape`, `LogWidgetKind`, `CblMatchStatus`,
 *          `ThumbnailFormat`, `ScanRunKind`, `SavedViewKind`, etc. Each is
 *          a tracked debt: when the Rust source derives `ToSchema` on an
 *          enum (following the
 *          [`preferences.rs`](../../../crates/server/src/auth/preferences.rs)
 *          template), the inline entry can be promoted to an alias.
 *
 * **Drift gate:** `just openapi-check` regenerates both `openapi.json` and
 * `types.generated.ts` and fails the build if either disagrees with the
 * checked-in copy.
 *
 * **To add a new type:** write the Rust `ToSchema`, run `just openapi`,
 * then either add an alias here or let
 * `web/scripts/build-types-shim.py` regenerate the partition.
 */

import type { components } from "./types.generated";

type Schemas = components["schemas"];

// ────────────── Aliased from codegen ──────────────
// Hand-edits to the *aliased* block below will be silently overridden by
// the next `just openapi` when the Rust source moves. To change one of
// these shapes, change the Rust DTO.
export type LibraryView = Schemas["LibraryView"];
export type CreateLibraryReq = Schemas["CreateLibraryReq"];
export type UpdateLibraryReq = Schemas["UpdateLibraryReq"];
export type SeriesView = Schemas["SeriesView"];
export type SeriesProgressSummary = Schemas["SeriesProgressSummary"];
export type SeriesListView = Schemas["SeriesListView"];
export type SortOrder = Schemas["SortOrder"];
export type UpdateSeriesReq = Schemas["UpdateSeriesReq"];
export type IssueSummaryView = Schemas["IssueSummaryView"];
export type IssueListView = Schemas["IssueListView"];
export type IssueSearchHit = Schemas["IssueSearchHit"];
export type IssueSearchView = Schemas["IssueSearchView"];
export type PersonHit = Schemas["PersonHit"];
export type CreatorRoleRail = Schemas["CreatorRoleRail"];
export type CreatorDetailView = Schemas["CreatorDetailView"];
export type PeopleListView = Schemas["PeopleListView"];
export type IssueDetailView = Schemas["IssueDetailView"];
export type IssueLink = Schemas["IssueLink"];
export type UpdateIssueReq = Schemas["UpdateIssueReq"];
export type NextInSeriesView = Schemas["NextInSeriesView"];
export type SetRatingReq = Schemas["SetRatingReq"];
export type RatingView = Schemas["RatingView"];
export type MeView = Schemas["MeResp"]; // renamed in codegen as MeResp
export type PreferencesReq = Schemas["PreferencesReq"];
export type SeriesResumeView = Schemas["SeriesResumeView"];
export type CblWindowEntry = Schemas["CblWindowEntry"];
export type CblWindowView = Schemas["CblWindowView"];
export type CblWindowPageView = Schemas["CblWindowPageView"];
export type ContinueReadingCard = Schemas["ContinueReadingCard"];
export type ContinueReadingView = Schemas["ContinueReadingView"];
export type OnDeckCard = Schemas["OnDeckCard"];
export type OnDeckView = Schemas["OnDeckView"];
export type CreateRailDismissalReq = Schemas["CreateDismissalReq"]; // renamed in codegen as CreateDismissalReq
export type NextUpSource = Schemas["NextUpSource"];
export type NextUpView = Schemas["NextUpView"];
export type ReadingSessionView = Schemas["ReadingSessionView"];
export type ReadingSessionListView = Schemas["ReadingSessionListView"];
export type ReadingLogEventSeries = Schemas["EventSeries"]; // renamed in codegen as EventSeries
export type ReadingLogEventIssue = Schemas["EventIssue"]; // renamed in codegen as EventIssue
export type ReadingLogPayload = Schemas["EventPayload"]; // renamed in codegen as EventPayload
export type ReadingLogEventView = Schemas["ReadingLogEventView"];
export type ReadingLogPageView = Schemas["ReadingLogPageView"];
export type LogWidgetView = Schemas["LogWidgetView"];
export type LogWidgetListView = Schemas["CursorPage_LogWidgetView"];
export type AddLogWidgetReq = Schemas["AddWidgetReq"]; // renamed in codegen as AddWidgetReq
export type ReadingDayBucket = Schemas["DayBucket"]; // renamed in codegen as DayBucket
export type AdminOverviewView = Schemas["OverviewView"]; // renamed in codegen as OverviewView
export type DeviceBucket = Schemas["DeviceBucket"];
export type AdminUserStatsRow = Schemas["AdminUserStatsRow"];
export type AdminUserStatsListView = Schemas["CursorPage_AdminUserStatsRow"];
export type EngagementPoint = Schemas["EngagementPoint"];
export type EngagementView = Schemas["EngagementView"];
export type DeadStockEntry = Schemas["DeadStockEntry"];
export type AbandonedEntry = Schemas["AbandonedEntry"];
export type FunnelBucket = Schemas["FunnelBucket"];
export type ContentInsightsView = Schemas["ContentInsightsView"];
export type MetadataCoverageView = Schemas["MetadataCoverageView"];
export type DataQualityView = Schemas["DataQualityView"];
export type LatestReleaseView = Schemas["LatestReleaseView"];
export type OcrModelView = Schemas["OcrModelView"];
export type OcrModelsView = Schemas["OcrModelsView"];
export type ServerInfoView = Schemas["ServerInfoView"];
export type LogEntryView = Schemas["LogEntryView"];
export type LogsResp = Schemas["LogsResp"];
export type FsDirEntry = Schemas["DirEntry"]; // renamed in codegen as DirEntry
export type FsListResp = Schemas["ListResp"]; // renamed in codegen as ListResp
export type ActivityEntryView = Schemas["ActivityEntryView"];
export type ActivityListView = Schemas["ActivityListView"];
export type AuthConfigView = Schemas["AuthConfigView"];
export type PublicAuthConfigView = Schemas["PublicAuthConfigView"];
export type SettingRegistryEntry = Schemas["RegistryEntry"]; // renamed in codegen as RegistryEntry
export type SettingResolvedEntry = Schemas["ResolvedEntry"]; // renamed in codegen as ResolvedEntry
export type SettingsView = Schemas["SettingsView"];
export type UpdateSettingsReq = Schemas["UpdateSettingsReq"];
export type EmailStatusView = Schemas["EmailStatusView"];
export type TestEmailResp = Schemas["TestEmailResp"];
export type OidcDiscoverReq = Schemas["OidcDiscoverReq"];
export type OidcDiscoverResp = Schemas["OidcDiscoverResp"];
export type ReadingStatsView = Schemas["ReadingStatsView"];
export type TopSeriesEntry = Schemas["TopSeriesEntry"];
export type TopNameEntry = Schemas["TopNameEntry"];
export type TopCreatorEntry = Schemas["TopCreatorEntry"];
export type DowHourCell = Schemas["DowHourCell"];
export type TimeOfDayCell = Schemas["TimeOfDayCell"];
export type TimeOfDayBuckets = Schemas["TimeOfDayBuckets"];
export type PacePoint = Schemas["PacePoint"];
export type RereadIssueEntry = Schemas["RereadIssueEntry"];
export type RereadSeriesEntry = Schemas["RereadSeriesEntry"];
export type CompletionView = Schemas["CompletionView"];
export type ClearHistoryResp = Schemas["ClearHistoryResp"];
export type AccountReq = Schemas["AccountReq"];
export type HealthIssueView = Schemas["HealthIssueView"];
export type CrossLibHealthIssueView = Schemas["CrossLibHealthIssueView"];
export type CrossLibScanRunView = Schemas["CrossLibScanRunView"];
export type ScanRunView = Schemas["ScanRunView"];
export type ScanResp = Schemas["ScanResp"];
export type ScanAllReq = Schemas["ScanAllReq"];
export type ScanAllItem = Schemas["ScanAllItem"];
export type ScanAllResp = Schemas["ScanAllResp"];
export type ScanPreviewView = Schemas["ScanPreviewView"];
export type DeleteLibraryResp = Schemas["DeleteLibraryResp"];
export type RemovedIssueView = Schemas["RemovedIssueView"];
export type RemovedSeriesView = Schemas["RemovedSeriesView"];
export type RemovedListView = Schemas["RemovedListView"];
export type QueueDepthView = Schemas["QueueDepthView"];
export type QueueClearTarget = Schemas["QueueClearTarget"];
export type QueueClearReq = Schemas["QueueClearReq"];
export type QueueClearResp = Schemas["QueueClearResp"];
export type ThumbnailsStatusView = Schemas["ThumbnailsStatusView"];
export type RegenerateResp = Schemas["RegenerateResp"];
export type DeleteAllResp = Schemas["DeleteAllResp"];
export type ThumbnailsSettingsView = Schemas["ThumbnailsSettingsView"];
export type UpdateThumbnailsSettingsReq = Schemas["UpdateThumbnailsSettingsReq"];
export type AdminUserView = Schemas["AdminUserView"];
export type LibraryAccessGrantView = Schemas["LibraryAccessGrantView"];
export type AdminUserDetailView = Schemas["AdminUserDetailView"];
export type AdminUserListView = Schemas["AdminUserListView"];
export type UpdateUserReq = Schemas["UpdateUserReq"];
export type LibraryAccessReq = Schemas["LibraryAccessReq"];
export type AuditEntryView = Schemas["AuditEntryView"];
export type AuditListView = Schemas["AuditListView"];
export type Field = Schemas["Field"];
export type Op = Schemas["Op"];
export type MatchMode = Schemas["MatchMode"];
export type Condition = Schemas["Condition"];
export type FilterDsl = Schemas["FilterDsl"];
export type SavedViewView = Schemas["SavedViewView"];
export type SavedViewListView = Schemas["SavedViewListView"];
export type CreateSavedViewReq = Schemas["CreateSavedViewReq"];
export type UpdateSavedViewReq = Schemas["UpdateSavedViewReq"];
export type PreviewReq = Schemas["PreviewReq"];
export type PinView = Schemas["PinView"];
export type PinReq = Schemas["PinReq"];
export type UnpinReq = Schemas["UnpinReq"];
export type SidebarEntryView = Schemas["SidebarEntryView"];
export type SidebarLayoutView = Schemas["SidebarLayoutView"];
export type UpdateEntryReq = Schemas["UpdateEntryReq"];
export type UpdateLayoutReq = Schemas["UpdateLayoutReq"];
export type CblStatsView = Schemas["CblStatsView"];
export type CblListView = Schemas["CblListView"];
export type CblListListView = Schemas["CblListListView"];
export type CblEntryView = Schemas["CblEntryView"];
export type CblEntryHydratedView = Schemas["CblEntryHydratedView"];
export type CblEntryListView = Schemas["CblEntryListView"];
export type CblDetailView = Schemas["CblDetailView"];
export type CreateCblListReq = Schemas["CreateCblListReq"];
export type UpdateCblListReq = Schemas["UpdateCblListReq"];
export type ManualMatchReq = Schemas["ManualMatchReq"];
export type RefreshLogEntryView = Schemas["RefreshLogEntryView"];
export type RefreshLogListView = Schemas["RefreshLogListView"];
export type CatalogSourceView = Schemas["CatalogSourceView"];
export type CatalogSourceListView = Schemas["CatalogSourceListView"];
export type CatalogEntryView = Schemas["CatalogEntryView"];
export type CatalogEntriesView = Schemas["CatalogEntriesView"];
export type CreateCatalogSourceReq = Schemas["CreateCatalogSourceReq"];
export type UpdateCatalogSourceReq = Schemas["UpdateCatalogSourceReq"];
export type CollectionEntryView = Schemas["CollectionEntryView"];
export type CollectionEntriesView = Schemas["CollectionEntriesView"];
export type CreateCollectionReq = Schemas["CreateCollectionReq"];
export type UpdateCollectionReq = Schemas["UpdateCollectionReq"];
export type AddEntryReq = Schemas["AddEntryReq"];
export type ReorderEntriesReq = Schemas["ReorderEntriesReq"];
export type PageView = Schemas["PageView"];
export type PageListView = Schemas["CursorPage_PageView"];
export type CreatePageReq = Schemas["CreatePageReq"];
export type UpdatePageReq = Schemas["UpdatePageReq"];
export type ReorderPagesReq = Schemas["ReorderPagesReq"];
// Marker types: kept narrower than the codegen because the Rust source
// uses `String` for `kind` and `serde_json::Value` for `region`/`selection`.
// MarkerKind / MarkerRegion / MarkerSelection are defined inline below.
// Move these to aliases once Rust derives ToSchema on the marker enums
// (audit-remediation residual; see preferences.rs for the template).
export type MarkerView = {
  id: string;
  user_id: string;
  series_id: string;
  issue_id: string;
  page_index: number;
  kind: MarkerKind;
  is_favorite: boolean;
  tags: string[];
  region?: MarkerRegion | null;
  selection?: MarkerSelection | null;
  body?: string | null;
  color?: string | null;
  created_at: string;
  updated_at: string;
  series_name?: string | null;
  series_slug?: string | null;
  issue_slug?: string | null;
  issue_title?: string | null;
  issue_number?: string | null;
};
export type TagEntryView = Schemas["TagEntryView"];
export type MarkerTagsView = Schemas["MarkerTagsView"];
export type MarkerListView = {
  items: MarkerView[];
  next_cursor?: string | null;
};
export type MarkerCountView = Schemas["MarkerCountView"];
export type MarkerSearchHit = {
  id: string;
  kind: MarkerKind;
  issue_id: string;
  series_id: string;
  page_index: number;
  region?: MarkerRegion | null;
  snippet?: string | null;
  series_name?: string | null;
  series_slug?: string | null;
  issue_slug?: string | null;
  issue_title?: string | null;
  issue_number?: string | null;
};
export type MarkerSearchView = { items: MarkerSearchHit[] };
export type IssueMarkersView = { items: MarkerView[] };
export type CreateMarkerReq = {
  issue_id: string;
  page_index: number;
  kind: MarkerKind;
  region?: MarkerRegion | null;
  selection?: MarkerSelection | null;
  body?: string | null;
  color?: string | null;
  is_favorite?: boolean | null;
  tags?: string[] | null;
};
export type UpdateMarkerReq = {
  body?: string | null;
  color?: string | null;
  region?: MarkerRegion | null;
  selection?: MarkerSelection | null;
  is_favorite?: boolean;
  tags?: string[];
};
export type SessionView = Schemas["SessionView"];
export type SessionListView = Schemas["SessionListView"];
export type AppPasswordView = Schemas["AppPasswordView"];
export type AppPasswordListView = Schemas["AppPasswordListView"];
export type AppPasswordCreatedView = Schemas["AppPasswordCreatedView"];
export type CreateAppPasswordReq = Schemas["CreateAppPasswordReq"];

// ────────────── Frontend-only / not yet derived for ToSchema ──────────────
// Each entry below is either a frontend-only computed type or a Rust type
// that hasn't been wired into the OpenAPI spec yet. When the Rust source
// derives ToSchema, move the corresponding entry up into the aliased block.

export type SeriesSort = "name" | "created_at" | "updated_at" | "year";

export type IssueSort =
  | "number"
  | "created_at"
  | "updated_at"
  | "year"
  | "page_count"
  | "user_rating";

export type PageInfo = {
  image: number;
  type?: string | null;
  double_page?: boolean | null;
  image_size?: number | null;
  key?: string | null;
  bookmark?: string | null;
  image_width?: number | null;
  image_height?: number | null;
};

export type UpsertProgressReq = {
  issue_id: string;
  page: number;
  finished?: boolean;
  device?: string | null;
};

export type ProgressView = {
  issue_id: string;
  page: number;
  percent: number;
  finished: boolean;
  updated_at: string;
};

export type UpsertSeriesProgressReq = {
  finished: boolean;
  device?: string | null;
  /** "Updating my collection — don't count toward today's reading
   *  activity." When `true` and `finished == true`, every written
   *  progress row carries `is_backfill = true` and is excluded from
   *  the reading log, Just Finished sort, and similar activity
   *  surfaces. Ignored when `finished` is false (the server clears
   *  the flag on every unread write). Default `false` mirrors the
   *  pre-v0.5.7 shape; UI callers opt in via the bulk-mark dialog. */
  backfill?: boolean;
};

export type UpsertSeriesProgressResp = {
  updated: number;
  skipped: number;
};

export type RailProgressInfo = {
  last_page: number;
  /** 0.0–1.0 fraction read. */
  percent: number;
  updated_at: string;
};

export type ReadingSessionUpsertReq = {
  /** Client-generated UUID v4 (or any 1-64 char unique tag). */
  client_session_id: string;
  issue_id: string;
  /** RFC 3339 timestamp. */
  started_at: string;
  /** Set only on the final flush. */
  ended_at?: string;
  active_ms: number;
  distinct_pages_read: number;
  page_turns: number;
  start_page: number;
  end_page: number;
  device?: string | null;
  view_mode?: "single" | "double" | "webtoon" | null;
  client_meta?: Record<string, unknown>;
};

export type ReadingStatsRange = "7d" | "30d" | "60d" | "90d" | "1y" | "all";

export type ReadingLogEventKind =
  | "issue_finished"
  | "series_finished"
  | "session_completed"
  | "marker_created";

export type ReadingLogFilters = {
  kinds?: ReadingLogEventKind[];
  /** RFC3339 lower bound (inclusive). */
  from?: string;
  /** RFC3339 upper bound (exclusive). */
  to?: string;
  library_id?: string;
  series_id?: string;
  limit?: number;
  /** When true, fetch hidden events alongside visible ones. UI uses
   *  it to power the "Show hidden" toggle on the reading log. */
  include_hidden?: boolean;
};

export type LogWidgetKind =
  | "chrono_feed"
  | "stats_hero"
  | "heatmap"
  | "top_creators"
  | "top_publishers"
  | "top_imprints"
  | "series_finishes"
  | "pace_chart"
  | "time_of_day"
  | "recent_bookmarks"
  | "currently_reading"
  | "note";

export type PatchLogWidgetReq = {
  /** Replacement config blob — the server stores it as-is. Clients
   *  merge with the previous value if they want partial updates. */
  config: Record<string, unknown>;
};

export type ReorderLogWidgetsReq = {
  /** Full set of widget ids in the new order. Server 400s when the
   *  set doesn't match the user's owned ids exactly. */
  ids: string[];
};

export type ReadingTotalsView = {
  sessions: number;
  active_ms: number;
  distinct_pages_read: number;
  distinct_issues: number;
  /** Days within the range with at least one session. */
  days_active: number;
  /** Consecutive days ending today (global). */
  current_streak: number;
  longest_streak: number;
};

export type AdminOverviewTotals = {
  libraries: number;
  series: number;
  issues: number;
  users: number;
};

export type AdminOpenHealth = {
  error: number;
  warning: number;
  info: number;
};

export type LogLevel = "error" | "warn" | "info" | "debug" | "trace";

export type ActivityKind = "audit" | "scan" | "health" | "reading";

export type SettingKind = "string" | "bool" | "uint" | "duration";

export type CreatorRole =
  | "writer"
  | "penciller"
  | "inker"
  | "colorist"
  | "letterer"
  | "cover_artist"
  | "editor"
  | "translator";

export type ScanRunKind = "library" | "series" | "issue";

export type ScanMode = "normal" | "content_verify";

export type ThumbnailFormat = "webp" | "jpeg" | "png";

export type ScanEvent =
  | { type: "scan.started"; library_id: string; scan_id: string; at: string }
  | {
      type: "scan.progress";
      library_id: string;
      scan_id: string;
      kind: "library" | "series" | "issue" | string;
      phase: string;
      unit: string;
      completed: number;
      total: number;
      current_label: string | null;
      files_seen: number;
      files_added: number;
      files_updated: number;
      files_unchanged: number;
      files_skipped: number;
      files_duplicate: number;
      issues_removed: number;
      health_issues: number;
      series_scanned: number;
      series_total: number;
      series_skipped_unchanged: number;
      files_total: number;
      root_files: number;
      empty_folders: number;
      elapsed_ms?: number;
      phase_elapsed_ms?: number;
      files_per_sec?: number;
      bytes_per_sec?: number;
      active_workers?: number;
      dirty_folders?: number;
      skipped_folders?: number;
      eta_ms?: number;
    }
  | {
      type: "scan.series_updated";
      library_id: string;
      series_id: string;
      name: string;
    }
  | {
      type: "scan.health_issue";
      library_id: string;
      scan_id: string;
      kind: string;
      severity: string;
      path: string | null;
    }
  | {
      type: "scan.completed";
      library_id: string;
      scan_id: string;
      added: number;
      updated: number;
      removed: number;
      duration_ms: number;
    }
  | {
      type: "scan.failed";
      library_id: string;
      scan_id: string;
      error: string;
    }
  | {
      type: "thumbs.started";
      library_id: string;
      issue_id: string;
      kind?: "cover" | "page_map" | "cover_page_map";
    }
  | {
      type: "thumbs.completed";
      library_id: string;
      issue_id: string;
      kind?: "cover" | "page_map" | "cover_page_map";
      pages: number;
    }
  | {
      type: "thumbs.failed";
      library_id: string;
      issue_id: string;
      kind?: "cover" | "page_map" | "cover_page_map";
      error: string;
    }
  | { type: "lagged"; skipped: number };

export type ApiError = {
  error: { code: string; message: string; details?: unknown };
};

export type SavedViewSortField =
  | "name"
  | "year"
  | "created_at"
  | "updated_at"
  | "last_read"
  | "read_progress";

export type SavedViewKind = "filter_series" | "cbl" | "system" | "collection";

export type SystemRailKey = "continue_reading" | "on_deck" | "want_to_read";

export type SidebarEntryKind =
  | "builtin"
  | "library"
  | "view"
  | "page"
  | "header"
  | "spacer";

export type CblSourceKind = "upload" | "url" | "catalog";

export type CblMatchStatus = "matched" | "ambiguous" | "missing" | "manual";

export type ImportSummary = {
  list_id: string;
  upstream_changed: boolean;
  matched: number;
  ambiguous: number;
  missing: number;
  manual: number;
  added: number;
  removed: number;
  reordered: number;
  rematched: number;
};

export type CollectionEntryKind = "series" | "issue";

export type MarkerKind = "bookmark" | "note" | "favorite" | "highlight";

export type MarkerShape = "rect" | "text" | "image";

export type MarkerRegion = {
  x: number;
  y: number;
  w: number;
  h: number;
  shape: MarkerShape;
};

export type MarkerSelection = {
  text?: string | null;
  image_hash?: string | null;
  ocr_confidence?: number | null;
};

export type MarkerTagMatch = "all" | "any";

export type RevokeAllSessionsResp = {
  /** Count of sessions transitioned from active → revoked. Doesn't
   *  include already-revoked / expired rows. */
  revoked: number;
};

export type AppPasswordScope = "read" | "read+progress";

// ───────── metadata-providers-1.0 ─────────
export type SearchStartedResp = Schemas["SearchStartedResp"];
export type CandidatesResp = Schemas["CandidatesResp"];
export type CandidateView = Schemas["CandidateView"];
export type ApplyAcceptedResp = Schemas["ApplyAcceptedResp"];
export type ApplyMode = Schemas["ApplyMode"];
export type ApplyCoverPolicy = Schemas["ApplyCoverPolicy"];
export type SyncStatusResp = Schemas["SyncStatusResp"];
export type ExternalIdsListResp = Schemas["ExternalIdsListResp"];
export type ExternalIdRow = Schemas["ExternalIdRow"];
export type AddExternalIdReq = Schemas["AddExternalIdReq"];
export type IssueCoversResp = Schemas["IssueCoversResp"];
export type IssueCoverRow = Schemas["IssueCoverRow"];
// M5 preview pane / proposed-diff
export type DiffResp = Schemas["DiffResp"];
export type ScalarDiffRow = Schemas["ScalarDiffRow"];
export type ExternalIdConflictRow = Schemas["ExternalIdConflictRow"];
export type ExternalIdNewRow = Schemas["ExternalIdNewRow"];
// M6 admin surface
export type DashboardResp = Schemas["DashboardResp"];
export type MatchQualityResp = Schemas["MatchQualityResp"];
export type MatchQualityWindow = Schemas["MatchQualityWindow"];
export type ProviderView = Schemas["ProviderView"];
export type QuotaView = Schemas["QuotaView"];
export type RunsListResp = Schemas["RunsListResp"];
export type RunRow = Schemas["RunRow"];
export type RunDetailResp = Schemas["RunDetailResp"];
export type CandidateRow = Schemas["CandidateRow"];
export type ReviewQueueResp = Schemas["ReviewQueueResp"];
export type ReviewItem = Schemas["ReviewItem"];
export type ProvidersListResp = Schemas["ProvidersListResp"];
export type TestProviderResp = Schemas["TestProviderResp"];

