//! Markers + Collections M2 — collections CRUD + entry management.
//!
//! Collections are user-owned saved views with `kind = 'collection'`,
//! backed by ordered `collection_entries` rows that reference either a
//! series or an issue (XOR). They sit alongside filter views and CBLs
//! in the saved-views surface; the dedicated `/me/collections/*` path
//! exposes collection-specific affordances (entry CRUD, lazy Want to
//! Read seeding) that don't make sense as part of the generic
//! `/me/saved-views/*` API.
//!
//! Want to Read is a per-user collection auto-seeded on the first GET
//! to `/me/collections` (and on the first add via the cover menu in
//! M3) so a brand-new user can drop something into "Want to Read"
//! without ever visiting the manager.
//!
//! `/me/saved-views/{id}/results` returns an empty `SeriesListView`
//! stub for collections, mirroring the CBL stub — the home rail and
//! detail page render via `/me/collections/{id}/entries`, which carries
//! both series and issue cards.

use axum::{
    Json, Router,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, patch, post},
};
use chrono::Utc;
use entity::{collection_entry, issue, saved_view, series, user_view_pin};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, ModelTrait,
    QueryFilter, QueryOrder, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::api::saved_views::{KIND_COLLECTION, SYSTEM_KEY_WANT_TO_READ, SavedViewView};
use crate::api::series::{IssueSummaryView, SeriesView, hydrate_series};
use crate::auth::CurrentUser;
use crate::state::AppState;

const MAX_ENTRIES_LIMIT: u64 = 200;
const MIN_ENTRIES_LIMIT: u64 = 1;
const DEFAULT_ENTRIES_LIMIT: u64 = 60;
const ENTRY_KIND_SERIES: &str = "series";
const ENTRY_KIND_ISSUE: &str = "issue";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me/collections", get(list).post(create))
        .route("/me/collections/{id}", patch(update).delete(delete_one))
        .route(
            "/me/collections/{id}/entries",
            get(list_entries).post(add_entry),
        )
        .route(
            "/me/collections/{id}/entries/{entry_id}",
            delete(remove_entry),
        )
        .route(
            "/me/collections/{id}/entries/reorder",
            post(reorder_entries),
        )
}

// ───── wire types ─────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CollectionEntryView {
    pub id: String,
    pub position: i32,
    /// `'series'` or `'issue'`.
    pub entry_kind: String,
    pub added_at: String,
    /// Populated when `entry_kind = 'series'`. `None` when the
    /// underlying series was cascade-deleted between the entry insert
    /// and this read. Hydrated to the full `SeriesView` shape so the
    /// home rail + collection detail page can reuse `<SeriesCard>`
    /// without a synthesize-defaults dance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<SeriesView>,
    /// Populated when `entry_kind = 'issue'`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<IssueSummaryView>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CollectionEntriesView {
    pub items: Vec<CollectionEntryView>,
    pub next_cursor: Option<String>,
    /// First-page total (entries belong to one collection, so this is a
    /// cheap COUNT). Useful for "N items" headers without re-fetching.
    pub total: Option<i64>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateCollectionReq {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateCollectionReq {
    #[serde(default)]
    pub name: Option<String>,
    /// Double-Option lets the client clear the description by sending
    /// an explicit `null`; omitting the field leaves it unchanged.
    #[serde(default)]
    pub description: Option<Option<String>>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AddEntryReq {
    /// `'series'` or `'issue'`.
    pub entry_kind: String,
    /// Series UUID or issue id (TEXT). Validated against the
    /// declared `entry_kind`.
    pub ref_id: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ReorderEntriesReq {
    pub entry_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ListEntriesQuery {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
}

// ───── helpers ─────

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

/// Fetch a collection by id and verify the caller owns it. Returns the
/// row on success; an HTTP response otherwise (404 / 403 / 500).
async fn fetch_owned(
    db: &impl ConnectionTrait,
    user_id: Uuid,
    id: Uuid,
) -> Result<saved_view::Model, axum::response::Response> {
    let row = saved_view::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "collections: fetch failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        })?;
    let Some(row) = row else {
        return Err(error(
            StatusCode::NOT_FOUND,
            "not_found",
            "collection not found",
        ));
    };
    if row.kind != KIND_COLLECTION {
        return Err(error(
            StatusCode::NOT_FOUND,
            "not_found",
            "collection not found",
        ));
    }
    if row.user_id != Some(user_id) {
        return Err(error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "not your collection",
        ));
    }
    Ok(row)
}

fn to_view(
    model: &saved_view::Model,
    pin: Option<&user_view_pin::Model>,
    pinned_on_pages: Vec<String>,
) -> SavedViewView {
    let pinned = pin.map(|p| p.pinned).unwrap_or(false);
    let show_in_sidebar = pin.map(|p| p.show_in_sidebar).unwrap_or(false);
    SavedViewView {
        id: model.id.to_string(),
        user_id: model.user_id.map(|u| u.to_string()),
        kind: model.kind.clone(),
        name: model.name.clone(),
        description: model.description.clone(),
        custom_year_start: model.custom_year_start,
        custom_year_end: model.custom_year_end,
        custom_tags: model.custom_tags.clone(),
        match_mode: None,
        conditions: None,
        sort_field: None,
        sort_order: None,
        result_limit: None,
        cbl_list_id: None,
        created_at: model.created_at.to_rfc3339(),
        updated_at: model.updated_at.to_rfc3339(),
        pinned,
        pinned_position: if pinned {
            pin.map(|p| p.position)
        } else {
            None
        },
        show_in_sidebar,
        is_system: model.user_id.is_none(),
        system_key: model.system_key.clone(),
        icon: pin.and_then(|p| p.icon.clone()),
        pinned_on_pages,
    }
}

/// Idempotently seed a Want to Read collection for `user_id`. Called
/// from `list` AND from `saved_views::list` so the sidebar picks up
/// the row on first page load even if the user never visits the
/// collections index. Safe to call repeatedly — the
/// `(user_id, system_key)` partial unique catches concurrent
/// insertions and the helper treats that as success.
pub(crate) async fn ensure_want_to_read_seeded(
    db: &impl ConnectionTrait,
    user_id: Uuid,
) -> Result<saved_view::Model, sea_orm::DbErr> {
    if let Some(existing) = saved_view::Entity::find()
        .filter(saved_view::Column::UserId.eq(user_id))
        .filter(saved_view::Column::SystemKey.eq(SYSTEM_KEY_WANT_TO_READ))
        .one(db)
        .await?
    {
        return Ok(existing);
    }
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    let insert = saved_view::ActiveModel {
        id: Set(id),
        user_id: Set(Some(user_id)),
        kind: Set(KIND_COLLECTION.into()),
        system_key: Set(Some(SYSTEM_KEY_WANT_TO_READ.into())),
        name: Set("Want to Read".into()),
        description: Set(Some("Series and issues you want to read later.".into())),
        custom_year_start: Set(None),
        custom_year_end: Set(None),
        custom_tags: Set(Vec::new()),
        match_mode: Set(None),
        conditions: Set(None),
        sort_field: Set(None),
        sort_order: Set(None),
        result_limit: Set(None),
        cbl_list_id: Set(None),
        auto_pin: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await;
    match insert {
        Ok(m) => {
            // Want to Read is surfaced in the sidebar via a hardcoded
            // entry in `main-nav.ts` (the Browse section), so we leave
            // `show_in_sidebar = false` to avoid a duplicate listing
            // under "Saved views". The pin row still lands so the
            // user's per-view icon override (`user_view_pins.icon`)
            // has a place to live should they customize it later.
            // Multi-page rails M1: every pin row lives on a page. The seed
            // belongs on the user's auto-created system page; if the lookup
            // fails (only possible in pathological test states) we still
            // return the saved view — the pin row is optional metadata.
            if let Ok(page_id) = crate::pages::system_page_id(db, user_id).await {
                let _ = user_view_pin::ActiveModel {
                    user_id: Set(user_id),
                    page_id: Set(page_id),
                    view_id: Set(m.id),
                    position: Set(0),
                    pinned: Set(false),
                    show_in_sidebar: Set(false),
                    icon: Set(Some("list-plus".into())),
                }
                .insert(db)
                .await;
            }
            Ok(m)
        }
        Err(_) => {
            // Concurrent seed by a parallel request — read the row that
            // landed and return it.
            saved_view::Entity::find()
                .filter(saved_view::Column::UserId.eq(user_id))
                .filter(saved_view::Column::SystemKey.eq(SYSTEM_KEY_WANT_TO_READ))
                .one(db)
                .await?
                .ok_or(sea_orm::DbErr::Custom(
                    "want_to_read seed lost to concurrent caller".into(),
                ))
        }
    }
}

// ───── handlers ─────

#[utoipa::path(
    get,
    path = "/me/collections",
    responses((status = 200, body = Vec<SavedViewView>))
)]
pub async fn list(State(app): State<AppState>, user: CurrentUser) -> impl IntoResponse {
    if let Err(e) = ensure_want_to_read_seeded(&app.db, user.id).await {
        tracing::warn!(user_id = %user.id, error = %e, "collections: want_to_read seed failed");
    }

    let rows = match saved_view::Entity::find()
        .filter(saved_view::Column::UserId.eq(user.id))
        .filter(saved_view::Column::Kind.eq(KIND_COLLECTION))
        .order_by_asc(saved_view::Column::Name)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "collections: list failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let pins = user_view_pin::Entity::find()
        .filter(user_view_pin::Column::UserId.eq(user.id))
        .all(&app.db)
        .await
        .unwrap_or_default();
    // Pin lookup for the legacy single-pin fields, plus a per-view
    // page list for the multi-pin picker on the saved-view detail page.
    let mut pinned_pages_by_view: HashMap<Uuid, Vec<String>> = HashMap::new();
    for p in &pins {
        if p.pinned {
            pinned_pages_by_view
                .entry(p.view_id)
                .or_default()
                .push(p.page_id.to_string());
        }
    }
    let pin_by_view: HashMap<Uuid, user_view_pin::Model> =
        pins.into_iter().map(|p| (p.view_id, p)).collect();

    // Want to Read first, then alpha by name. (Already alpha by query;
    // pull WTR to the top in a stable second pass.)
    let mut items: Vec<SavedViewView> = rows
        .iter()
        .map(|m| {
            let pages = pinned_pages_by_view.get(&m.id).cloned().unwrap_or_default();
            to_view(m, pin_by_view.get(&m.id), pages)
        })
        .collect();
    items.sort_by(|a, b| {
        let a_wtr = a.system_key.as_deref() == Some(SYSTEM_KEY_WANT_TO_READ);
        let b_wtr = b.system_key.as_deref() == Some(SYSTEM_KEY_WANT_TO_READ);
        match (a_wtr, b_wtr) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    Json(items).into_response()
}

#[utoipa::path(
    post,
    path = "/me/collections",
    request_body = CreateCollectionReq,
    responses((status = 201, body = SavedViewView))
)]
pub async fn create(
    State(app): State<AppState>,
    user: CurrentUser,
    Json(req): Json<CreateCollectionReq>,
) -> impl IntoResponse {
    let name = req.name.trim();
    if name.is_empty() {
        return error(StatusCode::BAD_REQUEST, "validation", "name required");
    }
    if name.len() > 200 {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "name must be 200 chars or fewer",
        );
    }
    let description = req
        .description
        .as_ref()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());

    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    let saved = match (saved_view::ActiveModel {
        id: Set(id),
        user_id: Set(Some(user.id)),
        kind: Set(KIND_COLLECTION.into()),
        system_key: Set(None),
        name: Set(name.to_owned()),
        description: Set(description),
        custom_year_start: Set(None),
        custom_year_end: Set(None),
        custom_tags: Set(Vec::new()),
        match_mode: Set(None),
        conditions: Set(None),
        sort_field: Set(None),
        sort_order: Set(None),
        result_limit: Set(None),
        cbl_list_id: Set(None),
        auto_pin: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .insert(&app.db)
    .await
    {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "collections: create failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    (StatusCode::CREATED, Json(to_view(&saved, None, Vec::new()))).into_response()
}

#[utoipa::path(
    patch,
    path = "/me/collections/{id}",
    params(("id" = String, Path,)),
    request_body = UpdateCollectionReq,
    responses((status = 200, body = SavedViewView))
)]
pub async fn update(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<UpdateCollectionReq>,
) -> impl IntoResponse {
    let row = match fetch_owned(&app.db, user.id, id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let mut am: saved_view::ActiveModel = row.clone().into();
    if let Some(name) = req.name.as_ref() {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return error(StatusCode::BAD_REQUEST, "validation", "name required");
        }
        if trimmed.len() > 200 {
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                "name must be 200 chars or fewer",
            );
        }
        am.name = Set(trimmed.to_owned());
    }
    if let Some(desc) = req.description.as_ref() {
        am.description = Set(desc
            .as_ref()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty()));
    }
    am.updated_at = Set(Utc::now().fixed_offset());
    let updated = match am.update(&app.db).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "collections: update failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    // Fetch every pin row for this view so the response mirrors the
    // list endpoint shape — `pinned_on_pages` keeps the multi-pin
    // picker honest after an inline rename + refresh.
    let all_pins: Vec<user_view_pin::Model> = user_view_pin::Entity::find()
        .filter(user_view_pin::Column::UserId.eq(user.id))
        .filter(user_view_pin::Column::ViewId.eq(updated.id))
        .all(&app.db)
        .await
        .unwrap_or_default();
    let pages: Vec<String> = all_pins
        .iter()
        .filter(|p| p.pinned)
        .map(|p| p.page_id.to_string())
        .collect();
    let system_pid = crate::pages::system_page_id(&app.db, user.id).await.ok();
    let pin = all_pins
        .iter()
        .find(|p| Some(p.page_id) == system_pid)
        .cloned()
        .or_else(|| {
            let mut sorted = all_pins.clone();
            sorted.sort_by_key(|p| p.position);
            sorted.into_iter().next()
        });
    Json(to_view(&updated, pin.as_ref(), pages)).into_response()
}

#[utoipa::path(
    delete,
    path = "/me/collections/{id}",
    params(("id" = String, Path,)),
    responses((status = 204))
)]
pub async fn delete_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> impl IntoResponse {
    let row = match fetch_owned(&app.db, user.id, id).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    // Want to Read is the per-user system collection — undeletable.
    // The user can empty it but the row itself is preserved so the
    // sidebar item + cover-menu "Add to Want to Read" action keep
    // working without a re-seed dance.
    if row.system_key.as_deref() == Some(SYSTEM_KEY_WANT_TO_READ) {
        return error(
            StatusCode::CONFLICT,
            "want_to_read_undeletable",
            "Want to Read cannot be deleted",
        );
    }
    if let Err(e) = row.delete(&app.db).await {
        tracing::error!(error = %e, "collections: delete failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    get,
    path = "/me/collections/{id}/entries",
    params(
        ("id" = String, Path,),
        ("cursor" = Option<String>, Query,),
        ("limit" = Option<u64>, Query,),
    ),
    responses((status = 200, body = CollectionEntriesView))
)]
pub async fn list_entries(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Query(q): Query<ListEntriesQuery>,
) -> impl IntoResponse {
    if let Err(resp) = fetch_owned(&app.db, user.id, id).await {
        return resp;
    }

    let limit = q
        .limit
        .unwrap_or(DEFAULT_ENTRIES_LIMIT)
        .clamp(MIN_ENTRIES_LIMIT, MAX_ENTRIES_LIMIT);

    // Cursor is `position:i32` encoded base64; the entry id isn't
    // needed as a tiebreaker because `(saved_view_id, position)` is
    // already unique.
    let after = q.cursor.as_deref().and_then(decode_position_cursor);

    let mut select = collection_entry::Entity::find()
        .filter(collection_entry::Column::SavedViewId.eq(id))
        .order_by_asc(collection_entry::Column::Position);
    if let Some(pos) = after {
        select = select.filter(collection_entry::Column::Position.gt(pos));
    }
    let rows = match select.limit(limit + 1).all(&app.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "collections: list entries failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let total_only_on_first_page = after.is_none();
    let total = if total_only_on_first_page {
        collection_entry::Entity::find()
            .filter(collection_entry::Column::SavedViewId.eq(id))
            .count(&app.db)
            .await
            .ok()
            .map(|c| c as i64)
    } else {
        None
    };

    let mut rows = rows;
    let next_cursor = if rows.len() as u64 > limit {
        let extra = rows.pop();
        extra.map(|r| encode_position_cursor(r.position - 1))
    } else {
        None
    };

    let items = hydrate_entries(&app, rows).await;
    Json(CollectionEntriesView {
        items,
        next_cursor,
        total,
    })
    .into_response()
}

#[utoipa::path(
    post,
    path = "/me/collections/{id}/entries",
    params(("id" = String, Path,)),
    request_body = AddEntryReq,
    responses((status = 201, body = CollectionEntryView))
)]
pub async fn add_entry(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<AddEntryReq>,
) -> impl IntoResponse {
    if let Err(resp) = fetch_owned(&app.db, user.id, id).await {
        return resp;
    }

    let (series_id, issue_id) = match req.entry_kind.as_str() {
        ENTRY_KIND_SERIES => match Uuid::parse_str(&req.ref_id) {
            Ok(uid) => (Some(uid), None),
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "validation",
                    "ref_id must be a UUID for entry_kind='series'",
                );
            }
        },
        ENTRY_KIND_ISSUE => {
            // Issue ids are BLAKE3 hex (TEXT). Light validation only
            // — the FK check at insert time is the source of truth.
            if req.ref_id.is_empty() || req.ref_id.len() > 128 {
                return error(StatusCode::BAD_REQUEST, "validation", "ref_id invalid");
            }
            (None, Some(req.ref_id.clone()))
        }
        _ => {
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                "entry_kind must be 'series' or 'issue'",
            );
        }
    };

    // Existence check before the insert — gives us a crisp 404 for
    // bogus ref_ids instead of a generic FK violation.
    if let Some(sid) = series_id {
        let exists = series::Entity::find_by_id(sid)
            .count(&app.db)
            .await
            .unwrap_or(0);
        if exists == 0 {
            return error(StatusCode::NOT_FOUND, "not_found", "series not found");
        }
    }
    if let Some(iid) = issue_id.as_deref() {
        let exists = issue::Entity::find_by_id(iid.to_owned())
            .count(&app.db)
            .await
            .unwrap_or(0);
        if exists == 0 {
            return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
        }
    }

    // Idempotent: if the partial unique catches a duplicate, return
    // 409 with a stable code so the client can toast "already in".
    let txn = match app.db.begin().await {
        Ok(t) => t,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    let next_pos = match collection_entry::Entity::find()
        .filter(collection_entry::Column::SavedViewId.eq(id))
        .order_by_desc(collection_entry::Column::Position)
        .one(&txn)
        .await
    {
        Ok(Some(m)) => m.position + 1,
        Ok(None) => 0,
        Err(_) => {
            let _ = txn.rollback().await;
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let entry_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    let insert = collection_entry::ActiveModel {
        id: Set(entry_id),
        saved_view_id: Set(id),
        position: Set(next_pos),
        entry_kind: Set(req.entry_kind.clone()),
        series_id: Set(series_id),
        issue_id: Set(issue_id.clone()),
        added_at: Set(now),
    }
    .insert(&txn)
    .await;
    let entry = match insert {
        Ok(m) => m,
        Err(e) => {
            let _ = txn.rollback().await;
            // Sniff for the idempotent-add partial unique.
            let msg = e.to_string();
            if msg.contains("collection_entries_series_uniq")
                || msg.contains("collection_entries_issue_uniq")
            {
                return error(
                    StatusCode::CONFLICT,
                    "already_in_collection",
                    "already in this collection",
                );
            }
            tracing::error!(error = %e, "collections: add entry failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if let Err(e) = txn.commit().await {
        tracing::error!(error = %e, "collections: add entry commit failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    let hydrated = hydrate_entries(&app, vec![entry]).await;
    let first = hydrated.into_iter().next().unwrap_or(CollectionEntryView {
        id: entry_id.to_string(),
        position: next_pos,
        entry_kind: req.entry_kind,
        added_at: now.to_rfc3339(),
        series: None,
        issue: None,
    });
    (StatusCode::CREATED, Json(first)).into_response()
}

#[utoipa::path(
    delete,
    path = "/me/collections/{id}/entries/{entry_id}",
    params(("id" = String, Path,), ("entry_id" = String, Path,)),
    responses((status = 204))
)]
pub async fn remove_entry(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((id, entry_id)): AxPath<(Uuid, Uuid)>,
) -> impl IntoResponse {
    if let Err(resp) = fetch_owned(&app.db, user.id, id).await {
        return resp;
    }
    let entry = collection_entry::Entity::find_by_id(entry_id)
        .filter(collection_entry::Column::SavedViewId.eq(id))
        .one(&app.db)
        .await;
    let entry = match entry {
        Ok(Some(e)) => e,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "entry not found"),
        Err(e) => {
            tracing::error!(error = %e, "collections: fetch entry failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if let Err(e) = entry.delete(&app.db).await {
        tracing::error!(error = %e, "collections: delete entry failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    StatusCode::NO_CONTENT.into_response()
}

#[utoipa::path(
    post,
    path = "/me/collections/{id}/entries/reorder",
    params(("id" = String, Path,)),
    request_body = ReorderEntriesReq,
    responses((status = 204))
)]
pub async fn reorder_entries(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<ReorderEntriesReq>,
) -> impl IntoResponse {
    if let Err(resp) = fetch_owned(&app.db, user.id, id).await {
        return resp;
    }

    let existing = match collection_entry::Entity::find()
        .filter(collection_entry::Column::SavedViewId.eq(id))
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "collections: reorder fetch failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let by_id: HashMap<Uuid, collection_entry::Model> =
        existing.iter().map(|e| (e.id, e.clone())).collect();

    // Validate every requested id belongs to this collection.
    for entry_id in &req.entry_ids {
        if !by_id.contains_key(entry_id) {
            return error(
                StatusCode::BAD_REQUEST,
                "validation",
                "every entry_id must belong to this collection",
            );
        }
    }
    // Require a full reorder — partial reorders are ambiguous against
    // the deferrable position uniqueness constraint.
    if req.entry_ids.len() != existing.len() {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "entry_ids must include every current entry",
        );
    }

    let txn = match app.db.begin().await {
        Ok(t) => t,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal"),
    };
    // The deferrable `(saved_view_id, position)` unique lets the new
    // positions land in one pass — Postgres defers the check to
    // commit, so intermediate row states with duplicate positions
    // never trip the constraint.
    for (i, entry_id) in req.entry_ids.iter().enumerate() {
        let row = by_id.get(entry_id).cloned().unwrap();
        let mut am: collection_entry::ActiveModel = row.into();
        am.position = Set(i as i32);
        if let Err(e) = am.update(&txn).await {
            let _ = txn.rollback().await;
            tracing::error!(error = %e, "collections: reorder write failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    }
    if let Err(e) = txn.commit().await {
        tracing::error!(error = %e, "collections: reorder commit failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    StatusCode::NO_CONTENT.into_response()
}

// ───── hydration + cursor helpers ─────

async fn hydrate_entries(
    app: &AppState,
    rows: Vec<collection_entry::Model>,
) -> Vec<CollectionEntryView> {
    if rows.is_empty() {
        return Vec::new();
    }
    let series_ids: Vec<Uuid> = rows.iter().filter_map(|r| r.series_id).collect();
    let issue_ids: Vec<String> = rows.iter().filter_map(|r| r.issue_id.clone()).collect();

    // Series batch: fetch models then hand off to the shared
    // `hydrate_series` helper so the home rail + detail page get the
    // same `issue_count` + `cover_url` shape that the rest of the app
    // already paints into `<SeriesCard>`.
    let series_rows: Vec<series::Model> = if series_ids.is_empty() {
        Vec::new()
    } else {
        series::Entity::find()
            .filter(series::Column::Id.is_in(series_ids))
            .all(&app.db)
            .await
            .unwrap_or_default()
    };
    let hydrated_series = hydrate_series(app, series_rows).await;
    let series_by_id: HashMap<Uuid, SeriesView> = hydrated_series
        .into_iter()
        .filter_map(|v| Uuid::parse_str(&v.id).ok().map(|id| (id, v)))
        .collect();

    // Issue batch: model + parent series slug for the summary view.
    let issue_rows: Vec<issue::Model> = if issue_ids.is_empty() {
        Vec::new()
    } else {
        issue::Entity::find()
            .filter(issue::Column::Id.is_in(issue_ids.clone()))
            .all(&app.db)
            .await
            .unwrap_or_default()
    };
    let issue_series_ids: Vec<Uuid> = issue_rows.iter().map(|i| i.series_id).collect();
    let issue_series_meta: HashMap<Uuid, (String, String)> = if issue_series_ids.is_empty() {
        HashMap::new()
    } else {
        series::Entity::find()
            .filter(series::Column::Id.is_in(issue_series_ids))
            .all(&app.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.id, (s.slug, s.name)))
            .collect()
    };
    let issue_by_id: HashMap<String, issue::Model> =
        issue_rows.into_iter().map(|i| (i.id.clone(), i)).collect();

    rows.into_iter()
        .map(|r| {
            let series = r.series_id.and_then(|sid| series_by_id.get(&sid).cloned());
            let issue = r.issue_id.as_deref().and_then(|iid| {
                issue_by_id.get(iid).map(|m| {
                    let (slug, name) = issue_series_meta
                        .get(&m.series_id)
                        .cloned()
                        .unwrap_or_default();
                    let view = IssueSummaryView::from_model(m.clone(), &slug);
                    if name.is_empty() {
                        view
                    } else {
                        view.with_series_name(name)
                    }
                })
            });
            CollectionEntryView {
                id: r.id.to_string(),
                position: r.position,
                entry_kind: r.entry_kind,
                added_at: r.added_at.to_rfc3339(),
                series,
                issue,
            }
        })
        .collect()
}

fn encode_position_cursor(after: i32) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(after.to_string().as_bytes())
}

fn decode_position_cursor(s: &str) -> Option<i32> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    decoded.parse::<i32>().ok()
}

// Pull in the count-style helper without importing all of sea_orm.
use sea_orm::PaginatorTrait;
use sea_orm::QuerySelect;
