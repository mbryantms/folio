//! Cover picks (post-audit T4.2, decision D8a).
//!
//! Two admin-only choices that the scanner and the provider pipeline
//! otherwise make on their own:
//!
//! - **Issue cover page** — `PATCH /series/{s}/issues/{i}/cover
//!   {page_index}` sets `issues.cover_page_index`, marks that page
//!   `Type="FrontCover"` in the issue's `pages` JSON (which the sidecar
//!   composer forwards into ComicInfo's `<Pages>` block), pins the column
//!   under the user tier so a rescan never moves it, queues the manual
//!   sidecar rewrite (writeback libraries get the marker in the archive;
//!   the scoped rescan then reads back the same page) and regenerates the
//!   cover thumbnail.
//! - **Series cover** — `PATCH /series/{s}/cover {issue_id}` records
//!   which issue's cover represents the series as an active
//!   `series_cover` row (`kind = "pick"`, `source_provider = "user"`,
//!   `source_external_id` = the issue id). Every series cover lookup
//!   ([`user_cover_picks`]) overlays it on the heuristic pick
//!   (`opds::fetch_cover_issues`, the series detail hero, the reading
//!   log); a pick whose issue is gone or removed falls back silently.
//!   `issue_id: null` clears the pick.
//!
//! Both are admin-level writes because they change what every user sees
//! and, on writeback libraries, rewrite the archive — the same gate as the
//! issue PATCH. There is no upload: a cover is always one of the archive's
//! own pages.

use std::collections::HashMap;

use axum::{
    Extension, Json,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::{issue, library, series_cover};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QuerySelect, Set,
    TransactionTrait,
};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use super::extractors::Validated;
use crate::audit::{self, AuditEntry};
use crate::auth::RequireAdmin;
use crate::jobs::post_scan;
use crate::library::thumbnails;
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

/// `series_cover.kind` of a user pick. Distinct from the provider image
/// kinds (`primary` / `logo` / `banner`) so a pick never collides with a
/// downloaded banner on the `(series_id, kind, ordinal)` unique index.
pub(crate) const PICK_KIND: &str = "pick";
const PICK_PROVIDER: &str = "user";

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(set_issue_cover))
        .routes(routes!(set_series_cover))
}

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct IssueCoverPickReq {
    /// 0-based index of the archive page to use as the cover.
    #[garde(range(min = 0))]
    pub page_index: i32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IssueCoverPickResp {
    pub cover_page_index: i32,
    /// Outcome of the sidecar rewrite enqueue: `enqueued`,
    /// `not_writeback`, or `refused: <reason>`.
    pub sidecar: String,
    /// `true` when a cover-thumbnail regeneration job was queued (the
    /// library has thumbnails enabled and no identical job was pending).
    pub thumbnail_enqueued: bool,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SeriesCoverPickReq {
    /// Issue whose cover represents the series. `null` clears the pick
    /// and the heuristic (first issue of the main run) applies again.
    pub issue_id: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesCoverPickResp {
    /// The pick now in force (`None` after a clear).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_id: Option<String>,
    /// Cover URL of the picked issue, for an immediate UI refresh.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
}

/// Choose which archive page is the issue's cover.
#[utoipa::path(
    operation_id = "cover_pick_set_issue_cover",    patch,
    path = "/series/{series_slug}/issues/{issue_slug}/cover",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    request_body = IssueCoverPickReq,
    responses(
        (status = 200, body = IssueCoverPickResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
        (status = 422, description = "page_index outside the archive"),
    )
)]
#[handler]
pub async fn set_issue_cover(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
    Validated(req): Validated<IssueCoverPickReq>,
) -> Response {
    let row = match crate::api::issues::find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let page_count = row.page_count.unwrap_or(0);
    if page_count > 0 && req.page_index >= page_count {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.page_index",
            &format!("page_index must be below the page count ({page_count})"),
        );
    }

    let mut am: issue::ActiveModel = row.clone().into();
    am.cover_page_index = Set(req.page_index);
    am.pages = Set(mark_front_cover(&row.pages, req.page_index));
    am.updated_at = Set(Utc::now().fixed_offset());
    // Column + user pin commit together, like the issue PATCH.
    let write = async {
        let txn = app.db.begin().await?;
        am.update(&txn).await?;
        crate::metadata::writers::write_issue_user_pins(&txn, &row.id, &["cover_page_index"])
            .await?;
        txn.commit().await
    };
    if let Err(e) = write.await {
        tracing::error!(issue_id = %row.id, error = %e, "cover pick: issue update failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    // Writeback libraries get the FrontCover marker in the archive; the
    // scoped rescan then ingests the same page (pinned anyway).
    let sidecar = crate::api::issues::manual_rewrite_after_edit(&app, &row.id, &actor, &ctx)
        .await
        .label();

    // The cover thumbnail is derived from `cover_page_index`: drop the
    // stale file (inline-generated covers are cached on disk too) and
    // queue a fresh one where the library runs the thumbnail worker.
    thumbnails::wipe_issue_cover(&app.cfg().data_path, &row.id);
    crate::api::admin_thumbs::clear_thumb_state(&app, &row).await;
    let thumbnails_enabled = library::Entity::find_by_id(row.library_id)
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .is_some_and(|l| l.thumbnails_enabled);
    let thumbnail_enqueued = thumbnails_enabled
        && post_scan::enqueue_thumb_job(&app, post_scan::ThumbsJob::cover(row.id.clone())).await;

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.issue.cover.set",
            target_type: Some("issue"),
            target_id: Some(row.id.clone()),
            payload: serde_json::json!({
                "page_index": req.page_index,
                "previous_page_index": row.cover_page_index,
                "sidecar": sidecar,
                "thumbnail_enqueued": thumbnail_enqueued,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(IssueCoverPickResp {
        cover_page_index: req.page_index,
        sidecar,
        thumbnail_enqueued,
    })
    .into_response()
}

/// Choose which issue's cover represents the series (or clear the pick).
#[utoipa::path(
    operation_id = "cover_pick_set_series_cover",    patch,
    path = "/series/{series_slug}/cover",
    params(("series_slug" = String, Path,)),
    request_body = SeriesCoverPickReq,
    responses(
        (status = 200, body = SeriesCoverPickResp),
        (status = 403, description = "admin only"),
        (status = 404, description = "series not found"),
        (status = 422, description = "issue_id is not an active issue of this series"),
    )
)]
#[handler]
pub async fn set_series_cover(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath(series_slug): AxPath<String>,
    Json(req): Json<SeriesCoverPickReq>,
) -> Response {
    let series_row = match crate::api::series::find_by_slug(&app.db, &series_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Some(issue_id) = req.issue_id.as_deref() {
        let member = issue::Entity::find_by_id(issue_id)
            .filter(issue::Column::SeriesId.eq(series_row.id))
            .filter(issue::Column::State.eq("active"))
            .filter(issue::Column::RemovedAt.is_null())
            .select_only()
            .column(issue::Column::Id)
            .into_tuple::<String>()
            .one(&app.db)
            .await;
        match member {
            Ok(Some(_)) => {}
            Ok(None) => {
                return error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation.issue_id",
                    "issue_id must be an active issue of this series",
                );
            }
            Err(e) => {
                tracing::error!(series_id = %series_row.id, error = %e, "cover pick: issue lookup failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
    }

    let write = async {
        let txn = app.db.begin().await?;
        // One active pick per series: retire the previous one first (the
        // unique index on `(series_id, kind, ordinal) WHERE is_active`
        // would otherwise reject the insert).
        series_cover::Entity::delete_many()
            .filter(series_cover::Column::SeriesId.eq(series_row.id))
            .filter(series_cover::Column::Kind.eq(PICK_KIND))
            .exec(&txn)
            .await?;
        if let Some(issue_id) = req.issue_id.as_deref() {
            series_cover::ActiveModel {
                id: Set(Uuid::now_v7()),
                series_id: Set(series_row.id),
                kind: Set(PICK_KIND.into()),
                ordinal: Set(0),
                source_provider: Set(Some(PICK_PROVIDER.into())),
                source_external_id: Set(Some(issue_id.to_owned())),
                source_url: Set(None),
                local_path: Set(String::new()),
                width: Set(None),
                height: Set(None),
                fetched_at: Set(Utc::now().fixed_offset()),
                is_active: Set(true),
            }
            .insert(&txn)
            .await?;
        }
        txn.commit().await
    };
    if let Err(e) = write.await {
        tracing::error!(series_id = %series_row.id, error = %e, "cover pick: series_cover write failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: actor.id,
            action: "admin.series.cover.set",
            target_type: Some("series"),
            target_id: Some(series_row.id.to_string()),
            payload: serde_json::json!({ "issue_id": req.issue_id }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(SeriesCoverPickResp {
        cover_url: req
            .issue_id
            .as_deref()
            .map(|id| format!("/issues/{id}/pages/0/thumb")),
        issue_id: req.issue_id,
    })
    .into_response()
}

/// The user's series cover picks for `series_ids`, validated: only picks
/// whose issue still exists, is active, not removed and belongs to that
/// series are returned. Everything else falls back to the heuristic.
pub(crate) async fn user_cover_picks<C: ConnectionTrait>(
    db: &C,
    series_ids: &[Uuid],
) -> HashMap<Uuid, String> {
    if series_ids.is_empty() {
        return HashMap::new();
    }
    let picks = match series_cover::Entity::find()
        .filter(series_cover::Column::SeriesId.is_in(series_ids.to_vec()))
        .filter(series_cover::Column::Kind.eq(PICK_KIND))
        .filter(series_cover::Column::IsActive.eq(true))
        .all(db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "cover pick: series_cover lookup failed; using the heuristic");
            return HashMap::new();
        }
    };
    let wanted: HashMap<String, Uuid> = picks
        .into_iter()
        .filter_map(|p| p.source_external_id.map(|iid| (iid, p.series_id)))
        .collect();
    if wanted.is_empty() {
        return HashMap::new();
    }
    let live = issue::Entity::find()
        .filter(issue::Column::Id.is_in(wanted.keys().cloned().collect::<Vec<_>>()))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .select_only()
        .column(issue::Column::Id)
        .column(issue::Column::SeriesId)
        .into_tuple::<(String, Uuid)>()
        .all(db)
        .await
        .unwrap_or_default();
    live.into_iter()
        .filter(|(iid, sid)| wanted.get(iid) == Some(sid))
        .map(|(iid, sid)| (sid, iid))
        .collect()
}

/// Rewrite the scanner's per-page JSON so exactly the page at `idx`
/// carries `type: "FrontCover"`. The composer forwards this list into
/// ComicInfo's `<Pages>` block, so this is what makes a writeback archive
/// carry the pick. A page with no entry yet gets a minimal one; entries
/// stay ordered by image index. Non-array input yields a single-entry
/// list.
pub(crate) fn mark_front_cover(pages: &serde_json::Value, idx: i32) -> serde_json::Value {
    let mut entries: Vec<serde_json::Map<String, serde_json::Value>> = pages
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_object().cloned()).collect())
        .unwrap_or_default();
    let mut found = false;
    for entry in &mut entries {
        let image = entry.get("image").and_then(serde_json::Value::as_i64);
        if image == Some(i64::from(idx)) {
            entry.insert(
                "type".into(),
                serde_json::Value::String("FrontCover".into()),
            );
            found = true;
        } else if entry.get("type").and_then(serde_json::Value::as_str) == Some("FrontCover") {
            entry.remove("type");
        }
    }
    if !found {
        let mut entry = serde_json::Map::new();
        entry.insert("image".into(), serde_json::Value::from(idx));
        entry.insert(
            "type".into(),
            serde_json::Value::String("FrontCover".into()),
        );
        entries.push(entry);
        entries.sort_by_key(|e| e.get("image").and_then(serde_json::Value::as_i64));
    }
    serde_json::Value::Array(entries.into_iter().map(serde_json::Value::Object).collect())
}

#[cfg(test)]
mod tests {
    use super::mark_front_cover;
    use serde_json::json;

    #[test]
    fn moves_the_front_cover_marker_between_existing_entries() {
        let pages = json!([
            {"image": 0, "type": "FrontCover", "image_width": 10},
            {"image": 1},
            {"image": 2, "type": "Story"}
        ]);
        let out = mark_front_cover(&pages, 1);
        assert_eq!(
            out,
            json!([
                {"image": 0, "image_width": 10},
                {"image": 1, "type": "FrontCover"},
                {"image": 2, "type": "Story"}
            ])
        );
    }

    #[test]
    fn inserts_an_entry_when_the_page_has_none() {
        let out = mark_front_cover(&json!([{"image": 0}, {"image": 5}]), 3);
        assert_eq!(
            out,
            json!([{"image": 0}, {"image": 3, "type": "FrontCover"}, {"image": 5}])
        );
        assert_eq!(
            mark_front_cover(&json!(null), 2),
            json!([{"image": 2, "type": "FrontCover"}])
        );
    }
}
