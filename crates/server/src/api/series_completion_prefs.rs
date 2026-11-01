//! `/me/series/{series_id}/completion-prefs` — whether annuals and
//! specials count toward one series' collection completeness for the
//! caller (remediation B9, owner decision D-7).
//!
//! Collection completeness compares the owned main run
//! (`issues.special_type IS NULL`) with `series.total_issues`. The
//! account setting `users.count_specials_in_completeness` (default
//! false) adds the series' tagged annuals / one-shots / specials / TPBs
//! to the owned count; a row in `user_series_completion_prefs` overrides
//! it for one series in either direction. No row = inherit.
//!
//! Every completeness surface resolves the same way through
//! [`CompletionPrefs`] (series cards and detail via `hydrate_series` /
//! `get_one`, the collection report) or the equivalent SQL in
//! `views::compile` (the `collection_completeness` filter):
//! `COALESCE(override, account setting)`.

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::user::UserRole;
use entity::{series, user, user_series_completion_prefs as prefs};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QuerySelect, Set};
use serde::{Deserialize, Serialize};
use server_macros::handler;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::api::extractors::Validated;
use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_one, put))
}

/// The caller's per-series choice. `inherit` (no stored row) follows the
/// account setting; `include` / `exclude` override it for this series.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SpecialsCompletion {
    #[default]
    Inherit,
    Include,
    Exclude,
}

impl SpecialsCompletion {
    fn from_override(v: Option<bool>) -> Self {
        match v {
            None => Self::Inherit,
            Some(true) => Self::Include,
            Some(false) => Self::Exclude,
        }
    }

    fn as_override(self) -> Option<bool> {
        match self {
            Self::Inherit => None,
            Self::Include => Some(true),
            Self::Exclude => Some(false),
        }
    }
}

/// The account setting plus the caller's overrides for a batch of series.
/// One query each, whatever the batch size.
#[derive(Debug, Clone, Default)]
pub(crate) struct CompletionPrefs {
    account_default: bool,
    overrides: HashMap<Uuid, bool>,
}

impl CompletionPrefs {
    /// Load the viewer's setting and their overrides for `series_ids`.
    /// A lookup failure soft-fails to the built-in default (specials
    /// excluded, no overrides) — the same figures as before B9.
    pub(crate) async fn load<C: ConnectionTrait>(
        db: &C,
        viewer: Uuid,
        series_ids: &[Uuid],
    ) -> Self {
        let account_default = user::Entity::find_by_id(viewer)
            .select_only()
            .column(user::Column::CountSpecialsInCompleteness)
            .into_tuple::<bool>()
            .one(db)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "completion prefs: account setting lookup failed");
                None
            })
            .unwrap_or(false);
        let overrides = if series_ids.is_empty() {
            HashMap::new()
        } else {
            prefs::Entity::find()
                .filter(prefs::Column::UserId.eq(viewer))
                .filter(prefs::Column::SeriesId.is_in(series_ids.iter().copied()))
                .select_only()
                .column(prefs::Column::SeriesId)
                .column(prefs::Column::CountSpecials)
                .into_tuple::<(Uuid, bool)>()
                .all(db)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "completion prefs: override lookup failed");
                    Vec::new()
                })
                .into_iter()
                .collect()
        };
        Self {
            account_default,
            overrides,
        }
    }

    /// Whether the series' specials count toward its completion for the
    /// viewer: the per-series override when set, else the account setting.
    pub(crate) fn counts_specials(&self, series_id: Uuid) -> bool {
        self.overrides
            .get(&series_id)
            .copied()
            .unwrap_or(self.account_default)
    }

    /// The viewer's per-series choice (`inherit` when none is stored).
    pub(crate) fn mode(&self, series_id: Uuid) -> SpecialsCompletion {
        SpecialsCompletion::from_override(self.overrides.get(&series_id).copied())
    }
}

/// The caller's choice for one series and what it resolves to.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SeriesCompletionPrefsView {
    /// `inherit` | `include` | `exclude`.
    pub count_specials: SpecialsCompletion,
    /// The resolved setting: true when this series' annuals and specials
    /// count toward its completion for the caller.
    pub effective: bool,
}

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct SeriesCompletionPrefsReq {
    /// `inherit` clears the override. An unknown value is a 422 at
    /// deserialisation.
    #[garde(skip)]
    pub count_specials: SpecialsCompletion,
}

/// Visible series row, or the 404 a hidden one gets (no existence leak).
/// A soft-deleted series is gone for readers; admins can still open it
/// (`series::get_one` ACL widening), so its ⋯ menu control works too.
async fn visible_series(
    app: &AppState,
    user: &CurrentUser,
    series_id: Uuid,
) -> Result<series::Model, Response> {
    match series::Entity::find_by_id(series_id).one(&app.db).await {
        Ok(Some(row))
            if (row.removed_at.is_none() || user.role == UserRole::Admin)
                && access::series_visible(app, user, &row).await =>
        {
            Ok(row)
        }
        Ok(_) => Err(error(
            StatusCode::NOT_FOUND,
            "not_found",
            "series not found",
        )),
        Err(e) => {
            tracing::error!(error = %e, "completion prefs: series lookup failed");
            Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ))
        }
    }
}

async fn view_for(app: &AppState, user_id: Uuid, series_id: Uuid) -> SeriesCompletionPrefsView {
    let prefs = CompletionPrefs::load(&app.db, user_id, &[series_id]).await;
    SeriesCompletionPrefsView {
        count_specials: prefs.mode(series_id),
        effective: prefs.counts_specials(series_id),
    }
}

/// Whether annuals and specials count toward this series' completion for
/// the caller.
#[utoipa::path(
    operation_id = "series_completion_prefs_get",    get,
    path = "/me/series/{series_id}/completion-prefs",
    params(("series_id" = String, Path,)),
    responses(
        (status = 200, body = SeriesCompletionPrefsView),
        (status = 401, description = "not authenticated"),
        (status = 404, description = "series not found")
    )
)]
#[handler]
pub async fn get_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(series_id): AxPath<Uuid>,
) -> Response {
    if let Err(resp) = visible_series(&app, &user, series_id).await {
        return resp;
    }
    Json(view_for(&app, user.id, series_id).await).into_response()
}

/// Set (`include` / `exclude`) or clear (`inherit`) the caller's override
/// for this series.
#[utoipa::path(
    operation_id = "series_completion_prefs_put",    put,
    path = "/me/series/{series_id}/completion-prefs",
    params(("series_id" = String, Path,)),
    request_body = SeriesCompletionPrefsReq,
    responses(
        (status = 200, body = SeriesCompletionPrefsView),
        (status = 401, description = "not authenticated"),
        (status = 404, description = "series not found"),
        (status = 422, description = "unknown value")
    )
)]
#[handler]
pub async fn put(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(series_id): AxPath<Uuid>,
    Validated(req): Validated<SeriesCompletionPrefsReq>,
) -> Response {
    if let Err(resp) = visible_series(&app, &user, series_id).await {
        return resp;
    }
    let write = async {
        match req.count_specials.as_override() {
            None => {
                prefs::Entity::delete_by_id((user.id, series_id))
                    .exec(&app.db)
                    .await?;
            }
            Some(v) => {
                let am = prefs::ActiveModel {
                    user_id: Set(user.id),
                    series_id: Set(series_id),
                    count_specials: Set(v),
                    updated_at: Set(Utc::now().fixed_offset()),
                };
                prefs::Entity::insert(am)
                    .on_conflict(
                        sea_orm::sea_query::OnConflict::columns([
                            prefs::Column::UserId,
                            prefs::Column::SeriesId,
                        ])
                        .update_columns([prefs::Column::CountSpecials, prefs::Column::UpdatedAt])
                        .to_owned(),
                    )
                    .exec(&app.db)
                    .await?;
            }
        }
        Ok::<_, sea_orm::DbErr>(())
    };
    if let Err(e) = write.await {
        tracing::error!(error = %e, "completion prefs: write failed");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
    }
    Json(view_for(&app, user.id, series_id).await).into_response()
}
