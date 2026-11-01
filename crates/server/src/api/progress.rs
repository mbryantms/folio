//! `POST /progress` (upsert), `GET /progress?since=…` (sync delta).
//!
//! Authoritative storage layer for reading progress, backed by the
//! `progress_records` table.
//!
//! **Cross-device conflict rule (the "reading run" model, WP-1.3).**
//! Every record carries a `run` counter (0 = first read). Within a run:
//!
//! - an *implicit* per-page write (`finished` omitted — what the reader
//!   sends as you turn pages) can only move `last_page` forward:
//!   `max(prev, page)`. A stale debounced write from another device can
//!   never drag the position backwards;
//! - an *explicit* write (`finished: true|false` — mark read / unread /
//!   the last-page auto-finish) stores `page` as given; the user said so;
//! - `finished` stays sticky on implicit writes (a bookmark jump never
//!   un-finishes an issue).
//!
//! `restart: true` opens run `n+1` at `page` (normally 0) with
//! `finished = false` — "Read from beginning", or reopening a finished
//! issue from the cover. A write tagged with a `run` older than the
//! server's is ignored: that device is still on the previous read and
//! silently follows the new run on its next open. Clients that don't
//! send `run` (OPDS / KOReader / Komga shims) write into the current run.
//! The reader tags a restart with the run it is leaving, which makes the
//! restart idempotent (targets `run + 1`, applied once) so the offline
//! progress outbox (WP-4.5) can replay any queued write safely.
//!
//! **Page-hash anchoring (WP-6.2).** Every single-issue write
//! (`upsert_for_run`, shared by the reader, OPDS, KOReader and Komga
//! shims) records `page_hash`, the hash of the page image at
//! `last_page`, so a rescan of a replaced archive keeps the resume
//! position on the same image (`reading::page_remap`). Bulk mark
//! read/unread writes (series, matching, CBL, multi-select) store no
//! hash: they anchor "first page" / "last page", which the ordinal
//! fallback already preserves.
//!
//! The spec's original §9 plan to swap this for Automerge CRDT sync was
//! reconsidered and dropped on 2026-05-15 (see spec §9 decision note).
//!
//! Error envelope: every error response flows through the shared
//! `crate::api::error` helper. The `X-Progress-Api` header that used to
//! ride every response was dropped in audit-remediation M3 — the
//! versioning shim was unused by clients and represented premature
//! infrastructure.

use axum::{
    Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::{
    issue,
    progress_record::{self, Entity as ProgressEntity},
};
use sea_orm::{ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::error;
use crate::api::issue_card::IssueCardRow;
use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        // Single-record `/progress` endpoints aren't in the OpenAPI spec —
        // they predate the `/me/progress/bulk` endpoints below and are kept
        // as a transitional surface. Register them as plain routes so the
        // spec only documents the bulk shape clients should target.
        .route(
            "/progress",
            axum::routing::post(upsert).get(list),
        )
        .routes(routes!(upsert_series))
        .routes(routes!(upsert_series_matching))
        .routes(routes!(upsert_bulk))
        .routes(routes!(upsert_series_bulk))
}

#[derive(Debug, Deserialize)]
pub struct UpsertReq {
    pub issue_id: String,
    pub page: i32,
    /// Optional — when present, sets the `finished` flag explicitly
    /// (e.g. "Mark as read", "Mark as unread", or the reader's
    /// last-page auto-finish). When `None`, the existing `finished`
    /// flag is preserved so per-page progress writes from the reader
    /// cannot accidentally clear a previously-finished issue (e.g.
    /// when the user jumps mid-issue via a bookmark deep-link).
    #[serde(default)]
    pub finished: Option<bool>,
    #[serde(default)]
    pub device: Option<String>,
    /// The reading run this write belongs to (from the record the
    /// client last read). A write tagged with a run older than the
    /// server's is ignored — that device is still on a previous read.
    /// Omitted by legacy clients: treated as the current run.
    #[serde(default)]
    pub run: Option<i32>,
    /// Open a new reading run at `page` (normally 0): bumps `run` and
    /// clears `finished`. The reader sends it on "Read from beginning"
    /// and when a finished issue is reopened from the cover.
    ///
    /// With `run: r` the restart is idempotent: it targets run `r + 1`
    /// and is applied at most once, so a replayed write (offline
    /// outbox, WP-4.5) never opens a second new run.
    #[serde(default)]
    pub restart: bool,
}

#[derive(Debug, Serialize)]
pub struct ProgressView {
    pub issue_id: String,
    pub page: i32,
    pub percent: f64,
    pub finished: bool,
    /// Reading run this position belongs to. Clients echo it back on
    /// per-page writes; see the module doc for the conflict rule.
    pub run: i32,
    pub updated_at: String,
    /// Authoritative timestamp the issue was flipped to finished;
    /// `None` for in-progress / unread rows.
    pub finished_at: Option<String>,
}

impl From<progress_record::Model> for ProgressView {
    fn from(m: progress_record::Model) -> Self {
        Self {
            issue_id: m.issue_id,
            page: m.last_page,
            percent: m.percent,
            finished: m.finished,
            run: m.run,
            updated_at: m.updated_at.to_rfc3339(),
            finished_at: m.finished_at.map(|t| t.to_rfc3339()),
        }
    }
}

/// Post-audit T3.7: partial failures return 200 with `failed` counted;
/// only a request where *every* attempted write failed is a 500, carrying
/// the first error so the client can say why.
fn all_writes_failed(
    updated: u32,
    skipped: u32,
    failed: u32,
    first_error: Option<&str>,
) -> Option<Response> {
    if failed > 0 && updated == 0 && skipped == 0 {
        return Some(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            &format!(
                "every progress write failed: {}",
                first_error.unwrap_or("unknown error")
            ),
        ));
    }
    None
}

/// The `finished_at` flip rule (encoded in [`PROGRESS_UPSERT_SQL_TEMPLATE`]):
///   - false → true:  stamp `now`
///   - true  → false: clear to `None`
///   - true  → true:  keep the previous timestamp (backfilled rows may
///     have inherited their `updated_at`; respect that)
///   - false → false: stay `None`
///
/// Resolve the `is_backfill` value for a write. The flag means "this
/// finish came from a catalog/sync write, not active reading"; it's
/// `true` only when (a) the new state is finished and (b) the caller
/// explicitly opted in via `backfill: true`. Every unread write
/// clears it back to `false`, regardless of the previous row's flag
/// — the user just said this issue isn't done, so the catalog/sync
/// origin is no longer load-bearing.
///
/// Per-issue reader writes pass `req_backfill = false` unconditionally
/// (the reader is by definition active reading). Only the bulk-mark
/// and whole-series endpoints expose the toggle.
pub(crate) fn resolve_is_backfill(next_finished: bool, req_backfill: bool) -> bool {
    next_finished && req_backfill
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// RFC 3339 timestamp; only rows updated strictly after this are returned.
    pub since: Option<String>,
    /// When present, return only the caller's record for this issue
    /// (zero or one row). The SSR issue/reader pages need exactly one
    /// record; without this they fetch the user's entire progress
    /// history to `.find()` it (audit G2).
    pub issue_id: Option<String>,
}

#[handler]
pub async fn upsert(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<UpsertReq>,
) -> Response {
    if req.page < 0 {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "page must be >= 0",
        );
    }
    // ACL: confirm the user can see the issue at all.
    let issue_row = match issue::Entity::find_by_id(req.issue_id.clone())
        .one(&app.db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "issue not found"),
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    if !access::issue_visible(&app, &user, &issue_row).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }

    let result = upsert_for_run(
        &app,
        user.id,
        &issue_row,
        req.page,
        req.finished,
        req.device,
        req.run,
        req.restart,
    )
    .await;
    match result {
        Ok(model) => (StatusCode::OK, Json(ProgressView::from(model))).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "progress upsert failed");
            super::error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

/// Crate-wide upsert helper used by `POST /progress` AND the OPDS
/// progress endpoints. The caller is responsible for ACL: this fn
/// trusts that `issue_row` is one the `user_id` is allowed to read.
/// Keeps the `finished`-is-sticky semantics intact across all callers.
///
/// Writes into the current reading run without a run tag (the shims
/// that call this don't know about runs). See [`upsert_for_run`].
pub(crate) async fn upsert_for(
    app: &AppState,
    user_id: uuid::Uuid,
    issue_row: &issue::Model,
    page: i32,
    finished: Option<bool>,
    device: Option<String>,
) -> Result<progress_record::Model, sea_orm::DbErr> {
    upsert_for_run(app, user_id, issue_row, page, finished, device, None, false).await
}

/// [`upsert_for`] with the reading-run inputs. The conflict rule (module
/// doc):
///
/// - `restart`: open run `prev.run + 1` at `page`, `finished` cleared
///   unless explicitly set. With a `run: r` tag the restart targets
///   `r + 1` and is idempotent: already at `r + 1` → an ordinary write
///   into that run; further ahead → ignored as an older run;
/// - `run` older than the stored run: the write is ignored and the
///   stored record returned unchanged;
/// - implicit write (`finished == None`): `last_page = max(prev, page)`;
/// - explicit write: `last_page = page`.
///
/// **Atomic (post-audit T1.3).** The whole rule is one
/// `INSERT … ON CONFLICT DO UPDATE … RETURNING` statement, so the
/// decision is made against the row *as locked by this statement*, not
/// a row read moments earlier: two devices writing at once can no
/// longer lose the furthest page, a restart can no longer be overwritten
/// back to the old run by a stale in-flight write, and two first writes
/// for the same issue no longer race into a primary-key violation. The
/// page hash is captured before the statement (archive I/O never sits
/// between read and write).
///
/// Truth table, `prev` = stored row, `r` = client `run` tag (may be
/// absent), `R` = `restart`:
///
/// | prev row | R | r                 | effect                                      |
/// |----------|---|-------------------|---------------------------------------------|
/// | none     | * | absent            | insert, run 0                               |
/// | none     | T | r                 | insert, run `max(r + 1, 0)`                 |
/// | none     | F | r                 | insert, run 0                               |
/// | some     | F | absent            | write into `prev.run`                       |
/// | some     | F | r < prev.run      | ignored                                     |
/// | some     | F | r ≥ prev.run      | write into `prev.run`                       |
/// | some     | T | absent            | restart: run `prev.run + 1`                 |
/// | some     | T | r ≥ prev.run      | restart: run `prev.run + 1`                 |
/// | some     | T | r = prev.run − 1  | already restarted: write into `prev.run`    |
/// | some     | T | r < prev.run − 1  | ignored                                     |
///
/// "write into run": implicit (`finished` absent) keeps
/// `max(prev.last_page, page)` and the stored `finished`; explicit sets
/// both as given. `finished_at` follows the flip rule documented above. The
/// stored page hash is kept when the resulting page is unchanged;
/// otherwise the hash captured for `page` is stored when `page` is the
/// resulting page, else NULL (ordinal fallback).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn upsert_for_run(
    app: &AppState,
    user_id: uuid::Uuid,
    issue_row: &issue::Model,
    page: i32,
    finished: Option<bool>,
    device: Option<String>,
    run: Option<i32>,
    restart: bool,
) -> Result<progress_record::Model, sea_orm::DbErr> {
    use sea_orm::{DbBackend, Statement};

    let now = Utc::now().fixed_offset();
    // Captured up front so no archive I/O sits between the row read and
    // the row write — the statement below is the only DB round-trip.
    let page_hash = crate::reading::page_hash::capture(app, issue_row, page).await;
    let page_count = issue_row.page_count.unwrap_or(0);

    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        PROGRESS_UPSERT_SQL.as_str(),
        [
            user_id.into(),
            issue_row.id.clone().into(),
            page.into(),
            finished.into(),
            now.into(),
            device.into(),
            restart.into(),
            run.into(),
            page_hash.into(),
            page_count.into(),
            // Per-issue reader writes are active reading, never backfill,
            // and take the computed percent.
            false.into(),
            Option::<f64>::None.into(),
        ],
    );
    let row = ProgressEntity::find()
        .from_raw_sql(stmt)
        .one(&app.db)
        .await?
        .ok_or_else(|| {
            sea_orm::DbErr::RecordNotFound("progress upsert returned no row".to_owned())
        })?;
    if let Some(r) = run
        && row.run > r + i32::from(restart)
    {
        tracing::debug!(
            issue_id = %issue_row.id,
            client_run = r,
            server_run = row.run,
            "progress write ignored: client is on an older reading run"
        );
    }
    Ok(row)
}

/// Explicit bulk write (mark read / unread) through the same atomic
/// statement as [`upsert_for_run`] (post-audit T3.7): no run tag, no
/// restart, no page hash (bulk marks anchor "first / last page", which
/// the ordinal fallback preserves — and capturing would open hundreds of
/// archives), the request's `is_backfill` flag, and an exact `percent`
/// (1.0 / 0.0) instead of `page / page_count`. Concurrent reader writes
/// can no longer be lost to a bulk mark's read-then-write.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn upsert_explicit(
    app: &AppState,
    user_id: uuid::Uuid,
    issue_id: &str,
    page_count: Option<i32>,
    page: i32,
    percent: f64,
    finished: bool,
    device: Option<String>,
    is_backfill: bool,
) -> Result<progress_record::Model, sea_orm::DbErr> {
    use sea_orm::{DbBackend, Statement};
    let now = Utc::now().fixed_offset();
    let page_count = page_count.unwrap_or(0);
    let stmt = Statement::from_sql_and_values(
        DbBackend::Postgres,
        PROGRESS_UPSERT_SQL.as_str(),
        [
            user_id.into(),
            issue_id.to_owned().into(),
            page.into(),
            Some(finished).into(),
            now.into(),
            device.into(),
            false.into(),
            Option::<i32>::None.into(),
            Option::<String>::None.into(),
            page_count.into(),
            is_backfill.into(),
            Some(percent).into(),
        ],
    );
    ProgressEntity::find()
        .from_raw_sql(stmt)
        .one(&app.db)
        .await?
        .ok_or_else(|| sea_orm::DbErr::RecordNotFound("progress upsert returned no row".to_owned()))
}

/// The single-statement reading-run merge (see [`upsert_for_run`] for
/// the truth table). Parameters:
///
/// `$1` user_id · `$2` issue_id · `$3` page · `$4` finished (nullable) ·
/// `$5` now · `$6` device (nullable) · `$7` restart · `$8` run tag
/// (nullable) · `$9` page hash for `$3` (nullable) · `$10` page count ·
/// `$11` is_backfill (the reader passes `false`; bulk marks pass the
/// request's flag) · `$12` explicit percent (nullable — bulk marks store
/// exactly 1.0 / 0.0 instead of `page / page_count`).
///
/// In the `DO UPDATE` arm `progress_records.*` is the locked existing
/// row. Every column is written through a `CASE` on the "ignored"
/// predicate so an ignored write leaves the row byte-identical
/// (`updated_at` included) and still `RETURNING`s it.
const PROGRESS_UPSERT_SQL_TEMPLATE: &str = r#"
INSERT INTO progress_records
    (user_id, issue_id, last_page, percent, finished, finished_at,
     updated_at, device, is_backfill, run, page_hash)
VALUES (
    $1::uuid, $2::text, $3::int4,
    COALESCE(
        $12::float8,
        CASE WHEN $10::int4 > 0
             THEN LEAST(1.0::float8, GREATEST(0.0::float8, $3::int4::float8 / $10::int4::float8))
             ELSE 0.0::float8 END
    ),
    COALESCE($4::bool, false),
    CASE WHEN COALESCE($4::bool, false) THEN $5::timestamptz ELSE NULL END,
    $5::timestamptz, $6::text, $11::bool,
    CASE WHEN $7::bool AND $8::int4 IS NOT NULL THEN GREATEST($8::int4 + 1, 0) ELSE 0 END,
    $9::text
)
ON CONFLICT (user_id, issue_id) DO UPDATE SET
    last_page = CASE WHEN __IGNORED__ THEN progress_records.last_page ELSE __NEXT_PAGE__ END,
    percent = CASE WHEN __IGNORED__ THEN progress_records.percent
                   WHEN $12::float8 IS NOT NULL THEN $12::float8
                   WHEN $10::int4 > 0
                   THEN LEAST(1.0::float8, GREATEST(0.0::float8, (__NEXT_PAGE__)::float8 / $10::int4::float8))
                   ELSE 0.0::float8 END,
    finished = CASE WHEN __IGNORED__ THEN progress_records.finished ELSE __NEXT_FINISHED__ END,
    finished_at = CASE WHEN __IGNORED__ THEN progress_records.finished_at
                       WHEN NOT progress_records.finished AND __NEXT_FINISHED__ THEN $5::timestamptz
                       WHEN progress_records.finished AND __NEXT_FINISHED__
                            THEN COALESCE(progress_records.finished_at, $5::timestamptz)
                       ELSE NULL END,
    updated_at = CASE WHEN __IGNORED__ THEN progress_records.updated_at ELSE $5::timestamptz END,
    device = CASE WHEN __IGNORED__ THEN progress_records.device ELSE $6::text END,
    is_backfill = CASE WHEN __IGNORED__ THEN progress_records.is_backfill ELSE $11::bool END,
    run = CASE WHEN __IGNORED__ THEN progress_records.run
               WHEN __RESTARTS__ THEN progress_records.run + 1
               ELSE progress_records.run END,
    page_hash = CASE WHEN __IGNORED__ THEN progress_records.page_hash
                     WHEN (__NEXT_PAGE__) = progress_records.last_page
                          AND progress_records.page_hash IS NOT NULL
                          THEN progress_records.page_hash
                     WHEN (__NEXT_PAGE__) = $3::int4 THEN $9::text
                     ELSE NULL END
RETURNING *
"#;

/// A write is ignored when the client's run tag is behind the stored
/// run — behind by more than one when restarting (a restart of run `r`
/// targets `r + 1`, so a stored `r + 1` is "already restarted", not
/// "older").
const IGNORED: &str = "($8::int4 IS NOT NULL AND progress_records.run > $8::int4 + \
                       (CASE WHEN $7::bool THEN 1 ELSE 0 END))";
/// A restart opens the next run unless a tag says it already happened
/// (`progress_records.run > r`).
const RESTARTS: &str = "($7::bool AND ($8::int4 IS NULL OR progress_records.run <= $8::int4))";
/// Restart → `page`; implicit write → furthest; explicit → `page`.
const NEXT_PAGE: &str = "(CASE WHEN __RESTARTS__ THEN $3::int4 \
                          WHEN $4::bool IS NULL THEN GREATEST(progress_records.last_page, $3::int4) \
                          ELSE $3::int4 END)";
/// Restart → explicit value or `false`; otherwise sticky unless explicit.
const NEXT_FINISHED: &str = "(CASE WHEN __RESTARTS__ THEN COALESCE($4::bool, false) \
                              ELSE COALESCE($4::bool, progress_records.finished) END)";

static PROGRESS_UPSERT_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let next_page = NEXT_PAGE.replace("__RESTARTS__", RESTARTS);
    let next_finished = NEXT_FINISHED.replace("__RESTARTS__", RESTARTS);
    PROGRESS_UPSERT_SQL_TEMPLATE
        .replace("__NEXT_PAGE__", &next_page)
        .replace("__NEXT_FINISHED__", &next_finished)
        .replace("__RESTARTS__", RESTARTS)
        .replace("__IGNORED__", IGNORED)
});

#[handler]
pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> Response {
    let mut query = ProgressEntity::find()
        .filter(progress_record::Column::UserId.eq(user.id))
        .order_by_asc(progress_record::Column::UpdatedAt);
    if let Some(issue_id) = q.issue_id.as_deref() {
        // No ACL walk needed: the UserId filter above already scopes
        // to the caller's own records, and a progress row's existence
        // for an issue the user once read is theirs to see.
        query = query.filter(progress_record::Column::IssueId.eq(issue_id));
    }
    if let Some(since) = q.since.as_deref() {
        match chrono::DateTime::parse_from_rfc3339(since) {
            Ok(ts) => {
                query = query.filter(progress_record::Column::UpdatedAt.gt(ts));
            }
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "validation",
                    "since must be RFC3339",
                );
            }
        }
    }
    let rows = match query.all(&app.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "progress list failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let views: Vec<ProgressView> = rows.into_iter().map(Into::into).collect();
    (StatusCode::OK, Json(serde_json::json!({"records": views}))).into_response()
}

/// Body for `POST /series/{id}/progress` — bulk read/unread for every active
/// issue in the series. `finished=true` writes a "fully read" record for each
/// (page = page_count - 1, percent = 1.0); `finished=false` resets to "unread"
/// (page = 0, percent = 0.0). Soft-deleted issues are excluded — they're not
/// visible to the user and writing progress for them would clutter the table.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpsertSeriesReq {
    pub finished: bool,
    /// Optional client tag; mirrors the per-issue endpoint so the same
    /// device label rolls through for sync-aware UIs.
    #[serde(default)]
    pub device: Option<String>,
    /// "Updating my collection — don't count toward today's reading
    /// activity." When `true` and `finished == true`, every written
    /// row carries `is_backfill = true` and is filtered out of the
    /// reading log, heatmap, daily-pages stat, streak counter, and
    /// the Just Finished sort. `false` (default) preserves the
    /// pre-v0.5.7 behaviour of treating bulk-marks as active reading.
    /// Ignored when `finished == false` — unread writes always clear
    /// the flag.
    #[serde(default)]
    pub backfill: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UpsertSeriesResp {
    /// Issue rows that received a new or updated progress record.
    pub updated: u32,
    /// Issues skipped because the user already has a matching `finished`
    /// state. Avoids unnecessary write traffic on idle re-clicks.
    pub skipped: u32,
    /// Issues whose write (or pre-write lookup) failed. Post-audit T3.7:
    /// these used to vanish into a log line, so "mark all read" could
    /// report success while rows were left untouched. `first_error` is
    /// the first failure's message; the handler answers 500 only when
    /// *every* attempted write failed.
    #[serde(default)]
    pub failed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_error: Option<String>,
}

#[utoipa::path(
    operation_id = "progress_upsert_series",    post,
    path = "/series/{slug}/progress",
    params(("slug" = String, Path,)),
    request_body = UpsertSeriesReq,
    responses(
        (status = 200, body = UpsertSeriesResp),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn upsert_series(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Json(req): Json<UpsertSeriesReq>,
) -> Response {
    let srow = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !access::series_visible(&app, &user, &srow).await {
        return error(StatusCode::NOT_FOUND, "not_found", "series not found");
    }

    // Only active, on-disk issues. The user can't read a removed issue, so
    // the read-state for those is meaningless and the row would clutter
    // future "unread series" queries.
    // WP-3.6: only id / page_count are read — project, don't hydrate the
    // wide rows of every issue in the series.
    let issues = match issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(srow.id))
        .filter(issue::Column::State.eq(issue::IssueState::Active))
        .filter(issue::Column::RemovedAt.is_null())
        .into_partial_model::<IssueCardRow>()
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "series-progress issue lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let mut updated: u32 = 0;
    let mut skipped: u32 = 0;
    let mut failed: u32 = 0;
    let mut first_error: Option<String> = None;

    for iss in issues {
        let target_page = if req.finished {
            iss.page_count.unwrap_or(1).max(1) - 1
        } else {
            0
        };
        let target_percent = if req.finished { 1.0 } else { 0.0 };

        // Read existing record so a redundant click doesn't bump
        // updated_at (the sync-delta endpoint orders on updated_at).
        let existing = match ProgressEntity::find_by_id((user.id, iss.id.clone()))
            .one(&app.db)
            .await
        {
            Ok(opt) => opt,
            Err(e) => {
                tracing::warn!(error = %e, "series-progress lookup failed");
                continue;
            }
        };

        match existing {
            Some(row) if row.finished == req.finished && row.last_page == target_page => {
                skipped += 1;
                continue;
            }
            Some(_) | None => {
                // Post-audit T3.7: one atomic statement (see `upsert_explicit`)
                // instead of read-then-update/insert, and a failure is counted
                // instead of vanishing into the log.
                let is_backfill = resolve_is_backfill(req.finished, req.backfill);
                match upsert_explicit(
                    &app,
                    user.id,
                    &iss.id,
                    iss.page_count,
                    target_page,
                    target_percent,
                    req.finished,
                    req.device.clone(),
                    is_backfill,
                )
                .await
                {
                    Ok(_) => updated += 1,
                    Err(e) => {
                        tracing::warn!(error = %e, issue_id = %iss.id, "series-progress write failed");
                        failed += 1;
                        first_error.get_or_insert_with(|| e.to_string());
                    }
                }
            }
        }
    }

    if let Some(resp) = all_writes_failed(updated, skipped, failed, first_error.as_deref()) {
        return resp;
    }
    (
        StatusCode::OK,
        Json(UpsertSeriesResp {
            updated,
            skipped,
            failed,
            first_error,
        }),
    )
        .into_response()
}

/// Body for `POST /series/{slug}/progress-matching` — bulk read/unread for
/// every active issue in the series matching the current UI query. This is the
/// server-side companion to "Select all matching": the client sends the same
/// search text it used for `/series/{slug}/issues?q=...`, and the server walks
/// the complete result set instead of requiring the browser to load every
/// cursor page first.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpsertSeriesMatchingReq {
    pub finished: bool,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub backfill: bool,
    #[serde(default)]
    pub q: Option<String>,
}

#[utoipa::path(
    operation_id = "progress_upsert_series_matching",    post,
    path = "/series/{slug}/progress-matching",
    params(("slug" = String, Path,)),
    request_body = UpsertSeriesMatchingReq,
    responses(
        (status = 200, body = UpsertBulkResp),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn upsert_series_matching(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Json(req): Json<UpsertSeriesMatchingReq>,
) -> Response {
    let srow = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !access::series_visible(&app, &user, &srow).await {
        return error(StatusCode::NOT_FOUND, "not_found", "series not found");
    }

    let mut query = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(srow.id))
        .filter(issue::Column::State.eq(issue::IssueState::Active))
        .filter(issue::Column::RemovedAt.is_null());

    if let Some(raw) = req.q.as_deref() {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            // Case-insensitive, multi-term: each word must match the issue
            // title OR its number, so "spider 5" narrows by both.
            use crate::util::search::{col_ilike, ilike_pattern};
            for token in trimmed.split_whitespace() {
                let pat = ilike_pattern(token);
                query = query.filter(
                    Condition::any()
                        .add(col_ilike(issue::Column::Title, &pat))
                        .add(col_ilike(issue::Column::NumberRaw, &pat)),
                );
            }
        }
    }

    let issues = match query.all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "series matching-progress issue lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let mut updated: u32 = 0;
    let mut skipped: u32 = 0;
    let mut failed: u32 = 0;
    let mut first_error: Option<String> = None;

    for iss in issues {
        let target_page = if req.finished {
            iss.page_count.unwrap_or(1).max(1) - 1
        } else {
            0
        };
        let target_percent = if req.finished { 1.0 } else { 0.0 };
        let existing = match ProgressEntity::find_by_id((user.id, iss.id.clone()))
            .one(&app.db)
            .await
        {
            Ok(opt) => opt,
            Err(e) => {
                tracing::warn!(error = %e, issue_id = %iss.id, "series matching-progress lookup failed");
                failed += 1;
                first_error.get_or_insert_with(|| e.to_string());
                continue;
            }
        };

        match existing {
            Some(row) if row.finished == req.finished && row.last_page == target_page => {
                skipped += 1;
            }
            Some(_) | None => {
                // Post-audit T3.7: one atomic statement (see `upsert_explicit`)
                // instead of read-then-update/insert, and a failure is counted
                // instead of vanishing into the log.
                let is_backfill = resolve_is_backfill(req.finished, req.backfill);
                match upsert_explicit(
                    &app,
                    user.id,
                    &iss.id,
                    iss.page_count,
                    target_page,
                    target_percent,
                    req.finished,
                    req.device.clone(),
                    is_backfill,
                )
                .await
                {
                    Ok(_) => updated += 1,
                    Err(e) => {
                        tracing::warn!(error = %e, issue_id = %iss.id, "series matching-progress write failed");
                        failed += 1;
                        first_error.get_or_insert_with(|| e.to_string());
                    }
                }
            }
        }
    }

    if let Some(resp) = all_writes_failed(updated, skipped, failed, first_error.as_deref()) {
        return resp;
    }
    (
        StatusCode::OK,
        Json(UpsertBulkResp {
            updated,
            skipped,
            forbidden: 0,
            not_found: 0,
            failed,
            first_error,
        }),
    )
        .into_response()
}

/// Body for `POST /me/progress/bulk` — bulk read/unread for an
/// arbitrary list of issue ids. Used by the multi-select toolbar
/// across the series / collection / view / CBL list pages. See
/// `~/.claude/plans/multi-select-bulk-actions-1.0.md` (M2).
///
/// Differs from `upsert_series` in two ways: (1) the caller supplies
/// the explicit issue-id list rather than "every active issue in
/// series X"; (2) ACL filtering walks each issue's library_id and
/// drops anything the caller can't see. The response counts skipped
/// rows separately so the toast can surface "N marked read, M
/// already read."
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpsertBulkReq {
    pub issue_ids: Vec<String>,
    pub finished: bool,
    #[serde(default)]
    pub device: Option<String>,
    /// See [`UpsertSeriesReq::backfill`] — same semantics: when
    /// `true` and `finished == true`, every row written carries
    /// `is_backfill = true` and is excluded from time-bound activity
    /// surfaces. Defaults to `false` for back-compat.
    #[serde(default)]
    pub backfill: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UpsertBulkResp {
    /// Issues whose progress row was created or updated.
    pub updated: u32,
    /// Issues already in the target state (no DB write — cheap re-click).
    pub skipped: u32,
    /// Issues the caller doesn't have library access to read. Silent
    /// from the user's perspective; surfaced here so an admin
    /// debugging a "mark read did nothing" report can see the
    /// filter fired.
    pub forbidden: u32,
    /// Issues whose id didn't resolve to a row (removed / never
    /// existed / typo). Treated the same as forbidden — the caller
    /// doesn't get to distinguish a missing row from one they're
    /// not allowed to see.
    pub not_found: u32,
    /// Issues whose write failed (post-audit T3.7); see
    /// [`UpsertSeriesResp::failed`].
    #[serde(default)]
    pub failed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_error: Option<String>,
}

#[utoipa::path(
    operation_id = "progress_upsert_bulk",    post,
    path = "/me/progress/bulk",
    request_body = UpsertBulkReq,
    responses(
        (status = 200, body = UpsertBulkResp),
        (status = 400, description = "validation"),
    )
)]
#[handler]
pub async fn upsert_bulk(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<UpsertBulkReq>,
) -> Response {
    // Cap the per-request id list to keep the loop bounded — a malicious
    // client could otherwise enqueue thousands of round-trips inside
    // one request. 500 covers any realistic multi-select session;
    // larger batches should be done via the per-series endpoint.
    const MAX_IDS: usize = 500;
    if req.issue_ids.len() > MAX_IDS {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            &format!("issue_ids cap is {MAX_IDS}"),
        );
    }
    // Empty list is a 200 OK with all-zero counts — easier on clients
    // that compute their selection list dynamically.
    if req.issue_ids.is_empty() {
        return (
            StatusCode::OK,
            Json(UpsertBulkResp {
                updated: 0,
                skipped: 0,
                forbidden: 0,
                not_found: 0,
                failed: 0,
                first_error: None,
            }),
        )
            .into_response();
    }
    // Dedup ids — a client that double-checks the same card shouldn't
    // get a 2x cost. Preserves order for predictable iteration.
    let mut seen = std::collections::HashSet::with_capacity(req.issue_ids.len());
    let ids: Vec<String> = req
        .issue_ids
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect();

    // WP-3.6: id / page_count + the ACL columns are all that's read.
    let rows = match issue::Entity::find()
        .filter(issue::Column::Id.is_in(ids.clone()))
        .filter(issue::Column::State.eq(issue::IssueState::Active))
        .filter(issue::Column::RemovedAt.is_null())
        .into_partial_model::<IssueCardRow>()
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "bulk-progress issue lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let found_ids: std::collections::HashSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    let not_found = ids
        .iter()
        .filter(|id| !found_ids.contains(id.as_str()))
        .count() as u32;

    // Pre-fetch the library-access set (membership + WP-2.7 age-rating
    // caps) in one query, so the per-issue ACL check is a HashSet hit
    // rather than a SELECT per row.
    let acl = access::for_user(&app, &user).await;

    let mut updated: u32 = 0;
    let mut skipped: u32 = 0;
    let mut failed: u32 = 0;
    let mut first_error: Option<String> = None;
    let mut forbidden: u32 = 0;

    for iss in rows {
        if !access::issue_allowed(&app, &acl, &iss).await {
            forbidden += 1;
            continue;
        }
        let target_page = if req.finished {
            iss.page_count.unwrap_or(1).max(1) - 1
        } else {
            0
        };
        let target_percent = if req.finished { 1.0 } else { 0.0 };

        let existing = match ProgressEntity::find_by_id((user.id, iss.id.clone()))
            .one(&app.db)
            .await
        {
            Ok(opt) => opt,
            Err(e) => {
                tracing::warn!(error = %e, issue_id = %iss.id, "bulk-progress lookup failed");
                failed += 1;
                first_error.get_or_insert_with(|| e.to_string());
                continue;
            }
        };

        match existing {
            Some(row) if row.finished == req.finished && row.last_page == target_page => {
                skipped += 1;
            }
            Some(_) | None => {
                // Post-audit T3.7: one atomic statement (see `upsert_explicit`)
                // instead of read-then-update/insert, and a failure is counted
                // instead of vanishing into the log.
                let is_backfill = resolve_is_backfill(req.finished, req.backfill);
                match upsert_explicit(
                    &app,
                    user.id,
                    &iss.id,
                    iss.page_count,
                    target_page,
                    target_percent,
                    req.finished,
                    req.device.clone(),
                    is_backfill,
                )
                .await
                {
                    Ok(_) => updated += 1,
                    Err(e) => {
                        tracing::warn!(error = %e, issue_id = %iss.id, "bulk-progress write failed");
                        failed += 1;
                        first_error.get_or_insert_with(|| e.to_string());
                    }
                }
            }
        }
    }

    if let Some(resp) = all_writes_failed(updated, skipped, failed, first_error.as_deref()) {
        return resp;
    }
    (
        StatusCode::OK,
        Json(UpsertBulkResp {
            updated,
            skipped,
            forbidden,
            not_found,
            failed,
            first_error,
        }),
    )
        .into_response()
}

/// Body for `POST /me/progress/series-bulk` — bulk read/unread
/// applied across every active issue of an arbitrary list of
/// series. Sister endpoint to `upsert_bulk` (which takes
/// issue_ids); used by the multi-select toolbar on filter views
/// where the cards are series. Each series in `series_ids` is
/// expanded server-side to its active issues, then walked through
/// the same `upsert_for` helper.
///
/// Plan: `~/.claude/plans/multi-select-bulk-actions-1.0.md`
/// (M6 extension per user request 2026-05-17 — filter views
/// originally weren't going to support mark-read since they're
/// series-only, but operating per-series-then-per-issue is the
/// natural semantics).
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpsertSeriesBulkReq {
    pub series_ids: Vec<uuid::Uuid>,
    pub finished: bool,
    #[serde(default)]
    pub device: Option<String>,
    /// See [`UpsertSeriesReq::backfill`].
    #[serde(default)]
    pub backfill: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UpsertSeriesBulkResp {
    /// Issue rows that received a new or updated progress record.
    pub updated: u32,
    /// Issues already in the target state.
    pub skipped: u32,
    /// Series the caller can't see (library access denied).
    pub forbidden_series: u32,
    /// Series whose id didn't resolve.
    pub not_found_series: u32,
    /// Issues whose write failed (post-audit T3.7); see
    /// [`UpsertSeriesResp::failed`].
    #[serde(default)]
    pub failed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_error: Option<String>,
}

#[utoipa::path(
    operation_id = "progress_upsert_series_bulk",    post,
    path = "/me/progress/series-bulk",
    request_body = UpsertSeriesBulkReq,
    responses(
        (status = 200, body = UpsertSeriesBulkResp),
        (status = 400, description = "validation"),
    )
)]
#[handler]
pub async fn upsert_series_bulk(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<UpsertSeriesBulkReq>,
) -> Response {
    // 100-series cap so a runaway client can't kick off a O(N*M)
    // walk across a library's whole index. With ~50 issues per
    // series typical, 100 series → ~5k progress writes max.
    const MAX_SERIES: usize = 100;
    if req.series_ids.len() > MAX_SERIES {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            &format!("series_ids cap is {MAX_SERIES}"),
        );
    }
    if req.series_ids.is_empty() {
        return (
            StatusCode::OK,
            Json(UpsertSeriesBulkResp {
                updated: 0,
                skipped: 0,
                forbidden_series: 0,
                not_found_series: 0,
                failed: 0,
                first_error: None,
            }),
        )
            .into_response();
    }
    // Dedup series ids.
    let mut seen = std::collections::HashSet::with_capacity(req.series_ids.len());
    let series_ids: Vec<uuid::Uuid> = req
        .series_ids
        .into_iter()
        .filter(|id| seen.insert(*id))
        .collect();

    // Pre-fetch series rows (need library_id for ACL).
    let series_rows = match entity::series::Entity::find()
        .filter(entity::series::Column::Id.is_in(series_ids.clone()))
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "series-bulk-progress series lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let found_ids: std::collections::HashSet<uuid::Uuid> =
        series_rows.iter().map(|r| r.id).collect();
    let not_found_series = series_ids
        .iter()
        .filter(|id| !found_ids.contains(id))
        .count() as u32;

    // Pre-fetch the library-access set (membership + WP-2.7 caps).
    let acl = access::for_user(&app, &user).await;

    let mut updated: u32 = 0;
    let mut skipped: u32 = 0;
    let mut failed: u32 = 0;
    let mut first_error: Option<String> = None;
    let mut forbidden_series: u32 = 0;

    for srow in series_rows {
        if !acl.series_ok(srow.library_id, srow.age_rating.as_deref()) {
            forbidden_series += 1;
            continue;
        }
        // Pull every active, on-disk issue for the series in one
        // query. Mirrors `upsert_series`'s filter.
        // WP-3.6: id / page_count only — project.
        let issues = match issue::Entity::find()
            .filter(issue::Column::SeriesId.eq(srow.id))
            .filter(issue::Column::State.eq(issue::IssueState::Active))
            .filter(issue::Column::RemovedAt.is_null())
            .into_partial_model::<IssueCardRow>()
            .all(&app.db)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, series_id = %srow.id, "series-bulk issue lookup failed");
                continue;
            }
        };
        for iss in issues {
            let target_page = if req.finished {
                iss.page_count.unwrap_or(1).max(1) - 1
            } else {
                0
            };
            let target_percent = if req.finished { 1.0 } else { 0.0 };
            let existing = match ProgressEntity::find_by_id((user.id, iss.id.clone()))
                .one(&app.db)
                .await
            {
                Ok(opt) => opt,
                Err(e) => {
                    tracing::warn!(error = %e, issue_id = %iss.id, "series-bulk lookup failed");
                    failed += 1;
                    first_error.get_or_insert_with(|| e.to_string());
                    continue;
                }
            };
            match existing {
                Some(row) if row.finished == req.finished && row.last_page == target_page => {
                    skipped += 1;
                }
                Some(_) | None => {
                    // Post-audit T3.7: one atomic statement (see `upsert_explicit`)
                    // instead of read-then-update/insert, and a failure is counted
                    // instead of vanishing into the log.
                    let is_backfill = resolve_is_backfill(req.finished, req.backfill);
                    match upsert_explicit(
                        &app,
                        user.id,
                        &iss.id,
                        iss.page_count,
                        target_page,
                        target_percent,
                        req.finished,
                        req.device.clone(),
                        is_backfill,
                    )
                    .await
                    {
                        Ok(_) => updated += 1,
                        Err(e) => {
                            tracing::warn!(error = %e, issue_id = %iss.id, "series-bulk write failed");
                            failed += 1;
                            first_error.get_or_insert_with(|| e.to_string());
                        }
                    }
                }
            }
        }
    }

    if let Some(resp) = all_writes_failed(updated, skipped, failed, first_error.as_deref()) {
        return resp;
    }
    (
        StatusCode::OK,
        Json(UpsertSeriesBulkResp {
            updated,
            skipped,
            forbidden_series,
            not_found_series,
            failed,
            first_error,
        }),
    )
        .into_response()
}
