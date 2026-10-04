//! `/series/{slug}/relationships` — typed series → series (and series →
//! story arc) edges (WP-7.1, taxonomy WP-7.5).
//!
//! - `GET` (any signed-in user who can see the series): the direct
//!   relationships, the series → arc edges, and the reading-order `chain`
//!   (`sequel_of` + `continues`). All ACL-filtered: a related series is
//!   listed only when the caller can see it too (library grant + age-rating
//!   cap; removed series are hidden from non-admins), an arc only when the
//!   caller can see one of its appearances, and a chain branch stops at the
//!   first series the caller can't see so nothing beyond a hidden link
//!   leaks.
//! - `POST` (admin): create an edge (+ its inverse for a series target) in
//!   one transaction, with optional scope (ranges, coverage, qualifier,
//!   note). Idempotent — an existing edge answers `200` with the existing
//!   row; a new one answers `201`.
//! - `PATCH /series/{slug}/relationships/{id}` (admin): change kind and/or
//!   scope; both halves stay in sync (a kind change is delete + create in
//!   one transaction).
//! - `DELETE /series/{slug}/relationships/{id}` (admin): remove an edge
//!   (and its inverse) by the row id the `GET` returned.
//! - `GET /relationship-kinds`: the kind catalogue (labels, inverses,
//!   groups, allowed qualifiers) the web pickers render.
//!
//! All writes go through [`crate::relationships`]; the audit actions are
//! `admin.series.relationship.{create,update,delete}`. The arc page's
//! tie-in list (`GET /arcs/{slug}/tie-ins`) lives in [`super::arcs`] and
//! delegates to [`arc_tie_ins_handler`].

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::{series, series_relationship as rel, story_arc};
use sea_orm::{
    ColumnTrait, EntityTrait, FromQueryResult, QueryFilter, Statement, TransactionTrait, Value,
};
use serde::{Deserialize, Deserializer, Serialize};
use shared::error::{ApiErrorCode, FieldError};
use shared::pagination::{CursorPage, decode_cursor, encode_cursor};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::extractors::Validated;
use super::series::SeriesView;
use super::{respond, respond_with_field_errors};
use crate::auth::{CurrentUser, RequireAdmin};
use crate::library::access;
use crate::middleware::RequestContext;
use crate::record_admin_action;
use crate::relationships::{
    self, PairError, RelationshipCoverage, RelationshipGroup, RelationshipKind,
    RelationshipQualifier, RelationshipSource, Scope,
};
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(update, delete))
        .routes(routes!(kinds))
}

// ───────── wire types ─────────

/// One direct relationship, from the requested series' point of view:
/// "this series `kind` `series`".
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesRelationshipView {
    /// Row id — pass to `PATCH` / `DELETE /series/{slug}/relationships/{id}`.
    pub id: String,
    pub kind: RelationshipKind,
    /// Display label for `kind` ("Sequel to", "Collected in", …), with a
    /// tie-in role folded in ("Prelude to").
    pub kind_label: String,
    pub group: RelationshipGroup,
    pub source: RelationshipSource,
    /// Suggestion confidence (0–1); `null` for manual edges.
    pub confidence: Option<f32>,
    pub created_at: String,
    /// Issue range on this series' side ("1-6").
    pub from_range: Option<String>,
    /// Issue range on the other series' side.
    pub to_range: Option<String>,
    /// Collects / reprints only.
    pub coverage: Option<RelationshipCoverage>,
    /// Continuation qualifier or tie-in role.
    pub qualifier: Option<RelationshipQualifier>,
    /// Display label for `qualifier` ("Relaunch").
    pub qualifier_label: Option<String>,
    pub note: Option<String>,
    /// The other series, hydrated like a library-grid card (cover, issue
    /// count, …).
    pub series: SeriesView,
}

/// A story arc as a relationship target.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RelationshipArcRef {
    pub id: String,
    /// `/arcs/{slug}` target.
    pub slug: String,
    pub name: String,
}

/// A series → story-arc edge ("this series is a tie-in to *arc*").
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesArcRelationshipView {
    pub id: String,
    pub kind: RelationshipKind,
    /// "Tie-in to", "Prelude to", … (the role folded in).
    pub kind_label: String,
    pub group: RelationshipGroup,
    pub source: RelationshipSource,
    pub confidence: Option<f32>,
    pub created_at: String,
    pub from_range: Option<String>,
    pub to_range: Option<String>,
    pub qualifier: Option<RelationshipQualifier>,
    pub qualifier_label: Option<String>,
    pub note: Option<String>,
    pub arc: RelationshipArcRef,
}

/// One series tying in to an arc (`GET /arcs/{slug}/tie-ins`).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ArcTieInView {
    /// The relationship row id.
    pub id: String,
    pub kind: RelationshipKind,
    /// "Tie-in to", "Prelude to", "Aftermath of", "Main story of".
    pub kind_label: String,
    pub qualifier: Option<RelationshipQualifier>,
    pub qualifier_label: Option<String>,
    pub from_range: Option<String>,
    pub to_range: Option<String>,
    pub note: Option<String>,
    pub source: RelationshipSource,
    pub created_at: String,
    /// The tying-in series.
    pub series: SeriesView,
}

/// One step of the reading-order chain.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesChainEntry {
    /// Signed reading-order offset: negative = read before this series,
    /// `0` = this series, positive = read after. Entries sharing a
    /// position are alternative branches.
    pub position: i32,
    pub series: SeriesView,
    /// Provider ranges inside this series: issues a provider files under a
    /// different provider series ("#600–611 continue as Fantastic Four
    /// (2012)"). The local series stays one step — membership is
    /// folder-pinned; these only label the boundary. Ordered by number.
    pub provider_splits: Vec<ChainSplitView>,
}

/// A reading-order sub-step from `series_provider_range` (range hygiene).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ChainSplitView {
    pub low: Option<String>,
    pub high: Option<String>,
    pub position: relationships::ChainSplitPosition,
    /// "#600–611".
    pub numbers: String,
    /// "continue as" / "begin as" / "are filed as" (singular for one issue).
    pub verb: String,
    /// The provider series, "Fantastic Four (2012)".
    pub target: String,
    /// `numbers verb target`.
    pub label: String,
    /// Every provider filing these numbers under that series.
    pub providers: Vec<ChainSplitProviderView>,
    /// A local series (visible to the caller) matched to that provider
    /// series, when the library has it.
    pub local_series: Option<super::series_external_relationships::ExternalLocalSeries>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ChainSplitProviderView {
    pub source: String,
    pub source_label: String,
    pub provider_series_id: String,
    pub url: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesRelationshipsResp {
    pub series_id: String,
    /// Direct series relationships, oldest first. Bounded by curation (each
    /// edge is admin-created or an accepted suggestion), so not paginated.
    pub relationships: Vec<SeriesRelationshipView>,
    /// Series → story-arc edges (tie-ins), oldest first; only arcs the
    /// caller can see.
    pub arcs: Vec<SeriesArcRelationshipView>,
    /// Reading-order chain through this series (`sequel_of` / `has_sequel`
    /// and `continues` / `continued_by`, depth ≤ 6 each way). Empty when
    /// the series has no such edges; otherwise includes this series at
    /// position 0.
    pub chain: Vec<SeriesChainEntry>,
    /// WP-7.8: relationships to provider series that aren't in the library
    /// ("Continued by: Saga (2018), not in your library"), oldest first.
    /// Provider links whose target is already local are left out (the
    /// suggestion engine proposes those).
    pub external: Vec<super::series_external_relationships::SeriesExternalRelationshipView>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RelationshipQualifierInfo {
    pub value: RelationshipQualifier,
    pub label: String,
}

/// One kind of the catalogue.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RelationshipKindInfo {
    pub kind: RelationshipKind,
    /// The reverse edge's kind (itself when `symmetric`).
    pub inverse: RelationshipKind,
    /// Read "this series *label* other" ("Sequel to").
    pub label: String,
    pub inverse_label: String,
    pub group: RelationshipGroup,
    /// Self-inverse (`crossover_with`, `see_also`, …).
    pub symmetric: bool,
    /// Accepted `qualifier` values (empty = none).
    pub qualifiers: Vec<RelationshipQualifierInfo>,
    /// `coverage` is accepted (collects / reprints and inverses).
    pub allows_coverage: bool,
    /// May target a story arc (`tie_in_to`).
    pub allows_arc_target: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RelationshipGroupInfo {
    pub group: RelationshipGroup,
    /// "Story", "Publication history", "Editions & contents", "Advanced".
    pub label: String,
}

/// The relationship kind catalogue (WP-7.5): groups in display order and
/// every kind in picker order (grouped, each pair adjacent).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RelationshipCatalogue {
    pub groups: Vec<RelationshipGroupInfo>,
    pub kinds: Vec<RelationshipKindInfo>,
}

/// `POST` body. Exactly one of `target` / `target_arc`.
#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct CreateSeriesRelationshipReq {
    /// The other series — slug or UUID.
    #[garde(skip)]
    #[serde(default)]
    pub target: Option<String>,
    /// A story arc instead of a series — slug or UUID (`tie_in_to` only).
    #[garde(skip)]
    #[serde(default)]
    pub target_arc: Option<String>,
    /// Read as "this series `kind` target" (e.g. `sequel_of`).
    #[garde(skip)]
    pub kind: RelationshipKind,
    /// Issue range on this series' side ("1-6", "1-6,Annual 1"; ≤ 100).
    #[garde(skip)]
    #[serde(default)]
    pub from_range: Option<String>,
    /// Issue range on the target's side (≤ 100).
    #[garde(skip)]
    #[serde(default)]
    pub to_range: Option<String>,
    /// Collects / reprints (and inverses) only.
    #[garde(skip)]
    #[serde(default)]
    pub coverage: Option<RelationshipCoverage>,
    /// Continuation qualifier (`continues` / `continued_by`) or tie-in role
    /// (`tie_in_to` / `has_tie_in`).
    #[garde(skip)]
    #[serde(default)]
    pub qualifier: Option<RelationshipQualifier>,
    /// Free text (≤ 500).
    #[garde(skip)]
    #[serde(default)]
    pub note: Option<String>,
}

/// `PATCH` body. Every field is optional: omit to keep, `null` to clear
/// (scope fields). Read from the `{slug}` series' side.
#[derive(Debug, Default, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct UpdateSeriesRelationshipReq {
    /// New kind (a change re-creates the pair; arc edges stay `tie_in_to`).
    #[garde(skip)]
    #[serde(default)]
    pub kind: Option<RelationshipKind>,
    #[garde(skip)]
    #[serde(default, deserialize_with = "deserialize_some")]
    #[schema(value_type = Option<String>)]
    pub from_range: Option<Option<String>>,
    #[garde(skip)]
    #[serde(default, deserialize_with = "deserialize_some")]
    #[schema(value_type = Option<String>)]
    pub to_range: Option<Option<String>>,
    #[garde(skip)]
    #[serde(default, deserialize_with = "deserialize_some")]
    #[schema(value_type = Option<RelationshipCoverage>)]
    pub coverage: Option<Option<RelationshipCoverage>>,
    #[garde(skip)]
    #[serde(default, deserialize_with = "deserialize_some")]
    #[schema(value_type = Option<RelationshipQualifier>)]
    pub qualifier: Option<Option<RelationshipQualifier>>,
    #[garde(skip)]
    #[serde(default, deserialize_with = "deserialize_some")]
    #[schema(value_type = Option<String>)]
    pub note: Option<Option<String>>,
}

/// `Some(None)` for an explicit `null`, `Some(Some(v))` for a value; an
/// omitted field stays `None` via `#[serde(default)]`.
fn deserialize_some<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

// ───────── handlers ─────────

#[utoipa::path(
    operation_id = "relationship_kinds", get,
    path = "/relationship-kinds",
    responses((status = 200, body = RelationshipCatalogue))
)]
#[handler]
pub async fn kinds(_user: CurrentUser) -> Response {
    Json(catalogue()).into_response()
}

/// The catalogue the `GET /relationship-kinds` handler serves.
pub fn catalogue() -> RelationshipCatalogue {
    RelationshipCatalogue {
        groups: RelationshipGroup::ALL
            .into_iter()
            .map(|g| RelationshipGroupInfo {
                group: g,
                label: g.label().to_owned(),
            })
            .collect(),
        kinds: RelationshipKind::ALL
            .into_iter()
            .map(|k| RelationshipKindInfo {
                kind: k,
                inverse: k.inverse(),
                label: k.label().to_owned(),
                inverse_label: k.inverse().label().to_owned(),
                group: k.group(),
                symmetric: k.is_self_inverse(),
                qualifiers: k
                    .qualifiers()
                    .iter()
                    .map(|q| RelationshipQualifierInfo {
                        value: *q,
                        label: q.label().to_owned(),
                    })
                    .collect(),
                allows_coverage: k.allows_coverage(),
                allows_arc_target: k.allows_arc_target(),
            })
            .collect(),
    }
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
    let arc_edges = match relationships::arc_edges(&app.db, s.id).await {
        Ok(e) => e,
        Err(e) => return internal(&e),
    };
    let chain = match relationships::chain(&app.db, s.id).await {
        Ok(c) => c,
        Err(e) => return internal(&e),
    };

    // One batch load + ACL pass for every series either list mentions.
    let mut ids: HashSet<Uuid> = edges.iter().filter_map(|e| e.to_series_id).collect();
    ids.extend(chain.iter().map(|n| n.series_id));
    ids.remove(&s.id);
    let mut visible: Vec<series::Model> = match visible_series(&app, &user, ids).await {
        Ok(v) => v.into_values().collect(),
        Err(e) => return internal(&e),
    };
    visible.push(s.clone());
    let views: HashMap<String, SeriesView> = super::series::hydrate_series(&app, visible, user.id)
        .await
        .into_iter()
        .map(|v| (v.id.clone(), v))
        .collect();

    let relationships = edges
        .into_iter()
        .filter_map(|e| {
            let view = views.get(&e.to_series_id?.to_string())?.clone();
            edge_view(e, view)
        })
        .collect();

    let arcs = match visible_arc_views(&app, &user, arc_edges).await {
        Ok(a) => a,
        Err(e) => return internal(&e),
    };

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
        let mut splits = match chain_split_views(&app, &user, &kept).await {
            Ok(x) => x,
            Err(e) => return internal(&e),
        };
        chain
            .into_iter()
            .filter(|n| kept.contains(&n.series_id))
            .filter_map(|n| {
                Some(SeriesChainEntry {
                    position: n.position,
                    series: views.get(&n.series_id.to_string())?.clone(),
                    provider_splits: splits.remove(&n.series_id).unwrap_or_default(),
                })
            })
            .collect()
    };

    let external =
        match super::series_external_relationships::external_views(&app, &user, s.id).await {
            Ok(x) => x,
            Err(e) => return internal(&e),
        };

    Json(SeriesRelationshipsResp {
        series_id: s.id.to_string(),
        relationships,
        arcs,
        chain: chain_view,
        external,
    })
    .into_response()
}

#[utoipa::path(
    operation_id = "series_relationships_create", post,
    path = "/series/{slug}/relationships",
    params(("slug" = String, Path)),
    request_body = CreateSeriesRelationshipReq,
    responses(
        (status = 201, body = SeriesRelationshipView, description = "edge (and its inverse) created; an arc target answers `SeriesArcRelationshipView`"),
        (status = 200, body = SeriesRelationshipView, description = "edge already existed; returned unchanged"),
        (status = 403, description = "admin only"),
        (status = 404, description = "series or target not found"),
        (status = 409, description = "contradicts an existing relationship"),
        (status = 422, description = "self-relationship / scope invalid for the kind / invalid body"),
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
    let target = req
        .target
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let target_arc = req
        .target_arc
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let mut issues = Vec::new();
    match (target, target_arc) {
        (None, None) => issues.push(field("target", "target or target_arc is required")),
        (Some(_), Some(_)) => issues.push(field(
            "target_arc",
            "send either target or target_arc, not both",
        )),
        _ => {}
    }
    for (name, v) in [("target", target), ("target_arc", target_arc)] {
        if v.is_some_and(|v| v.chars().count() > 400) {
            issues.push(field(name, "must be at most 400 characters"));
        }
    }
    if !issues.is_empty() {
        return validation(issues);
    }
    let scope = Scope {
        from_range: req.from_range,
        to_range: req.to_range,
        coverage: req.coverage,
        qualifier: req.qualifier,
        note: req.note,
    }
    .normalized();

    if let Some(arc_ref) = target_arc {
        return create_arc(&app, actor.id, &ctx, &s, arc_ref, req.kind, scope).await;
    }
    let Some(target) = target else {
        return validation(vec![field("target", "target is required")]);
    };
    let target = match super::series::find_by_slug(&app.db, target).await {
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
        let out = relationships::create_pair_scoped(
            &txn,
            s.id,
            target.id,
            req.kind,
            RelationshipSource::Manual,
            None,
            Some(actor.id),
            &scope,
        )
        .await?;
        txn.commit().await.map_err(PairError::Db)?;
        Ok::<_, PairError>(out)
    }
    .await;
    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => return pair_error(e),
    };

    if outcome.created {
        // WP-7.4: relationships are a similar-series signal.
        app.similarity.invalidate_all();
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
                "scope": scope_json(&Scope::of(&outcome.forward)),
            }),
        );
    }

    let Some(view) = super::series::hydrate_series(&app, vec![target], actor.id)
        .await
        .into_iter()
        .next()
    else {
        return readback_failed();
    };
    let Some(body) = edge_view(outcome.forward, view) else {
        return readback_failed();
    };
    let status = if outcome.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    (status, Json(body)).into_response()
}

/// `POST` with `target_arc`: a single series → arc row.
async fn create_arc(
    app: &AppState,
    actor: Uuid,
    ctx: &RequestContext,
    s: &series::Model,
    arc_ref: &str,
    kind: RelationshipKind,
    scope: Scope,
) -> Response {
    if !kind.allows_arc_target() {
        return pair_error(PairError::ArcKind { kind });
    }
    let arc = match find_arc(app, arc_ref).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return respond(
                StatusCode::NOT_FOUND,
                ApiErrorCode::NotFound,
                "target story arc not found",
            );
        }
        Err(e) => return internal(&e),
    };
    let outcome = async {
        let txn = app.db.begin().await.map_err(PairError::Db)?;
        let out = relationships::create_arc_edge(
            &txn,
            s.id,
            arc.id,
            kind,
            RelationshipSource::Manual,
            None,
            Some(actor),
            &scope,
        )
        .await?;
        txn.commit().await.map_err(PairError::Db)?;
        Ok::<_, PairError>(out)
    }
    .await;
    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => return pair_error(e),
    };
    if outcome.created {
        app.similarity.invalidate_all();
        record_admin_action!(
            db = &app.db,
            ctx = ctx,
            actor = actor,
            action = "admin.series.relationship.create",
            target = ("series", s.id.to_string()),
            payload = serde_json::json!({
                "relationship_id": outcome.row.id.to_string(),
                "from_series_id": s.id.to_string(),
                "to_arc_id": arc.id.to_string(),
                "kind": kind.as_str(),
                "scope": scope_json(&Scope::of(&outcome.row)),
            }),
        );
    }
    let body = arc_edge_view(outcome.row, arc_ref_of(&arc));
    let status = if outcome.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    (status, Json(body)).into_response()
}

#[utoipa::path(
    operation_id = "series_relationships_update", patch,
    path = "/series/{slug}/relationships/{id}",
    params(("slug" = String, Path), ("id" = String, Path, description = "relationship row id (either half)")),
    request_body = UpdateSeriesRelationshipReq,
    responses(
        (status = 200, body = SeriesRelationshipView, description = "updated edge from `{slug}`'s side (an arc edge answers `SeriesArcRelationshipView`); a kind change returns the new row id"),
        (status = 400, description = "malformed id"),
        (status = 403, description = "admin only"),
        (status = 404, description = "series / relationship not found"),
        (status = 409, description = "the new kind contradicts or duplicates an existing relationship"),
        (status = 422, description = "scope invalid for the kind / arc edge given a non-arc kind"),
    )
)]
#[handler]
pub async fn update(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path((slug, id)): Path<(String, String)>,
    Validated(req): Validated<UpdateSeriesRelationshipReq>,
) -> Response {
    let s = match super::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let Ok(rel_id) = Uuid::parse_str(&id) else {
        return bad_id();
    };
    let row = match rel::Entity::find_by_id(rel_id).one(&app.db).await {
        Ok(r) => r,
        Err(e) => return internal(&e),
    };
    // Read from `{slug}`'s side: either half is accepted, and the inverse
    // half is swapped for its partner so `kind` / ranges read "this series
    // `kind` other".
    let forward = match row {
        Some(r) => match relationships::row_from_perspective(&app.db, r, s.id).await {
            Ok(f) => f,
            Err(e) => return internal(&e),
        },
        None => None,
    };
    let Some(forward) = forward else {
        return respond(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NotFound,
            "no such relationship for this series",
        );
    };
    let Ok(old_kind) = forward.kind.parse::<RelationshipKind>() else {
        return internal(&sea_orm::DbErr::Custom("bad relationship kind".into()));
    };
    let kind = req.kind.unwrap_or(old_kind);
    let old = Scope::of(&forward);
    let scope = Scope {
        from_range: req.from_range.unwrap_or(old.from_range),
        to_range: req.to_range.unwrap_or(old.to_range),
        coverage: req.coverage.unwrap_or(old.coverage),
        qualifier: req.qualifier.unwrap_or(old.qualifier),
        note: req.note.unwrap_or(old.note),
    }
    .normalized();

    let outcome = async {
        let txn = app.db.begin().await.map_err(PairError::Db)?;
        let out = relationships::update_edge(&txn, forward, kind, scope).await?;
        txn.commit().await.map_err(PairError::Db)?;
        Ok::<_, PairError>(out)
    }
    .await;
    let out = match outcome {
        Ok(o) => o,
        Err(e) => return pair_error(e),
    };

    // WP-7.4: relationships are a similar-series signal.
    app.similarity.invalidate_all();
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.series.relationship.update",
        target = ("series", s.id.to_string()),
        payload = serde_json::json!({
            "relationship_id": out.forward.id.to_string(),
            "previous_relationship_id": out.before.id.to_string(),
            "inverse_id": out.inverse.as_ref().map(|i| i.id.to_string()),
            "from_series_id": s.id.to_string(),
            "to_series_id": out.forward.to_series_id.map(|u| u.to_string()),
            "to_arc_id": out.forward.to_arc_id.map(|u| u.to_string()),
            "kind_before": out.before.kind,
            "kind": out.forward.kind,
            "kind_changed": out.kind_changed,
            "scope_before": scope_json(&Scope::of(&out.before)),
            "scope": scope_json(&Scope::of(&out.forward)),
        }),
    );

    if let Some(arc_id) = out.forward.to_arc_id {
        let arc = match story_arc::Entity::find_by_id(arc_id).one(&app.db).await {
            Ok(Some(a)) => a,
            Ok(None) => return readback_failed(),
            Err(e) => return internal(&e),
        };
        return Json(arc_edge_view(out.forward, arc_ref_of(&arc))).into_response();
    }
    let Some(to) = out.forward.to_series_id else {
        return readback_failed();
    };
    let target = match series::Entity::find_by_id(to).one(&app.db).await {
        Ok(Some(t)) => t,
        Ok(None) => return readback_failed(),
        Err(e) => return internal(&e),
    };
    let Some(view) = super::series::hydrate_series(&app, vec![target], actor.id)
        .await
        .into_iter()
        .next()
    else {
        return readback_failed();
    };
    match edge_view(out.forward, view) {
        Some(body) => Json(body).into_response(),
        None => readback_failed(),
    }
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
        return bad_id();
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
    // WP-7.4: relationships are a similar-series signal.
    app.similarity.invalidate_all();

    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.series.relationship.delete",
        target = ("series", s.id.to_string()),
        payload = serde_json::json!({
            "relationship_id": row.id.to_string(),
            "from_series_id": row.from_series_id.to_string(),
            "to_series_id": row.to_series_id.map(|u| u.to_string()),
            "to_arc_id": row.to_arc_id.map(|u| u.to_string()),
            "kind": row.kind,
            "source": row.source,
            "scope": scope_json(&Scope::of(&row)),
        }),
    );
    StatusCode::NO_CONTENT.into_response()
}

/// Keyset position for `GET /arcs/{slug}/tie-ins`:
/// `(role rank, created_at, id)` — see [`TIE_IN_ROLE_RANK_SQL`].
#[derive(Debug, Serialize, Deserialize)]
struct TieInCursor {
    #[serde(rename = "k")]
    rank: i32,
    #[serde(rename = "t")]
    created_at: String,
    #[serde(rename = "i")]
    id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct TieInIdRow {
    id: Uuid,
    rank: i32,
}

/// Reading order of tie-in roles, so the arc page can group a paginated
/// list by role without reordering as pages load: prelude, main story,
/// tie-in (and unset), aftermath.
const TIE_IN_ROLE_RANK_SQL: &str = "(CASE r.qualifier WHEN 'prelude' THEN 0 \
     WHEN 'main' THEN 1 WHEN 'aftermath' THEN 3 ELSE 2 END)";

#[derive(Debug, FromQueryResult)]
struct CountRow {
    n: i64,
}

/// `GET /arcs/{slug}/tie-ins` core: series with a `tie_in_to` edge to the
/// arc, grouped by role (prelude, main story, tie-in, aftermath) and
/// oldest first within a role, cursor-paginated (`total` on the first
/// page). The arc must be visible to the caller (same 404 gate as
/// `/arcs/{slug}`); tying-in series are filtered by the entity pages'
/// series ACL in SQL so pages are never short. Admins also see removed
/// series (WP-7.7, as `series::list` does).
pub(crate) async fn arc_tie_ins_handler(
    app: &AppState,
    user: &CurrentUser,
    slug: &str,
    cursor: Option<&str>,
    limit: Option<u64>,
) -> Response {
    use super::entity_pages::{self as ep, EntityKind};
    let after = match cursor {
        Some(c) => match decode_cursor::<TieInCursor>(c) {
            Ok(k) => match chrono::DateTime::parse_from_rfc3339(&k.created_at) {
                Ok(t) => Some((k.rank, t, k.id)),
                Err(_) => return bad_cursor(),
            },
            Err(_) => return bad_cursor(),
        },
        None => None,
    };
    let (arc, visible, _, _) =
        match ep::resolve_visible_for_user(app, user, EntityKind::Arc, slug).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let limit = limit.unwrap_or(60).clamp(1, 100);

    let mut params: Vec<Value> = vec![Value::from(arc.id)];
    let include_removed = user.role == "admin";
    let Some(svis) = ep::series_visible_sql_for(&visible, &mut params, include_removed) else {
        return Json(CursorPage::<ArcTieInView>::paginated(
            Vec::new(),
            None,
            Some(0),
        ))
        .into_response();
    };
    let base = format!(
        "FROM series_relationship r JOIN series s ON s.id = r.from_series_id \
         WHERE r.to_arc_id = $1 AND {svis}"
    );
    let total = if after.is_none() {
        let n = CountRow::find_by_statement(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            format!("SELECT COUNT(*)::bigint AS n {base}"),
            params.clone(),
        ))
        .one(&app.db)
        .await;
        match n {
            Ok(r) => Some(r.map_or(0, |r| u64::try_from(r.n).unwrap_or(0))),
            Err(e) => return internal(&e),
        }
    } else {
        None
    };
    let mut page_params = params;
    let keyset = match after {
        Some((rank, t, id)) => {
            page_params.push(Value::from(rank));
            page_params.push(Value::from(t));
            page_params.push(Value::from(id));
            let n = page_params.len();
            format!(
                " AND ({TIE_IN_ROLE_RANK_SQL}, r.created_at, r.id) > (${}, ${}, ${})",
                n - 2,
                n - 1,
                n
            )
        }
        None => String::new(),
    };
    page_params.push(Value::from(i64::try_from(limit + 1).unwrap_or(101)));
    let limit_param = page_params.len();
    let ids = TieInIdRow::find_by_statement(Statement::from_sql_and_values(
        app.db.get_database_backend(),
        format!(
            "SELECT r.id, {TIE_IN_ROLE_RANK_SQL} AS rank {base}{keyset} \
             ORDER BY rank, r.created_at, r.id LIMIT ${limit_param}"
        ),
        page_params,
    ))
    .all(&app.db)
    .await;
    let mut ranked: Vec<(Uuid, i32)> = match ids {
        Ok(r) => r.into_iter().map(|r| (r.id, r.rank)).collect(),
        Err(e) => return internal(&e),
    };
    let has_more = ranked.len() as u64 > limit;
    ranked.truncate(usize::try_from(limit).unwrap_or(100));
    let rank_of: HashMap<Uuid, i32> = ranked.iter().copied().collect();
    let ids: Vec<Uuid> = ranked.into_iter().map(|(id, _)| id).collect();

    let rows = match rel::Entity::find()
        .filter(rel::Column::Id.is_in(ids.clone()))
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => return internal(&e),
    };
    let mut by_id: HashMap<Uuid, rel::Model> = rows.into_iter().map(|r| (r.id, r)).collect();
    let series_ids: Vec<Uuid> = by_id.values().map(|r| r.from_series_id).collect();
    let series_rows = match series::Entity::find()
        .filter(series::Column::Id.is_in(series_ids))
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => return internal(&e),
    };
    let views: HashMap<String, SeriesView> =
        super::series::hydrate_series(app, series_rows, user.id)
            .await
            .into_iter()
            .map(|v| (v.id.clone(), v))
            .collect();
    let mut last: Option<(i32, String, Uuid)> = None;
    let items: Vec<ArcTieInView> = ids
        .iter()
        .filter_map(|id| {
            let r = by_id.remove(id)?;
            last = Some((
                rank_of.get(&r.id).copied().unwrap_or(2),
                r.created_at.to_rfc3339(),
                r.id,
            ));
            let series = views.get(&r.from_series_id.to_string())?.clone();
            let kind: RelationshipKind = r.kind.parse().ok()?;
            let scope = Scope::of(&r);
            Some(ArcTieInView {
                id: r.id.to_string(),
                kind,
                kind_label: kind.display_label(scope.qualifier),
                qualifier: scope.qualifier,
                qualifier_label: scope.qualifier.map(|q| q.label().to_owned()),
                from_range: scope.from_range,
                to_range: scope.to_range,
                note: scope.note,
                source: r.source.parse().unwrap_or(RelationshipSource::Manual),
                created_at: r.created_at.to_rfc3339(),
                series,
            })
        })
        .collect();
    let next_cursor = if has_more {
        last.and_then(|(rank, t, id)| {
            encode_cursor(&TieInCursor {
                rank,
                created_at: t,
                id,
            })
            .ok()
        })
    } else {
        None
    };
    Json(CursorPage::paginated(items, next_cursor, total)).into_response()
}

// ───────── helpers ─────────

/// Load `ids` and keep only the series `user` may see: library grant +
/// age-rating cap, and (for non-admins) not removed. One query for the
/// rows plus at most one for the grants.
/// Provider-range sub-steps for the kept chain nodes, with each step's
/// local holder series resolved through the caller's ACL (a hidden series
/// is simply not linked).
async fn chain_split_views(
    app: &AppState,
    user: &CurrentUser,
    kept: &HashSet<Uuid>,
) -> Result<HashMap<Uuid, Vec<ChainSplitView>>, sea_orm::DbErr> {
    let ids: Vec<Uuid> = kept.iter().copied().collect();
    let splits = relationships::chain_splits(&app.db, &ids).await?;
    let holders: HashSet<Uuid> = splits
        .values()
        .flatten()
        .flat_map(|s| s.local_series_ids.iter().copied())
        .collect();
    let visible = visible_series(app, user, holders).await?;
    Ok(splits
        .into_iter()
        .map(|(sid, steps)| {
            let views = steps
                .into_iter()
                .map(|s| {
                    let local_series = s
                        .local_series_ids
                        .iter()
                        .find_map(|id| visible.get(id))
                        .map(
                            |m| super::series_external_relationships::ExternalLocalSeries {
                                id: m.id.to_string(),
                                slug: m.slug.clone(),
                                name: m.name.clone(),
                                year: m.year,
                            },
                        );
                    ChainSplitView {
                        label: s.label(),
                        low: s.low,
                        high: s.high,
                        position: s.position,
                        numbers: s.numbers,
                        verb: s.verb.to_owned(),
                        target: s.target,
                        providers: s
                            .providers
                            .into_iter()
                            .map(|p| ChainSplitProviderView {
                                source_label: crate::metadata::identifier::Source::from_str(
                                    &p.source,
                                )
                                .map_or_else(|_| p.source.clone(), |x| x.label().to_owned()),
                                source: p.source,
                                provider_series_id: p.provider_series_id,
                                url: p.url,
                            })
                            .collect(),
                        local_series,
                    }
                })
                .collect();
            (sid, views)
        })
        .collect())
}

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

/// Arc edges whose arc the caller can see (an appearance in a visible
/// issue or series — the `/arcs/{slug}` 404 rule), as views.
async fn visible_arc_views(
    app: &AppState,
    user: &CurrentUser,
    edges: Vec<rel::Model>,
) -> Result<Vec<SeriesArcRelationshipView>, sea_orm::DbErr> {
    if edges.is_empty() {
        return Ok(Vec::new());
    }
    let arc_ids: Vec<Uuid> = edges.iter().filter_map(|e| e.to_arc_id).collect();
    let acl = access::for_user(app, user).await;
    let visible = super::entity_pages::visible_arc_ids(app, &acl, &arc_ids).await?;
    let arcs: HashMap<Uuid, story_arc::Model> = story_arc::Entity::find()
        .filter(story_arc::Column::Id.is_in(visible.iter().copied().collect::<Vec<_>>()))
        .all(&app.db)
        .await?
        .into_iter()
        .map(|a| (a.id, a))
        .collect();
    Ok(edges
        .into_iter()
        .filter_map(|e| {
            let arc = arcs.get(&e.to_arc_id?)?;
            Some(arc_edge_view(e, arc_ref_of(arc)))
        })
        .collect())
}

/// A story arc by UUID or slug.
async fn find_arc(app: &AppState, key: &str) -> Result<Option<story_arc::Model>, sea_orm::DbErr> {
    if let Ok(id) = Uuid::parse_str(key) {
        return story_arc::Entity::find_by_id(id).one(&app.db).await;
    }
    story_arc::Entity::find()
        .filter(story_arc::Column::Slug.eq(key))
        .one(&app.db)
        .await
}

fn arc_ref_of(a: &story_arc::Model) -> RelationshipArcRef {
    RelationshipArcRef {
        id: a.id.to_string(),
        slug: a.slug.clone(),
        name: a.name.clone(),
    }
}

/// WP-7.7: how many direct relationships the series page's Related tab
/// lists for `user` — series edges whose other end is visible plus arc
/// edges whose arc is visible (the same filters as the `GET`), so the tab
/// label can show a count without the full relationships payload. `None`
/// on a DB error (the label then shows no count).
pub(crate) async fn visible_relationship_count(
    app: &AppState,
    user: &CurrentUser,
    series_id: Uuid,
) -> Option<i64> {
    let edges = relationships::direct(&app.db, series_id).await.ok()?;
    let arc_edges = relationships::arc_edges(&app.db, series_id).await.ok()?;
    let ids: HashSet<Uuid> = edges.iter().filter_map(|e| e.to_series_id).collect();
    let visible = visible_series(app, user, ids).await.ok()?;
    let series_n = edges
        .iter()
        .filter(|e| e.to_series_id.is_some_and(|id| visible.contains_key(&id)))
        .count();
    let arc_n = if arc_edges.is_empty() {
        0
    } else {
        let arc_ids: Vec<Uuid> = arc_edges.iter().filter_map(|e| e.to_arc_id).collect();
        let acl = access::for_user(app, user).await;
        let vis = super::entity_pages::visible_arc_ids(app, &acl, &arc_ids)
            .await
            .ok()?;
        arc_edges
            .iter()
            .filter(|e| e.to_arc_id.is_some_and(|id| vis.contains(&id)))
            .count()
    };
    // WP-7.8: external ("not in your library") links show in the tab too.
    // One indexed lookup when the series has none (the common case).
    let ext_n = super::series_external_relationships::external_views(app, user, series_id)
        .await
        .ok()?
        .len();
    i64::try_from(series_n + arc_n + ext_n).ok()
}

/// One story arc this series ties in to, for the OPDS "related" links
/// (WP-7.7).
pub(crate) struct RelatedArcLink {
    /// "Tie-in to", "Prelude to", … (role folded in).
    pub label: String,
    pub arc: story_arc::Model,
}

/// Arc edges of `series_id` whose arc `user` can see, in creation order —
/// the OPDS feeds' arc "related" links. Errors degrade to an empty list.
pub(crate) async fn visible_related_arcs(
    app: &AppState,
    user: &CurrentUser,
    series_id: Uuid,
) -> Vec<RelatedArcLink> {
    let Ok(edges) = relationships::arc_edges(&app.db, series_id).await else {
        return Vec::new();
    };
    if edges.is_empty() {
        return Vec::new();
    }
    let arc_ids: Vec<Uuid> = edges.iter().filter_map(|e| e.to_arc_id).collect();
    let acl = access::for_user(app, user).await;
    let Ok(visible) = super::entity_pages::visible_arc_ids(app, &acl, &arc_ids).await else {
        return Vec::new();
    };
    let Ok(arcs) = story_arc::Entity::find()
        .filter(story_arc::Column::Id.is_in(visible.iter().copied().collect::<Vec<_>>()))
        .all(&app.db)
        .await
    else {
        return Vec::new();
    };
    let arcs: HashMap<Uuid, story_arc::Model> = arcs.into_iter().map(|a| (a.id, a)).collect();
    edges
        .into_iter()
        .filter_map(|e| {
            let kind = e.kind.parse::<RelationshipKind>().ok()?;
            let qualifier = e.qualifier.as_deref().and_then(|q| q.parse().ok());
            Some(RelatedArcLink {
                label: kind.display_label(qualifier),
                arc: arcs.get(&e.to_arc_id?)?.clone(),
            })
        })
        .collect()
}

/// One related series for the OPDS "related" links.
pub(crate) struct RelatedLink {
    pub kind: RelationshipKind,
    /// [`RelationshipKind::display_label`] (tie-in role folded in).
    pub label: String,
    pub series: series::Model,
}

/// Direct relationships of `series_id` whose other end `user` may see, in
/// creation order — the OPDS feeds' "related" links. Errors degrade to an
/// empty list (the links are optional).
pub(crate) async fn visible_related(
    app: &AppState,
    user: &CurrentUser,
    series_id: Uuid,
) -> Vec<RelatedLink> {
    let Ok(edges) = relationships::direct(&app.db, series_id).await else {
        return Vec::new();
    };
    if edges.is_empty() {
        return Vec::new();
    }
    let ids = edges.iter().filter_map(|e| e.to_series_id).collect();
    let Ok(visible) = visible_series(app, user, ids).await else {
        return Vec::new();
    };
    edges
        .into_iter()
        .filter_map(|e| {
            let kind = e.kind.parse::<RelationshipKind>().ok()?;
            let qualifier = e.qualifier.as_deref().and_then(|q| q.parse().ok());
            Some(RelatedLink {
                kind,
                label: kind.display_label(qualifier),
                series: visible.get(&e.to_series_id?)?.clone(),
            })
        })
        .collect()
}

/// [`edge_view`] for sibling modules (the external-link promotion answer).
pub(crate) fn edge_view_pub(e: rel::Model, series: SeriesView) -> Option<SeriesRelationshipView> {
    edge_view(e, series)
}

fn edge_view(e: rel::Model, series: SeriesView) -> Option<SeriesRelationshipView> {
    let kind: RelationshipKind = e.kind.parse().ok()?;
    let source: RelationshipSource = e.source.parse().unwrap_or(RelationshipSource::Manual);
    let scope = Scope::of(&e);
    Some(SeriesRelationshipView {
        id: e.id.to_string(),
        kind,
        kind_label: kind.display_label(scope.qualifier),
        group: kind.group(),
        source,
        confidence: e.confidence,
        created_at: e.created_at.to_rfc3339(),
        from_range: scope.from_range,
        to_range: scope.to_range,
        coverage: scope.coverage,
        qualifier: scope.qualifier,
        qualifier_label: scope.qualifier.map(|q| q.label().to_owned()),
        note: scope.note,
        series,
    })
}

fn arc_edge_view(e: rel::Model, arc: RelationshipArcRef) -> SeriesArcRelationshipView {
    let kind: RelationshipKind = e.kind.parse().unwrap_or(RelationshipKind::TieInTo);
    let scope = Scope::of(&e);
    SeriesArcRelationshipView {
        id: e.id.to_string(),
        kind,
        kind_label: kind.display_label(scope.qualifier),
        group: kind.group(),
        source: e.source.parse().unwrap_or(RelationshipSource::Manual),
        confidence: e.confidence,
        created_at: e.created_at.to_rfc3339(),
        from_range: scope.from_range,
        to_range: scope.to_range,
        qualifier: scope.qualifier,
        qualifier_label: scope.qualifier.map(|q| q.label().to_owned()),
        note: scope.note,
        arc,
    }
}

fn scope_json(s: &Scope) -> serde_json::Value {
    serde_json::json!({
        "from_range": s.from_range,
        "to_range": s.to_range,
        "coverage": s.coverage.map(|c| c.as_str()),
        "qualifier": s.qualifier.map(|q| q.as_str()),
        "note": s.note,
    })
}

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

fn pair_error(e: PairError) -> Response {
    match e {
        PairError::SelfEdge => respond(
            StatusCode::UNPROCESSABLE_ENTITY,
            ApiErrorCode::Validation,
            "a series cannot be related to itself",
        ),
        e @ (PairError::Conflict { .. } | PairError::Duplicate { .. }) => {
            respond(StatusCode::CONFLICT, ApiErrorCode::Conflict, e.to_string())
        }
        e @ PairError::InvalidConfidence => respond(
            StatusCode::UNPROCESSABLE_ENTITY,
            ApiErrorCode::Validation,
            e.to_string(),
        ),
        e @ PairError::ArcKind { .. } => validation(vec![field("kind", &e.to_string())]),
        PairError::InvalidScope(issues) => validation(
            issues
                .into_iter()
                .map(|i| FieldError {
                    field: i.field.to_owned(),
                    message: i.message,
                })
                .collect(),
        ),
        PairError::Db(e) => internal(&e),
    }
}

fn bad_id() -> Response {
    respond(
        StatusCode::BAD_REQUEST,
        ApiErrorCode::Validation,
        "invalid relationship id",
    )
}

fn bad_cursor() -> Response {
    respond(
        StatusCode::BAD_REQUEST,
        ApiErrorCode::Validation,
        "invalid cursor",
    )
}

fn readback_failed() -> Response {
    respond(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorCode::Internal,
        "relationship saved but readback failed",
    )
}

fn internal(e: &sea_orm::DbErr) -> Response {
    tracing::error!(error = %e, "series relationship query failed");
    respond(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorCode::Internal,
        "internal",
    )
}
