//! `/series/{slug}/relationships` — typed series → series edges (WP-7.1).
//!
//! - `GET` (any signed-in user who can see the series): the direct
//!   relationships plus the sequel/prequel reading-order `chain`. Both are
//!   ACL-filtered: a related series is listed only when the caller can see
//!   it too (library grant + age-rating cap; removed series are hidden from
//!   non-admins), and a chain branch stops at the first series the caller
//!   can't see so nothing beyond a hidden link leaks.
//! - `POST` (admin): create an edge + its inverse in one transaction.
//!   Idempotent — an existing edge answers `200` with the existing row; a
//!   new one answers `201`.
//! - `DELETE /series/{slug}/relationships/{id}` (admin): remove an edge
//!   (and its inverse) by the row id the `GET` returned.
//!
//! All writes go through [`crate::relationships`]; the audit actions are
//! `admin.series.relationship.create` / `admin.series.relationship.delete`.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::{series, series_relationship as rel};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, TransactionTrait};
use serde::{Deserialize, Serialize};
use shared::error::ApiErrorCode;
use std::collections::{HashMap, HashSet};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::extractors::Validated;
use super::respond;
use super::series::SeriesView;
use crate::auth::{CurrentUser, RequireAdmin};
use crate::library::access;
use crate::middleware::RequestContext;
use crate::record_admin_action;
use crate::relationships::{self, PairError, RelationshipKind, RelationshipSource};
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(delete))
}

/// One direct relationship, from the requested series' point of view:
/// "this series `kind` `series`".
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesRelationshipView {
    /// Row id — pass to `DELETE /series/{slug}/relationships/{id}`.
    pub id: String,
    pub kind: RelationshipKind,
    /// Display label for `kind` ("Sequel of", "Collected in", …).
    pub kind_label: String,
    pub source: RelationshipSource,
    /// Suggestion confidence (0–1); `null` for manual edges.
    pub confidence: Option<f32>,
    pub created_at: String,
    /// The other series, hydrated like a library-grid card (cover, issue
    /// count, …).
    pub series: SeriesView,
}

/// One step of the sequel/prequel reading-order chain.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesChainEntry {
    /// Signed reading-order offset: negative = read before this series,
    /// `0` = this series, positive = read after. Entries sharing a
    /// position are alternative branches.
    pub position: i32,
    pub series: SeriesView,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesRelationshipsResp {
    pub series_id: String,
    /// Direct relationships, oldest first. Bounded by curation (each edge
    /// is admin-created or an accepted suggestion), so not paginated.
    pub relationships: Vec<SeriesRelationshipView>,
    /// Sequel/prequel chain through this series in reading order (depth
    /// ≤ 6 each way). Empty when the series has no sequel/prequel edges;
    /// otherwise includes this series at position 0.
    pub chain: Vec<SeriesChainEntry>,
}

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct CreateSeriesRelationshipReq {
    /// The other series — slug or UUID.
    #[garde(length(min = 1, max = 400), custom(non_blank))]
    pub target: String,
    /// Read as "this series `kind` target" (e.g. `sequel_of`).
    #[garde(skip)]
    pub kind: RelationshipKind,
}

fn non_blank(value: &str, _: &()) -> garde::Result {
    if value.trim().is_empty() {
        return Err(garde::Error::new("target is required"));
    }
    Ok(())
}

#[utoipa::path(
    operation_id = "series_relationships_list", get,
    path = "/series/{slug}/relationships",
    params(("slug" = String, Path)),
    responses(
        (status = 200, body = SeriesRelationshipsResp),
        (status = 404, description = "series not found or not visible"),
    )
)]
#[handler]
pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
) -> Response {
    let s = match super::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    // Same gate as `GET /series/{slug}`: invisible ⇒ 404 (don't confirm
    // the series exists).
    if !access::series_visible(&app, &user, &s).await {
        return respond(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NotFound,
            "series not found",
        );
    }

    let edges = match relationships::direct(&app.db, s.id).await {
        Ok(e) => e,
        Err(e) => return internal(&e),
    };
    let chain = match relationships::chain(&app.db, s.id).await {
        Ok(c) => c,
        Err(e) => return internal(&e),
    };

    // One batch load + ACL pass for every series either list mentions.
    let mut ids: HashSet<Uuid> = edges.iter().map(|e| e.to_series_id).collect();
    ids.extend(chain.iter().map(|n| n.series_id));
    ids.remove(&s.id);
    let mut visible: Vec<series::Model> = match visible_series(&app, &user, ids).await {
        Ok(v) => v.into_values().collect(),
        Err(e) => return internal(&e),
    };
    visible.push(s.clone());
    let views: HashMap<String, SeriesView> = super::series::hydrate_series(&app, visible)
        .await
        .into_iter()
        .map(|v| (v.id.clone(), v))
        .collect();

    let relationships = edges
        .into_iter()
        .filter_map(|e| {
            let view = views.get(&e.to_series_id.to_string())?.clone();
            edge_view(e, view)
        })
        .collect();

    // Prune the chain: a node survives only when it is visible AND the
    // node it was reached through survived, so a hidden link hides
    // everything beyond it. Nodes come ordered by |position| within each
    // side, so parents are always decided before their children.
    let mut kept: HashSet<Uuid> = HashSet::from([s.id]);
    let mut ordered = chain.clone();
    ordered.sort_by_key(|n| n.position.abs());
    for n in &ordered {
        if n.position == 0 {
            continue;
        }
        let parent_ok = n.parent_id.is_some_and(|p| kept.contains(&p));
        if parent_ok && views.contains_key(&n.series_id.to_string()) {
            kept.insert(n.series_id);
        }
    }
    let chain_view: Vec<SeriesChainEntry> = if kept.len() <= 1 {
        Vec::new()
    } else {
        chain
            .into_iter()
            .filter(|n| kept.contains(&n.series_id))
            .filter_map(|n| {
                Some(SeriesChainEntry {
                    position: n.position,
                    series: views.get(&n.series_id.to_string())?.clone(),
                })
            })
            .collect()
    };

    Json(SeriesRelationshipsResp {
        series_id: s.id.to_string(),
        relationships,
        chain: chain_view,
    })
    .into_response()
}

#[utoipa::path(
    operation_id = "series_relationships_create", post,
    path = "/series/{slug}/relationships",
    params(("slug" = String, Path)),
    request_body = CreateSeriesRelationshipReq,
    responses(
        (status = 201, body = SeriesRelationshipView, description = "edge (and its inverse) created"),
        (status = 200, body = SeriesRelationshipView, description = "edge already existed; returned unchanged"),
        (status = 403, description = "admin only"),
        (status = 404, description = "series or target not found"),
        (status = 409, description = "contradicts an existing relationship"),
        (status = 422, description = "self-relationship / invalid body"),
    )
)]
#[handler]
pub async fn create(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path(slug): Path<String>,
    Validated(req): Validated<CreateSeriesRelationshipReq>,
) -> Response {
    let s = match super::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let target = match super::series::find_by_slug(&app.db, req.target.trim()).await {
        Ok(t) => t,
        Err(_) => {
            return respond(
                StatusCode::NOT_FOUND,
                ApiErrorCode::NotFound,
                "target series not found",
            );
        }
    };
    if target.id == s.id {
        return respond(
            StatusCode::UNPROCESSABLE_ENTITY,
            ApiErrorCode::Validation,
            "a series cannot be related to itself",
        );
    }

    let outcome = async {
        let txn = app.db.begin().await.map_err(PairError::Db)?;
        let out = relationships::create_pair(
            &txn,
            s.id,
            target.id,
            req.kind,
            RelationshipSource::Manual,
            None,
            Some(actor.id),
        )
        .await?;
        txn.commit().await.map_err(PairError::Db)?;
        Ok::<_, PairError>(out)
    }
    .await;
    let outcome = match outcome {
        Ok(o) => o,
        Err(PairError::SelfEdge) => {
            return respond(
                StatusCode::UNPROCESSABLE_ENTITY,
                ApiErrorCode::Validation,
                "a series cannot be related to itself",
            );
        }
        Err(e @ PairError::Conflict { .. }) => {
            return respond(StatusCode::CONFLICT, ApiErrorCode::Conflict, e.to_string());
        }
        Err(e @ PairError::InvalidConfidence) => {
            return respond(
                StatusCode::UNPROCESSABLE_ENTITY,
                ApiErrorCode::Validation,
                e.to_string(),
            );
        }
        Err(PairError::Db(e)) => return internal(&e),
    };

    if outcome.created {
        record_admin_action!(
            db = &app.db,
            ctx = &ctx,
            actor = actor.id,
            action = "admin.series.relationship.create",
            target = ("series", s.id.to_string()),
            payload = serde_json::json!({
                "relationship_id": outcome.forward.id.to_string(),
                "inverse_id": outcome.inverse.id.to_string(),
                "from_series_id": s.id.to_string(),
                "to_series_id": target.id.to_string(),
                "kind": req.kind.as_str(),
                "inverse_kind": req.kind.inverse().as_str(),
            }),
        );
    }

    let Some(view) = super::series::hydrate_series(&app, vec![target])
        .await
        .into_iter()
        .next()
    else {
        return respond(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            "relationship saved but readback failed",
        );
    };
    let Some(body) = edge_view(outcome.forward, view) else {
        return respond(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            "relationship saved but readback failed",
        );
    };
    let status = if outcome.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    (status, Json(body)).into_response()
}

#[utoipa::path(
    operation_id = "series_relationships_delete", delete,
    path = "/series/{slug}/relationships/{id}",
    params(("slug" = String, Path), ("id" = String, Path, description = "relationship row id")),
    responses(
        (status = 204, description = "edge and its inverse removed"),
        (status = 400, description = "malformed id"),
        (status = 403, description = "admin only"),
        (status = 404, description = "series / relationship not found"),
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
    let Ok(rel_id) = Uuid::parse_str(&id) else {
        return respond(
            StatusCode::BAD_REQUEST,
            ApiErrorCode::Validation,
            "invalid relationship id",
        );
    };
    // Scope to this series so a stray id can't reach across. Either half
    // of the pair is accepted (the GET lists this series' outgoing half).
    let row = match rel::Entity::find_by_id(rel_id)
        .filter(
            sea_orm::Condition::any()
                .add(rel::Column::FromSeriesId.eq(s.id))
                .add(rel::Column::ToSeriesId.eq(s.id)),
        )
        .one(&app.db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return respond(
                StatusCode::NOT_FOUND,
                ApiErrorCode::NotFound,
                "no such relationship for this series",
            );
        }
        Err(e) => return internal(&e),
    };

    let deleted = async {
        let txn = app.db.begin().await?;
        let out = relationships::delete_pair_by_id(&txn, row.id).await?;
        txn.commit().await?;
        Ok::<_, sea_orm::DbErr>(out)
    }
    .await;
    if let Err(e) = deleted {
        return internal(&e);
    }

    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.series.relationship.delete",
        target = ("series", s.id.to_string()),
        payload = serde_json::json!({
            "relationship_id": row.id.to_string(),
            "from_series_id": row.from_series_id.to_string(),
            "to_series_id": row.to_series_id.to_string(),
            "kind": row.kind,
            "source": row.source,
        }),
    );
    StatusCode::NO_CONTENT.into_response()
}

/// Load `ids` and keep only the series `user` may see: library grant +
/// age-rating cap, and (for non-admins) not removed. One query for the
/// rows plus at most one for the grants.
async fn visible_series(
    app: &AppState,
    user: &CurrentUser,
    ids: HashSet<Uuid>,
) -> Result<HashMap<Uuid, series::Model>, sea_orm::DbErr> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = series::Entity::find()
        .filter(series::Column::Id.is_in(ids.into_iter().collect::<Vec<_>>()))
        .all(&app.db)
        .await?;
    let acl = access::for_user(app, user).await;
    let is_admin = user.role == "admin";
    Ok(rows
        .into_iter()
        .filter(|o| {
            acl.series_ok(o.library_id, o.age_rating.as_deref())
                && (is_admin || o.removed_at.is_none())
        })
        .map(|o| (o.id, o))
        .collect())
}

/// Direct relationships of `series_id` whose other end `user` may see, as
/// `(kind, other series)` in creation order — the OPDS feeds' "related"
/// links. Errors degrade to an empty list (the links are optional).
pub(crate) async fn visible_related(
    app: &AppState,
    user: &CurrentUser,
    series_id: Uuid,
) -> Vec<(RelationshipKind, series::Model)> {
    let Ok(edges) = relationships::direct(&app.db, series_id).await else {
        return Vec::new();
    };
    if edges.is_empty() {
        return Vec::new();
    }
    let ids = edges.iter().map(|e| e.to_series_id).collect();
    let Ok(visible) = visible_series(app, user, ids).await else {
        return Vec::new();
    };
    edges
        .into_iter()
        .filter_map(|e| {
            let kind = e.kind.parse::<RelationshipKind>().ok()?;
            Some((kind, visible.get(&e.to_series_id)?.clone()))
        })
        .collect()
}

fn edge_view(e: rel::Model, series: SeriesView) -> Option<SeriesRelationshipView> {
    let kind: RelationshipKind = e.kind.parse().ok()?;
    let source: RelationshipSource = e.source.parse().unwrap_or(RelationshipSource::Manual);
    Some(SeriesRelationshipView {
        id: e.id.to_string(),
        kind,
        kind_label: kind.label().to_owned(),
        source,
        confidence: e.confidence,
        created_at: e.created_at.to_rfc3339(),
        series,
    })
}

fn internal(e: &sea_orm::DbErr) -> Response {
    tracing::error!(error = %e, "series relationship query failed");
    respond(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorCode::Internal,
        "internal",
    )
}
