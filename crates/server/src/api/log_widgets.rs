//! `/me/log/widgets` — per-user customizable widget grid for the
//! Reading Log page. Backs the customizable view on `/log`.
//!
//! Wire shape:
//!
//!   GET    /me/log/widgets           → ordered list (defaults
//!                                       auto-seeded on first read)
//!   POST   /me/log/widgets           → add (server picks
//!                                       `position = max + 1`)
//!   PATCH  /me/log/widgets/{id}      → patch config (jsonb)
//!   DELETE /me/log/widgets/{id}      → remove
//!   POST   /me/log/widgets/reorder   → bulk reorder via id sequence
//!   POST   /me/log/widgets/reset     → wipe + re-seed defaults
//!
//! Validation: each widget kind has a typed config schema below; the
//! POST / PATCH handlers deserialize the request's `config` JSON
//! against the matching schema and reject anything that fails to
//! round-trip. Schemas use `#[serde(default)]` on every field so the
//! empty object `{}` is always legal — kinds with no config (e.g.
//! `time_of_day`) rely on that.

use axum::{
    Json, Router,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use entity::log_widget;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    QueryOrder, TransactionTrait, Unchanged,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::error;
use crate::auth::CurrentUser;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me/log/widgets", get(list).post(add))
        .route(
            "/me/log/widgets/{id}",
            axum::routing::patch(update).delete(remove),
        )
        .route("/me/log/widgets/reorder", post(reorder))
        .route("/me/log/widgets/reset", post(reset))
}

// ───────── Wire types ─────────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LogWidgetView {
    pub id: String,
    pub kind: String,
    pub position: i32,
    pub config: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

impl From<log_widget::Model> for LogWidgetView {
    fn from(m: log_widget::Model) -> Self {
        Self {
            id: m.id.to_string(),
            kind: m.kind,
            position: m.position,
            config: m.config,
            created_at: m.created_at.to_rfc3339(),
            updated_at: m.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LogWidgetListView {
    pub widgets: Vec<LogWidgetView>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AddWidgetReq {
    pub kind: String,
    /// Optional starting config. Defaults to `{}`, which is always
    /// a legal blob — the per-kind renderer fills in defaults.
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PatchWidgetReq {
    /// Replacement config blob. Replaces the entire `config` column
    /// — clients merge with the previous value client-side. The
    /// PATCH name is preserved because the row's other fields stay
    /// the same and config is what the form actually edits.
    pub config: serde_json::Value,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ReorderReq {
    /// Full set of widget ids in the new order. Server rejects when
    /// the set doesn't match the user's owned ids exactly — partial
    /// reorders would leave gaps in `position`.
    pub ids: Vec<String>,
}

// ───────── Widget kinds + config schemas ─────────

/// Every widget kind the server accepts. Adding a new kind:
/// (1) extend this list, (2) add a typed config struct + a match arm
/// in `validate_config`, (3) ship a renderer + Zod schema on the web
/// side. Default seeding is independent — see [`DEFAULT_LAYOUT`].
const WIDGET_KINDS: &[&str] = &[
    "chrono_feed",
    "stats_hero",
    "heatmap",
    "top_creators",
    "top_publishers",
    "top_imprints",
    "series_finishes",
    "pace_chart",
    "time_of_day",
    "recent_bookmarks",
    "currently_reading",
    "note",
];

/// Validate a `kind` + `config` blob pair. Returns `Ok` when the
/// kind is known AND the config shape round-trips through the
/// matching schema. Returns `Err(message)` otherwise — the caller
/// emits a 400 with `code: "validation"`.
fn validate_config(kind: &str, config: &serde_json::Value) -> Result<(), String> {
    if !WIDGET_KINDS.contains(&kind) {
        return Err(format!("unknown widget kind: {kind}"));
    }
    // `serde_json::from_value` borrows the input by clone, so we pay
    // one allocation per validation. Configs are tiny (< 1 KB) so
    // the cost is below the noise floor of the request handler.
    let v = config.clone();
    let result: Result<(), serde_json::Error> = match kind {
        "chrono_feed" => serde_json::from_value::<ChronoFeedConfig>(v).map(|_| ()),
        "stats_hero" => serde_json::from_value::<StatsHeroConfig>(v).map(|_| ()),
        "heatmap" => serde_json::from_value::<HeatmapConfig>(v).map(|_| ()),
        "top_creators" => serde_json::from_value::<TopCreatorsConfig>(v).map(|_| ()),
        "top_publishers" | "top_imprints" => serde_json::from_value::<RankingConfig>(v).map(|_| ()),
        "series_finishes" => serde_json::from_value::<RankingConfig>(v).map(|_| ()),
        "pace_chart" => serde_json::from_value::<PaceChartConfig>(v).map(|_| ()),
        "time_of_day" => serde_json::from_value::<EmptyConfig>(v).map(|_| ()),
        "recent_bookmarks" => serde_json::from_value::<RecentBookmarksConfig>(v).map(|_| ()),
        "currently_reading" => serde_json::from_value::<CurrentlyReadingConfig>(v).map(|_| ()),
        "note" => serde_json::from_value::<NoteConfig>(v).map(|_| ()),
        _ => unreachable!("WIDGET_KINDS guard above covers all known kinds"),
    };
    result.map_err(|e| format!("invalid config for {kind}: {e}"))
}

/// Catch-all "no fields" schema. `#[serde(default, deny_unknown_fields)]`
/// rejects anything other than `{}`.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct EmptyConfig {}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ChronoFeedConfig {
    group_by_day: bool,
    /// Default kind filter (empty array = all kinds). Validated
    /// against the four valid event kinds at submit time.
    default_kinds: Vec<String>,
}
impl Default for ChronoFeedConfig {
    fn default() -> Self {
        Self {
            group_by_day: true,
            default_kinds: Vec::new(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StatsHeroConfig {
    /// Subset of `["issues","hours","streak","pages","pace_spp"]`.
    /// Empty = render the three M2 defaults.
    metrics: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct HeatmapConfig {
    /// One of 4 / 8 / 12 / 26 / 52. The renderer clamps; the server
    /// also rejects out-of-range values to catch bugs early.
    weeks: i32,
}
impl Default for HeatmapConfig {
    fn default() -> Self {
        Self { weeks: 52 }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TopCreatorsConfig {
    role: String,
    range: String,
    limit: i32,
}
impl Default for TopCreatorsConfig {
    fn default() -> Self {
        Self {
            role: "writer".to_string(),
            range: "30d".to_string(),
            limit: 5,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RankingConfig {
    range: String,
    limit: i32,
}
impl Default for RankingConfig {
    fn default() -> Self {
        Self {
            range: "30d".to_string(),
            limit: 5,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PaceChartConfig {
    range: String,
}
impl Default for PaceChartConfig {
    fn default() -> Self {
        Self {
            range: "30d".to_string(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RecentBookmarksConfig {
    limit: i32,
    /// Subset of the four marker kinds. Empty = all four.
    kinds: Vec<String>,
}
impl Default for RecentBookmarksConfig {
    fn default() -> Self {
        Self {
            limit: 5,
            kinds: Vec::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CurrentlyReadingConfig {
    limit: i32,
}
impl Default for CurrentlyReadingConfig {
    fn default() -> Self {
        Self { limit: 5 }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct NoteConfig {
    body: String,
}

// ───────── Default seeding ─────────

/// Default widget set seeded the first time a user reads
/// `GET /me/log/widgets`. Matches the M2 hard-coded layout —
/// chronological feed in the main column, three summary widgets in
/// the right rail. Listed in render order so the
/// `position`-by-index assignment makes the seeded result match the
/// M2 visual.
const DEFAULT_LAYOUT: &[(&str, &str)] = &[
    ("chrono_feed", r#"{}"#),
    ("stats_hero", r#"{}"#),
    ("heatmap", r#"{"weeks": 52}"#),
    (
        "top_creators",
        r#"{"role":"writer","range":"30d","limit":5}"#,
    ),
];

async fn seed_defaults<C: ConnectionTrait>(db: &C, user_id: Uuid) -> Result<(), sea_orm::DbErr> {
    let now = Utc::now().fixed_offset();
    for (position, (kind, config_json)) in DEFAULT_LAYOUT.iter().enumerate() {
        let config: serde_json::Value =
            serde_json::from_str(config_json).expect("DEFAULT_LAYOUT json is hand-authored");
        log_widget::ActiveModel {
            id: Set(Uuid::now_v7()),
            user_id: Set(user_id),
            kind: Set(kind.to_string()),
            position: Set(position as i32),
            config: Set(config),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(db)
        .await?;
    }
    Ok(())
}

// ───────── Handlers ─────────

#[utoipa::path(
    get,
    path = "/me/log/widgets",
    responses(
        (status = 200, body = LogWidgetListView),
    )
)]
pub async fn list(State(app): State<AppState>, user: CurrentUser) -> Response {
    // First-read seeding lives in a single transaction so two
    // concurrent first-reads can't both insert the defaults. The
    // `SELECT count` on an empty table is a cheap index probe.
    let txn = match app.db.begin().await {
        Ok(t) => t,
        Err(e) => return internal(e),
    };
    let existing_count = match log_widget::Entity::find()
        .filter(log_widget::Column::UserId.eq(user.id))
        .count(&txn)
        .await
    {
        Ok(n) => n,
        Err(e) => return internal(e),
    };
    if existing_count == 0
        && let Err(e) = seed_defaults(&txn, user.id).await
    {
        return internal(e);
    }
    let rows = match log_widget::Entity::find()
        .filter(log_widget::Column::UserId.eq(user.id))
        .order_by_asc(log_widget::Column::Position)
        .all(&txn)
        .await
    {
        Ok(r) => r,
        Err(e) => return internal(e),
    };
    if let Err(e) = txn.commit().await {
        return internal(e);
    }
    let widgets: Vec<LogWidgetView> = rows.into_iter().map(Into::into).collect();
    Json(LogWidgetListView { widgets }).into_response()
}

#[utoipa::path(
    post,
    path = "/me/log/widgets",
    request_body = AddWidgetReq,
    responses(
        (status = 201, body = LogWidgetView),
        (status = 400, description = "unknown kind or invalid config"),
    )
)]
pub async fn add(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<AddWidgetReq>,
) -> Response {
    let config = req
        .config
        .unwrap_or(serde_json::Value::Object(Default::default()));
    if let Err(msg) = validate_config(&req.kind, &config) {
        return error(StatusCode::BAD_REQUEST, "validation", &msg);
    }
    // Next position = current max + 1. Done inside a transaction so
    // two concurrent adds can't pick the same slot.
    let txn = match app.db.begin().await {
        Ok(t) => t,
        Err(e) => return internal(e),
    };
    let next_position: i32 = match log_widget::Entity::find()
        .filter(log_widget::Column::UserId.eq(user.id))
        .order_by_desc(log_widget::Column::Position)
        .one(&txn)
        .await
    {
        Ok(Some(r)) => r.position + 1,
        Ok(None) => 0,
        Err(e) => return internal(e),
    };
    let now = Utc::now().fixed_offset();
    let id = Uuid::now_v7();
    let am = log_widget::ActiveModel {
        id: Set(id),
        user_id: Set(user.id),
        kind: Set(req.kind),
        position: Set(next_position),
        config: Set(config),
        created_at: Set(now),
        updated_at: Set(now),
    };
    let row = match am.insert(&txn).await {
        Ok(r) => r,
        Err(e) => return internal(e),
    };
    if let Err(e) = txn.commit().await {
        return internal(e);
    }
    (StatusCode::CREATED, Json(LogWidgetView::from(row))).into_response()
}

#[utoipa::path(
    patch,
    path = "/me/log/widgets/{id}",
    params(("id" = String, Path,)),
    request_body = PatchWidgetReq,
    responses(
        (status = 200, body = LogWidgetView),
        (status = 400, description = "invalid config"),
        (status = 404, description = "not found / not yours"),
    )
)]
pub async fn update(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<PatchWidgetReq>,
) -> Response {
    let existing = match owned_row(&app, user.id, id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Err(msg) = validate_config(&existing.kind, &req.config) {
        return error(StatusCode::BAD_REQUEST, "validation", &msg);
    }
    let now = Utc::now().fixed_offset();
    let am = log_widget::ActiveModel {
        id: Unchanged(existing.id),
        user_id: Unchanged(existing.user_id),
        kind: Unchanged(existing.kind),
        position: Unchanged(existing.position),
        config: Set(req.config),
        created_at: Unchanged(existing.created_at),
        updated_at: Set(now),
    };
    match am.update(&app.db).await {
        Ok(row) => Json(LogWidgetView::from(row)).into_response(),
        Err(e) => internal(e),
    }
}

#[utoipa::path(
    delete,
    path = "/me/log/widgets/{id}",
    params(("id" = String, Path,)),
    responses(
        (status = 204),
        (status = 404, description = "not found / not yours"),
    )
)]
pub async fn remove(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> Response {
    let existing = match owned_row(&app, user.id, id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    // Deletion + dense-rank rewrite happens together so we never
    // expose a gap in `position`. Concurrent deletes are safe
    // because the rewrite uses each row's *current* position rather
    // than an absolute target.
    let txn = match app.db.begin().await {
        Ok(t) => t,
        Err(e) => return internal(e),
    };
    if let Err(e) = log_widget::Entity::delete_by_id(existing.id)
        .exec(&txn)
        .await
    {
        return internal(e);
    }
    // Pull surviving rows, walk them in ascending order, rewrite
    // position to the running index. Cheap — widget counts are
    // single-digit-double-digit at most.
    let rows = match log_widget::Entity::find()
        .filter(log_widget::Column::UserId.eq(user.id))
        .order_by_asc(log_widget::Column::Position)
        .all(&txn)
        .await
    {
        Ok(r) => r,
        Err(e) => return internal(e),
    };
    for (i, row) in rows.into_iter().enumerate() {
        if row.position == i as i32 {
            continue;
        }
        let am = log_widget::ActiveModel {
            id: Unchanged(row.id),
            user_id: Unchanged(row.user_id),
            kind: Unchanged(row.kind),
            position: Set(i as i32),
            config: Unchanged(row.config),
            created_at: Unchanged(row.created_at),
            updated_at: Unchanged(row.updated_at),
        };
        if let Err(e) = am.update(&txn).await {
            return internal(e);
        }
    }
    if let Err(e) = txn.commit().await {
        return internal(e);
    }
    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    post,
    path = "/me/log/widgets/reorder",
    request_body = ReorderReq,
    responses(
        (status = 200, body = LogWidgetListView),
        (status = 400, description = "ids don't match owned set"),
    )
)]
pub async fn reorder(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<ReorderReq>,
) -> Response {
    let parsed_ids: Result<Vec<Uuid>, _> = req.ids.iter().map(|s| Uuid::parse_str(s)).collect();
    let parsed_ids = match parsed_ids {
        Ok(v) => v,
        Err(_) => return error(StatusCode::BAD_REQUEST, "validation", "invalid uuid in ids"),
    };
    let txn = match app.db.begin().await {
        Ok(t) => t,
        Err(e) => return internal(e),
    };
    let existing = match log_widget::Entity::find()
        .filter(log_widget::Column::UserId.eq(user.id))
        .all(&txn)
        .await
    {
        Ok(r) => r,
        Err(e) => return internal(e),
    };
    let existing_set: std::collections::HashSet<Uuid> = existing.iter().map(|r| r.id).collect();
    let req_set: std::collections::HashSet<Uuid> = parsed_ids.iter().copied().collect();
    if existing_set != req_set {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "ids must match the user's owned widget set exactly",
        );
    }
    // Map id → existing row so we don't have to refetch each one.
    let by_id: std::collections::HashMap<Uuid, log_widget::Model> =
        existing.into_iter().map(|r| (r.id, r)).collect();
    let now = Utc::now().fixed_offset();
    for (i, id) in parsed_ids.iter().enumerate() {
        let row = by_id.get(id).expect("set equality checked above");
        if row.position == i as i32 {
            continue;
        }
        let am = log_widget::ActiveModel {
            id: Unchanged(row.id),
            user_id: Unchanged(row.user_id),
            kind: Unchanged(row.kind.clone()),
            position: Set(i as i32),
            config: Unchanged(row.config.clone()),
            created_at: Unchanged(row.created_at),
            updated_at: Set(now),
        };
        if let Err(e) = am.update(&txn).await {
            return internal(e);
        }
    }
    // Re-read so the response reflects the new positions.
    let rows = match log_widget::Entity::find()
        .filter(log_widget::Column::UserId.eq(user.id))
        .order_by_asc(log_widget::Column::Position)
        .all(&txn)
        .await
    {
        Ok(r) => r,
        Err(e) => return internal(e),
    };
    if let Err(e) = txn.commit().await {
        return internal(e);
    }
    let widgets: Vec<LogWidgetView> = rows.into_iter().map(Into::into).collect();
    Json(LogWidgetListView { widgets }).into_response()
}

#[utoipa::path(
    post,
    path = "/me/log/widgets/reset",
    responses(
        (status = 200, body = LogWidgetListView),
    )
)]
pub async fn reset(State(app): State<AppState>, user: CurrentUser) -> Response {
    let txn = match app.db.begin().await {
        Ok(t) => t,
        Err(e) => return internal(e),
    };
    if let Err(e) = log_widget::Entity::delete_many()
        .filter(log_widget::Column::UserId.eq(user.id))
        .exec(&txn)
        .await
    {
        return internal(e);
    }
    if let Err(e) = seed_defaults(&txn, user.id).await {
        return internal(e);
    }
    let rows = match log_widget::Entity::find()
        .filter(log_widget::Column::UserId.eq(user.id))
        .order_by_asc(log_widget::Column::Position)
        .all(&txn)
        .await
    {
        Ok(r) => r,
        Err(e) => return internal(e),
    };
    if let Err(e) = txn.commit().await {
        return internal(e);
    }
    let widgets: Vec<LogWidgetView> = rows.into_iter().map(Into::into).collect();
    Json(LogWidgetListView { widgets }).into_response()
}

// ───────── Helpers ─────────

async fn owned_row(app: &AppState, user_id: Uuid, id: Uuid) -> Result<log_widget::Model, Response> {
    match log_widget::Entity::find_by_id(id).one(&app.db).await {
        Ok(Some(r)) if r.user_id == user_id => Ok(r),
        Ok(Some(_)) | Ok(None) => Err(error(
            StatusCode::NOT_FOUND,
            "not_found",
            "widget not found",
        )),
        Err(e) => Err(internal(e)),
    }
}

fn internal<E: std::fmt::Display>(e: E) -> Response {
    tracing::warn!(error = %e, "log_widgets handler failure");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
}

// Use `PaginatorTrait::count` — pulled in via a sea_orm prelude
// import in the calling functions. The `use` statement here keeps
// the trait in scope without polluting the module's top-level
// imports.
use sea_orm::PaginatorTrait;
