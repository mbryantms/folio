# User-data export format

`GET /api/me/export` returns one JSON document with everything the
calling user owns. It is the app-level dump that a `pg_dump` is not: a
Postgres backup restores the whole server, but it cannot survive a
library re-import that changes issue ids, and it cannot be handed to
another Folio instance. The export can.

There is **no import endpoint** (owner decision, roadmap 2.2). The
document is a durable, self-describing copy of the user's data and the
base shape for the notes export (roadmap 5.1). Restoring a server is
still `pg_restore` per [`docs/install/backup.md`](../install/backup.md).

- Handler: [`crates/server/src/api/account_export.rs`](../../crates/server/src/api/account_export.rs)
- Tests: [`crates/server/tests/user_export.rs`](../../crates/server/tests/user_export.rs)
  (section counts, identity keys, per-user isolation) and the query-count
  bound in [`perf_regressions.rs`](../../crates/server/tests/perf_regressions.rs)
- UI: "Export my data" on `/settings/account`
  ([`DataExportCard`](../../web/components/settings/DataExportCard.tsx))
- Auth: cookie or Bearer; only the caller's rows. Rate-limited at
  6 / min / IP (`rate_limit::USER_EXPORT`).
- Response headers: `Content-Type: application/json`,
  `Content-Disposition: attachment; filename="folio-export-YYYY-MM-DD.json"`.

## Envelope

```json
{
  "format": "folio-user-export",
  "version": 1,
  "exported_at": "2026-09-29T10:15:00+00:00",
  "user": { "id": "<uuid>", "email": "a@example.com", "display_name": "A" },
  "sections": { ... }
}
```

| Field | Notes |
|---|---|
| `format` | Always `folio-user-export`. |
| `version` | Integer. Bumped on any breaking change to the envelope or a section (see the changelog at the end). Additive fields do not bump it. |
| `exported_at` | RFC 3339. |
| `user` | The exporting account. `email` is `null` for OIDC users without an email claim. |
| `sections` | One key per section below. Every list section is present even when empty (`[]`); `want_to_read` is `null` when never seeded; `preferences` is always an object. |

All timestamps are RFC 3339 strings. All ids are Folio's internal ids
(`uuid` for most tables, BLAKE3 hex for issues) and are included for
traceability, never as the only key.

## Identity keys

Every reference to an issue or a series carries enough to re-resolve it
on a rebuilt library, where `issues.id` may differ.

**`IssueRef`**

| Field | Type | Notes |
|---|---|---|
| `issue_id` | string | `issues.id` (BLAKE3 hex). |
| `content_hash` | string \| null | `issues.content_hash` — the primary portable key. |
| `series_id` | uuid \| null | `issues.series_id`. |
| `series_name` | string \| null | `series.name`. |
| `series_year` | int \| null | `series.year` (null when untagged). |
| `issue_number` | string \| null | `issues.number_raw`, exactly as tagged. |
| `library_slug` | string \| null | `libraries.slug`. |

**`SeriesRef`**

| Field | Type | Notes |
|---|---|---|
| `series_id` | uuid | `series.id`. |
| `series_name` | string \| null | `series.name`. |
| `series_year` | int \| null | `series.year`. |
| `library_slug` | string \| null | `libraries.slug`. |

Hydrated fields are `null` only when the referenced row no longer exists
(the row would normally have been cascade-deleted with it). Soft-removed
issues and series still hydrate — the export never drops a user row
because its target is pending removal.

Fallback resolution order for a consumer: `content_hash` →
`(series_name, series_year, issue_number)` → `issue_id`.

## Sections

### `progress` — `progress_records`

One row per issue the user has opened or marked.

| Field | Type |
|---|---|
| `issue` | `IssueRef` |
| `run` | int — reading-run counter; 0 is the first read, each explicit re-read opens the next run (see [`reading-progress.md`](reading-progress.md)) |
| `last_page` | int (0-based) within the current run |
| `percent` | float 0..1 |
| `finished` | bool |
| `finished_at` | timestamp \| null |
| `is_backfill` | bool — set by bulk/sync writes, not active reading |
| `device` | string \| null |
| `updated_at` | timestamp |

### `markers` — `markers`

Bookmarks, notes, favorites and highlights. Every column is exported.

| Field | Type |
|---|---|
| `id` | uuid |
| `issue` | `IssueRef` |
| `page_index` | int (0-based) |
| `kind` | `bookmark` \| `note` \| `favorite` \| `highlight` |
| `is_favorite` | bool (legacy per-marker star) |
| `tags` | string[] |
| `region` | object \| null — `{x, y, w, h, shape}` in 0–100 % of the page |
| `selection` | object \| null — `{text?, image_hash?, ocr_confidence?}` |
| `body` | string \| null — Markdown; required for notes |
| `color` | string \| null — palette token |
| `hidden_from_log` | bool |
| `created_at`, `updated_at` | timestamp |

### `collections` — `saved_views` (`kind = collection`) + `collection_entries`

User-authored collections. The system Want to Read collection is split
out into `want_to_read` and is **not** repeated here.

| Field | Type |
|---|---|
| `id` | uuid |
| `name` | string |
| `description` | string \| null |
| `system_key` | always `null` in this section |
| `custom_tags` | string[] |
| `preserve_canonical_order` | bool (OPDS feed ordering) |
| `created_at`, `updated_at` | timestamp |
| `entries` | `CollectionEntry[]` in `position` order |

`CollectionEntry`:

| Field | Type |
|---|---|
| `position` | int (0-based) |
| `entry_kind` | `issue` \| `series` |
| `issue` | `IssueRef` \| null — set when `entry_kind = issue` |
| `series` | `SeriesRef` \| null — set when `entry_kind = series` |
| `added_at` | timestamp |

### `want_to_read` — the per-user system collection

Same shape as one `collections` element, with `system_key = "want_to_read"`.
`null` when the user has never visited a surface that seeds it.

### `saved_views` — `saved_views` (`kind != collection`)

Filter views and CBL-backed views the user owns.

| Field | Type |
|---|---|
| `id` | uuid |
| `kind` | `filter_series` \| `cbl` |
| `name` | string |
| `description` | string \| null |
| `custom_year_start`, `custom_year_end` | int \| null |
| `custom_tags` | string[] |
| `match_mode` | `all` \| `any` \| null |
| `conditions` | array \| null — the filter DSL, `[{group_id, field, op, value}]` |
| `sort_field`, `sort_order` | string \| null |
| `result_limit` | int \| null |
| `cbl_list_id` | uuid \| null — for `kind = cbl` |
| `created_at`, `updated_at` | timestamp |

CBL lists themselves are not embedded; they have their own XML export
(`GET /api/me/cbl-lists/{id}/export`) and are re-importable from file
or URL.

### `ratings` — `user_ratings`

| Field | Type |
|---|---|
| `target_type` | `issue` \| `series` |
| `issue` | `IssueRef` \| null — set when `target_type = issue` |
| `series` | `SeriesRef` \| null — set when `target_type = series` |
| `rating` | float, 0..5 in half steps |
| `created_at`, `updated_at` | timestamp |

### `custom_pages` — `user_page` + `user_view_pins`

Every page including the system Home page (`is_system = true`), each
with its pinned rails.

| Field | Type |
|---|---|
| `id` | uuid |
| `name`, `slug` | string |
| `is_system` | bool |
| `position` | int |
| `description` | string \| null |
| `created_at`, `updated_at` | timestamp |
| `pins` | `ViewPin[]` in `position` order |

`ViewPin`:

| Field | Type |
|---|---|
| `view_id` | uuid |
| `view_kind` | `filter_series` \| `cbl` \| `system` \| `collection` \| null |
| `view_name` | string \| null |
| `view_system_key` | string \| null — e.g. `continue_reading`, `on_deck`, `want_to_read` |
| `position` | int |
| `pinned` | bool — rail visible on the page |
| `show_in_sidebar` | bool |
| `icon` | string \| null — Lucide key override |

Pins may reference system views the user does not own; `view_kind` /
`view_name` / `view_system_key` are hydrated so the pin is recognisable
after a rebuild.

### `sidebar` — `user_sidebar_entries`

Per-entry overrides of the left navigation. Missing entries mean
"default".

| Field | Type |
|---|---|
| `kind` | `builtin` \| `library` \| `view` \| `header` \| `spacer` |
| `ref_id` | string — registry key, library uuid, or view uuid |
| `visible` | bool |
| `position` | int |
| `label` | string \| null |

### `rail_dismissals` — `rail_dismissals`

| Field | Type |
|---|---|
| `target_kind` | `issue` \| `series` \| `cbl` |
| `target_id` | string |
| `dismissed_at` | timestamp |

### `reading_log` — `reading_sessions`

Every session the user owns, including ones hidden from the activity
feed (`hidden_from_log = true`).

| Field | Type |
|---|---|
| `id` | uuid |
| `issue` | `IssueRef` |
| `client_session_id` | string |
| `started_at`, `last_heartbeat_at` | timestamp |
| `ended_at` | timestamp \| null |
| `active_ms` | int |
| `distinct_pages_read`, `page_turns` | int |
| `start_page`, `end_page`, `furthest_page` | int (0-based) |
| `device` | string \| null |
| `view_mode` | `single` \| `double` \| `webtoon` \| null |
| `client_meta` | object |
| `hidden_from_log` | bool |

### `preferences` — columns on `users`

Raw stored values; `null` means "no preference, use the default".

| Field | Type |
|---|---|
| `default_reading_direction` | `ltr` \| `rtl` \| `auto` \| null |
| `default_fit_mode` | `width` \| `height` \| `original` \| null |
| `default_view_mode` | `single` \| `double` \| `webtoon` \| null |
| `default_page_strip` | bool |
| `default_page_animation` | `off` \| `slide` \| `fade` \| null |
| `default_cover_solo` | bool |
| `theme` | `system` \| `dark` \| `light` \| `amber` \| null |
| `accent_color` | string \| null |
| `density` | `comfortable` \| `compact` \| null |
| `keybinds` | object — `{ action_name: key_string }` reader overrides |
| `activity_tracking_enabled` | bool |
| `timezone` | IANA string |
| `reading_min_active_ms`, `reading_min_pages`, `reading_idle_ms` | int |
| `language` | BCP-47 tag |
| `exclude_from_aggregates` | bool |
| `show_marker_count` | bool |
| `opds_wtr_reorder`, `opds_progress_glyphs` | bool |
| `max_rails_per_page` | int |

## What is not included

- Anything not owned by the user: library metadata, issue metadata edits
  (these live on the issue and are covered by sidecar writeback), admin
  settings, other users' data.
- Credentials: password hash, TOTP secret, app passwords, sessions.
- CBL list bodies (see `saved_views` above).
- Library-access grants (an admin decision, not user data).

## Query shape

The handler issues one bounded query per section, one hydrate for pinned
views, and three IN-batched identity lookups (issues → series →
libraries, 1000 ids per batch). No per-row lookups; the bound is pinned
by `MAX_QUERIES_USER_EXPORT` in `perf_regressions.rs`. The document is
built in memory — its size is bounded by a single user's activity, not
by the library.

## Changelog

- **v1** (2026-09-29, WP-2.1) — initial shape.
