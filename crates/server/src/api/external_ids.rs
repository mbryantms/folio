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
//!
//! The ids ride in both sidecars (ComicInfo's typed id elements and
//! `<Notes>` tokens, MetronInfo's `<ID source=…>`), so in a writeback
//! library every add / unlink enqueues the same rewrite a `PATCH` does
//! (`metadata::manual_writeback`, audit 2026-11 N-M5); a series-level id
//! fans out over the series like a series identity edit.
//!
//! The lists take `CurrentUser` + library visibility, so every reader
//! sees the ids. The add / unlink writes take `RequireAdmin` (audit
//! 2026-11 E13), the same gate as the series and issue `PATCH`: an id
//! steers matching, coverage and, in writeback libraries, the composed
//! sidecars of a shared series, so it is not a per-reader setting.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::{external_id, issue, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use super::extractors::Validated;
use crate::audit::{self, AuditEntry};
use crate::auth::{CurrentUser, RequireAdmin};
use crate::metadata::identifier::{Identifier, Source};
use crate::metadata::writers::{self, SetBy};
use crate::middleware::RequestContext;
use crate::middleware::rate_limit;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    // Per-user bucket (E12, audit N-A5): the add / unlink writes share one
    // `METADATA_WRITE` instance in their own sub-router, so the lists stay
    // unbucketed. Each edit is one click in the external-ids card.
    let writes = OpenApiRouter::new()
        .routes(routes!(add_series))
        .routes(routes!(delete_series))
        .routes(routes!(add_issue))
        .routes(routes!(delete_issue))
        .route_layer(rate_limit::METADATA_WRITE.build_per_user());
    OpenApiRouter::new()
        .routes(routes!(list_series))
        .routes(routes!(list_issue))
        .merge(writes)
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

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct AddExternalIdReq {
    /// `"comicvine" | "metron" | "gcd" | "marvel" | "locg" | "mal" |
    /// "anilist" | "mangaupdates" | "isbn" | "upc" | "asin" | "doi"`.
    /// Aliases accepted (`"cv"` → ComicVine, etc.).
    #[garde(custom(known_source))]
    pub source: String,
    /// Non-empty after trimming.
    #[garde(custom(external_id_present))]
    pub external_id: String,
    /// Optional override; defaults to the canonical URL template for
    /// `(source, entity_type)` when one exists.
    #[garde(skip)]
    pub external_url: Option<String>,
}

/// Same parser the handler uses, so the 422 names `source` in
/// `error.details` and the handler's later `parse()` cannot disagree.
fn known_source(value: &str, _: &()) -> garde::Result {
    if Source::from_str(value).is_err() {
        return Err(garde::Error::new("unknown source"));
    }
    Ok(())
}

fn external_id_present(value: &str, _: &()) -> garde::Result {
    if value.trim().is_empty() {
        return Err(garde::Error::new("external_id required"));
    }
    Ok(())
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
    if !crate::library::access::series_visible(&app, &user, &s).await {
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
        (status = 400, description = "malformed JSON body"),
        (status = 422, description = "unknown source or empty external_id (field-level `details`)"),
        (status = 403, description = "admin only"),
        (status = 404, description = "series not found"),
        (status = 429, description = "metadata write rate limit"),
    )
)]
#[handler]
pub async fn add_series(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    Validated(req): Validated<AddExternalIdReq>,
) -> Response {
    let s = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    match upsert_user_identifier(&app, &user, &ctx, "series", &s.id.to_string(), &req).await {
        Ok(done) => {
            audit::record(&app.db, done.audit).await;
            done.response
        }
        Err(resp) => resp,
    }
}

#[utoipa::path(
    operation_id = "external_ids_delete_series",    delete,
    path = "/series/{slug}/external-ids/{source}",
    params(("slug" = String, Path), ("source" = String, Path)),
    responses(
        (status = 204, description = "unlinked"),
        (status = 400, description = "invalid source"),
        (status = 403, description = "admin only"),
        (status = 404, description = "series / link not found"),
        (status = 429, description = "metadata write rate limit"),
    )
)]
#[handler]
pub async fn delete_series(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, source)): Path<(String, String)>,
) -> Response {
    let s = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    match delete_identifier(&app, &user, &ctx, "series", &s.id.to_string(), &source).await {
        Ok(done) => {
            audit::record(&app.db, done.audit).await;
            done.response
        }
        Err(resp) => resp,
    }
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
    let Some((s, issue_id)) = find_series_issue(&app, &slug, &issue_slug).await else {
        return error(StatusCode::NOT_FOUND, "issue.not_found", "issue not found");
    };
    if !crate::library::access::series_visible(&app, &user, &s).await {
        return error(
            StatusCode::FORBIDDEN,
            "auth.forbidden",
            "library access denied",
        );
    }
    let rows = fetch_rows(&app, "issue", &issue_id).await;
    Json(ExternalIdsListResp {
        entity_type: "issue".into(),
        entity_id: issue_id,
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
        (status = 400, description = "malformed JSON body"),
        (status = 422, description = "unknown source or empty external_id (field-level `details`)"),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
        (status = 429, description = "metadata write rate limit"),
    )
)]
#[handler]
pub async fn add_issue(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, issue_slug)): Path<(String, String)>,
    Validated(req): Validated<AddExternalIdReq>,
) -> Response {
    let Some((_, issue_id)) = find_series_issue(&app, &slug, &issue_slug).await else {
        return error(StatusCode::NOT_FOUND, "issue.not_found", "issue not found");
    };
    match upsert_user_identifier(&app, &user, &ctx, "issue", &issue_id, &req).await {
        Ok(done) => {
            audit::record(&app.db, done.audit).await;
            done.response
        }
        Err(resp) => resp,
    }
}

#[utoipa::path(
    operation_id = "external_ids_delete_issue",    delete,
    path = "/series/{slug}/issues/{issue_slug}/external-ids/{source}",
    params(("slug" = String, Path), ("issue_slug" = String, Path), ("source" = String, Path)),
    responses(
        (status = 204, description = "unlinked"),
        (status = 400, description = "invalid source"),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue / link not found"),
        (status = 429, description = "metadata write rate limit"),
    )
)]
#[handler]
pub async fn delete_issue(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, issue_slug, source)): Path<(String, String, String)>,
) -> Response {
    let Some((_, issue_id)) = find_series_issue(&app, &slug, &issue_slug).await else {
        return error(StatusCode::NOT_FOUND, "issue.not_found", "issue not found");
    };
    match delete_identifier(&app, &user, &ctx, "issue", &issue_id, &source).await {
        Ok(done) => {
            audit::record(&app.db, done.audit).await;
            done.response
        }
        Err(resp) => resp,
    }
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

/// A committed id edit: the audit row the handler records (in its own
/// body, so `audit-check` sees every admin write audited) and the
/// response it returns.
struct Edited {
    audit: AuditEntry<'static>,
    response: Response,
}

fn edit_audit(
    user: &CurrentUser,
    ctx: &RequestContext,
    entity_type: &str,
    entity_id: &str,
    actions: (&'static str, &'static str),
    payload: serde_json::Value,
) -> AuditEntry<'static> {
    let (series_action, issue_action) = actions;
    let (action, target_type) = match entity_type {
        "series" => (series_action, "series"),
        _ => (issue_action, "issue"),
    };
    AuditEntry {
        actor_id: user.id,
        action,
        target_type: Some(target_type),
        target_id: Some(entity_id.to_owned()),
        payload,
        ip: ctx.ip_string(),
        user_agent: ctx.user_agent.clone(),
    }
}

async fn upsert_user_identifier(
    app: &AppState,
    user: &CurrentUser,
    ctx: &RequestContext,
    entity_type: &str,
    entity_id: &str,
    req: &AddExternalIdReq,
) -> Result<Edited, Response> {
    // `Validated<AddExternalIdReq>` already ran `known_source`, so this
    // parse only recovers the typed value; a failure is a server bug.
    let Ok(source) = req.source.parse::<Source>() else {
        return Err(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "validated source failed to parse",
        ));
    };
    let id_value = req.external_id.trim();
    let url = req
        .external_url
        .clone()
        .or_else(|| crate::metadata::identifier::canonical_url(source, entity_type, id_value));
    let identifier = Identifier {
        source,
        id: id_value.to_owned(),
        url,
    };
    match writers::set_external_id_promoting(
        &app.db,
        entity_type,
        entity_id,
        &identifier,
        SetBy::User,
    )
    .await
    {
        Ok((writers::SetExternalIdOutcome::SkippedConflict { .. }, _)) => {
            return Err(error(
                StatusCode::CONFLICT,
                "metadata.external_id_taken",
                "that identifier is already assigned to another item — remove it there first",
            ));
        }
        Ok((_, promoted_pairs)) => {
            // WP-8.2: an external link promoted to a series relationship
            // is a similar-series signal.
            if promoted_pairs > 0 {
                app.similarity.invalidate_all();
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "external_id set failed");
            return Err(error(
                StatusCode::BAD_GATEWAY,
                "internal",
                "external_id write failed",
            ));
        }
    }
    let sidecar_rewrite = manual_rewrite(app, user, ctx, entity_type, entity_id).await;
    let audit = edit_audit(
        user,
        ctx,
        entity_type,
        entity_id,
        (
            "admin.series.external_id_set",
            "admin.issue.external_id_set",
        ),
        serde_json::json!({
            "source": source.as_str(),
            "external_id": identifier.id,
            "sidecar_rewrite": sidecar_rewrite,
        }),
    );
    // Read back so we return the row exactly as stored. The write is
    // committed either way, so a failed readback is still audited.
    let rows = fetch_rows(app, entity_type, entity_id).await;
    let response = match rows.into_iter().find(|r| r.source == source.as_str()) {
        Some(row) => (StatusCode::CREATED, Json(row)).into_response(),
        None => error(
            StatusCode::BAD_GATEWAY,
            "internal",
            "external_id write succeeded but readback failed",
        ),
    };
    Ok(Edited { audit, response })
}

async fn delete_identifier(
    app: &AppState,
    user: &CurrentUser,
    ctx: &RequestContext,
    entity_type: &str,
    entity_id: &str,
    source_str: &str,
) -> Result<Edited, Response> {
    let Ok(source) = source_str.parse::<Source>() else {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "metadata.invalid_source",
            "unknown source",
        ));
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
        return Err(error(
            StatusCode::NOT_FOUND,
            "metadata.link_not_found",
            "no link to remove",
        ));
    }
    if let Err(e) = writers::delete_external_id(&app.db, entity_type, entity_id, source).await {
        tracing::warn!(error = %e, "delete_external_id failed");
        return Err(error(StatusCode::BAD_GATEWAY, "internal", "delete failed"));
    }
    let sidecar_rewrite = manual_rewrite(app, user, ctx, entity_type, entity_id).await;
    let audit = edit_audit(
        user,
        ctx,
        entity_type,
        entity_id,
        (
            "admin.series.external_id_unlink",
            "admin.issue.external_id_unlink",
        ),
        serde_json::json!({
            "source": source.as_str(),
            "sidecar_rewrite": sidecar_rewrite,
        }),
    );
    Ok(Edited {
        audit,
        response: StatusCode::NO_CONTENT.into_response(),
    })
}

/// WP-2.10 for identifier edits (audit 2026-11 N-M5): push the committed
/// id change into the archive's sidecars when the library is in writeback
/// mode. An issue id queues one rewrite with its own scoped rescan; a
/// series id fans out one rewrite per active issue plus a single
/// series-scoped rescan, exactly like the series PATCH. Never fails the
/// request — the database already holds the edit; the outcome lands on
/// the audit row (`enqueued`, `not_writeback`, `refused: …`). Libraries
/// without writeback get `not_writeback` and no file is touched.
async fn manual_rewrite(
    app: &AppState,
    user: &CurrentUser,
    ctx: &RequestContext,
    entity_type: &str,
    entity_id: &str,
) -> String {
    use crate::metadata::manual_writeback::{
        Actor, IssueEnqueue, enqueue_issue_rewrite, enqueue_series_rewrite,
    };
    let actor = Actor {
        id: Some(user.id),
        ip: ctx.ip_string(),
        user_agent: ctx.user_agent.clone(),
    };
    if entity_type == "series" {
        let Ok(series_id) = Uuid::parse_str(entity_id) else {
            return "gone".into();
        };
        return match enqueue_series_rewrite(app, series_id, &actor).await {
            Ok(outcome) => outcome.label(),
            Err(e) => {
                tracing::error!(
                    series_id = entity_id,
                    error = %e,
                    "external id edit: sidecar rewrite enqueue failed"
                );
                format!("enqueue failed: {e}")
            }
        };
    }
    match enqueue_issue_rewrite(app, entity_id, &actor, false).await {
        Ok(outcome) => {
            if let IssueEnqueue::Refused(reason) = &outcome {
                tracing::warn!(
                    issue_id = entity_id,
                    reason,
                    "external id edit: sidecar rewrite refused"
                );
            }
            outcome.label()
        }
        Err(e) => {
            tracing::error!(
                issue_id = entity_id,
                error = %e,
                "external id edit: sidecar rewrite enqueue failed"
            );
            format!("enqueue failed: {e}")
        }
    }
}

/// The series and the issue's id. Projects the id only: the full
/// `issue::Model` carries the large `comic_info_raw`.
async fn find_series_issue(
    app: &AppState,
    series_slug: &str,
    issue_slug: &str,
) -> Option<(series::Model, String)> {
    let s = crate::api::series::find_by_slug(&app.db, series_slug)
        .await
        .ok()?;
    let issue_id = issue::Entity::find()
        .select_only()
        .column(issue::Column::Id)
        .filter(issue::Column::SeriesId.eq(s.id))
        .filter(issue::Column::Slug.eq(issue_slug))
        .into_tuple::<String>()
        .one(&app.db)
        .await
        .ok()
        .flatten()?;
    Some((s, issue_id))
}
