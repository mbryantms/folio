//! Relationship suggestions: the admin API over
//! [`crate::relationships::suggestions`] (WP-7.2), plus the review-UI
//! additions of WP-7.3 (bulk accept / reject, reopen, the `stale` filter).
//!
//! | method | path | |
//! |---|---|---|
//! | `GET`  | `/admin/relationship-suggestions` | cursor-paginated list (`status`, `bucket`, `library_id`) |
//! | `POST` | `/admin/relationship-suggestions/{id}/accept` | body `{ "kind"?: RelationshipKind }` |
//! | `POST` | `/admin/relationship-suggestions/{id}/reject` | |
//! | `POST` | `/admin/relationship-suggestions/{id}/reopen` | rejected → pending |
//! | `POST` | `/admin/relationship-suggestions/bulk-accept` | `{ "ids": [...] }` or `{ "bucket": "high", "library_id"? }` |
//! | `POST` | `/admin/relationship-suggestions/bulk-reject` | `{ "ids": [...] }` |
//! | `POST` | `/admin/relationship-suggestions/run` | enqueue a run (`?library_id=` or every library) |
//! | `GET`  | `/series/{slug}/relationship-suggestions` | pending suggestions touching one series |
//!
//! All admin-only (`RequireAdmin`); mutations write
//! `admin.relationship_suggestion.{accept,reject,reopen,bulk_accept,bulk_reject,run}`
//! audit rows — **one** per bulk batch.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::{library, series, series_relationship_suggestion as sug};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};
use shared::error::ApiErrorCode;
use shared::pagination::{decode_cursor, encode_cursor};
use std::collections::{BTreeMap, HashMap, HashSet};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::extractors::Validated;
use super::respond;
use super::series::SeriesView;
use crate::auth::RequireAdmin;
use crate::middleware::RequestContext;
use crate::record_admin_action;
use crate::relationships::RelationshipKind;
use crate::relationships::suggestions::{
    self, ReviewError, SuggestionBucket, SuggestionCursor, SuggestionFilter, SuggestionStatus,
};
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(run))
        .routes(routes!(accept))
        .routes(routes!(reject))
        .routes(routes!(reopen))
        .routes(routes!(bulk_accept))
        .routes(routes!(bulk_reject))
        .routes(routes!(list_for_series))
}

/// One suggestion, both series hydrated like library-grid cards (covers
/// for the review UI). Reads "`from_series` `kind` `to_series`".
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RelationshipSuggestionView {
    pub id: String,
    pub from_series: SeriesView,
    pub to_series: SeriesView,
    /// Canonical kind: `sequel_of`, `spin_off_of`, `collects`,
    /// `crossover_with`, `same_universe` or `see_also`.
    pub kind: RelationshipKind,
    /// Display label for `kind` ("Sequel of", …).
    pub kind_label: String,
    /// 0–1.
    pub confidence: f32,
    pub bucket: SuggestionBucket,
    /// Human-readable explanation (one clause per evidence source).
    pub reason: String,
    /// Structured evidence: `{ "sources": [ { "source": "story_arc", "confidence": 0.65, "reason": "…", … } ] }`.
    #[schema(value_type = Object)]
    pub evidence: serde_json::Value,
    pub status: SuggestionStatus,
    /// Kind actually created when accepted with an override (`modified`).
    pub accepted_kind: Option<RelationshipKind>,
    pub created_at: String,
    pub updated_at: String,
    pub reviewed_at: Option<String>,
    pub reviewed_by: Option<String>,
}

/// Bucket counts for the review UI's confidence tabs (first page only).
#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct SuggestionBucketCounts {
    pub high: u64,
    pub medium: u64,
    pub low: u64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RelationshipSuggestionListView {
    pub items: Vec<RelationshipSuggestionView>,
    pub next_cursor: Option<String>,
    /// Matching suggestions across all pages. First page only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// Per-bucket counts for the current filter minus `bucket`. First page
    /// only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket_counts: Option<SuggestionBucketCounts>,
}

/// `status` filter: one status, or `all` (every status except `stale`).
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionStatusFilter {
    #[default]
    Pending,
    Accepted,
    Rejected,
    Modified,
    Stale,
    All,
}

impl SuggestionStatusFilter {
    fn status(self) -> Option<SuggestionStatus> {
        match self {
            Self::Pending => Some(SuggestionStatus::Pending),
            Self::Accepted => Some(SuggestionStatus::Accepted),
            Self::Rejected => Some(SuggestionStatus::Rejected),
            Self::Modified => Some(SuggestionStatus::Modified),
            Self::Stale => Some(SuggestionStatus::Stale),
            Self::All => None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SuggestionListQuery {
    #[serde(default)]
    pub status: SuggestionStatusFilter,
    #[serde(default)]
    pub bucket: Option<SuggestionBucket>,
    #[serde(default)]
    pub library_id: Option<Uuid>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct SeriesSuggestionQuery {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
}

#[derive(Debug, Default, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct AcceptRelationshipSuggestionReq {
    /// Accept as a different kind (read "from `kind` to"). Omit to accept
    /// the suggested kind. A different kind records the suggestion as
    /// `modified`.
    #[garde(skip)]
    #[serde(default)]
    pub kind: Option<RelationshipKind>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AcceptRelationshipSuggestionResp {
    pub suggestion: RelationshipSuggestionView,
    /// The `from → to` edge row (pass to
    /// `DELETE /series/{slug}/relationships/{id}` to undo).
    pub relationship_id: String,
    pub inverse_id: String,
    /// The kind created (`from kind to`).
    pub kind: RelationshipKind,
    /// `false` when the edge already existed (nothing new was inserted).
    pub created: bool,
}

#[derive(Debug, Deserialize)]
pub struct RunQuery {
    #[serde(default)]
    pub library_id: Option<Uuid>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RunRelationshipSuggestionsResp {
    /// Libraries a run was queued for.
    pub enqueued: Vec<String>,
    /// Libraries skipped because a run was already queued.
    pub already_queued: Vec<String>,
}

/// Bulk accept: explicit `ids`, **or** a bucket selection. Exactly one of
/// `ids` / `bucket` must be set.
///
/// - `ids`: 1–500 suggestion ids, processed in order (duplicates once).
/// - `bucket`: only `"high"` — the pending high-confidence rows (optionally
///   one `library_id`), highest confidence first, at most 500 per request.
///   The response's `remaining` says how many are left; send the request
///   again to take the next batch. Medium / low rows are only bulk-accepted
///   by explicit ids, after a reviewer has looked at them.
#[derive(Debug, Default, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct BulkAcceptRelationshipSuggestionsReq {
    #[garde(length(min = 1, max = suggestions::MAX_BULK))]
    #[serde(default)]
    pub ids: Option<Vec<Uuid>>,
    #[garde(skip)]
    #[serde(default)]
    pub bucket: Option<SuggestionBucket>,
    /// Bucket mode only: restrict to one library.
    #[garde(skip)]
    #[serde(default)]
    pub library_id: Option<Uuid>,
}

/// Bulk reject: 1–500 explicit ids (no bucket mode — a reviewer picks them).
#[derive(Debug, Default, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct BulkRejectRelationshipSuggestionsReq {
    #[garde(length(min = 1, max = suggestions::MAX_BULK))]
    pub ids: Vec<Uuid>,
}

/// Why one item of a bulk request was skipped.
#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BulkReviewFailureCode {
    NotFound,
    /// Already accepted / rejected / modified.
    AlreadyReviewed,
    /// The engine no longer proposes it.
    Stale,
    /// Contradicts an existing directional relationship.
    Conflict,
    /// `create_pair` refused for another reason.
    Invalid,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkReviewFailureView {
    pub id: String,
    pub code: BulkReviewFailureCode,
    pub message: String,
}

/// Result of one bulk batch. The batch commits as a whole; per-item
/// refusals land in `failed` instead of failing the request.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkReviewRelationshipSuggestionsResp {
    /// Distinct ids processed.
    pub requested: u32,
    /// Ids accepted (or rejected), in processing order.
    pub succeeded: Vec<String>,
    pub failed: Vec<BulkReviewFailureView>,
    /// Accept only: edge pairs newly inserted (an accept whose edge
    /// already existed doesn't count).
    pub created: u32,
    /// Bucket mode only: pending rows still in the bucket after this batch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<u64>,
}

/// `POST /{id}/reopen` result: the row, now pending again.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ReopenRelationshipSuggestionResp {
    pub suggestion: RelationshipSuggestionView,
}

const DEFAULT_LIMIT: u64 = 50;

#[utoipa::path(
    operation_id = "relationship_suggestions_list", get,
    path = "/admin/relationship-suggestions",
    params(
        ("status" = Option<String>, Query, description = "`pending` (default), `accepted`, `rejected`, `modified`, `stale`, or `all` (every status except `stale`)"),
        ("bucket" = Option<String>, Query, description = "`high`, `medium` or `low`"),
        ("library_id" = Option<String>, Query, description = "only suggestions in this library"),
        ("cursor" = Option<String>, Query,),
        ("limit" = Option<u64>, Query, description = "1..=200, default 50"),
    ),
    responses(
        (status = 200, body = RelationshipSuggestionListView),
        (status = 400, description = "bad cursor or query"),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn list(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<SuggestionListQuery>,
) -> Response {
    let filter = SuggestionFilter {
        status: q.status.status(),
        bucket: q.bucket,
        library_id: q.library_id,
        series_id: None,
    };
    list_page(&app, &filter, q.cursor.as_deref(), q.limit).await
}

#[utoipa::path(
    operation_id = "relationship_suggestions_for_series", get,
    path = "/series/{slug}/relationship-suggestions",
    params(
        ("slug" = String, Path),
        ("cursor" = Option<String>, Query,),
        ("limit" = Option<u64>, Query, description = "1..=200, default 50"),
    ),
    responses(
        (status = 200, body = RelationshipSuggestionListView, description = "pending suggestions with this series on either end"),
        (status = 400, description = "bad cursor"),
        (status = 403, description = "admin only"),
        (status = 404, description = "series not found"),
    )
)]
#[handler]
pub async fn list_for_series(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Path(slug): Path<String>,
    Query(q): Query<SeriesSuggestionQuery>,
) -> Response {
    let s = match super::series::find_by_slug(&app.db, &slug).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let filter = SuggestionFilter {
        status: Some(SuggestionStatus::Pending),
        series_id: Some(s.id),
        ..Default::default()
    };
    list_page(&app, &filter, q.cursor.as_deref(), q.limit).await
}

async fn list_page(
    app: &AppState,
    filter: &SuggestionFilter,
    cursor: Option<&str>,
    limit: Option<u64>,
) -> Response {
    let cursor = match cursor {
        None => None,
        Some(raw) => match decode_cursor::<SuggestionCursor>(raw) {
            Ok(c) => Some(c),
            Err(_) => {
                return respond(
                    StatusCode::BAD_REQUEST,
                    ApiErrorCode::BadCursor,
                    "invalid cursor",
                );
            }
        },
    };
    let page =
        match suggestions::list(&app.db, filter, cursor, limit.unwrap_or(DEFAULT_LIMIT)).await {
            Ok(p) => p,
            Err(e) => return internal(&e),
        };
    let items = match hydrate(app, page.items).await {
        Ok(v) => v,
        Err(e) => return internal(&e),
    };
    let next_cursor = page.next_cursor.and_then(|c| encode_cursor(&c).ok());
    Json(RelationshipSuggestionListView {
        items,
        next_cursor,
        total: page.total,
        bucket_counts: page.bucket_counts.map(bucket_counts),
    })
    .into_response()
}

fn bucket_counts(m: BTreeMap<String, u64>) -> SuggestionBucketCounts {
    SuggestionBucketCounts {
        high: m.get("high").copied().unwrap_or(0),
        medium: m.get("medium").copied().unwrap_or(0),
        low: m.get("low").copied().unwrap_or(0),
    }
}

#[utoipa::path(
    operation_id = "relationship_suggestions_accept", post,
    path = "/admin/relationship-suggestions/{id}/accept",
    params(("id" = String, Path, description = "suggestion id")),
    request_body = AcceptRelationshipSuggestionReq,
    responses(
        (status = 200, body = AcceptRelationshipSuggestionResp, description = "edge pair created (or already present) and suggestion marked accepted / modified"),
        (status = 400, description = "malformed id"),
        (status = 403, description = "admin only"),
        (status = 404, description = "suggestion not found"),
        (status = 409, description = "already reviewed, or contradicts an existing relationship"),
    )
)]
#[handler]
pub async fn accept(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
    Validated(req): Validated<AcceptRelationshipSuggestionReq>,
) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return bad_id();
    };
    let out = match suggestions::accept(&app.db, id, actor.id, req.kind).await {
        Ok(o) => o,
        Err(e) => return review_error(e),
    };
    if out.pair.created {
        // WP-7.4: relationships are a similar-series signal. The service
        // fn only has a connection, so every caller that accepts (this
        // handler, WP-7.3's bulk accept) invalidates after it commits.
        app.similarity.invalidate_all();
    }
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.relationship_suggestion.accept",
        target = ("relationship_suggestion", id.to_string()),
        payload = serde_json::json!({
            "from_series_id": out.suggestion.from_series_id.to_string(),
            "to_series_id": out.suggestion.to_series_id.to_string(),
            "suggested_kind": out.suggestion.kind,
            "kind": out.kind.as_str(),
            "status": out.suggestion.status,
            "confidence": out.suggestion.confidence,
            "relationship_id": out.pair.forward.id.to_string(),
            "inverse_id": out.pair.inverse.id.to_string(),
            "created": out.pair.created,
        }),
    );
    let relationship_id = out.pair.forward.id.to_string();
    let inverse_id = out.pair.inverse.id.to_string();
    let created = out.pair.created;
    let kind = out.kind;
    let view = match hydrate(&app, vec![out.suggestion]).await {
        Ok(mut v) if !v.is_empty() => v.remove(0),
        Ok(_) => return readback_failed(),
        Err(e) => return internal(&e),
    };
    Json(AcceptRelationshipSuggestionResp {
        suggestion: view,
        relationship_id,
        inverse_id,
        kind,
        created,
    })
    .into_response()
}

#[utoipa::path(
    operation_id = "relationship_suggestions_reject", post,
    path = "/admin/relationship-suggestions/{id}/reject",
    params(("id" = String, Path, description = "suggestion id")),
    responses(
        (status = 200, body = RelationshipSuggestionView, description = "suggestion marked rejected; it will not be proposed again"),
        (status = 400, description = "malformed id"),
        (status = 403, description = "admin only"),
        (status = 404, description = "suggestion not found"),
        (status = 409, description = "already reviewed"),
    )
)]
#[handler]
pub async fn reject(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return bad_id();
    };
    let row = match suggestions::reject(&app.db, id, actor.id).await {
        Ok(r) => r,
        Err(e) => return review_error(e),
    };
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.relationship_suggestion.reject",
        target = ("relationship_suggestion", id.to_string()),
        payload = serde_json::json!({
            "from_series_id": row.from_series_id.to_string(),
            "to_series_id": row.to_series_id.to_string(),
            "kind": row.kind,
            "confidence": row.confidence,
        }),
    );
    match hydrate(&app, vec![row]).await {
        Ok(mut v) if !v.is_empty() => Json(v.remove(0)).into_response(),
        Ok(_) => readback_failed(),
        Err(e) => internal(&e),
    }
}

#[utoipa::path(
    operation_id = "relationship_suggestions_reopen", post,
    path = "/admin/relationship-suggestions/{id}/reopen",
    params(("id" = String, Path, description = "suggestion id")),
    responses(
        (status = 200, body = ReopenRelationshipSuggestionResp, description = "rejection cleared: the suggestion is pending again"),
        (status = 400, description = "malformed id"),
        (status = 403, description = "admin only"),
        (status = 404, description = "suggestion not found"),
        (status = 409, description = "not rejected"),
    )
)]
#[handler]
pub async fn reopen(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return bad_id();
    };
    let (before, after) = match suggestions::reopen(&app.db, id).await {
        Ok(r) => r,
        Err(e) => return review_error(e),
    };
    // The row's review stamp is cleared; the audit row keeps it.
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.relationship_suggestion.reopen",
        target = ("relationship_suggestion", id.to_string()),
        payload = serde_json::json!({
            "from_series_id": after.from_series_id.to_string(),
            "to_series_id": after.to_series_id.to_string(),
            "kind": after.kind,
            "previous_status": before.status,
            "rejected_at": before.reviewed_at.map(|t| t.to_rfc3339()),
            "rejected_by": before.reviewed_by.map(|u| u.to_string()),
        }),
    );
    match hydrate(&app, vec![after]).await {
        Ok(mut v) if !v.is_empty() => Json(ReopenRelationshipSuggestionResp {
            suggestion: v.remove(0),
        })
        .into_response(),
        Ok(_) => readback_failed(),
        Err(e) => internal(&e),
    }
}

#[utoipa::path(
    operation_id = "relationship_suggestions_bulk_accept", post,
    path = "/admin/relationship-suggestions/bulk-accept",
    request_body = BulkAcceptRelationshipSuggestionsReq,
    responses(
        (status = 200, body = BulkReviewRelationshipSuggestionsResp, description = "batch committed; per-item refusals listed in `failed`"),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
        (status = 422, description = "neither or both of `ids` / `bucket`, a bucket other than `high`, `library_id` without `bucket`, or more than 500 ids"),
    )
)]
#[handler]
pub async fn bulk_accept(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Validated(req): Validated<BulkAcceptRelationshipSuggestionsReq>,
) -> Response {
    if let Err(resp) = validate_bulk_accept(&req) {
        return resp;
    }
    let (ids, mode) = match (&req.ids, req.bucket) {
        (Some(ids), _) => (ids.clone(), "ids"),
        (None, Some(bucket)) => {
            if let Some(lib) = req.library_id {
                match library::Entity::find_by_id(lib).one(&app.db).await {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        return respond(
                            StatusCode::NOT_FOUND,
                            ApiErrorCode::NotFound,
                            "library not found",
                        );
                    }
                    Err(e) => return internal(&e),
                }
            }
            match suggestions::pending_ids_in_bucket(
                &app.db,
                bucket,
                req.library_id,
                suggestions::MAX_BULK,
            )
            .await
            {
                Ok((ids, _)) => (ids, "bucket"),
                Err(e) => return internal(&e),
            }
        }
        (None, None) => unreachable!("validate_bulk_accept requires ids or bucket"),
    };
    let out = match suggestions::bulk_accept(&app.db, &ids, actor.id).await {
        Ok(o) => o,
        Err(e) => return internal(&e),
    };
    if out.created > 0 {
        // One invalidation per batch (WP-7.4 similar series).
        app.similarity.invalidate_all();
    }
    let remaining = match req.bucket {
        Some(bucket) if req.ids.is_none() => {
            match suggestions::pending_ids_in_bucket(&app.db, bucket, req.library_id, 0).await {
                Ok((_, total)) => Some(total),
                Err(e) => return internal(&e),
            }
        }
        _ => None,
    };
    let resp = bulk_resp(&ids, &out, remaining);
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.relationship_suggestion.bulk_accept",
        payload = serde_json::json!({
            "mode": mode,
            "bucket": req.bucket.map(SuggestionBucket::as_str),
            "library_id": req.library_id.map(|l| l.to_string()),
            "requested": resp.requested,
            "accepted": resp.succeeded.len(),
            "created": resp.created,
            "failed": resp.failed.len(),
            "accepted_ids": resp.succeeded,
            "failures": resp.failed.iter().map(|f| serde_json::json!({
                "id": f.id, "code": f.code,
            })).collect::<Vec<_>>(),
            "remaining": resp.remaining,
        }),
    );
    Json(resp).into_response()
}

#[utoipa::path(
    operation_id = "relationship_suggestions_bulk_reject", post,
    path = "/admin/relationship-suggestions/bulk-reject",
    request_body = BulkRejectRelationshipSuggestionsReq,
    responses(
        (status = 200, body = BulkReviewRelationshipSuggestionsResp, description = "batch committed; per-item refusals listed in `failed`"),
        (status = 403, description = "admin only"),
        (status = 422, description = "no ids, or more than 500"),
    )
)]
#[handler]
pub async fn bulk_reject(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Validated(req): Validated<BulkRejectRelationshipSuggestionsReq>,
) -> Response {
    let out = match suggestions::bulk_reject(&app.db, &req.ids, actor.id).await {
        Ok(o) => o,
        Err(e) => return internal(&e),
    };
    let resp = bulk_resp(&req.ids, &out, None);
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.relationship_suggestion.bulk_reject",
        payload = serde_json::json!({
            "requested": resp.requested,
            "rejected": resp.succeeded.len(),
            "failed": resp.failed.len(),
            "rejected_ids": resp.succeeded,
            "failures": resp.failed.iter().map(|f| serde_json::json!({
                "id": f.id, "code": f.code,
            })).collect::<Vec<_>>(),
        }),
    );
    Json(resp).into_response()
}

/// Cross-field rules for [`bulk_accept`] (garde covers the id count).
#[allow(clippy::result_large_err)]
fn validate_bulk_accept(req: &BulkAcceptRelationshipSuggestionsReq) -> Result<(), Response> {
    let field = |field: &str, message: &str| shared::error::FieldError {
        field: field.to_owned(),
        message: message.to_owned(),
    };
    let mut errs = Vec::new();
    match (&req.ids, req.bucket) {
        (Some(_), Some(_)) => errs.push(field("bucket", "send either `ids` or `bucket`, not both")),
        (None, None) => errs.push(field("ids", "send `ids` or `bucket`")),
        (None, Some(b)) if b != SuggestionBucket::High => errs.push(field(
            "bucket",
            "only the `high` bucket can be bulk-accepted; pick medium / low rows by id",
        )),
        _ => {}
    }
    if req.library_id.is_some() && req.ids.is_some() {
        errs.push(field("library_id", "`library_id` only applies to `bucket`"));
    }
    if errs.is_empty() {
        return Ok(());
    }
    let summary = errs
        .iter()
        .map(|e| format!("{}: {}", e.field, e.message))
        .collect::<Vec<_>>()
        .join("; ");
    Err(super::respond_with_field_errors(
        StatusCode::UNPROCESSABLE_ENTITY,
        ApiErrorCode::Validation,
        summary,
        errs,
    ))
}

fn bulk_resp(
    ids: &[Uuid],
    out: &suggestions::BulkOutcome,
    remaining: Option<u64>,
) -> BulkReviewRelationshipSuggestionsResp {
    let distinct: HashSet<&Uuid> = ids.iter().take(suggestions::MAX_BULK).collect();
    BulkReviewRelationshipSuggestionsResp {
        requested: u32::try_from(distinct.len()).unwrap_or(u32::MAX),
        succeeded: out.succeeded.iter().map(|r| r.id.to_string()).collect(),
        failed: out
            .failed
            .iter()
            .map(|f| BulkReviewFailureView {
                id: f.id.to_string(),
                code: failure_code(&f.error),
                message: f.error.to_string(),
            })
            .collect(),
        created: u32::try_from(out.created).unwrap_or(u32::MAX),
        remaining,
    }
}

fn failure_code(e: &ReviewError) -> BulkReviewFailureCode {
    match e {
        ReviewError::NotFound => BulkReviewFailureCode::NotFound,
        ReviewError::AlreadyReviewed {
            status: SuggestionStatus::Stale,
        } => BulkReviewFailureCode::Stale,
        ReviewError::AlreadyReviewed { .. } => BulkReviewFailureCode::AlreadyReviewed,
        ReviewError::Pair(crate::relationships::PairError::Conflict { .. }) => {
            BulkReviewFailureCode::Conflict
        }
        ReviewError::Pair(_) | ReviewError::Db(_) => BulkReviewFailureCode::Invalid,
    }
}

#[utoipa::path(
    operation_id = "relationship_suggestions_run", post,
    path = "/admin/relationship-suggestions/run",
    params(("library_id" = Option<String>, Query, description = "one library; omit for every library")),
    responses(
        (status = 202, body = RunRelationshipSuggestionsResp, description = "runs queued"),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
    )
)]
#[handler]
pub async fn run(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Query(q): Query<RunQuery>,
) -> Response {
    let libraries: Vec<Uuid> = match q.library_id {
        Some(id) => match library::Entity::find_by_id(id).one(&app.db).await {
            Ok(Some(l)) => vec![l.id],
            Ok(None) => {
                return respond(
                    StatusCode::NOT_FOUND,
                    ApiErrorCode::NotFound,
                    "library not found",
                );
            }
            Err(e) => return internal(&e),
        },
        None => match library::Entity::find()
            .order_by_asc(library::Column::Name)
            .all(&app.db)
            .await
        {
            Ok(v) => v.into_iter().map(|l| l.id).collect(),
            Err(e) => return internal(&e),
        },
    };
    let mut enqueued = Vec::new();
    let mut already_queued = Vec::new();
    for lib in libraries {
        if crate::jobs::relationship_suggest::enqueue(&app, lib).await {
            enqueued.push(lib.to_string());
        } else {
            already_queued.push(lib.to_string());
        }
    }
    record_admin_action!(
        db = &app.db,
        ctx = &ctx,
        actor = actor.id,
        action = "admin.relationship_suggestion.run",
        payload = serde_json::json!({
            "library_id": q.library_id.map(|l| l.to_string()),
            "enqueued": enqueued,
            "already_queued": already_queued,
        }),
    );
    (
        StatusCode::ACCEPTED,
        Json(RunRelationshipSuggestionsResp {
            enqueued,
            already_queued,
        }),
    )
        .into_response()
}

/// Rows → views with both series hydrated in one batch. A row whose series
/// vanished (can't happen under the FK cascade, but a race with a delete
/// could) is dropped.
async fn hydrate(
    app: &AppState,
    rows: Vec<sug::Model>,
) -> Result<Vec<RelationshipSuggestionView>, sea_orm::DbErr> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let ids: HashSet<Uuid> = rows
        .iter()
        .flat_map(|r| [r.from_series_id, r.to_series_id])
        .collect();
    let models = series::Entity::find()
        .filter(series::Column::Id.is_in(ids.into_iter().collect::<Vec<_>>()))
        .all(&app.db)
        .await?;
    let views: HashMap<String, SeriesView> = super::series::hydrate_series(app, models)
        .await
        .into_iter()
        .map(|v| (v.id.clone(), v))
        .collect();
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let kind: RelationshipKind = r.kind.parse().ok()?;
            Some(RelationshipSuggestionView {
                id: r.id.to_string(),
                from_series: views.get(&r.from_series_id.to_string())?.clone(),
                to_series: views.get(&r.to_series_id.to_string())?.clone(),
                kind,
                kind_label: kind.label().to_owned(),
                confidence: r.confidence,
                bucket: r.bucket.parse().unwrap_or(SuggestionBucket::Low),
                reason: r.reason,
                evidence: r.evidence,
                status: r.status.parse().unwrap_or(SuggestionStatus::Pending),
                accepted_kind: r.accepted_kind.as_deref().and_then(|k| k.parse().ok()),
                created_at: r.created_at.to_rfc3339(),
                updated_at: r.updated_at.to_rfc3339(),
                reviewed_at: r.reviewed_at.map(|t| t.to_rfc3339()),
                reviewed_by: r.reviewed_by.map(|u| u.to_string()),
            })
        })
        .collect())
}

fn review_error(e: ReviewError) -> Response {
    match e {
        ReviewError::NotFound => respond(
            StatusCode::NOT_FOUND,
            ApiErrorCode::NotFound,
            "suggestion not found",
        ),
        ReviewError::AlreadyReviewed { .. } => {
            respond(StatusCode::CONFLICT, ApiErrorCode::Conflict, e.to_string())
        }
        ReviewError::Pair(crate::relationships::PairError::Conflict { .. }) => {
            respond(StatusCode::CONFLICT, ApiErrorCode::Conflict, e.to_string())
        }
        ReviewError::Pair(_) => respond(
            StatusCode::UNPROCESSABLE_ENTITY,
            ApiErrorCode::Validation,
            e.to_string(),
        ),
        ReviewError::Db(d) => internal(&d),
    }
}

fn bad_id() -> Response {
    respond(
        StatusCode::BAD_REQUEST,
        ApiErrorCode::Validation,
        "invalid suggestion id",
    )
}

fn readback_failed() -> Response {
    respond(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorCode::Internal,
        "suggestion saved but readback failed",
    )
}

fn internal(e: &sea_orm::DbErr) -> Response {
    tracing::error!(error = %e, "relationship suggestion query failed");
    respond(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorCode::Internal,
        "internal",
    )
}
