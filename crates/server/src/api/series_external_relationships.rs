//! External (not-in-library) relationship targets (WP-7.8).
//!
//! - `GET /series/{slug}/relationships` carries them as `external` (built
//!   by [`external_views`]): provider links (Metron `associated`) and
//!   admin-added links to provider series the library doesn't have.
//! - `POST /series/{slug}/external-relationships` (admin): add one. When the
//!   provider series is already matched to a local series the link is
//!   created as an ordinary relationship pair instead (`201` with
//!   `relationship` set).
//! - `DELETE /series/{slug}/external-relationships/{id}` (admin): remove a
//!   user link; a provider link is dismissed (never re-created by a later
//!   apply).
//!
//! Writes go through [`crate::relationships::external`]; audit actions
//! `admin.series.external_relationship.{create,delete}`.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::{series, series_external_relationship as ext};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, TransactionTrait};
use serde::{Deserialize, Serialize};
use shared::error::{ApiErrorCode, FieldError};
use std::collections::{HashMap, HashSet};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::extractors::Validated;
use super::series_relationships::SeriesRelationshipView;
use super::{respond, respond_with_field_errors};
use crate::auth::{CurrentUser, RequireAdmin};
use crate::library::access;
use crate::middleware::RequestContext;
use crate::record_admin_action;
use crate::relationships::external::{
    self as extmod, ExternalSetBy, ExternalSource, UserLink, UserLinkOutcome,
};
use crate::relationships::{
    PairError, RelationshipGroup, RelationshipKind, RelationshipQualifier, Scope,
};
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create))
        .routes(routes!(delete))
}

// ───────── wire types ─────────

/// A local series a link resolved to (shown instead of "not in your
/// library" until the next suggestion run promotes it).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ExternalLocalSeries {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub year: Option<i32>,
}

/// One relationship to a provider series that isn't in the library, from
/// the requested series' point of view: "this series `kind` *name*".
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesExternalRelationshipView {
    /// Row id — pass to `DELETE /series/{slug}/external-relationships/{id}`.
    pub id: String,
    pub kind: RelationshipKind,
    /// "Continued by", "Collected in", … (tie-in role folded in).
    pub kind_label: String,
    pub group: RelationshipGroup,
    pub qualifier: Option<RelationshipQualifier>,
    pub qualifier_label: Option<String>,
    pub source: ExternalSource,
    /// "Metron", "ComicVine", "GCD".
    pub source_label: String,
    pub provider_series_id: String,
    /// The provider's series name (falls back to the id).
    pub name: String,
    pub year: Option<i32>,
    /// Canonical provider page (attribution link).
    pub url: Option<String>,
    /// `provider` (from provider data) or `user` (added by an admin).
    pub set_by: ExternalSetBy,
    /// 0–1 for provider links; `null` for user links.
    pub confidence: Option<f32>,
    pub created_at: String,
    /// Set when the provider series is already matched to a local series
    /// the caller can see (a user link not yet promoted).
    pub local_series: Option<ExternalLocalSeries>,
}

/// `POST /series/{slug}/external-relationships` body: "this series `kind`
/// the provider series".
#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct CreateExternalRelationshipReq {
    #[garde(skip)]
    pub kind: RelationshipKind,
    /// Continuation qualifier / tie-in role (only for the kinds that take
    /// one).
    #[garde(skip)]
    #[serde(default)]
    pub qualifier: Option<RelationshipQualifier>,
    #[garde(skip)]
    pub source: ExternalSource,
    /// The provider's numeric series id.
    #[garde(skip)]
    pub provider_series_id: String,
    /// Display name (1–300 characters).
    #[garde(skip)]
    pub name: String,
    #[garde(skip)]
    #[serde(default)]
    pub year: Option<i32>,
}

/// `POST` result: exactly one of `external` (the stored external link) or
/// `relationship` (the provider series is already in the library, so an
/// ordinary relationship pair was created instead).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CreateExternalRelationshipResp {
    pub external: Option<SeriesExternalRelationshipView>,
    pub relationship: Option<SeriesRelationshipView>,
}

// ───────── read side ─────────

/// The external links of series `series_id` for `user`'s view of the series page.
///
/// - Dismissed rows are hidden.
/// - A **provider** row whose target resolves to a local series (marked
///   promoted, or resolving now) is hidden: the pair is the suggestion
///   engine's to propose, not a "missing volume".
/// - A **user** row resolving to a local series the caller can see is
///   listed with `local_series` (degrades gracefully until the next
///   suggestion run promotes it into a relationship); one the caller can't
///   see stays a plain external row (it's provider data, nothing leaks).
///
/// The caller has already passed the series' own ACL gate.
pub(crate) async fn external_views(
    app: &AppState,
    user: &CurrentUser,
    series_id: Uuid,
) -> Result<Vec<SeriesExternalRelationshipView>, sea_orm::DbErr> {
    let rows = ext::Entity::find()
        .filter(ext::Column::FromSeriesId.eq(series_id))
        .filter(ext::Column::DismissedAt.is_null())
        .order_by_asc(ext::Column::FirstSetAt)
        .order_by_asc(ext::Column::Id)
        .all(&app.db)
        .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let resolved = extmod::resolve_rows(&app.db, &ids).await?;
    let local_ids: HashSet<Uuid> = rows
        .iter()
        .filter(|r| r.set_by == "user")
        .filter_map(|r| resolved.get(&r.id).copied())
        .collect();
    let visible: HashMap<Uuid, series::Model> = if local_ids.is_empty() {
        HashMap::new()
    } else {
        let acl = access::for_user(app, user).await;
        let is_admin = user.role == "admin";
        series::Entity::find()
            .filter(series::Column::Id.is_in(local_ids.into_iter().collect::<Vec<_>>()))
            .all(&app.db)
            .await?
            .into_iter()
            .filter(|o| {
                acl.series_ok(o.library_id, o.age_rating.as_deref())
                    && (is_admin || o.removed_at.is_none())
            })
            .map(|o| (o.id, o))
            .collect()
    };
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let local = resolved.get(&r.id).copied();
            if r.set_by != "user" && (r.promoted_series_id.is_some() || local.is_some()) {
                return None;
            }
            let local_series = local
                .and_then(|id| visible.get(&id))
                .map(|m| ExternalLocalSeries {
                    id: m.id.to_string(),
                    slug: m.slug.clone(),
                    name: m.name.clone(),
                    year: m.year,
                });
            view_of(r, local_series)
        })
        .collect())
}

fn view_of(
    r: ext::Model,
    local_series: Option<ExternalLocalSeries>,
) -> Option<SeriesExternalRelationshipView> {
    let kind: RelationshipKind = r.kind.parse().ok()?;
    let source = ExternalSource::parse(&r.source)?;
    let qualifier: Option<RelationshipQualifier> =
        r.qualifier.as_deref().and_then(|q| q.parse().ok());
    Some(SeriesExternalRelationshipView {
        id: r.id.to_string(),
        kind,
        kind_label: kind.display_label(qualifier),
        group: kind.group(),
        qualifier,
        qualifier_label: qualifier.map(|q| q.label().to_owned()),
        source,
        source_label: source.label().to_owned(),
        url: r
            .provider_series_url
            .clone()
            .or_else(|| source.series_url(&r.provider_series_id)),
        name: r
            .provider_series_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| format!("{} series {}", source.label(), r.provider_series_id)),
        provider_series_id: r.provider_series_id,
        year: r.provider_year,
        set_by: ExternalSetBy::parse(&r.set_by),
        confidence: r.confidence,
        created_at: r.first_set_at.to_rfc3339(),
        local_series,
    })
}

// ───────── handlers ─────────

#[utoipa::path(
    operation_id = "series_external_relationships_create", post,
    path = "/series/{slug}/external-relationships",
    params(("slug" = String, Path)),
    request_body = CreateExternalRelationshipReq,
    responses(
        (status = 201, body = CreateExternalRelationshipResp, description = "external link stored, or (already in the library) a relationship pair created"),
        (status = 200, body = CreateExternalRelationshipResp, description = "the same link already existed; returned unchanged"),
        (status = 403, description = "admin only"),
        (status = 404, description = "series not found"),
        (status = 409, description = "the target is local and the kind contradicts an existing relationship"),
        (status = 422, description = "invalid provider id / name / year / qualifier"),
    )
)]
#[handler]
pub async fn create(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    Validated(req): Validated<CreateExternalRelationshipReq>,
) -> Response {
    let s = match super::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let pid = req.provider_series_id.trim().to_owned();
    let name = req.name.trim().to_owned();
    let mut issues = Vec::new();
    if pid.is_empty() || pid.len() > 12 || !pid.bytes().all(|b| b.is_ascii_digit()) {
        issues.push(field(
            "provider_series_id",
            "must be the provider's numeric series id",
        ));
    }
    if name.is_empty() {
        issues.push(field("name", "is required"));
    } else if name.chars().count() > 300 || name.chars().any(char::is_control) {
        issues.push(field(
            "name",
            "must be at most 300 characters, without control characters",
        ));
    }
    if req.year.is_some_and(|y| !(1800..=2200).contains(&y)) {
        issues.push(field("year", "must be between 1800 and 2200"));
    }
    let scope = Scope {
        qualifier: req.qualifier,
        ..Scope::default()
    };
    issues.extend(scope.validate(req.kind).into_iter().map(|i| FieldError {
        field: i.field.to_owned(),
        message: i.message,
    }));
    if !issues.is_empty() {
        return validation(issues);
    }

    let link = UserLink {
        kind: req.kind,
        qualifier: req.qualifier,
        source: req.source,
        provider_series_id: pid,
        name,
        year: req.year,
    };
    let outcome = async {
        let txn = app.db.begin().await.map_err(PairError::Db)?;
        let out = extmod::create_user_link(&txn, s.id, &link, actor.id).await?;
        txn.commit().await.map_err(PairError::Db)?;
        Ok::<_, PairError>(out)
    }
    .await;
    let outcome = match outcome {
        Ok(o) => o,
        Err(PairError::SelfEdge) => {
            return validation(vec![field(
                "provider_series_id",
                "that provider series is this series",
            )]);
        }
        Err(e @ (PairError::Conflict { .. } | PairError::Duplicate { .. })) => {
            return respond(StatusCode::CONFLICT, ApiErrorCode::Conflict, e.to_string());
        }
        Err(PairError::Db(e)) => return internal(&e),
        Err(e) => {
            return respond(
                StatusCode::UNPROCESSABLE_ENTITY,
                ApiErrorCode::Validation,
                e.to_string(),
            );
        }
    };

    match outcome {
        UserLinkOutcome::Existing(row) => {
            let Some(view) = view_of(row, None) else {
                return internal_msg();
            };
            (
                StatusCode::OK,
                Json(CreateExternalRelationshipResp {
                    external: Some(view),
                    relationship: None,
                }),
            )
                .into_response()
        }
        UserLinkOutcome::Created(row) => {
            record_admin_action!(
                db = &app.db,
                ctx = &ctx,
                actor = actor.id,
                action = "admin.series.external_relationship.create",
                target = ("series", s.id.to_string()),
                payload = serde_json::json!({
                    "external_relationship_id": row.id.to_string(),
                    "kind": row.kind,
                    "qualifier": row.qualifier,
                    "source": row.source,
                    "provider_series_id": row.provider_series_id,
                    "name": row.provider_series_name,
                    "year": row.provider_year,
                    "promoted_relationship_id": serde_json::Value::Null,
                }),
            );
            let Some(view) = view_of(row, None) else {
                return internal_msg();
            };
            (
                StatusCode::CREATED,
                Json(CreateExternalRelationshipResp {
                    external: Some(view),
                    relationship: None,
                }),
            )
                .into_response()
        }
        UserLinkOutcome::Promoted(pair) => {
            app.similarity.invalidate_all();
            record_admin_action!(
                db = &app.db,
                ctx = &ctx,
                actor = actor.id,
                action = "admin.series.external_relationship.create",
                target = ("series", s.id.to_string()),
                payload = serde_json::json!({
                    "external_relationship_id": serde_json::Value::Null,
                    "kind": link.kind.as_str(),
                    "qualifier": link.qualifier.map(|q| q.as_str()),
                    "source": link.source.as_str(),
                    "provider_series_id": link.provider_series_id,
                    "name": link.name,
                    "year": link.year,
                    "promoted_relationship_id": pair.forward.id.to_string(),
                    "to_series_id": pair.forward.to_series_id.map(|u| u.to_string()),
                    "created": pair.created,
                }),
            );
            let target = match pair.forward.to_series_id {
                Some(t) => series::Entity::find_by_id(t)
                    .one(&app.db)
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };
            let Some(target) = target else {
                return internal_msg();
            };
            let Some(sv) = super::series::hydrate_series(&app, vec![target])
                .await
                .into_iter()
                .next()
            else {
                return internal_msg();
            };
            let Some(view) = super::series_relationships::edge_view_pub(pair.forward, sv) else {
                return internal_msg();
            };
            (
                StatusCode::CREATED,
                Json(CreateExternalRelationshipResp {
                    external: None,
                    relationship: Some(view),
                }),
            )
                .into_response()
        }
    }
}

#[utoipa::path(
    operation_id = "series_external_relationships_delete", delete,
    path = "/series/{slug}/external-relationships/{id}",
    params(("slug" = String, Path), ("id" = String, Path, description = "external relationship row id")),
    responses(
        (status = 204, description = "user link deleted, or provider link dismissed"),
        (status = 400, description = "malformed id"),
        (status = 403, description = "admin only"),
        (status = 404, description = "series / external relationship not found"),
    )
)]
#[handler]
pub async fn delete(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, id)): Path<(String, String)>,
) -> Response {
    let s = match super::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let Ok(row_id) = Uuid::parse_str(&id) else {
        return respond(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::Validation,
            "invalid external relationship id",
        );
    };
    let row = match ext::Entity::find_by_id(row_id)
        .filter(ext::Column::FromSeriesId.eq(s.id))
        .filter(ext::Column::DismissedAt.is_null())
        .one(&app.db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return respond(
                StatusCode::NOT_FOUND,
                ApiErrorCode::NotFound,
                "no such external relationship for this series",
            );
        }
        Err(e) => return internal(&e),
    };
    if let Err(e) = extmod::remove_link(&app.db, &row, actor.id).await {
        return internal(&e);
    }
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.series.external_relationship.delete",
        target = ("series", s.id.to_string()),
        payload = serde_json::json!({
            "external_relationship_id": row.id.to_string(),
            "kind": row.kind,
            "source": row.source,
            "provider_series_id": row.provider_series_id,
            "name": row.provider_series_name,
            "set_by": row.set_by,
            // A provider link is dismissed (kept as rejection memory).
            "dismissed": row.set_by != "user",
        }),
    );
    StatusCode::NO_CONTENT.into_response()
}

// ───────── helpers ─────────

fn field(name: &str, message: &str) -> FieldError {
    FieldError {
        field: name.to_owned(),
        message: message.to_owned(),
    }
}

fn validation(fields: Vec<FieldError>) -> Response {
    let message = fields
        .iter()
        .map(|f| format!("{}: {}", f.field, f.message))
        .collect::<Vec<_>>()
        .join("; ");
    respond_with_field_errors(
        StatusCode::UNPROCESSABLE_ENTITY,
        ApiErrorCode::Validation,
        message,
        fields,
    )
}

fn internal(e: &sea_orm::DbErr) -> Response {
    tracing::error!(error = %e, "external relationship query failed");
    internal_msg()
}

fn internal_msg() -> Response {
    respond(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorCode::Internal,
        "internal",
    )
}
