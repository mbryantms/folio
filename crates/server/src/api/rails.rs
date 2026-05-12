//! Home-page system rails: Continue Reading + On Deck + per-user dismissals.
//!
//! Endpoints:
//!   - `GET    /me/continue-reading`               — partially-read issues
//!   - `GET    /me/on-deck`                        — (M3) — next-up issues + CBLs
//!   - `POST   /me/rail-dismissals`                — hide a target from a rail
//!   - `DELETE /me/rail-dismissals/{kind}/{id}`    — undo a dismissal
//!
//! Dismissals auto-restore: a row stays in the table once written, but
//! gets filtered out of the rail query as soon as the target has new
//! activity past `dismissed_at`. That way the user always sees an
//! up-to-date "where was I?" list without needing a separate restore
//! click for every paused issue they re-opened.
//!
//! See plan: `Continue Reading + On Deck home rails` for the full data
//! model, dismiss semantics, and rail composition rules.
//!
//! M1 ships the Continue Reading endpoint + dismiss endpoints. M3 adds
//! On Deck. Both rails share the same dismissal table, so M3 only needs
//! to extend the query side.

use axum::{
    Extension, Json, Router,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete as axum_delete, get, post},
};
use chrono::Utc;
use entity::{issue, rail_dismissal, series};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DbBackend, EntityTrait, FromQueryResult, QueryFilter, Set,
    Statement, sea_query::Expr,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::series::IssueSummaryView;
use crate::auth::CurrentUser;
use crate::library::access;
use crate::middleware::RequestContext;
use crate::state::AppState;

const DISMISS_KIND_ISSUE: &str = "issue";
const DISMISS_KIND_SERIES: &str = "series";
const DISMISS_KIND_CBL: &str = "cbl";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me/continue-reading", get(continue_reading))
        .route("/me/on-deck", get(on_deck))
        .route("/me/rail-dismissals", post(create_dismissal))
        .route(
            "/me/rail-dismissals/{kind}/{target_id}",
            axum_delete(delete_dismissal),
        )
}

// ───── Response shapes ─────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProgressInfo {
    /// Last viewed page index (0-based).
    pub last_page: i32,
    /// 0.0–1.0 fraction read; computed from `last_page / page_count`.
    pub percent: f64,
    /// RFC 3339 timestamp of the most recent progress write.
    pub updated_at: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ContinueReadingCard {
    pub issue: IssueSummaryView,
    pub series_name: String,
    pub progress: ProgressInfo,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ContinueReadingView {
    pub items: Vec<ContinueReadingCard>,
}

/// Discriminated union for the On Deck rail. Each card represents one
/// "what's next" suggestion — either the next-unread issue in a series the
/// user has finished at least one issue of, or the next-unread entry in a
/// CBL list the user is working through.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OnDeckCard {
    SeriesNext {
        issue: IssueSummaryView,
        series_name: String,
        last_activity: String,
    },
    CblNext {
        issue: IssueSummaryView,
        cbl_list_id: String,
        cbl_list_name: String,
        /// 1-based position of the entry within its CBL list (matches the
        /// "#N" badge the CBL detail UI shows).
        position: i32,
        last_activity: String,
    },
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OnDeckView {
    pub items: Vec<OnDeckCard>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateDismissalReq {
    /// One of `'issue'`, `'series'`, `'cbl'`.
    pub target_kind: String,
    pub target_id: String,
}

// ───── Handlers ─────

/// In-progress issues for the calling user, ordered by most recent
/// activity. Excludes finished issues, issues that haven't actually been
/// opened (`last_page = 0`), removed/encrypted issues, issues the user
/// can't see (library ACL), and issues whose dismissal is still current
/// (no newer progress write past the dismissal timestamp).
///
/// Implementation note: dismissals require comparing `progress_records.
/// updated_at` against `rail_dismissals.dismissed_at`, so we let Postgres
/// do that in one SQL pass rather than re-filtering rows in Rust.
#[utoipa::path(
    get,
    path = "/me/continue-reading",
    responses((status = 200, body = ContinueReadingView))
)]
pub async fn continue_reading(State(app): State<AppState>, user: CurrentUser) -> Response {
    let acl = access::for_user(&app, &user).await;

    #[derive(Debug, FromQueryResult)]
    struct Row {
        issue_id: String,
        last_page: i32,
        percent: f64,
        progress_updated_at: chrono::DateTime<chrono::FixedOffset>,
        library_id: Uuid,
        series_slug: String,
        series_name: String,
    }

    // We pull the columns we need to render `IssueSummaryView` + the
    // progress overlay + the parent series slug in one query. The
    // `library_id` round-trips so we can apply the ACL filter in Rust
    // (the same pattern the rest of the API uses for non-admin users).
    let rows: Vec<Row> = match Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT
                p.issue_id            AS issue_id,
                p.last_page           AS last_page,
                p.percent             AS percent,
                p.updated_at          AS progress_updated_at,
                i.library_id          AS library_id,
                s.slug                AS series_slug,
                s.name                AS series_name
            FROM progress_records p
            JOIN issues  i ON i.id = p.issue_id
            JOIN series  s ON s.id = i.series_id
            LEFT JOIN rail_dismissals d
              ON d.user_id = p.user_id
             AND d.target_kind = 'issue'
             AND d.target_id   = p.issue_id
            WHERE p.user_id   = $1
              AND p.finished  = false
              AND p.last_page > 0
              AND i.state     = 'active'
              AND i.removed_at IS NULL
              AND (d.dismissed_at IS NULL OR p.updated_at > d.dismissed_at)
            ORDER BY p.updated_at DESC
            LIMIT 24
        "#,
        [user.id.into()],
    ))
    .all(&app.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "rails: continue-reading query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    if rows.is_empty() {
        return Json(ContinueReadingView { items: Vec::new() }).into_response();
    }

    // Hydrate full issue::Model rows for `IssueSummaryView::from_model`.
    // One batched fetch keeps it O(1) round-trips even for the full 24.
    let issue_ids: Vec<String> = rows.iter().map(|r| r.issue_id.clone()).collect();
    let issue_rows = match issue::Entity::find()
        .filter(issue::Column::Id.is_in(issue_ids.clone()))
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "rails: continue-reading hydrate failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let issue_by_id: std::collections::HashMap<String, issue::Model> =
        issue_rows.into_iter().map(|i| (i.id.clone(), i)).collect();

    let items: Vec<ContinueReadingCard> = rows
        .into_iter()
        .filter_map(|row| {
            if !acl.contains(row.library_id) {
                return None;
            }
            let issue_model = issue_by_id.get(&row.issue_id)?.clone();
            Some(ContinueReadingCard {
                issue: IssueSummaryView::from_model(issue_model, &row.series_slug)
                    .with_series_name(row.series_name.clone()),
                series_name: row.series_name,
                progress: ProgressInfo {
                    last_page: row.last_page,
                    percent: row.percent,
                    updated_at: row.progress_updated_at.to_rfc3339(),
                },
            })
        })
        .collect();

    Json(ContinueReadingView { items }).into_response()
}

/// "What's next" suggestions for the home page. Returns a mix of
/// `series_next` cards (next-unread issue in a series the user has read
/// at least one finished issue of, with no in-progress issue blocking the
/// queue) and `cbl_next` cards (lowest-position unfinished matched entry
/// in a CBL the user has any progress in). Series with an active in-
/// progress issue are skipped — they already surface in Continue Reading.
#[utoipa::path(
    get,
    path = "/me/on-deck",
    responses((status = 200, body = OnDeckView))
)]
pub async fn on_deck(State(app): State<AppState>, user: CurrentUser) -> Response {
    let acl = access::for_user(&app, &user).await;
    let mut items: Vec<(chrono::DateTime<chrono::FixedOffset>, OnDeckCard)> = Vec::new();

    // ───── series_next candidates ─────
    //
    // Series the user has any progress in, MAX(updated_at) per series, but
    // excluding series with a still-in-progress issue (those land in
    // Continue Reading instead). Dismissals are honored with auto-restore
    // (the dismissal expires once new progress lands past `dismissed_at`).
    #[derive(Debug, FromQueryResult)]
    struct SeriesRow {
        series_id: Uuid,
        series_name: String,
        last_activity: chrono::DateTime<chrono::FixedOffset>,
        library_id: Uuid,
    }
    let series_candidates: Vec<SeriesRow> =
        match SeriesRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                WITH started AS (
                    SELECT i.series_id, MAX(p.updated_at) AS last_activity
                    FROM progress_records p
                    JOIN issues i ON i.id = p.issue_id
                    WHERE p.user_id = $1
                      AND i.state = 'active'
                      AND i.removed_at IS NULL
                    GROUP BY i.series_id
                ),
                in_progress AS (
                    SELECT DISTINCT i.series_id
                    FROM progress_records p
                    JOIN issues i ON i.id = p.issue_id
                    WHERE p.user_id = $1
                      AND p.finished = false
                      AND p.last_page > 0
                      AND i.state = 'active'
                      AND i.removed_at IS NULL
                )
                SELECT s.id AS series_id, s.name AS series_name,
                       started.last_activity AS last_activity,
                       s.library_id AS library_id
                FROM started
                JOIN series s ON s.id = started.series_id
                LEFT JOIN rail_dismissals d
                  ON d.user_id = $1
                 AND d.target_kind = 'series'
                 AND d.target_id = s.id::text
                WHERE started.series_id NOT IN (SELECT series_id FROM in_progress)
                  AND (d.dismissed_at IS NULL OR started.last_activity > d.dismissed_at)
                ORDER BY started.last_activity DESC
                LIMIT 40
            "#,
            [user.id.into()],
        ))
        .all(&app.db)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "rails: on-deck series query failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };

    for row in &series_candidates {
        if !acl.contains(row.library_id) {
            continue;
        }
        let next = match pick_next_in_series(&app, user.id, row.series_id).await {
            Ok(opt) => opt,
            Err(resp) => return resp,
        };
        let Some(issue_model) = next else { continue };
        // Resolve the slug now (the join column wasn't in our CTE).
        let series_row = match series::Entity::find_by_id(row.series_id).one(&app.db).await {
            Ok(Some(s)) => s,
            _ => continue,
        };
        items.push((
            row.last_activity,
            OnDeckCard::SeriesNext {
                issue: IssueSummaryView::from_model(issue_model, &series_row.slug)
                    .with_series_name(series_row.name.clone()),
                series_name: row.series_name.clone(),
                last_activity: row.last_activity.to_rfc3339(),
            },
        ));
    }

    // ───── cbl_next candidates ─────
    //
    // CBLs where the user has any matched-issue progress + at least one
    // matched entry that isn't yet finished. We pull the (cbl, last_activity,
    // next_entry_issue_id) tuple in one query per list for clarity; the
    // total candidate count is bounded by the user's actual CBL usage so
    // the round-trip count is small in practice.
    #[derive(Debug, FromQueryResult)]
    struct CblCandidate {
        cbl_list_id: Uuid,
        cbl_list_name: String,
        last_activity: chrono::DateTime<chrono::FixedOffset>,
    }
    let cbl_candidates: Vec<CblCandidate> =
        match CblCandidate::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                SELECT cl.id AS cbl_list_id, cl.parsed_name AS cbl_list_name,
                       MAX(p.updated_at) AS last_activity
                FROM progress_records p
                JOIN cbl_entries e ON e.matched_issue_id = p.issue_id
                JOIN cbl_lists  cl ON cl.id = e.cbl_list_id
                LEFT JOIN rail_dismissals d
                  ON d.user_id = $1
                 AND d.target_kind = 'cbl'
                 AND d.target_id = cl.id::text
                WHERE p.user_id = $1
                  AND (cl.owner_user_id IS NULL OR cl.owner_user_id = $1)
                GROUP BY cl.id, cl.parsed_name, d.dismissed_at
                HAVING d.dismissed_at IS NULL OR MAX(p.updated_at) > d.dismissed_at
                ORDER BY MAX(p.updated_at) DESC
                LIMIT 40
            "#,
            [user.id.into()],
        ))
        .all(&app.db)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "rails: on-deck cbl query failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };

    for cand in &cbl_candidates {
        let next = match pick_next_in_cbl(&app, user.id, cand.cbl_list_id, &acl).await {
            Ok(opt) => opt,
            Err(resp) => return resp,
        };
        let Some((issue_model, series_slug, series_name, position)) = next else {
            continue;
        };
        items.push((
            cand.last_activity,
            OnDeckCard::CblNext {
                issue: IssueSummaryView::from_model(issue_model, &series_slug)
                    .with_series_name(series_name),
                cbl_list_id: cand.cbl_list_id.to_string(),
                cbl_list_name: cand.cbl_list_name.clone(),
                position,
                last_activity: cand.last_activity.to_rfc3339(),
            },
        ));
    }

    // Merge by most-recent activity desc + cap.
    items.sort_by(|a, b| b.0.cmp(&a.0));
    items.truncate(24);
    let payload = OnDeckView {
        items: items.into_iter().map(|(_, c)| c).collect(),
    };
    Json(payload).into_response()
}

/// Server-side port of the client's `pickNextIssue` algorithm
/// ([web/lib/reading-state.ts]) — applied to a single series. Returns the
/// next-unread `issue::Model` or `None` if every active issue is already
/// finished / there are no active issues at all.
///
/// Called from the On Deck handler only after we've already filtered out
/// series with an in-progress issue, so step 1 of the original algorithm
/// (continue resumable in-progress) is a no-op here. We still apply step
/// 2 (first unfinished) and skip step 3 (all-finished restart) because
/// "Read again" doesn't belong in an On Deck queue.
async fn pick_next_in_series(
    app: &AppState,
    user_id: Uuid,
    series_id: Uuid,
) -> Result<Option<issue::Model>, Response> {
    use entity::progress_record;
    use sea_orm::QueryOrder;

    let issues: Vec<issue::Model> = match issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(series_id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .order_by_asc(Expr::cust("sort_number IS NULL"))
        .order_by_asc(issue::Column::SortNumber)
        .order_by_asc(issue::Column::Id)
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "rails: pick_next_in_series issues lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    if issues.is_empty() {
        return Ok(None);
    }
    let issue_ids: Vec<String> = issues.iter().map(|i| i.id.clone()).collect();
    let progress_rows = match progress_record::Entity::find()
        .filter(progress_record::Column::UserId.eq(user_id))
        .filter(progress_record::Column::IssueId.is_in(issue_ids))
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "rails: pick_next_in_series progress lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    let progress_by_id: std::collections::HashMap<String, progress_record::Model> = progress_rows
        .into_iter()
        .map(|p| (p.issue_id.clone(), p))
        .collect();

    // First not-finished issue in sort order. Caller has already excluded
    // series with mid-issue in-progress rows, so any unread / unstarted
    // issue here is the right pick.
    for iss in issues {
        let progress = progress_by_id.get(&iss.id);
        let finished = progress.map(|p| p.finished).unwrap_or(false);
        if !finished {
            return Ok(Some(iss));
        }
    }
    Ok(None)
}

/// For a CBL list, find the lowest-position matched entry whose issue is
/// not yet finished + is visible to the user (library ACL). Returns the
/// issue + parent series (slug, name) + 1-based position when found, or
/// `None` when every matched entry is finished or no matched entry
/// passes the ACL filter.
async fn pick_next_in_cbl(
    app: &AppState,
    user_id: Uuid,
    cbl_list_id: Uuid,
    acl: &access::VisibleLibraries,
) -> Result<Option<(issue::Model, String, String, i32)>, Response> {
    use entity::{cbl_entry, progress_record};
    use sea_orm::QueryOrder;

    let entries = match cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(cbl_list_id))
        .filter(cbl_entry::Column::MatchedIssueId.is_not_null())
        .order_by_asc(cbl_entry::Column::Position)
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "rails: pick_next_in_cbl entries lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    if entries.is_empty() {
        return Ok(None);
    }

    // Pull progress rows for all matched issues at once so per-entry
    // status checks are O(1).
    let matched_ids: Vec<String> = entries
        .iter()
        .filter_map(|e| e.matched_issue_id.clone())
        .collect();
    let progress_rows = match progress_record::Entity::find()
        .filter(progress_record::Column::UserId.eq(user_id))
        .filter(progress_record::Column::IssueId.is_in(matched_ids))
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "rails: pick_next_in_cbl progress lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    let progress_by_issue: std::collections::HashMap<String, progress_record::Model> =
        progress_rows
            .into_iter()
            .map(|p| (p.issue_id.clone(), p))
            .collect();

    for entry in entries {
        let Some(issue_id) = entry.matched_issue_id.clone() else {
            continue;
        };
        let finished = progress_by_issue
            .get(&issue_id)
            .map(|p| p.finished)
            .unwrap_or(false);
        if finished {
            continue;
        }
        // Resolve the issue + parent series for ACL + slug.
        let Ok(Some(issue_model)) = issue::Entity::find_by_id(issue_id).one(&app.db).await else {
            continue;
        };
        if !acl.contains(issue_model.library_id) {
            continue;
        }
        let (series_slug, series_name) = match series::Entity::find_by_id(issue_model.series_id)
            .one(&app.db)
            .await
        {
            Ok(Some(s)) => (s.slug, s.name),
            _ => continue,
        };
        return Ok(Some((
            issue_model,
            series_slug,
            series_name,
            entry.position + 1,
        )));
    }
    Ok(None)
}

#[utoipa::path(
    post,
    path = "/me/rail-dismissals",
    request_body = CreateDismissalReq,
    responses((status = 204))
)]
pub async fn create_dismissal(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<CreateDismissalReq>,
) -> Response {
    let kind = req.target_kind.trim();
    if !is_valid_kind(kind) {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "target_kind must be one of: issue, series, cbl",
        );
    }
    let target_id = req.target_id.trim();
    if target_id.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "target_id is required",
        );
    }
    // Validate the target actually exists and is visible to the user.
    // Bail with 404 (not 403) so we don't disclose existence of items
    // the user can't see.
    if let Err(resp) = ensure_target_visible(&app, &user, kind, target_id).await {
        return resp;
    }

    let now = Utc::now().fixed_offset();
    let am = rail_dismissal::ActiveModel {
        user_id: Set(user.id),
        target_kind: Set(kind.to_owned()),
        target_id: Set(target_id.to_owned()),
        dismissed_at: Set(now),
    };
    // Re-dismissing an already-dismissed target updates the timestamp
    // so the rail's "auto-restore on newer progress" comparison resets.
    let conn = &app.db;
    let already =
        rail_dismissal::Entity::find_by_id((user.id, kind.to_owned(), target_id.to_owned()))
            .one(conn)
            .await;
    match already {
        Ok(Some(_)) => {
            if let Err(e) = am.update(conn).await {
                tracing::error!(error = %e, "rails: dismissal update failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
        Ok(None) => {
            if let Err(e) = am.insert(conn).await {
                tracing::error!(error = %e, "rails: dismissal insert failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "rails: dismissal lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    }

    crate::audit::record(
        &app.db,
        crate::audit::AuditEntry {
            actor_id: user.id,
            action: "user.rail.dismiss",
            target_type: Some("rail_dismissal"),
            target_id: Some(format!("{kind}:{target_id}")),
            payload: serde_json::json!({ "target_kind": kind, "target_id": target_id }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    delete,
    path = "/me/rail-dismissals/{kind}/{target_id}",
    params(
        ("kind"      = String, Path,),
        ("target_id" = String, Path,),
    ),
    responses((status = 204))
)]
pub async fn delete_dismissal(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    AxPath((kind, target_id)): AxPath<(String, String)>,
) -> Response {
    if !is_valid_kind(&kind) {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "kind must be one of: issue, series, cbl",
        );
    }
    let res = rail_dismissal::Entity::delete_by_id((user.id, kind.clone(), target_id.clone()))
        .exec(&app.db)
        .await;
    match res {
        Ok(r) if r.rows_affected == 0 => {
            return error(
                StatusCode::NOT_FOUND,
                "not_found",
                "no dismissal for that target",
            );
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!(error = %e, "rails: dismissal delete failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    }
    crate::audit::record(
        &app.db,
        crate::audit::AuditEntry {
            actor_id: user.id,
            action: "user.rail.restore",
            target_type: Some("rail_dismissal"),
            target_id: Some(format!("{kind}:{target_id}")),
            payload: serde_json::json!({ "target_kind": kind, "target_id": target_id }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    StatusCode::NO_CONTENT.into_response()
}

// ───── Helpers ─────

fn is_valid_kind(kind: &str) -> bool {
    matches!(
        kind,
        DISMISS_KIND_ISSUE | DISMISS_KIND_SERIES | DISMISS_KIND_CBL
    )
}

/// 404 if the target doesn't exist or the user can't see it (library
/// ACL / list ownership), so we don't leak existence.
async fn ensure_target_visible(
    app: &AppState,
    user: &CurrentUser,
    kind: &str,
    target_id: &str,
) -> Result<(), Response> {
    let acl = access::for_user(app, user).await;
    match kind {
        DISMISS_KIND_ISSUE => {
            let row = issue::Entity::find_by_id(target_id.to_owned())
                .one(&app.db)
                .await
                .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"))?;
            let issue = row.ok_or_else(not_found)?;
            if !acl.contains(issue.library_id) {
                return Err(not_found());
            }
            Ok(())
        }
        DISMISS_KIND_SERIES => {
            let id = Uuid::parse_str(target_id).map_err(|_| {
                error(
                    StatusCode::BAD_REQUEST,
                    "validation",
                    "series target_id must be UUID",
                )
            })?;
            let row = series::Entity::find_by_id(id)
                .one(&app.db)
                .await
                .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"))?;
            let series = row.ok_or_else(not_found)?;
            if !acl.contains(series.library_id) {
                return Err(not_found());
            }
            Ok(())
        }
        DISMISS_KIND_CBL => {
            let id = Uuid::parse_str(target_id).map_err(|_| {
                error(
                    StatusCode::BAD_REQUEST,
                    "validation",
                    "cbl target_id must be UUID",
                )
            })?;
            let row = entity::cbl_list::Entity::find_by_id(id)
                .one(&app.db)
                .await
                .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"))?;
            let list = row.ok_or_else(not_found)?;
            // CBL lists are either user-owned or admin/global. Treat
            // visibility the same way `/me/cbl-lists` does — the user
            // can see their own + any admin-owned list.
            if let Some(owner) = list.owner_user_id
                && owner != user.id
            {
                return Err(not_found());
            }
            Ok(())
        }
        _ => Err(error(StatusCode::BAD_REQUEST, "validation", "invalid kind")),
    }
}

fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", "target not found")
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
