//! `/me/series/{series_id}/reader-prefs` — the caller's per-series reader
//! overrides (post-audit T4.4, decision D9a).
//!
//! The reader keeps four choices per series: reading direction, fit mode,
//! view mode and cover solo. They used to live only in the browser's
//! localStorage, so they never followed the account to another device.
//! This surface stores them on the server; the web store mirrors the
//! server row into localStorage when the reader opens (so the offline
//! reader still has them) and pushes every change back.
//!
//! Resolution in the reader is `server per-series > localStorage
//! per-series > user default > built-in`. A slice is "no override" when
//! it is NULL / absent; `PUT` takes PATCH semantics (absent = leave,
//! `null` = clear) and a row whose every slice is cleared is deleted.
//! Page strip and page animation stay account-wide (D9a).

use axum::{
    Json,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::{series, user_series_reader_prefs as prefs};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::api::form_or_json::FormOrJson;
use crate::auth::CurrentUser;
use crate::auth::preferences::{FitMode, ReadingDirection, ViewMode};
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_one, put, delete_one))
}

/// The caller's overrides for one series. A missing field is "no
/// override" — the reader falls through to the account default.
#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct SeriesReaderPrefsView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reading_direction: Option<ReadingDirection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fit_mode: Option<FitMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_mode: Option<ViewMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_solo: Option<bool>,
}

impl SeriesReaderPrefsView {
    fn from_row(row: &prefs::Model) -> Self {
        Self {
            reading_direction: row.reading_direction.as_deref().and_then(parse_token),
            fit_mode: row.fit_mode.as_deref().and_then(parse_token),
            view_mode: row.view_mode.as_deref().and_then(parse_token),
            cover_solo: row.cover_solo,
        }
    }

    fn is_empty(&self) -> bool {
        self.reading_direction.is_none()
            && self.fit_mode.is_none()
            && self.view_mode.is_none()
            && self.cover_solo.is_none()
    }
}

/// PATCH semantics: an absent field leaves the slice alone, `null`
/// clears it, a value sets it.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct SeriesReaderPrefsPatch {
    #[serde(default, deserialize_with = "deserialize_some")]
    pub reading_direction: Option<Option<ReadingDirection>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub fit_mode: Option<Option<FitMode>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub view_mode: Option<Option<ViewMode>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub cover_solo: Option<Option<bool>>,
}

/// Present-but-null deserialises to `Some(None)` so PATCH semantics work.
fn deserialize_some<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// The DB keeps the enums' wire tokens; an unknown token (a future
/// variant rolled back, a hand edit) reads as "no override".
fn parse_token<T: serde::de::DeserializeOwned>(s: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).ok()
}

fn token<T: Serialize>(v: &T) -> Option<String> {
    serde_json::to_value(v)
        .ok()
        .and_then(|j| j.as_str().map(str::to_owned))
}

/// The caller's reader overrides for a series (empty object when none).
#[utoipa::path(
    operation_id = "series_reader_prefs_get",    get,
    path = "/me/series/{series_id}/reader-prefs",
    params(("series_id" = String, Path,)),
    responses(
        (status = 200, body = SeriesReaderPrefsView),
        (status = 401, description = "not authenticated")
    )
)]
#[handler]
pub async fn get_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(series_id): AxPath<Uuid>,
) -> Response {
    match prefs::Entity::find_by_id((user.id, series_id))
        .one(&app.db)
        .await
    {
        Ok(Some(row)) => Json(SeriesReaderPrefsView::from_row(&row)).into_response(),
        Ok(None) => Json(SeriesReaderPrefsView::default()).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "series reader prefs: lookup failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

/// Set or clear the caller's reader overrides for a series (PATCH
/// semantics per field). Clearing every slice removes the row.
#[utoipa::path(
    operation_id = "series_reader_prefs_put",    put,
    path = "/me/series/{series_id}/reader-prefs",
    params(("series_id" = String, Path,)),
    request_body = SeriesReaderPrefsPatch,
    responses(
        (status = 200, body = SeriesReaderPrefsView),
        (status = 401, description = "not authenticated"),
        (status = 404, description = "series not found"),
        (status = 422, description = "unknown preference token")
    )
)]
#[handler]
pub async fn put(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(series_id): AxPath<Uuid>,
    FormOrJson { data: req, .. }: FormOrJson<SeriesReaderPrefsPatch>,
) -> Response {
    match series::Entity::find_by_id(series_id).one(&app.db).await {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "series not found"),
        Err(e) => {
            tracing::error!(error = %e, "series reader prefs: series lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    }
    let existing = match prefs::Entity::find_by_id((user.id, series_id))
        .one(&app.db)
        .await
    {
        Ok(row) => row,
        Err(e) => {
            tracing::error!(error = %e, "series reader prefs: lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let mut next = existing
        .as_ref()
        .map(SeriesReaderPrefsView::from_row)
        .unwrap_or_default();
    if let Some(v) = req.reading_direction {
        next.reading_direction = v;
    }
    if let Some(v) = req.fit_mode {
        next.fit_mode = v;
    }
    if let Some(v) = req.view_mode {
        next.view_mode = v;
    }
    if let Some(v) = req.cover_solo {
        next.cover_solo = v;
    }

    let write = async {
        if next.is_empty() {
            if existing.is_some() {
                prefs::Entity::delete_by_id((user.id, series_id))
                    .exec(&app.db)
                    .await?;
            }
            return Ok::<_, sea_orm::DbErr>(());
        }
        let now = Utc::now().fixed_offset();
        let am = prefs::ActiveModel {
            user_id: Set(user.id),
            series_id: Set(series_id),
            reading_direction: Set(next.reading_direction.as_ref().and_then(token)),
            fit_mode: Set(next.fit_mode.as_ref().and_then(token)),
            view_mode: Set(next.view_mode.as_ref().and_then(token)),
            cover_solo: Set(next.cover_solo),
            updated_at: Set(now),
        };
        if existing.is_some() {
            am.update(&app.db).await?;
        } else {
            am.insert(&app.db).await?;
        }
        Ok(())
    };
    if let Err(e) = write.await {
        tracing::error!(error = %e, "series reader prefs: write failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    Json(next).into_response()
}

/// Remove every override for the series ("Reset this series to defaults").
#[utoipa::path(
    operation_id = "series_reader_prefs_delete",    delete,
    path = "/me/series/{series_id}/reader-prefs",
    params(("series_id" = String, Path,)),
    responses(
        (status = 204, description = "cleared (also when nothing was stored)"),
        (status = 401, description = "not authenticated")
    )
)]
#[handler]
pub async fn delete_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(series_id): AxPath<Uuid>,
) -> Response {
    match prefs::Entity::delete_by_id((user.id, series_id))
        .exec(&app.db)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "series reader prefs: delete failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}
