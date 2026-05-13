/**
 * Hand-written API types for Phase 1a. Will be replaced by `openapi-typescript`
 * generated `paths`/`components` once `just openapi` is wired in CI. Until then
 * we keep them here so the web compiles without a server-side codegen step.
 */

export type LibraryView = {
  id: string;
  /** URL-safe identifier, globally unique across libraries. */
  slug: string;
  name: string;
  root_path: string;
  default_language: string;
  default_reading_direction: string;
  dedupe_by_content: boolean;
  last_scan_at: string | null;
  /** Library Scanner v1 (M4) settings. */
  ignore_globs: string[];
  report_missing_comicinfo: boolean;
  file_watch_enabled: boolean;
  soft_delete_days: number;
  scan_schedule_cron: string | null;
  /** When true, the post-scan pipeline auto-enqueues page-strip thumbnails
   *  alongside the always-on cover thumbnails. Default false. */
  generate_page_thumbs_on_scan: boolean;
};

export type CreateLibraryReq = {
  name: string;
  root_path: string;
  default_language?: string;
  default_reading_direction?: string;
  scan_now?: boolean;
  /** Persistent setting: when true, every post-scan pass enqueues page
   *  thumbnails. Cover thumbnails are always generated. Default false. */
  generate_page_thumbs_on_scan?: boolean;
};

export type UpdateLibraryReq = {
  ignore_globs?: string[];
  report_missing_comicinfo?: boolean;
  file_watch_enabled?: boolean;
  soft_delete_days?: number;
  /** `null` clears the cron; an empty string is treated as null server-side. */
  scan_schedule_cron?: string | null;
  /** Admin override for the URL slug. Server slugifies + validates uniqueness. */
  slug?: string;
  /** Toggle the per-library opt-in for auto-generating page thumbnails on
   *  every post-scan pass. */
  generate_page_thumbs_on_scan?: boolean;
};

export type SeriesView = {
  id: string;
  /** URL-safe identifier, globally unique across series. */
  slug: string;
  library_id: string;
  name: string;
  year: number | null;
  volume: number | null;
  publisher: string | null;
  status: string;
  total_issues: number | null;
  age_rating: string | null;
  summary: string | null;
  language_code: string;
  /** External-database IDs (ComicVine volume / Metron series). Set by the
   *  scanner from ComicInfo or by admins via PATCH /series/{slug}. */
  comicvine_id: number | null;
  metron_id: number | null;
  issue_count: number | null;
  cover_url: string | null;
  created_at: string;
  updated_at: string;
  /**
   * Aggregated CSV-style ComicInfo fields, frequency-ordered (most frequent
   * first). Populated only by `GET /series/{id}` to keep list payloads small.
   */
  writers?: string[];
  pencillers?: string[];
  inkers?: string[];
  colorists?: string[];
  letterers?: string[];
  cover_artists?: string[];
  genres?: string[];
  tags?: string[];
  characters?: string[];
  teams?: string[];
  locations?: string[];
  /** Sum of `page_count` across active, on-disk issues. Detail-only. */
  total_page_count?: number | null;
  last_issue_added_at?: string | null;
  last_issue_updated_at?: string | null;
  /** Earliest / latest publication year across the series's issues — drives
   *  the "Released" stat's range display. Null on the list endpoint or
   *  when no issue has a parsed year. */
  earliest_year?: number | null;
  latest_year?: number | null;
  /** Server-computed read progress for the calling user across the entire
   *  series. Sidesteps the client-side 100-issue page cap. Detail-only. */
  progress_summary?: SeriesProgressSummary | null;
  /** Calling user's 0..=5 rating for the series. Half-star precision.
   *  Null means "not rated". */
  user_rating?: number | null;
};

export type SeriesProgressSummary = {
  total: number;
  finished: number;
  in_progress: number;
  /** Sum of page_count across the issues the user has finished. Used by
   *  the series page's "Reading load" stat to estimate remaining minutes. */
  finished_pages: number;
};

export type SeriesListView = {
  items: SeriesView[];
  next_cursor: string | null;
  /** Total matching rows across all pages — populated only on the
   *  first page (no cursor). `null` on subsequent pages and on saved-
   *  view results (which are capped, so a precise total isn't useful). */
  total?: number | null;
};

export type SeriesSort = "name" | "created_at" | "updated_at" | "year";
export type IssueSort =
  | "number"
  | "created_at"
  | "updated_at"
  | "year"
  | "page_count"
  | "user_rating";
export type SortOrder = "asc" | "desc";

export type UpdateSeriesReq = {
  /** `null` clears the override; whitespace-only treated as null server-side. */
  match_key?: string | null;
  /** Admin override for the URL slug. Server slugifies + validates uniqueness. */
  slug?: string;
  /** Publication status — one of `continuing`, `ended`, `cancelled`,
   *  `hiatus`, `limited`. Validated server-side; case-insensitive. */
  status?: string;
  /** ComicVine volume id. `null` clears, omit to leave untouched. */
  comicvine_id?: number | null;
  /** Metron series id. `null` clears, omit to leave untouched. */
  metron_id?: number | null;
  /** Series-level summary. `null` clears (the API falls back to the first
   *  issue's summary on read). Omit to leave untouched. */
  summary?: string | null;
};

export type IssueSummaryView = {
  id: string;
  /** URL-safe identifier, unique within the parent series. */
  slug: string;
  series_id: string;
  /** Slug of the parent series. */
  series_slug: string;
  /** Parent series name, denormalized so card components can fall back
   *  to `"<series> #<number>"` when the issue has no title. Populated
   *  by rail-feeding endpoints (Continue Reading, On Deck, CBL window,
   *  Collections). Absent on endpoints where the JOIN cost outweighed
   *  the benefit (per-series listing, server-side admin scans, …) —
   *  callers fall back to the prior `#N` / `"Untitled"` defaults. */
  series_name?: string | null;
  title: string | null;
  number: string | null;
  sort_number: number | null;
  year: number | null;
  page_count: number | null;
  state: string;
  cover_url: string | null;
  created_at: string;
  updated_at: string;
};

export type IssueListView = {
  items: IssueSummaryView[];
  next_cursor: string | null;
  /** See [`SeriesListView.total`] — same first-page-only semantics. */
  total?: number | null;
};

/** Cross-library issue-search hit (manual-match popover backbone). */
export type IssueSearchHit = IssueSummaryView & {
  series_name: string;
};

export type IssueSearchView = {
  items: IssueSearchHit[];
};

/** Global-search M4: distinct creator-name hit, with role rollup +
 *  credit count across both series and issue credit junctions. */
export type PersonHit = {
  person: string;
  roles: string[];
  credit_count: number;
};

export type PeopleListView = {
  items: PersonHit[];
};

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

export type IssueDetailView = {
  id: string;
  /** URL-safe identifier, unique within the parent series. */
  slug: string;
  /** Slug of the parent series — handy for building nested URLs. */
  series_slug: string;
  series_id: string;
  library_id: string;
  file_path: string;
  state: string;
  title: string | null;
  number: string | null;
  sort_number: number | null;
  volume: number | null;
  year: number | null;
  month: number | null;
  day: number | null;
  summary: string | null;
  notes: string | null;
  publisher: string | null;
  writer: string | null;
  penciller: string | null;
  inker: string | null;
  colorist: string | null;
  letterer: string | null;
  cover_artist: string | null;
  editor: string | null;
  translator: string | null;
  imprint: string | null;
  characters: string | null;
  teams: string | null;
  locations: string | null;
  alternate_series: string | null;
  tags: string | null;
  genre: string | null;
  language_code: string | null;
  age_rating: string | null;
  manga: string | null;
  format: string | null;
  black_and_white: boolean | null;
  page_count: number | null;
  story_arc: string | null;
  story_arc_number: string | null;
  web_url: string | null;
  gtin: string | null;
  /** External-database IDs. ComicVine `4000-N` strips to the integer N. */
  comicvine_id: number | null;
  metron_id: number | null;
  /** Calling user's 0..=5 rating for this issue. Null means "not rated". */
  user_rating: number | null;
  /** File size in bytes from the on-disk row at last scan. */
  file_size: number;
  created_at: string;
  updated_at: string;
  /** User-curated extra links (label optional, url required). Distinct from
   *  `web_url` (the ComicInfo single link). Mutated via PATCH /issues/{id}. */
  additional_links: IssueLink[];
  /** Names of fields the user has overridden via PATCH /issues/{id}. The
   *  scanner skips these on a rescan; clients can use this to flag locally
   *  edited rows. */
  user_edited: string[];
  pages: PageInfo[];
  comic_info_raw: Record<string, unknown>;
};

export type IssueLink = {
  label?: string | null;
  url: string;
};

/** Body for `PATCH /series/{series_slug}/issues/{issue_slug}`. Each field
 *  independently optional; sending `null` clears a nullable column.
 *  `additional_links` is replace-all. The server records every touched
 *  field in `user_edited` so the scanner skips them on rescan. */
export type UpdateIssueReq = {
  // Identity / publication
  title?: string | null;
  /** Maps to `number_raw` on the entity (e.g. "1", "1.5", "Annual 2"). */
  number?: string | null;
  volume?: number | null;
  year?: number | null;
  month?: number | null;
  day?: number | null;
  summary?: string | null;
  notes?: string | null;
  publisher?: string | null;
  imprint?: string | null;

  // Credits
  writer?: string | null;
  penciller?: string | null;
  inker?: string | null;
  colorist?: string | null;
  letterer?: string | null;
  cover_artist?: string | null;
  editor?: string | null;
  translator?: string | null;

  // Cast / setting / story
  characters?: string | null;
  teams?: string | null;
  locations?: string | null;
  alternate_series?: string | null;
  story_arc?: string | null;
  story_arc_number?: string | null;

  // Classification
  genre?: string | null;
  tags?: string | null;
  language_code?: string | null;
  age_rating?: string | null;
  format?: string | null;
  black_and_white?: boolean | null;
  /** One of `Yes`, `YesAndRightToLeft`, `No`, or null. */
  manga?: string | null;

  // Ordering / external
  sort_number?: number | null;
  web_url?: string | null;
  gtin?: string | null;
  comicvine_id?: number | null;
  metron_id?: number | null;

  additional_links?: IssueLink[];
};

/** Result of `GET /series/{series_slug}/issues/{issue_slug}/next`. */
export type NextInSeriesView = {
  items: IssueSummaryView[];
};

/** Body for the rating endpoints. `null` clears the rating. Half-star
 *  precision (rating × 2 must be an integer) is enforced server-side. */
export type SetRatingReq = {
  rating: number | null;
};

export type RatingView = {
  rating: number | null;
};

export type MeView = {
  id: string;
  email: string | null;
  display_name: string;
  role: string;
  csrf_token: string;
  /** Phase 3: per-user reader direction. `null` means "auto" (no global override). */
  default_reading_direction?: string | null;
  /** M4: reader default fit mode. `null` defers to the reader's built-in default. */
  default_fit_mode?: "width" | "height" | "original" | null;
  /** M4: reader default view mode. `null` defers to per-series detection. */
  default_view_mode?: "single" | "double" | "webtoon" | null;
  /** M4: open the reader with the page strip visible. */
  default_page_strip?: boolean;
  /** Default for double-page view's "cover stands alone" toggle.
   *  `true` matches the printed-comic convention. */
  default_cover_solo: boolean;
  /** M4: theme token. */
  theme?: "system" | "dark" | "light" | "amber" | null;
  /** M4: accent palette token. */
  accent_color?: "amber" | "blue" | "emerald" | "rose" | null;
  /** M4: density token. */
  density?: "comfortable" | "compact" | null;
  /** M4: per-action keybind overrides. Empty object means "use defaults". */
  keybinds?: Record<string, string>;
  /** M6a: per-user opt-out for reading-activity capture. */
  activity_tracking_enabled: boolean;
  /** M6a: IANA timezone for daily-bucket aggregations. */
  timezone: string;
  /** M6a: minimum active ms before a session is recorded. */
  reading_min_active_ms: number;
  /** M6a: minimum distinct pages before a session is recorded. */
  reading_min_pages: number;
  /** M6a: idle threshold (ms) after which the client ends the session. */
  reading_idle_ms: number;
  /** Human-URLs M3: BCP-47 language tag, drives next-intl + the
   *  `NEXT_LOCALE` cookie. */
  language: string;
  /** Stats v2: opt-out from server-wide aggregation. When true, admin
   *  dashboards exclude this user's sessions from totals/top-series/etc.
   *  Default false. */
  exclude_from_aggregates: boolean;
  /** Markers M8: when true, the Bookmarks sidebar row renders a count
   *  badge sourced from /me/markers/count. Default false. */
  show_marker_count: boolean;
};

/** PATCH /me/preferences body. Each field independently optional; `null`
 *  clears the stored value where the type allows. Absent leaves the field
 *  untouched. */
export type PreferencesReq = {
  default_reading_direction?: "ltr" | "rtl" | null;
  default_fit_mode?: "width" | "height" | "original" | null;
  default_view_mode?: "single" | "double" | "webtoon" | null;
  default_page_strip?: boolean;
  default_cover_solo?: boolean;
  theme?: "system" | "dark" | "light" | "amber" | null;
  accent_color?: "amber" | "blue" | "emerald" | "rose" | null;
  density?: "comfortable" | "compact" | null;
  keybinds?: Record<string, string>;
  /** M6a. */
  activity_tracking_enabled?: boolean;
  /** M6a — IANA tz, e.g. `'America/Los_Angeles'`. */
  timezone?: string;
  /** M6a — 1000..=600_000 ms. */
  reading_min_active_ms?: number;
  /** M6a — 1..=200. */
  reading_min_pages?: number;
  /** M6a — 30000..=1_800_000 ms. */
  reading_idle_ms?: number;
  /** Human-URLs M3 — BCP-47 language tag, validated against the server's
   *  SUPPORTED_LOCALES list. */
  language?: string;
  /** Stats v2 privacy toggle. */
  exclude_from_aggregates?: boolean;
  /** Markers M8: per-user toggle for the sidebar Bookmarks count badge. */
  show_marker_count?: boolean;
};

/** Body for `POST /progress` (per-issue). */
export type UpsertProgressReq = {
  issue_id: string;
  page: number;
  finished?: boolean;
  device?: string | null;
};

/** Per-issue progress record. */
export type ProgressView = {
  issue_id: string;
  page: number;
  percent: number;
  finished: boolean;
  updated_at: string;
};

/** Body for `POST /series/{id}/progress` — bulk mark-all-read/unread. */
export type UpsertSeriesProgressReq = {
  finished: boolean;
  device?: string | null;
};

/** Response from `POST /series/{id}/progress`. */
export type UpsertSeriesProgressResp = {
  updated: number;
  skipped: number;
};

/** Response from `GET /series/{slug}/resume` — the issue (+ resume page)
 *  the user should land on when they tap a series "play" affordance. */
export type SeriesResumeView = {
  series_slug: string;
  /** `null` when the series has no readable issues. */
  issue_slug: string | null;
  issue_id: string | null;
  /** 0-based; `0` for unread / re-read paths. */
  page: number;
  state: "unread" | "in_progress" | "finished";
};

// ---------- CBL reading window (home rail) ----------

/** One entry in the CBL reading-window response. Matches `IssueSummaryView`
 *  plus the per-user progress overlay so the rail can render finished /
 *  in-progress / unread cards without a second round-trip. */
export type CblWindowEntry = {
  issue: IssueSummaryView;
  /** 0-based position within the CBL — matches the `#N` badge other
   *  surfaces use. */
  position: number;
  finished: boolean;
  last_page: number;
  /** 0.0–1.0 fraction read. */
  percent: number;
};

export type CblWindowView = {
  items: CblWindowEntry[];
  /** Index within `items` of the user's current (first-unfinished)
   *  entry. `null` when every matched entry is finished. */
  current_index: number | null;
  total_matched: number;
  total_entries: number;
};

// ---------- Home rails (Continue reading / On deck) ----------

/** Per-issue progress overlay attached to rail cards. Mirrors `ProgressView`
 *  but uses the `last_page` name from the server's join (the rails endpoint
 *  reads `progress_records.last_page`, not the M3 `page` alias). */
export type RailProgressInfo = {
  last_page: number;
  /** 0.0–1.0 fraction read. */
  percent: number;
  updated_at: string;
};

export type ContinueReadingCard = {
  issue: IssueSummaryView;
  /** Parent series name, denormalized so the card doesn't need a separate
   *  lookup. The issue carries its own series_slug for navigation. */
  series_name: string;
  progress: RailProgressInfo;
};

export type ContinueReadingView = {
  items: ContinueReadingCard[];
};

/** Discriminated union for the On Deck rail.
 *
 *  - `series_next`: the user finished one or more issues in a series and
 *    has no in-progress one. We surface the next issue in sort order.
 *  - `cbl_next`: the user has progress in a CBL reading list. We surface
 *    the lowest-position matched entry whose issue isn't finished yet. */
export type OnDeckCard =
  | {
      kind: "series_next";
      issue: IssueSummaryView;
      series_name: string;
      last_activity: string;
    }
  | {
      kind: "cbl_next";
      issue: IssueSummaryView;
      cbl_list_id: string;
      cbl_list_name: string;
      /** 1-based, matches the CBL detail page's "#N" badge. */
      position: number;
      last_activity: string;
    };

export type OnDeckView = {
  items: OnDeckCard[];
};

/** Request body for `POST /me/rail-dismissals`. */
export type CreateRailDismissalReq = {
  /** One of `'issue' | 'series' | 'cbl'`. */
  target_kind: "issue" | "series" | "cbl";
  target_id: string;
};

// ---------- Reading sessions (M6a) ----------

/** Body for `POST /me/reading-sessions`. Idempotent over `(user_id,
 *  client_session_id)`. The same client_session_id is used for the
 *  initial write, every 30s heartbeat, and the final close — server takes
 *  the max of monotonic counters and only sets `ended_at` once. */
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

export type ReadingSessionView = {
  id: string;
  issue_id: string;
  series_id: string;
  client_session_id: string;
  started_at: string;
  ended_at: string | null;
  last_heartbeat_at: string;
  active_ms: number;
  distinct_pages_read: number;
  page_turns: number;
  start_page: number;
  end_page: number;
  furthest_page: number;
  device: string | null;
  view_mode: "single" | "double" | "webtoon" | null;
  /** Issue title from `issues.title` (joined). Populated by the list
   *  endpoint; null on the upsert response. */
  issue_title?: string | null;
  /** Issue number from `issues.number_raw`. */
  issue_number?: string | null;
  /** Series name from `series.name`. */
  series_name?: string | null;
};

export type ReadingSessionListView = {
  records: ReadingSessionView[];
  next_cursor: string | null;
};

export type ReadingStatsRange = "7d" | "30d" | "60d" | "90d" | "1y" | "all";

export type ReadingDayBucket = {
  /** YYYY-MM-DD in user's timezone. */
  date: string;
  sessions: number;
  active_ms: number;
  pages: number;
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

// ---------- Admin observability (M6c) ----------

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

export type AdminOverviewView = {
  totals: AdminOverviewTotals;
  scans_in_flight: number;
  open_health: AdminOpenHealth;
  sessions_today: number;
  active_readers_now: number;
  reads_per_day: ReadingDayBucket[];
  /** System-wide most-read series in the last 30 days. */
  top_series_all_users: TopSeriesEntry[];
};

export type DeviceBucket = {
  device: string;
  sessions: number;
  active_ms: number;
};

export type AdminUserStatsRow = {
  user_id: string;
  display_name: string;
  email: string | null;
  role: string;
  state: string;
  last_active_at: string | null;
  sessions_30d: number;
  active_ms_30d: number;
  sessions_all_time: number;
  active_ms_all_time: number;
  top_series_name: string | null;
  device_breakdown: DeviceBucket[];
  excluded_from_aggregates: boolean;
};

export type AdminUserStatsListView = {
  users: AdminUserStatsRow[];
};

export type EngagementPoint = {
  /** `YYYY-MM-DD` (UTC). */
  date: string;
  dau: number;
  wau: number;
  mau: number;
};

export type EngagementView = {
  /** Last 90 days, oldest first. */
  series: EngagementPoint[];
  devices_30d: DeviceBucket[];
};

export type DeadStockEntry = {
  series_id: string;
  name: string;
  publisher: string | null;
  library_id: string;
  library_name: string;
  issue_count: number;
};

export type AbandonedEntry = {
  series_id: string;
  name: string;
  sessions: number;
  unfinished_issues: number;
  readers: number;
};

export type FunnelBucket = {
  bucket: "0-25" | "25-50" | "50-75" | "75-99" | "100" | string;
  issues: number;
};

export type ContentInsightsView = {
  dead_stock: DeadStockEntry[];
  abandoned: AbandonedEntry[];
  completion_funnel: FunnelBucket[];
};

export type MetadataCoverageView = {
  total_issues: number;
  missing_writer: number;
  missing_cover_artist: number;
  missing_page_count: number;
  missing_genre: number;
  missing_publisher: number;
};

export type DataQualityView = {
  orphan_sessions: number;
  long_sessions: number;
  dangling_sessions: number;
  metadata: MetadataCoverageView;
};

export type ServerInfoView = {
  version: string;
  build_sha: string;
  uptime_secs: number;
  postgres_ok: boolean;
  redis_ok: boolean;
  scheduler_running: boolean;
  watchers_enabled: number;
};

// ---------- Admin logs / activity / auth (M6d, M6e) ----------

export type LogLevel = "error" | "warn" | "info" | "debug" | "trace";

export type LogEntryView = {
  id: number;
  timestamp: string;
  level: LogLevel | string;
  target: string;
  message: string;
  fields: Record<string, string>;
};

export type LogsResp = {
  entries: LogEntryView[];
  /** Highest id returned; pass back as `?since=` to tail. */
  watermark: number;
  capacity: number;
};

/** `GET /admin/fs/list` — directory listing for the New Library picker. */
export type FsDirEntry = {
  name: string;
  /** Absolute path inside the container; pass back as `?path=` to drill in. */
  path: string;
};

export type FsListResp = {
  /** Canonical absolute path of the listed directory. */
  path: string;
  /** Canonical absolute path of the configured library root —
   *  `COMIC_LIBRARY_PATH`. The picker uses this to gray out the "up"
   *  button when the user reaches the root. */
  root: string;
  entries: FsDirEntry[];
};

export type ActivityKind = "audit" | "scan" | "health" | "reading";

export type ActivityEntryView = {
  kind: ActivityKind | string;
  source_id: string;
  timestamp: string;
  summary: string;
  payload: Record<string, unknown>;
};

export type ActivityListView = {
  entries: ActivityEntryView[];
  next_cursor: string | null;
};

export type AuthConfigView = {
  auth_mode: "oidc" | "local" | "both" | string;
  oidc: {
    configured: boolean;
    issuer: string | null;
    client_id: string | null;
    trust_unverified_email: boolean;
  };
  local: {
    enabled: boolean;
    registration_open: boolean;
    smtp_configured: boolean;
  };
};

/** Unauthenticated subset of [`AuthConfigView`]. Served by `/auth/config`
 *  so the sign-in page can render the right CTAs without leaking issuer
 *  / client_id / trust-unverified flag. */
export type PublicAuthConfigView = {
  auth_mode: "oidc" | "local" | "both" | string;
  oidc_enabled: boolean;
  registration_open: boolean;
};

// ---------- Runtime-editable settings (`/admin/settings`) ----------

export type SettingKind = "string" | "bool" | "uint" | "duration";

export type SettingRegistryEntry = {
  key: string;
  kind: SettingKind;
  is_secret: boolean;
};

export type SettingResolvedEntry = {
  key: string;
  /** Secret rows are returned as the literal string "<set>"; the API
   *  never echoes the plaintext. */
  value: unknown;
  is_secret: boolean;
};

export type SettingsView = {
  registry: SettingRegistryEntry[];
  values: SettingResolvedEntry[];
};

/** PATCH /admin/settings body — flat key→value map. Use `null` to delete
 *  a row. Unknown keys are rejected with 400 settings.unknown_key. */
export type UpdateSettingsReq = Record<string, unknown>;

// ---------- Email pipeline status (`/admin/email/status`) ----------

export type EmailStatusView = {
  configured: boolean;
  last_send_at: string | null;
  last_send_ok: boolean | null;
  last_error: string | null;
  last_duration_ms: number | null;
};

export type TestEmailResp = {
  delivered: boolean;
  duration_ms: number;
  to: string;
};

// ---------- OIDC discovery probe (`/admin/auth/oidc/discover`) ----------

export type OidcDiscoverReq = {
  issuer: string;
};

export type OidcDiscoverResp = {
  issuer: string;
  authorization_endpoint: string | null;
  token_endpoint: string | null;
  jwks_uri: string | null;
  end_session_endpoint: string | null;
  userinfo_endpoint: string | null;
  scopes_supported: string[] | null;
};

// ---------- (continues below) ----------

export type ReadingStatsView = {
  range: ReadingStatsRange;
  timezone: string;
  totals: ReadingTotalsView;
  per_day: ReadingDayBucket[];
  /** Empty when the request is issue-scoped. */
  top_series: TopSeriesEntry[];
  top_genres: TopNameEntry[];
  top_tags: TopNameEntry[];
  /** Empty when the request is issue-scoped. */
  top_publishers: TopNameEntry[];
  top_imprints: TopNameEntry[];
  /** Top creators across read series, partitioned per role (writer, penciller,
   *  inker, colorist, letterer, cover_artist, editor, translator). Up to 10
   *  rows per role; client slices by `role`. */
  top_creators: TopCreatorEntry[];
  /** Sparse 7×24 grid of day-of-week × hour (in user's tz). Only non-zero
   *  cells emitted. `dow` follows Postgres EXTRACT (0 = Sunday). */
  dow_hour: DowHourCell[];
  time_of_day: TimeOfDayBuckets;
  /** Per-session pace samples. Sessions with `<3` distinct pages dropped. */
  pace_series: PacePoint[];
  reread_top_issues: RereadIssueEntry[];
  reread_top_series: RereadSeriesEntry[];
  completion: CompletionView;
  /** RFC 3339 timestamp of the earliest session in scope; null if no
   *  sessions. */
  first_read_at: string | null;
  /** RFC 3339 timestamp of the latest session in scope. */
  last_read_at: string | null;
};

export type TopSeriesEntry = {
  series_id: string;
  name: string;
  sessions: number;
  active_ms: number;
};

export type TopNameEntry = {
  name: string;
  sessions: number;
  active_ms: number;
};

export type CreatorRole =
  | "writer"
  | "penciller"
  | "inker"
  | "colorist"
  | "letterer"
  | "cover_artist"
  | "editor"
  | "translator";

export type TopCreatorEntry = {
  role: CreatorRole | string;
  person: string;
  sessions: number;
  active_ms: number;
};

export type DowHourCell = {
  /** 0 = Sunday … 6 = Saturday. */
  dow: number;
  /** 0–23, user's local timezone. */
  hour: number;
  sessions: number;
  active_ms: number;
};

export type TimeOfDayCell = {
  sessions: number;
  active_ms: number;
};

export type TimeOfDayBuckets = {
  morning: TimeOfDayCell;
  afternoon: TimeOfDayCell;
  evening: TimeOfDayCell;
  night: TimeOfDayCell;
};

export type PacePoint = {
  /** RFC 3339 session-start timestamp. */
  started_at: string;
  /** Average seconds per distinct page within the session. */
  sec_per_page: number;
};

export type RereadIssueEntry = {
  issue_id: string;
  title: string | null;
  number_raw: string | null;
  series_id: string;
  series_name: string;
  reads: number;
  active_ms: number;
};

export type RereadSeriesEntry = {
  series_id: string;
  name: string;
  distinct_issues: number;
  reads: number;
  active_ms: number;
};

export type CompletionView = {
  completed: number;
  started: number;
  /** `completed / started`, 0–1. 0.0 when `started == 0`. */
  rate: number;
};

export type ClearHistoryResp = {
  deleted: number;
};

/** PATCH /me/account body. */
export type AccountReq = {
  display_name?: string;
  /** Local users only. Sending for an OIDC user yields a 403. */
  email?: string;
  /** Required when changing the password. */
  current_password?: string;
  new_password?: string;
};

// ---------- Library Scanner v1 ----------

export type HealthIssueView = {
  id: string;
  scan_id: string | null;
  kind: string;
  severity: string;
  fingerprint: string;
  payload: unknown;
  first_seen_at: string;
  last_seen_at: string;
  resolved_at: string | null;
  dismissed_at: string | null;
};

export type ScanRunKind = "library" | "series" | "issue";
export type ScanMode = "normal" | "metadata_refresh" | "content_verify";

export type ScanRunView = {
  id: string;
  state: string;
  started_at: string;
  ended_at: string | null;
  error: string | null;
  /** Free-form stats JSON (added/updated/removed/duration_ms etc.). */
  stats: unknown;
  /** Trigger discriminator. Drives the History tab's filter chips. */
  kind: ScanRunKind;
  /** Target series id when `kind` is `series` or `issue`. */
  series_id: string | null;
  /** Joined series name for the row's target label. `null` if the
   *  underlying series row was deleted. */
  series_name: string | null;
  /** Originating issue id when `kind` is `issue`. */
  issue_id: string | null;
};

export type ScanResp = {
  scan_id: string;
  state: string;
  coalesced: boolean;
  mode: ScanMode | string;
  coalesced_into: string | null;
  queued_followup: boolean;
  reason: string;
  kind: "library" | "series" | "issue" | string;
  library_id: string;
  series_id?: string | null;
  issue_id?: string | null;
};

export type ScanPreviewView = {
  mode: ScanMode | string;
  dirty_folders: number;
  known_issue_count: number;
  thumbnail_backlog: number;
  last_scan_duration_ms: number | null;
  last_scan_state: string | null;
  watcher_status: string;
  reason: string;
};

/** `DELETE /libraries/{id}` — hard-deletes the library, all series, all
 *  issues, scan history, health issues, and on-disk thumbnails. The audit
 *  log row survives (audit is append-only). */
export type DeleteLibraryResp = {
  deleted_library: string;
  deleted_issues: number;
  deleted_series: number;
  thumbs_swept: number;
};

export type RemovedIssueView = {
  id: string;
  series_id: string;
  file_path: string;
  removed_at: string;
  removal_confirmed_at: string | null;
};

export type RemovedSeriesView = {
  id: string;
  name: string;
  folder_path: string | null;
  removed_at: string;
  removal_confirmed_at: string | null;
};

export type RemovedListView = {
  issues: RemovedIssueView[];
  series: RemovedSeriesView[];
};

/** `GET /admin/queue-depth` — apalis pending-job counts per queue. */
export type QueueDepthView = {
  scan: number;
  scan_series: number;
  post_scan_thumbs: number;
  post_scan_search: number;
  post_scan_dictionary: number;
  total: number;
};

export type QueueClearTarget = "all" | "scans" | "thumbnails";

export type QueueClearReq = {
  target: QueueClearTarget;
};

export type QueueClearResp = {
  target: QueueClearTarget;
  deleted_keys: number;
  before: QueueDepthView;
  after: QueueDepthView;
  running_jobs_may_finish: boolean;
};

/** `GET /admin/libraries/{id}/thumbnails-status` — counts for the
 *  Thumbnails card on the library overview. */
export type ThumbnailsStatusView = {
  total: number;
  generated: number;
  missing: number;
  errored: number;
  cover_generated: number;
  cover_missing: number;
  cover_queued: number;
  cover_running: number;
  cover_failed: number;
  page_total: number;
  page_generated: number;
  page_missing: number;
  page_map_generated: number;
  page_map_missing: number;
  page_map_queued: number;
  page_map_running: number;
  page_map_failed: number;
  /** Whole-server queue depth, not per-library — apalis can't filter. */
  in_flight: number;
  current_version: number;
};

export type RegenerateResp = {
  enqueued: number;
};

export type DeleteAllResp = {
  deleted: number;
};

/** `GET/PATCH /admin/libraries/{id}/thumbnails-settings`. */
export type ThumbnailsSettingsView = {
  enabled: boolean;
  /** One of `webp` | `jpeg` | `png`. */
  format: ThumbnailFormat;
  /** Cover thumbnail encoder quality, 0..=100. */
  cover_quality: number;
  /** Reader page thumbnail encoder quality, 0..=100. */
  page_quality: number;
};

export type ThumbnailFormat = "webp" | "jpeg" | "png";

export type UpdateThumbnailsSettingsReq = {
  enabled?: boolean;
  format?: ThumbnailFormat;
  cover_quality?: number;
  page_quality?: number;
};

// ---------- Scan event stream (WS /ws/scan-events) ----------

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

// ---------- Admin: users + audit (M3) ----------

export type AdminUserView = {
  id: string;
  email: string | null;
  display_name: string;
  role: string;
  state: string;
  email_verified: boolean;
  created_at: string;
  last_login_at: string | null;
  /** Granted libraries; always 0 for admins (admins implicitly see all). */
  library_count: number;
};

export type LibraryAccessGrantView = {
  library_id: string;
  library_name: string;
  role: string;
};

export type AdminUserDetailView = AdminUserView & {
  library_access: LibraryAccessGrantView[];
};

export type AdminUserListView = {
  items: AdminUserView[];
  next_cursor: string | null;
};

export type UpdateUserReq = {
  display_name?: string;
  /** `admin` | `user` */
  role?: string;
  /** `pending_verification` | `active` | `disabled` */
  state?: string;
};

export type LibraryAccessReq = {
  library_ids: string[];
};

export type AuditEntryView = {
  id: string;
  actor_id: string;
  actor_type: string;
  /** Human-readable label resolved server-side (display name + email). */
  actor_label: string | null;
  action: string;
  target_type: string | null;
  target_id: string | null;
  /** Human-readable label for `user` and `library` targets; `null` otherwise. */
  target_label: string | null;
  payload: unknown;
  ip: string | null;
  user_agent: string | null;
  created_at: string;
};

export type AuditListView = {
  items: AuditEntryView[];
  next_cursor: string | null;
};

// ---------- Saved views (filter + CBL) ----------

/** All filterable fields. Mirrors `crates/server/src/views/registry.rs`. */
export type Field =
  | "library"
  | "name"
  | "year"
  | "volume"
  | "total_issues"
  | "publisher"
  | "imprint"
  | "status"
  | "age_rating"
  | "language_code"
  | "created_at"
  | "updated_at"
  | "genres"
  | "tags"
  | "writer"
  | "penciller"
  | "inker"
  | "colorist"
  | "letterer"
  | "cover_artist"
  | "editor"
  | "translator"
  | "read_progress"
  | "last_read"
  | "read_count";

export type Op =
  | "contains"
  | "starts_with"
  | "equals"
  | "not_equals"
  | "is"
  | "is_not"
  | "in"
  | "not_in"
  | "gt"
  | "gte"
  | "lt"
  | "lte"
  | "between"
  | "before"
  | "after"
  | "relative"
  | "includes_any"
  | "includes_all"
  | "excludes"
  | "is_true"
  | "is_false";

export type MatchMode = "all" | "any";

export type SavedViewSortField =
  | "name"
  | "year"
  | "created_at"
  | "updated_at"
  | "last_read"
  | "read_progress";

/** One condition row in a filter DSL. `group_id` always 0 in v1. */
export type Condition = {
  group_id?: number;
  field: Field;
  op: Op;
  /** Shape varies by `(field, op)`. The compiler validates server-side. */
  value?: unknown;
};

export type FilterDsl = {
  match_mode: MatchMode;
  conditions: Condition[];
};

export type SavedViewKind = "filter_series" | "cbl" | "system" | "collection";

/** Built-in rail identifier when `kind === 'system'` or, for the
 *  per-user manual collection, `'want_to_read'` when `kind === 'collection'`. */
export type SystemRailKey = "continue_reading" | "on_deck" | "want_to_read";

export type SavedViewView = {
  id: string;
  /** `'filter_series' | 'cbl'`. */
  kind: SavedViewKind;
  /** `null` for system views (admin-curated, visible to every user). */
  user_id?: string | null;
  is_system: boolean;
  name: string;
  description?: string | null;
  custom_year_start?: number | null;
  custom_year_end?: number | null;
  custom_tags: string[];
  /** Populated when `kind === 'filter_series'`. */
  match_mode?: MatchMode | null;
  conditions?: Condition[] | null;
  sort_field?: SavedViewSortField | null;
  sort_order?: SortOrder | null;
  result_limit?: number | null;
  /** Populated when `kind === 'cbl'`. */
  cbl_list_id?: string | null;
  /** Whether the calling user has this view pinned. */
  pinned: boolean;
  pinned_position?: number | null;
  /** Whether the calling user wants this view to appear in the
   *  left sidebar's "Saved views" section. */
  show_in_sidebar: boolean;
  /** Identifies the built-in rail when `kind === 'system'`. */
  system_key?: SystemRailKey | null;
  /** Per-user icon override key. `null` falls back to a kind-based
   *  default resolved client-side via the rail-icon registry. */
  icon?: string | null;
  created_at: string;
  updated_at: string;
};

export type SavedViewListView = {
  items: SavedViewView[];
};

export type CreateSavedViewReq = {
  kind: SavedViewKind;
  name: string;
  description?: string | null;
  custom_year_start?: number | null;
  custom_year_end?: number | null;
  custom_tags?: string[] | null;
  /** Required when `kind === 'filter_series'`. */
  filter?: FilterDsl | null;
  sort_field?: SavedViewSortField | null;
  sort_order?: SortOrder | null;
  result_limit?: number | null;
  /** Required when `kind === 'cbl'`. */
  cbl_list_id?: string | null;
};

export type UpdateSavedViewReq = {
  name?: string | null;
  description?: string | null;
  custom_year_start?: number | null;
  custom_year_end?: number | null;
  custom_tags?: string[] | null;
  filter?: FilterDsl | null;
  sort_field?: SavedViewSortField | null;
  sort_order?: SortOrder | null;
  result_limit?: number | null;
};

export type PreviewReq = {
  filter: FilterDsl;
  sort_field: SavedViewSortField;
  sort_order: SortOrder;
  result_limit: number;
};

export type PinView = {
  view_id: string;
  pinned: boolean;
  position?: number | null;
};

// ---------- CBL reading lists ----------

export type CblSourceKind = "upload" | "url" | "catalog";

export type CblMatchStatus = "matched" | "ambiguous" | "missing" | "manual";

export type CblStatsView = {
  total: number;
  matched: number;
  ambiguous: number;
  missing: number;
  manual: number;
  /** Count of matched entries the calling user has finished. Drives the
   *  per-user reading-progress pill on the home rail header. */
  read_count: number;
};

export type CblListView = {
  id: string;
  owner_user_id?: string | null;
  source_kind: CblSourceKind;
  source_url?: string | null;
  catalog_source_id?: string | null;
  catalog_path?: string | null;
  github_blob_sha?: string | null;
  parsed_name: string;
  parsed_matchers_present: boolean;
  num_issues_declared?: number | null;
  description?: string | null;
  refresh_schedule?: string | null;
  imported_at: string;
  last_refreshed_at?: string | null;
  last_match_run_at?: string | null;
  created_at: string;
  updated_at: string;
  stats: CblStatsView;
};

export type CblListListView = {
  items: CblListView[];
};

export type CblEntryView = {
  id: string;
  position: number;
  series_name: string;
  issue_number: string;
  volume?: string | null;
  year?: string | null;
  cv_series_id?: number | null;
  cv_issue_id?: number | null;
  matched_issue_id?: string | null;
  match_status: CblMatchStatus;
  match_method?: string | null;
  match_confidence?: number | null;
  ambiguous_candidates?: unknown;
  matched_at?: string | null;
};

export type CblDetailView = CblListView & {
  entries: CblEntryView[];
};

export type CreateCblListReq =
  | {
      kind: "url";
      url: string;
      name?: string | null;
      description?: string | null;
      refresh_schedule?: string | null;
    }
  | {
      kind: "catalog";
      catalog_source_id: string;
      catalog_path: string;
      name?: string | null;
      description?: string | null;
      refresh_schedule?: string | null;
    };

export type UpdateCblListReq = {
  description?: string | null;
  refresh_schedule?: string | null;
};

export type ManualMatchReq = {
  issue_id: string;
};

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

export type RefreshLogEntryView = {
  id: string;
  ran_at: string;
  trigger: string;
  upstream_changed: boolean;
  prev_blob_sha?: string | null;
  new_blob_sha?: string | null;
  added_count: number;
  removed_count: number;
  reordered_count: number;
  rematched_count: number;
  diff_summary?: unknown;
};

export type RefreshLogListView = {
  items: RefreshLogEntryView[];
};

export type CatalogSourceView = {
  id: string;
  display_name: string;
  github_owner: string;
  github_repo: string;
  github_branch: string;
  enabled: boolean;
  last_indexed_at?: string | null;
};

export type CatalogSourceListView = {
  items: CatalogSourceView[];
};

export type CatalogEntryView = {
  path: string;
  name: string;
  publisher: string;
  sha: string;
  size: number;
};

export type CatalogEntriesView = {
  source_id: string;
  items: CatalogEntryView[];
};

export type CreateCatalogSourceReq = {
  display_name: string;
  github_owner: string;
  github_repo: string;
  github_branch?: string | null;
};

export type UpdateCatalogSourceReq = {
  display_name?: string | null;
  github_branch?: string | null;
  enabled?: boolean | null;
};

// ─── Collections (markers + collections M2) ───

export type CollectionEntryKind = "series" | "issue";

export type CollectionEntryView = {
  id: string;
  position: number;
  entry_kind: CollectionEntryKind;
  added_at: string;
  /** Populated when `entry_kind === 'series'`. `null` if the underlying
   *  series was cascade-deleted between insert and read. Returned as a
   *  full `SeriesView` so the home rail + collection detail page can
   *  drop straight into `<SeriesCard>`. */
  series?: SeriesView | null;
  /** Populated when `entry_kind === 'issue'`. */
  issue?: IssueSummaryView | null;
};

export type CollectionEntriesView = {
  items: CollectionEntryView[];
  next_cursor?: string | null;
  /** First-page total only (omitted on subsequent pages). */
  total?: number | null;
};

export type CreateCollectionReq = {
  name: string;
  description?: string | null;
};

export type UpdateCollectionReq = {
  name?: string | null;
  /** Send `""` (empty string) to clear; omit to leave unchanged. */
  description?: string | null;
};

export type AddEntryReq = {
  entry_kind: CollectionEntryKind;
  /** Series UUID or issue id (TEXT/BLAKE3 hex). */
  ref_id: string;
};

export type ReorderEntriesReq = {
  /** Must include every current entry id — partial reorders rejected. */
  entry_ids: string[];
};

// ─── Markers (markers + collections M5) ───

export type MarkerKind = "bookmark" | "note" | "highlight";
export type MarkerShape = "rect" | "text" | "image";

/** Rect region anchored on the page's natural pixel dims. All four
 *  positional fields are 0-100 percent floats so the overlay survives
 *  resize / zoom / fit-mode changes without recomputation. `shape`
 *  classifies the selection mode the client used. */
export type MarkerRegion = {
  x: number;
  y: number;
  w: number;
  h: number;
  shape: MarkerShape;
};

/** OCR'd text + cropped-pixel hash metadata for text/image-aware
 *  highlights. v1 stores opaquely; future milestones will let users
 *  search by text and re-find a panel by its image hash. */
export type MarkerSelection = {
  text?: string | null;
  image_hash?: string | null;
  ocr_confidence?: number | null;
};

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

export type TagEntryView = {
  tag: string;
  count: number;
};

export type MarkerTagsView = {
  items: TagEntryView[];
};

export type MarkerTagMatch = "all" | "any";

export type MarkerListView = {
  items: MarkerView[];
  next_cursor?: string | null;
};

export type MarkerCountView = {
  total: number;
};

export type IssueMarkersView = {
  items: MarkerView[];
};

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
  /** Replace tag list. Send `[]` to clear, omit to leave unchanged. */
  tags?: string[];
};

// ────────────── Sessions (M5 — /me/sessions) ──────────────

export type SessionView = {
  id: string;
  created_at: string;
  last_used_at: string;
  expires_at: string;
  user_agent: string | null;
  ip: string | null;
  /** True when this row matches the caller's refresh cookie. */
  current: boolean;
};

export type SessionListView = {
  sessions: SessionView[];
};

export type RevokeAllSessionsResp = {
  /** Count of sessions transitioned from active → revoked. Doesn't
   *  include already-revoked / expired rows. */
  revoked: number;
};

// ────────────── App passwords (M7 — /me/app-passwords) ──────────────

/** App-password scope. `read` is the default and grants browse + page-stream
 *  + download. `read+progress` additionally lets the token write reading
 *  progress via the OPDS progress endpoint and the KOReader sync shim. */
export type AppPasswordScope = "read" | "read+progress";

export type AppPasswordView = {
  id: string;
  label: string;
  scope: AppPasswordScope;
  created_at: string;
  last_used_at: string | null;
};

export type AppPasswordListView = {
  items: AppPasswordView[];
};

export type AppPasswordCreatedView = {
  id: string;
  label: string;
  scope: AppPasswordScope;
  created_at: string;
  /** The plaintext token. Shown once and never retrievable again. */
  plaintext: string;
};

export type CreateAppPasswordReq = {
  label: string;
  /** Optional. Defaults server-side to `read` when omitted. */
  scope?: AppPasswordScope;
};
