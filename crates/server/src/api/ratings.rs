//! Per-user ratings on issues and series (0..=5 in half-star steps).
//!
//! Two parallel endpoint pairs share the same persistence (`user_ratings`
//! table) and the same body shape: `{ "rating": 0.0..=5.0 | null }`. A null
//! rating clears the row. Half-star precision is enforced server-side so
//! the UI can't drift the schema by sending arbitrary floats.
//!
//! - `PUT /series/{slug}/rating`               — series rating
//! - `PUT /series/{slug}/issues/{slug}/rating` — per-issue rating
//!
//! The matching `GET /me/ratings/...` round-trips aren't strictly needed:
//! `GET /series/{slug}` and `GET /series/{slug}/issues/{slug}` already
//! surface the calling user's rating inline (`user_rating`).

use axum::{
    Json, Router,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::IntoResponse,
    routing::put,
};
use entity::user_rating;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};

use crate::auth::CurrentUser;
use crate::state::AppState;

pub const MIN_RATING: f64 = 0.0;
pub const MAX_RATING: f64 = 5.0;
/// Half-star precision: rating * 2 must be an integer.
pub const STEP: f64 = 0.5;

pub const TARGET_TYPE_ISSUE: &str = "issue";
pub const TARGET_TYPE_SERIES: &str = "series";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/series/{series_slug}/rating", put(set_series_rating))
        .route(
            "/series/{series_slug}/issues/{issue_slug}/rating",
            put(set_issue_rating),
        )
}

/// Body for both rating endpoints. `null` clears the rating.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetRatingReq {
    pub rating: Option<f64>,
}

/// Response shape — mirrors what the GET endpoints inline so callers don't
/// need to refetch to refresh local state.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RatingView {
    pub rating: Option<f64>,
}

/// Validate the rating is finite, within [0, 5], and on a half-star step.
pub fn validate_rating(rating: f64) -> Result<f64, &'static str> {
    if !rating.is_finite() {
        return Err("rating must be finite");
    }
    if !(MIN_RATING..=MAX_RATING).contains(&rating) {
        return Err("rating must be between 0 and 5");
    }
    let scaled = rating * 2.0;
    if (scaled - scaled.round()).abs() > 1e-6 {
        return Err("rating must be on a half-star step");
    }
    // Snap to the canonical half-step value so downstream comparisons /
    // SQL writes don't carry fp drift.
    Ok((scaled.round()) / 2.0)
}

#[utoipa::path(
    put,
    path = "/series/{series_slug}/rating",
    params(("series_slug" = String, Path,)),
    request_body = SetRatingReq,
    responses(
        (status = 200, body = RatingView),
        (status = 400, description = "validation error"),
        (status = 404, description = "series not found"),
    )
)]
pub async fn set_series_rating(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Json(req): Json<SetRatingReq>,
) -> impl IntoResponse {
    let series_row = match crate::api::series::find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !visible_in_library(&app, &user, series_row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "series not found");
    }
    write_rating(
        &app,
        user.id,
        TARGET_TYPE_SERIES,
        &series_row.id.to_string(),
        req.rating,
    )
    .await
}

#[utoipa::path(
    put,
    path = "/series/{series_slug}/issues/{issue_slug}/rating",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    request_body = SetRatingReq,
    responses(
        (status = 200, body = RatingView),
        (status = 400, description = "validation error"),
        (status = 404, description = "issue not found"),
    )
)]
pub async fn set_issue_rating(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
    Json(req): Json<SetRatingReq>,
) -> impl IntoResponse {
    let issue_row =
        match crate::api::issues::find_by_slugs(&app.db, &series_slug, &issue_slug).await {
            Ok(r) => r,
            Err(resp) => return resp,
        };
    if !visible_in_library(&app, &user, issue_row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }
    write_rating(&app, user.id, TARGET_TYPE_ISSUE, &issue_row.id, req.rating).await
}

/// Upsert (or delete on null) the rating. Validation rejects out-of-range
/// or non-half-step values so we never persist garbage.
async fn write_rating(
    app: &AppState,
    user_id: uuid::Uuid,
    target_type: &str,
    target_id: &str,
    rating: Option<f64>,
) -> axum::response::Response {
    if let Some(r) = rating {
        let snapped = match validate_rating(r) {
            Ok(v) => v,
            Err(msg) => {
                return error(StatusCode::BAD_REQUEST, "validation.rating", msg);
            }
        };
        let now = chrono::Utc::now().fixed_offset();
        // Manual upsert via find-then-insert/update — sea-orm's
        // `on_conflict` requires more boilerplate than this short path.
        let existing = user_rating::Entity::find()
            .filter(user_rating::Column::UserId.eq(user_id))
            .filter(user_rating::Column::TargetType.eq(target_type))
            .filter(user_rating::Column::TargetId.eq(target_id))
            .one(&app.db)
            .await;
        let result = match existing {
            Ok(Some(row)) => {
                let mut am: user_rating::ActiveModel = row.into();
                am.rating = Set(snapped);
                am.updated_at = Set(now);
                am.update(&app.db).await.map(|m| m.rating)
            }
            Ok(None) => {
                let am = user_rating::ActiveModel {
                    user_id: Set(user_id),
                    target_type: Set(target_type.to_owned()),
                    target_id: Set(target_id.to_owned()),
                    rating: Set(snapped),
                    created_at: Set(now),
                    updated_at: Set(now),
                };
                am.insert(&app.db).await.map(|m| m.rating)
            }
            Err(e) => {
                tracing::error!(error = %e, "rating lookup failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };
        match result {
            Ok(rating) => Json(RatingView {
                rating: Some(rating),
            })
            .into_response(),
            Err(e) => {
                tracing::error!(error = %e, "rating upsert failed");
                error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
            }
        }
    } else {
        // Clear: delete row, idempotent.
        if let Err(e) = user_rating::Entity::delete_many()
            .filter(user_rating::Column::UserId.eq(user_id))
            .filter(user_rating::Column::TargetType.eq(target_type))
            .filter(user_rating::Column::TargetId.eq(target_id))
            .exec(&app.db)
            .await
        {
            tracing::error!(error = %e, "rating clear failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
        Json(RatingView { rating: None }).into_response()
    }
}

async fn visible_in_library(app: &AppState, user: &CurrentUser, lib_id: uuid::Uuid) -> bool {
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

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
