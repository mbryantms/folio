//! Linked reading lists 2.0: a personal copy of a reading list.
//!
//! A shared list is the admin's to arrange: a reader can't add an event
//! of their own to it, start a chapter or link a next part. `POST
//! /me/cbl-lists/{id}/copy` makes a list of the caller's from one they
//! can see, carrying over
//!
//! - the entries (the stored file, re-matched; matches made by hand are
//!   copied, since the matcher can't remake them),
//! - where it came from (a catalog or URL list keeps refreshing), unless
//!   the caller already has a list from the same place,
//! - the chapter breaks,
//! - the links to events and the next part, for linked lists the caller
//!   can see. They point at the same lists the original links to: a
//!   personal list may link shared lists, so nothing else is copied.
//!
//! The copy and the original do not stay in sync. Progress is per issue,
//! so what the reader has read shows in both.

use axum::{
    Json,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::reading_list_link::{self};
use entity::{cbl_list, reading_list_break};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter,
    Statement,
};
use serde::Serialize;
use server_macros::handler;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

use super::cbl_links::{internal, link_write_lock};
use super::cbl_lists::{SourceMeta, fetch_list, insert_list_from_parsed};
use super::error;
use crate::auth::CurrentUser;
use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(copy_list))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CopiedListRef {
    pub list_id: String,
    /// The caller's view of the copy (`/views/{view_id}`).
    pub view_id: Option<String>,
    pub name: String,
    /// Event and next-part links carried over.
    pub links: i32,
    /// Chapter breaks carried over.
    pub chapters: i32,
}

#[utoipa::path(
    operation_id = "cbl_lists_copy",
    post,
    path = "/me/cbl-lists/{id}/copy",
    params(("id" = String, Path,)),
    responses(
        (status = 201, body = CopiedListRef),
        (status = 404, description = "list not found"),
        (status = 422, description = "the per-user list limit is reached"),
        (status = 429, description = "per-user CBL import quota exceeded"),
    )
)]
#[handler]
pub async fn copy_list(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> Response {
    let source = match fetch_list(&app.db, id).await {
        Ok(l) if l.owner_user_id.is_none_or(|o| o == user.id) => l,
        // Someone else's personal list answers like an unknown one.
        Ok(_) => return error(StatusCode::NOT_FOUND, "not_found", "list not found"),
        Err(r) => return r,
    };
    let parsed = match parsers::cbl::parse(source.raw_xml.as_bytes()) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, list = %source.id, "cbl copy: stored file no longer parses");
            return internal();
        }
    };
    // A copy keeps its source (and so keeps refreshing, and Find events
    // keeps working) unless the caller already has a list from the same
    // place: two of their lists on one catalog file would both answer
    // "the caller's list for this file".
    let already_theirs = {
        let mut q = cbl_list::Entity::find()
            .filter(cbl_list::Column::OwnerUserId.eq(user.id))
            .filter(cbl_list::Column::SourceKind.eq(source.source_kind.clone()));
        q = match (&source.catalog_path, &source.source_url) {
            (Some(path), _) => q
                .filter(cbl_list::Column::CatalogSourceId.eq(source.catalog_source_id))
                .filter(cbl_list::Column::CatalogPath.eq(path.clone())),
            (None, Some(url)) => q.filter(cbl_list::Column::SourceUrl.eq(url.clone())),
            (None, None) => q.filter(cbl_list::Column::Id.eq(Uuid::nil())),
        };
        match sea_orm::PaginatorTrait::count(q, &app.db).await {
            Ok(n) => n > 0,
            Err(_) => return internal(),
        }
    };
    let meta = match (
        source.source_kind.as_str(),
        source.catalog_source_id,
        source.catalog_path.clone(),
        source.source_url.clone(),
    ) {
        _ if already_theirs => SourceMeta::Upload {
            name_override: None,
            description: source.description.clone(),
        },
        ("catalog", Some(catalog_source_id), Some(catalog_path), _) => SourceMeta::Catalog {
            catalog_source_id,
            catalog_path,
            blob_sha: source.github_blob_sha.clone().unwrap_or_default(),
            description: source.description.clone(),
            refresh_schedule: source.refresh_schedule.clone(),
        },
        ("url", _, _, Some(url)) => SourceMeta::Url {
            url,
            etag: source.source_etag.clone(),
            last_modified: source.source_last_modified.clone(),
            description: source.description.clone(),
            refresh_schedule: source.refresh_schedule.clone(),
        },
        _ => SourceMeta::Upload {
            name_override: None,
            description: source.description.clone(),
        },
    };
    if !matches!(meta, SourceMeta::Upload { .. })
        && let Err(resp) = crate::cbl::quota::check_remote_list_cap(&app, &user).await
    {
        return resp;
    }
    if let Err(resp) = crate::cbl::quota::consume_import(&app, &user).await {
        return resp;
    }
    let copy =
        match insert_list_from_parsed(&app, Some(user.id), &parsed, &source.raw_xml, meta).await {
            Ok(l) => l,
            Err(resp) => return resp,
        };

    // Same file, so the same positions: carry the matches made by hand.
    if let Err(e) = app
        .db
        .execute_raw(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            "UPDATE cbl_entries t SET \
                matched_issue_id = s.matched_issue_id, match_status = s.match_status, \
                match_method = s.match_method, match_confidence = s.match_confidence, \
                ambiguous_candidates = s.ambiguous_candidates, \
                user_resolved_at = s.user_resolved_at, matched_at = s.matched_at \
             FROM cbl_entries s \
             WHERE s.cbl_list_id = $1 AND t.cbl_list_id = $2 \
               AND s.position = t.position AND s.match_status = 'manual'",
            [source.id.into(), copy.id.into()],
        ))
        .await
    {
        tracing::warn!(error = %e, "cbl copy: couldn't carry the manual matches");
    }

    let chapters = copy_breaks(&app, &source, &copy, &user).await;
    let links = copy_links(&app, &source, &copy, &user).await;
    let view_id = match super::saved_views::ensure_cbl_view(&app.db, user.id, &copy).await {
        Ok((v, _)) => Some(v.to_string()),
        Err(e) => {
            tracing::warn!(error = %e, "cbl copy: couldn't create the copy's view");
            None
        }
    };
    (
        StatusCode::CREATED,
        Json(CopiedListRef {
            list_id: copy.id.to_string(),
            view_id,
            name: copy.parsed_name,
            links,
            chapters,
        }),
    )
        .into_response()
}

/// Copy the source's chapter breaks; returns how many. A failure leaves
/// the copy without (some of) them rather than failing the copy.
async fn copy_breaks(
    app: &AppState,
    source: &cbl_list::Model,
    copy: &cbl_list::Model,
    user: &CurrentUser,
) -> i32 {
    let rows = match reading_list_break::Entity::find()
        .filter(reading_list_break::Column::ListId.eq(source.id))
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "cbl copy: couldn't read the chapter breaks");
            return 0;
        }
    };
    let now = Utc::now().fixed_offset();
    let mut copied = 0;
    for b in rows {
        let mut am: reading_list_break::ActiveModel = b.into();
        am.id = Set(Uuid::now_v7());
        am.list_id = Set(copy.id);
        am.created_by = Set(Some(user.id));
        am.created_at = Set(now);
        am.updated_at = Set(now);
        match am.insert(&app.db).await {
            Ok(_) => copied += 1,
            Err(e) => tracing::warn!(error = %e, "cbl copy: couldn't copy a chapter break"),
        }
    }
    copied
}

/// Copy the source's links to lists the caller can see; returns how many.
/// The source already obeys the link rules (one level of events, one next
/// part, no loop), and a new list is nobody's event or next part, so the
/// copies obey them too.
async fn copy_links(
    app: &AppState,
    source: &cbl_list::Model,
    copy: &cbl_list::Model,
    user: &CurrentUser,
) -> i32 {
    let Ok(lock) = link_write_lock(app).await else {
        return 0;
    };
    let rows = match reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ParentListId.eq(source.id))
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "cbl copy: couldn't read the links");
            return 0;
        }
    };
    let children: Vec<Uuid> = rows.iter().map(|l| l.child_list_id).collect();
    let visible: std::collections::HashSet<Uuid> =
        match super::cbl_link_nav::lists_lite(&app.db, &children).await {
            Ok(lists) => lists
                .into_iter()
                .filter(|l| l.visible_to(user.id))
                .map(|l| l.id)
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "cbl copy: couldn't read the linked lists");
                return 0;
            }
        };
    let now = Utc::now().fixed_offset();
    let mut copied = 0;
    for l in rows {
        if !visible.contains(&l.child_list_id) {
            continue;
        }
        let mut am: reading_list_link::ActiveModel = l.into();
        am.id = Set(Uuid::now_v7());
        am.parent_list_id = Set(copy.id);
        am.created_by = Set(Some(user.id));
        am.created_at = Set(now);
        am.updated_at = Set(now);
        match am.insert(&app.db).await {
            Ok(_) => copied += 1,
            Err(e) => tracing::warn!(error = %e, "cbl copy: couldn't copy a link"),
        }
    }
    if let Err(e) = lock.commit().await {
        tracing::warn!(error = %e, "cbl copy: releasing the write lock failed");
    }
    copied
}
