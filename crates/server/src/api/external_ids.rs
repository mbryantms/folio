//! External-ID CRUD for series + issue (metadata-providers-1.0 M5).
//!
//! Surface the `external_ids` table to the `<ExternalIdsCard>` UI so
//! the user can list / add / edit / unlink the per-source identifiers
//! that link a Folio entity back to ComicVine / Metron / GCD / etc.
//!
//! User-set rows are sacred — the M4 Apply jobs check
//! `field_provenance` via `set_external_id`'s built-in user-precedence
//! guard, so an explicit unlink + re-add via this API correctly marks
//! the row `set_by='user'` and protects it from non-user overwrites.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::{external_id, issue, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::error;
use crate::audit::{self, AuditEntry};
use crate::auth::CurrentUser;
use crate::metadata::identifier::{Identifier, Source};
use crate::metadata::writers::{self, SetBy};
use crate::middleware::RequestContext;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_series))
        .routes(routes!(add_series))
        .routes(routes!(delete_series))
        .routes(routes!(list_issue))
        .routes(routes!(add_issue))
        .routes(routes!(delete_issue))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ExternalIdRow {
    pub source: String,
    pub source_label: String,
    pub external_id: String,
    pub external_url: Option<String>,
    pub set_by: String,
    pub first_set_at: String,
    pub last_synced_at: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ExternalIdsListResp {
    pub entity_type: String,
    pub entity_id: String,
    pub rows: Vec<ExternalIdRow>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AddExternalIdReq {
    /// `"comicvine" | "metron" | "gcd" | "marvel" | "locg" | "mal" |
    /// "anilist" | "mangaupdates" | "isbn" | "upc" | "asin" | "doi"`.
    /// Aliases accepted (`"cv"` → ComicVine, etc.).
    pub source: String,
    pub external_id: String,
    /// Optional override; defaults to the canonical URL template for
    /// `(source, entity_type)` when one exists.
    pub external_url: Option<String>,
}

// ───────── series ─────────

#[utoipa::path(
    operation_id = "external_ids_list_series",    get,
    path = "/series/{slug}/external-ids",
    params(("slug" = String, Path)),
    responses(
        (status = 200, body = ExternalIdsListResp),
        (status = 403, description = "library access denied"),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn list_series(
    State(app): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
) -> Response {
    let s = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if !user_can_see_library(&app, &user, s.library_id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }
    let rows = fetch_rows(&app, "series", &s.id.to_string()).await;
    Json(ExternalIdsListResp {
        entity_type: "series".into(),
        entity_id: s.id.to_string(),
        rows,
    })
    .into_response()
}

#[utoipa::path(
    operation_id = "external_ids_add_series",    post,
    path = "/series/{slug}/external-ids",
    params(("slug" = String, Path)),
    request_body = AddExternalIdReq,
    responses(
        (status = 201, body = ExternalIdRow),
        (status = 400, description = "invalid source"),
        (status = 403, description = "library access denied"),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn add_series(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    Json(req): Json<AddExternalIdReq>,
) -> Response {
    let s = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if !user_can_see_library(&app, &user, s.library_id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }
    upsert_user_identifier(&app, &user, &ctx, "series", &s.id.to_string(), &req).await
}

#[utoipa::path(
    operation_id = "external_ids_delete_series",    delete,
    path = "/series/{slug}/external-ids/{source}",
    params(("slug" = String, Path), ("source" = String, Path)),
    responses(
        (status = 204, description = "unlinked"),
        (status = 400, description = "invalid source"),
        (status = 403, description = "library access denied"),
        (status = 404, description = "series / link not found"),
    )
)]
#[handler]
pub async fn delete_series(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, source)): Path<(String, String)>,
) -> Response {
    let s = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if !user_can_see_library(&app, &user, s.library_id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }
    delete_identifier(&app, &user, &ctx, "series", &s.id.to_string(), &source).await
}

// ───────── issue ─────────

#[utoipa::path(
    operation_id = "external_ids_list_issue",    get,
    path = "/series/{slug}/issues/{issue_slug}/external-ids",
    params(("slug" = String, Path), ("issue_slug" = String, Path)),
    responses(
        (status = 200, body = ExternalIdsListResp),
        (status = 403, description = "library access denied"),
        (status = 404, description = "issue not found"),
    )
)]
#[handler]
pub async fn list_issue(
    State(app): State<AppState>,
    user: CurrentUser,
    Path((slug, issue_slug)): Path<(String, String)>,
) -> Response {
    let Some((s, i)) = find_series_issue(&app, &slug, &issue_slug).await else {
        return error(StatusCode::NOT_FOUND, "issue.not_found", "issue not found");
    };
    if !user_can_see_library(&app, &user, s.library_id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }
    let rows = fetch_rows(&app, "issue", &i.id).await;
    Json(ExternalIdsListResp {
        entity_type: "issue".into(),
        entity_id: i.id.clone(),
        rows,
    })
    .into_response()
}

#[utoipa::path(
    operation_id = "external_ids_add_issue",    post,
    path = "/series/{slug}/issues/{issue_slug}/external-ids",
    params(("slug" = String, Path), ("issue_slug" = String, Path)),
    request_body = AddExternalIdReq,
    responses(
        (status = 201, body = ExternalIdRow),
        (status = 400, description = "invalid source"),
        (status = 403, description = "library access denied"),
        (status = 404, description = "issue not found"),
    )
)]
#[handler]
pub async fn add_issue(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, issue_slug)): Path<(String, String)>,
    Json(req): Json<AddExternalIdReq>,
) -> Response {
    let Some((s, i)) = find_series_issue(&app, &slug, &issue_slug).await else {
        return error(StatusCode::NOT_FOUND, "issue.not_found", "issue not found");
    };
    if !user_can_see_library(&app, &user, s.library_id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }
    upsert_user_identifier(&app, &user, &ctx, "issue", &i.id, &req).await
}

#[utoipa::path(
    operation_id = "external_ids_delete_issue",    delete,
    path = "/series/{slug}/issues/{issue_slug}/external-ids/{source}",
    params(("slug" = String, Path), ("issue_slug" = String, Path), ("source" = String, Path)),
    responses(
        (status = 204, description = "unlinked"),
        (status = 400, description = "invalid source"),
        (status = 403, description = "library access denied"),
        (status = 404, description = "issue / link not found"),
    )
)]
#[handler]
pub async fn delete_issue(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, issue_slug, source)): Path<(String, String, String)>,
) -> Response {
    let Some((s, i)) = find_series_issue(&app, &slug, &issue_slug).await else {
        return error(StatusCode::NOT_FOUND, "issue.not_found", "issue not found");
    };
    if !user_can_see_library(&app, &user, s.library_id).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }
    delete_identifier(&app, &user, &ctx, "issue", &i.id, &source).await
}

// ───────── shared ─────────

pub(crate) async fn fetch_rows(
    app: &AppState,
    entity_type: &str,
    entity_id: &str,
) -> Vec<ExternalIdRow> {
    let rows = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq(entity_type))
        .filter(external_id::Column::EntityId.eq(entity_id))
        .all(&app.db)
        .await
        .unwrap_or_default();
    rows.into_iter()
        .filter_map(|r| {
            let source = Source::from_str(&r.source).ok()?;
            Some(ExternalIdRow {
                source: source.as_str().to_owned(),
                source_label: source.label().to_owned(),
                external_id: r.external_id,
                external_url: r.external_url,
                set_by: r.set_by,
                first_set_at: r.first_set_at.to_rfc3339(),
                last_synced_at: r.last_synced_at.to_rfc3339(),
            })
        })
        .collect()
}

async fn upsert_user_identifier(
    app: &AppState,
    user: &CurrentUser,
    ctx: &RequestContext,
    entity_type: &str,
    entity_id: &str,
    req: &AddExternalIdReq,
) -> Response {
    let Ok(source) = req.source.parse::<Source>() else {
        return error(
            StatusCode::BAD_REQUEST,
            "metadata.invalid_source",
            "unknown source",
        );
    };
    let id_value = req.external_id.trim();
    if id_value.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "metadata.invalid_external_id",
            "external_id required",
        );
    }
    let url = req
        .external_url
        .clone()
        .or_else(|| crate::metadata::identifier::canonical_url(source, entity_type, id_value));
    let identifier = Identifier {
        source,
        id: id_value.to_owned(),
        url,
    };
    match writers::set_external_id(&app.db, entity_type, entity_id, &identifier, SetBy::User).await
    {
        Ok(writers::SetExternalIdOutcome::SkippedConflict { .. }) => {
            return error(
                StatusCode::CONFLICT,
                "metadata.external_id_taken",
                "that identifier is already assigned to another item — remove it there first",
            );
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(error = %e, "external_id set failed");
            return error(
                StatusCode::BAD_GATEWAY,
                "internal",
                "external_id write failed",
            );
        }
    }
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: match entity_type {
                "series" => "admin.series.external_id_set",
                _ => "admin.issue.external_id_set",
            },
            target_type: Some(match entity_type {
                "series" => "series",
                _ => "issue",
            }),
            target_id: Some(entity_id.to_owned()),
            payload: serde_json::json!({
                "source": source.as_str(),
                "external_id": identifier.id,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    // Read back so we return the row exactly as stored.
    let rows = fetch_rows(app, entity_type, entity_id).await;
    let Some(row) = rows.into_iter().find(|r| r.source == source.as_str()) else {
        return error(
            StatusCode::BAD_GATEWAY,
            "internal",
            "external_id write succeeded but readback failed",
        );
    };
    (StatusCode::CREATED, Json(row)).into_response()
}

async fn delete_identifier(
    app: &AppState,
    user: &CurrentUser,
    ctx: &RequestContext,
    entity_type: &str,
    entity_id: &str,
    source_str: &str,
) -> Response {
    let Ok(source) = source_str.parse::<Source>() else {
        return error(
            StatusCode::BAD_REQUEST,
            "metadata.invalid_source",
            "unknown source",
        );
    };
    let existed = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq(entity_type))
        .filter(external_id::Column::EntityId.eq(entity_id))
        .filter(external_id::Column::Source.eq(source.as_str()))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .is_some();
    if !existed {
        return error(
            StatusCode::NOT_FOUND,
            "metadata.link_not_found",
            "no link to remove",
        );
    }
    if let Err(e) = writers::delete_external_id(&app.db, entity_type, entity_id, source).await {
        tracing::warn!(error = %e, "delete_external_id failed");
        return error(StatusCode::BAD_GATEWAY, "internal", "delete failed");
    }
    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: match entity_type {
                "series" => "admin.series.external_id_unlink",
                _ => "admin.issue.external_id_unlink",
            },
            target_type: Some(match entity_type {
                "series" => "series",
                _ => "issue",
            }),
            target_id: Some(entity_id.to_owned()),
            payload: serde_json::json!({ "source": source.as_str() }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    StatusCode::NO_CONTENT.into_response()
}

async fn user_can_see_library(app: &AppState, user: &CurrentUser, lib_id: uuid::Uuid) -> bool {
    if user.role == "admin" {
        return true;
    }
    use entity::library_user_access;
    library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user.id))
        .filter(library_user_access::Column::LibraryId.eq(lib_id))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .is_some()
}

async fn find_series_issue(
    app: &AppState,
    series_slug: &str,
    issue_slug: &str,
) -> Option<(series::Model, issue::Model)> {
    let s = crate::api::series::find_by_slug(&app.db, series_slug)
        .await
        .ok()?;
    let i = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(s.id))
        .filter(issue::Column::Slug.eq(issue_slug))
        .one(&app.db)
        .await
        .ok()
        .flatten()?;
    Some((s, i))
}
