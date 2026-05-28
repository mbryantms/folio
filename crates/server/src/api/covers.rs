//! Cover-gallery endpoints (metadata-providers-1.0 M5.2).
//!
//! Lists `issue_cover` rows (primary + variants + backs + incentives)
//! for the `<CoverGallery>` UI on the issue page. Series-cover
//! equivalent (`GET /series/{slug}/covers`) ships when the M4 Apply
//! layer starts writing `series_cover` rows (currently it doesn't —
//! M4 only handles primary issue covers).
//!
//! The response carries `source_url` directly (CDN URL the provider
//! returned). The frontend renders `<img src={source_url}>` for
//! variants we never downloaded; primary covers fall back to the
//! existing page-thumb URL since the on-disk artifact may not have
//! been persisted to `issue_cover` for legacy issues.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::{issue, issue_cover, library_user_access};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::Serialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::auth::CurrentUser;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(list_issue_covers))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IssueCoverRow {
    pub id: Uuid,
    pub issue_id: String,
    /// `"primary" | "variant" | "back" | "incentive"`.
    pub kind: String,
    pub ordinal: i32,
    pub source_provider: Option<String>,
    pub source_external_id: Option<String>,
    /// CDN URL the provider returned. Kept for attribution + the
    /// "open original" link; not the primary render source once the
    /// image is stored locally.
    pub source_url: Option<String>,
    /// URL the frontend should render `<img src>` from. Points at the
    /// local byte endpoint (`/issues/{id}/covers/{cover_id}`) when the
    /// cover was downloaded to disk, else falls back to `source_url`
    /// (provider CDN hotlink). `None` only when neither exists.
    pub image_url: Option<String>,
    pub variant_label: Option<String>,
    pub variant_artist_person_id: Option<Uuid>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub fetched_at: String,
    pub is_active: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IssueCoversResp {
    pub issue_id: String,
    pub covers: Vec<IssueCoverRow>,
    /// Fallback URL the frontend can render when no `issue_cover` row
    /// exists (legacy issues whose primary cover is the page-thumb
    /// pipeline). Always points at page 0's cover thumb.
    pub fallback_primary_url: String,
}

#[utoipa::path(
    operation_id = "issue_covers_list",    get,
    path = "/issues/{id}/covers",
    params(("id" = String, Path)),
    responses(
        (status = 200, body = IssueCoversResp),
        (status = 403, description = "library access denied"),
        (status = 404, description = "issue not found"),
    )
)]
#[handler]
pub async fn list_issue_covers(
    State(app): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> Response {
    let Some(issue_row) = issue::Entity::find_by_id(&id)
        .one(&app.db)
        .await
        .ok()
        .flatten()
    else {
        return error(StatusCode::NOT_FOUND, "issue.not_found", "issue not found");
    };
    if !user_can_see_library(&app, &user, issue_row.library_id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }
    let rows = issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(&id))
        .filter(issue_cover::Column::IsActive.eq(true))
        .order_by_asc(issue_cover::Column::Kind)
        .order_by_asc(issue_cover::Column::Ordinal)
        .all(&app.db)
        .await
        .unwrap_or_default();
    let covers: Vec<IssueCoverRow> = rows
        .into_iter()
        .map(|r| {
            // Prefer the locally-stored artifact; fall back to the
            // provider CDN URL for rows we haven't downloaded (legacy /
            // soft-fallback hotlinks).
            let image_url = if !r.local_path.is_empty() {
                Some(format!(
                    "/issues/{}/covers/{}",
                    urlencode(&r.issue_id),
                    r.id
                ))
            } else {
                r.source_url.clone()
            };
            IssueCoverRow {
                id: r.id,
                issue_id: r.issue_id,
                kind: r.kind,
                ordinal: r.ordinal,
                source_provider: r.source_provider,
                source_external_id: r.source_external_id,
                source_url: r.source_url,
                image_url,
                variant_label: r.variant_label,
                variant_artist_person_id: r.variant_artist_person_id,
                width: r.width,
                height: r.height,
                fetched_at: r.fetched_at.to_rfc3339(),
                is_active: r.is_active,
            }
        })
        .collect();
    Json(IssueCoversResp {
        issue_id: id.clone(),
        covers,
        fallback_primary_url: format!("/issues/{}/pages/0/thumb", urlencode(&id)),
    })
    .into_response()
}

async fn user_can_see_library(app: &AppState, user: &CurrentUser, lib_id: Uuid) -> bool {
    if user.role == "admin" {
        return true;
    }
    library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user.id))
        .filter(library_user_access::Column::LibraryId.eq(lib_id))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .is_some()
}

fn urlencode(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}
