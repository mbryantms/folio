//! Starter reading lists for the first-run tour (PR C), see
//! [`crate::starter_lists`].
//!
//! - `GET    /me/starter-lists`                    the shelf the tour shows
//! - `GET    /admin/starter-lists`                 same, plus hidden picks
//! - `POST   /admin/starter-lists`                 add a catalog file
//! - `PUT    /admin/starter-lists/visibility`      hide / show a pick
//! - `DELETE /admin/starter-lists/{id}`            remove an added pick
//! - `POST   /admin/starter-lists/refresh-coverage` recompute coverage now
//!
//! Importing is the ordinary catalog import (`POST /me/cbl-lists` with
//! `kind = "catalog"`) followed by saving the view, so quotas, refresh
//! schedules and matching behave exactly as from the import dialog.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{DateTime, FixedOffset};
use entity::catalog_source;
use sea_orm::{ConnectionTrait, EntityTrait, FromQueryResult, Statement, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::audit::{self, AuditEntry};
use crate::auth::{CurrentUser, RequireAdmin};
use crate::library::access;
use crate::middleware::RequestContext;
use crate::starter_lists::{self as sl, MANIFEST};
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_mine))
        .routes(routes!(admin_list, admin_add))
        .routes(routes!(admin_set_visibility))
        .routes(routes!(admin_delete))
        .routes(routes!(admin_refresh_coverage))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct StarterListView {
    pub catalog_source_id: String,
    pub catalog_path: String,
    pub title: String,
    /// Grouping label (`Marvel`, `DC`, `Image`, …).
    pub publisher: String,
    /// Why this is a good place to start.
    pub reason: String,
    /// `shipped` (Folio's curated pick) or `admin` (added on this server).
    pub origin: String,
    /// Admin listing only; the reader listing never includes hidden picks.
    pub hidden: bool,
    /// The override row behind an admin-added or hidden pick (the id
    /// `DELETE /admin/starter-lists/{id}` takes).
    pub override_id: Option<String>,
    /// `ready`, `pending` (first computation running) or `error`.
    pub coverage_status: String,
    /// Entries in the list.
    pub total: i32,
    /// Of those, issues in this library the caller can read.
    pub available: i64,
    pub coverage_computed_at: Option<DateTime<FixedOffset>>,
    /// Set when the caller already has this file as a reading list (their
    /// own import, else one an admin shared with everyone), so the tour
    /// links to it instead of importing it again.
    pub imported_view_id: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct StarterListListView {
    pub items: Vec<StarterListView>,
}

#[derive(FromQueryResult)]
struct OverrideRow {
    id: Uuid,
    catalog_source_id: Uuid,
    catalog_path: String,
    hidden: bool,
    title: Option<String>,
    publisher: Option<String>,
    reason: Option<String>,
}

#[derive(FromQueryResult)]
struct CoverageRow {
    catalog_source_id: Uuid,
    catalog_path: String,
    total: i32,
    error: Option<String>,
    computed_at: DateTime<FixedOffset>,
    available: i64,
}

#[derive(FromQueryResult)]
struct ImportedRow {
    catalog_source_id: Uuid,
    catalog_path: String,
    view_id: Uuid,
}

#[utoipa::path(
    operation_id = "starter_lists_list", get,
    path = "/me/starter-lists",
    responses((status = 200, body = StarterListListView))
)]
#[handler]
pub async fn list_mine(State(app): State<AppState>, user: CurrentUser) -> impl IntoResponse {
    match build(&app, &user, false).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => internal(e),
    }
}

#[utoipa::path(
    operation_id = "starter_lists_admin_list", get,
    path = "/admin/starter-lists",
    responses((status = 200, body = StarterListListView), (status = 403))
)]
#[handler]
pub async fn admin_list(State(app): State<AppState>, admin: RequireAdmin) -> impl IntoResponse {
    match build(&app, &admin.0, true).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => internal(e),
    }
}

fn internal(e: sea_orm::DbErr) -> axum::response::Response {
    tracing::error!(error = %e, "starter_lists: query failed");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
}

/// The merged shelf: shipped picks (minus hidden, unless `include_hidden`)
/// in manifest order, then admin additions oldest first. Kicks off a
/// background coverage run for anything missing or stale.
async fn build(
    app: &AppState,
    user: &CurrentUser,
    include_hidden: bool,
) -> Result<StarterListListView, sea_orm::DbErr> {
    let backend = app.db.get_database_backend();
    let overrides = OverrideRow::find_by_statement(Statement::from_sql_and_values(
        backend,
        "SELECT id, catalog_source_id, catalog_path, hidden, title, publisher, reason \
         FROM starter_list_overrides ORDER BY created_at, id",
        [],
    ))
    .all(&app.db)
    .await?;
    // Sources the admin disabled or removed drop their picks.
    let enabled: std::collections::HashSet<Uuid> = catalog_source::Entity::find()
        .all(&app.db)
        .await?
        .into_iter()
        .filter(|s| s.enabled)
        .map(|s| s.id)
        .collect();

    let mut items: Vec<StarterListView> = Vec::new();
    let by_key: HashMap<(Uuid, &str), &OverrideRow> = overrides
        .iter()
        .map(|o| ((o.catalog_source_id, o.catalog_path.as_str()), o))
        .collect();
    if enabled.contains(&sl::DIESELTECH_SOURCE_ID) {
        for m in MANIFEST {
            let ov = by_key.get(&(sl::DIESELTECH_SOURCE_ID, m.path));
            let hidden = ov.is_some_and(|o| o.hidden);
            if hidden && !include_hidden {
                continue;
            }
            items.push(view(
                sl::DIESELTECH_SOURCE_ID,
                m.path,
                m.title,
                m.publisher,
                m.reason,
                "shipped",
                hidden,
                ov.map(|o| o.id),
            ));
        }
    }
    for o in &overrides {
        if sl::shipped(o.catalog_source_id, &o.catalog_path).is_some()
            || !enabled.contains(&o.catalog_source_id)
            || (o.hidden && !include_hidden)
        {
            continue;
        }
        let fallback_title = file_stem(&o.catalog_path);
        items.push(view(
            o.catalog_source_id,
            &o.catalog_path,
            o.title.as_deref().unwrap_or(&fallback_title),
            o.publisher
                .as_deref()
                .unwrap_or_else(|| o.catalog_path.split('/').next().unwrap_or("Other")),
            o.reason.as_deref().unwrap_or(""),
            "admin",
            o.hidden,
            Some(o.id),
        ));
    }

    // Coverage, filtered to what this caller can read.
    let acl = access::for_user(app, user).await;
    let mut params: Vec<Value> = Vec::new();
    let vis = access::visible_issues_sql(&acl, "i", &mut params);
    let coverage = CoverageRow::find_by_statement(Statement::from_sql_and_values(
        backend,
        format!(
            "SELECT c.catalog_source_id, c.catalog_path, c.total, c.error, c.computed_at, \
                    (SELECT count(*) FROM issues i \
                      WHERE i.id = ANY(c.matched_issue_ids) AND i.state = 'active'{vis}) AS available \
             FROM starter_list_coverage c"
        ),
        params,
    ))
    .all(&app.db)
    .await?;
    let coverage: HashMap<(Uuid, String), CoverageRow> = coverage
        .into_iter()
        .map(|c| ((c.catalog_source_id, c.catalog_path.clone()), c))
        .collect();

    let imported = ImportedRow::find_by_statement(Statement::from_sql_and_values(
        backend,
        "SELECT l.catalog_source_id, l.catalog_path, v.id AS view_id \
         FROM saved_views v JOIN cbl_lists l ON l.id = v.cbl_list_id \
         WHERE v.kind = 'cbl' AND (v.user_id = $1 OR v.user_id IS NULL) \
           AND l.catalog_source_id IS NOT NULL AND l.catalog_path IS NOT NULL \
         ORDER BY (v.user_id IS NULL), v.created_at",
        [user.id.into()],
    ))
    .all(&app.db)
    .await?;
    let mut imported_by: HashMap<(Uuid, String), Uuid> = HashMap::new();
    for r in imported {
        imported_by
            .entry((r.catalog_source_id, r.catalog_path))
            .or_insert(r.view_id);
    }

    let mut stale = Vec::new();
    for item in &mut items {
        let key = (
            Uuid::parse_str(&item.catalog_source_id).unwrap_or_default(),
            item.catalog_path.clone(),
        );
        match coverage.get(&key) {
            Some(c) => {
                item.total = c.total;
                item.available = c.available;
                item.coverage_computed_at = Some(c.computed_at);
                item.coverage_status = if c.error.is_some() { "error" } else { "ready" }.into();
                if sl::is_stale(c.computed_at, c.error.is_some()) {
                    stale.push(key.clone());
                }
            }
            None => stale.push(key.clone()),
        }
        item.imported_view_id = imported_by.get(&key).map(ToString::to_string);
    }
    if !stale.is_empty() {
        sl::spawn_coverage(app, stale);
    }
    Ok(StarterListListView { items })
}

#[allow(clippy::too_many_arguments)]
fn view(
    source_id: Uuid,
    path: &str,
    title: &str,
    publisher: &str,
    reason: &str,
    origin: &str,
    hidden: bool,
    override_id: Option<Uuid>,
) -> StarterListView {
    StarterListView {
        catalog_source_id: source_id.to_string(),
        catalog_path: path.to_owned(),
        title: title.to_owned(),
        publisher: publisher.to_owned(),
        reason: reason.to_owned(),
        origin: origin.to_owned(),
        hidden,
        override_id: override_id.map(|id| id.to_string()),
        coverage_status: "pending".into(),
        total: 0,
        available: 0,
        coverage_computed_at: None,
        imported_view_id: None,
    }
}

fn file_stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".cbl").unwrap_or(name).to_owned()
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AddStarterListReq {
    pub catalog_source_id: Uuid,
    pub catalog_path: String,
    /// Defaults to the file name.
    #[serde(default)]
    pub title: Option<String>,
    /// Defaults to the first folder of the path.
    #[serde(default)]
    pub publisher: Option<String>,
    pub reason: String,
}

#[utoipa::path(
    operation_id = "starter_lists_admin_add", post,
    path = "/admin/starter-lists",
    request_body = AddStarterListReq,
    responses(
        (status = 201, body = StarterListListView),
        (status = 404, description = "unknown catalog source, or the path is not in its index"),
        (status = 409, description = "already a starter list"),
        (status = 422, description = "empty path or reason"),
    )
)]
#[handler]
pub async fn admin_add(
    State(app): State<AppState>,
    admin: RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<AddStarterListReq>,
) -> impl IntoResponse {
    let path = req.catalog_path.trim().to_owned();
    let reason = req.reason.trim().to_owned();
    if path.is_empty() || reason.is_empty() {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "catalog_path and reason are required",
        );
    }
    let source = match catalog_source::Entity::find_by_id(req.catalog_source_id)
        .one(&app.db)
        .await
    {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "catalog source"),
        Err(e) => return internal(e),
    };
    // Check against the cached index when there is one; an uncached
    // source is accepted and a bad path surfaces as a coverage error.
    if let Some(index) = source
        .index_json
        .as_ref()
        .and_then(|v| serde_json::from_value::<crate::cbl::catalog::CatalogIndex>(v.clone()).ok())
        && !index.entries.iter().any(|e| e.path == path)
    {
        return error(
            StatusCode::NOT_FOUND,
            "not_found",
            "path is not in the catalog",
        );
    }
    if sl::shipped(source.id, &path).is_some() {
        return error(StatusCode::CONFLICT, "conflict", "already a starter list");
    }
    let clean = |s: Option<String>| s.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    let id = Uuid::now_v7();
    let res = app
        .db
        .execute_raw(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            "INSERT INTO starter_list_overrides \
               (id, catalog_source_id, catalog_path, hidden, title, publisher, reason, created_by) \
             VALUES ($1, $2, $3, false, $4, $5, $6, $7) \
             ON CONFLICT (catalog_source_id, catalog_path) DO NOTHING",
            [
                id.into(),
                source.id.into(),
                path.clone().into(),
                clean(req.title).into(),
                clean(req.publisher).into(),
                reason.into(),
                admin.0.id.into(),
            ],
        ))
        .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => {
            return error(StatusCode::CONFLICT, "conflict", "already a starter list");
        }
        Ok(_) => {}
        Err(e) => return internal(e),
    }
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: admin.0.id,
            action: "admin.starter_list.add",
            target_type: Some("starter_list"),
            target_id: Some(id.to_string()),
            payload: serde_json::json!({ "catalog_source_id": source.id, "catalog_path": path }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    match build(&app, &admin.0, true).await {
        Ok(v) => (StatusCode::CREATED, Json(v)).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetStarterListVisibilityReq {
    pub catalog_source_id: Uuid,
    pub catalog_path: String,
    pub hidden: bool,
}

#[utoipa::path(
    operation_id = "starter_lists_admin_set_visibility", put,
    path = "/admin/starter-lists/visibility",
    request_body = SetStarterListVisibilityReq,
    responses((status = 200, body = StarterListListView), (status = 404))
)]
#[handler]
pub async fn admin_set_visibility(
    State(app): State<AppState>,
    admin: RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<SetStarterListVisibilityReq>,
) -> impl IntoResponse {
    let backend = app.db.get_database_backend();
    let res = if sl::shipped(req.catalog_source_id, &req.catalog_path).is_some() {
        // Shipped pick: the override row exists only to carry `hidden`.
        app.db
            .execute_raw(Statement::from_sql_and_values(
                backend,
                "INSERT INTO starter_list_overrides \
                   (id, catalog_source_id, catalog_path, hidden, created_by) \
                 VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT (catalog_source_id, catalog_path) \
                 DO UPDATE SET hidden = EXCLUDED.hidden, updated_at = now()",
                [
                    Uuid::now_v7().into(),
                    req.catalog_source_id.into(),
                    req.catalog_path.clone().into(),
                    req.hidden.into(),
                    admin.0.id.into(),
                ],
            ))
            .await
    } else {
        app.db
            .execute_raw(Statement::from_sql_and_values(
                backend,
                "UPDATE starter_list_overrides SET hidden = $3, updated_at = now() \
                 WHERE catalog_source_id = $1 AND catalog_path = $2",
                [
                    req.catalog_source_id.into(),
                    req.catalog_path.clone().into(),
                    req.hidden.into(),
                ],
            ))
            .await
    };
    match res {
        Ok(r) if r.rows_affected() == 0 => {
            return error(StatusCode::NOT_FOUND, "not_found", "not a starter list");
        }
        Ok(_) => {}
        Err(e) => return internal(e),
    }
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: admin.0.id,
            action: if req.hidden {
                "admin.starter_list.hide"
            } else {
                "admin.starter_list.show"
            },
            target_type: Some("starter_list"),
            target_id: None,
            payload: serde_json::json!({
                "catalog_source_id": req.catalog_source_id,
                "catalog_path": req.catalog_path,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    match build(&app, &admin.0, true).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => internal(e),
    }
}

#[utoipa::path(
    operation_id = "starter_lists_admin_delete", delete,
    path = "/admin/starter-lists/{id}",
    params(("id" = String, Path,)),
    responses((status = 200, body = StarterListListView), (status = 404))
)]
#[handler]
pub async fn admin_delete(
    State(app): State<AppState>,
    admin: RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let Ok(id) = Uuid::parse_str(&id) else {
        return error(StatusCode::NOT_FOUND, "not_found", "starter list");
    };
    // Deleting a shipped pick's row just un-hides it; deleting an added
    // pick removes it.
    let res = app
        .db
        .execute_raw(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            "DELETE FROM starter_list_overrides WHERE id = $1",
            [id.into()],
        ))
        .await;
    match res {
        Ok(r) if r.rows_affected() == 0 => {
            return error(StatusCode::NOT_FOUND, "not_found", "starter list");
        }
        Ok(_) => {}
        Err(e) => return internal(e),
    }
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: admin.0.id,
            action: "admin.starter_list.remove",
            target_type: Some("starter_list"),
            target_id: Some(id.to_string()),
            payload: serde_json::json!({}),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    match build(&app, &admin.0, true).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => internal(e),
    }
}

#[utoipa::path(
    operation_id = "starter_lists_admin_refresh_coverage", post,
    path = "/admin/starter-lists/refresh-coverage",
    responses((status = 202, description = "coverage recomputation started"))
)]
#[handler]
pub async fn admin_refresh_coverage(
    State(app): State<AppState>,
    admin: RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
) -> impl IntoResponse {
    let v = match build(&app, &admin.0, true).await {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let keys: Vec<(Uuid, String)> = v
        .items
        .iter()
        .filter_map(|i| {
            Uuid::parse_str(&i.catalog_source_id)
                .ok()
                .map(|id| (id, i.catalog_path.clone()))
        })
        .collect();
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: admin.0.id,
            action: "admin.starter_list.refresh_coverage",
            target_type: Some("starter_list"),
            target_id: None,
            payload: serde_json::json!({ "lists": keys.len() }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    sl::spawn_coverage(&app, keys);
    StatusCode::ACCEPTED.into_response()
}
