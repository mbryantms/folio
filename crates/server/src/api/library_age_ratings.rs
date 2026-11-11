//! `GET /admin/libraries/age-ratings` — how each library's series spread
//! over the age-rating ladder, for the age-cap control on the admin user
//! page (audit U-09).
//!
//! One row per library: the number of series at each ladder rung (in
//! ladder order, every rung present, zeros included) and the number with
//! no rating. The web works out, for whatever cap and `hide_unrated`
//! choice the admin has selected, how many series a grant hides and how
//! many unrated series it shows, without a request per change.
//!
//! The counts follow the series-level cap rule exactly
//! ([`crate::library::access::VisibleLibraries::series_filter`]): only
//! active series (`removed_at IS NULL`, [`crate::library::access::active_series`]),
//! ranked from the stored `series.age_rating` after the same
//! [`age_rating::sql_norm`] normalisation and over the same spellings
//! (rungs and aliases, [`series_rating::ladder_values_sql`]). A rating
//! with no rank (NULL, empty, `Unknown`, off-ladder text) counts as
//! unrated. Issue-level ratings are not counted: an issue rated above the
//! cap inside a visible series is hidden too, and no series figure shows
//! that. `tests/library_age_ratings.rs` pins the figures to what
//! `GET /api/series` actually shows a capped reader.
//!
//! **Query.** One grouped statement over `series` for every library at
//! once: a whole-table aggregate, so it reads every series row whatever
//! the index (no index would narrow it); it runs once per admin user page
//! load. The query count is fixed: the admin check, the library rows, the
//! aggregate.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use entity::library;
use sea_orm::{EntityTrait, FromQueryResult, QueryOrder, Statement};
use serde::Serialize;
use std::collections::HashMap;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::auth::RequireAdmin;
use crate::library::age_rating::{self, LADDER};
use crate::library::series_rating;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(library_age_ratings))
}

/// Series at one ladder rung.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AgeRatingRungCount {
    /// The rung's canonical ladder spelling (a valid cap value).
    pub rating: String,
    /// Active series whose rating ranks at this rung, aliases included
    /// (`Mature` counts under `Mature 17+`).
    pub series_count: u64,
}

/// One library's series, by age rating.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LibraryAgeRatingsView {
    pub library_id: String,
    /// Every ladder rung, youngest audience first, zeros included.
    pub rungs: Vec<AgeRatingRungCount>,
    /// Active series with no rating the ladder ranks (none, `Unknown`,
    /// `Rating Pending` or a word the ladder does not know).
    pub unrated_count: u64,
    /// Every active series of the library: the rungs plus `unrated_count`.
    pub series_count: u64,
}

#[derive(FromQueryResult)]
struct Row {
    library_id: Uuid,
    rank: Option<i32>,
    n: i64,
}

#[utoipa::path(
    operation_id = "admin_library_age_ratings",    get,
    path = "/admin/libraries/age-ratings",
    responses(
        (status = 200, body = Vec<LibraryAgeRatingsView>),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn library_age_ratings(
    State(app): State<AppState>,
    _admin: RequireAdmin,
) -> impl IntoResponse {
    let libs = match library::Entity::find()
        .order_by_asc(library::Column::Name)
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "library age ratings: list libraries failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let sql = format!(
        "WITH ladder(norm, rank, canon) AS {ladder} \
         SELECT s.library_id, l.rank::int AS rank, COUNT(*)::bigint AS n \
           FROM series s \
           LEFT JOIN ladder l ON l.norm = {norm} \
          WHERE s.removed_at IS NULL \
          GROUP BY s.library_id, l.rank",
        ladder = series_rating::ladder_values_sql(),
        norm = age_rating::sql_norm("s.age_rating"),
    );
    let rows =
        match Row::find_by_statement(Statement::from_string(app.db.get_database_backend(), sql))
            .all(&app.db)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "library age ratings: aggregate failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };

    // library -> (count per rung, unrated)
    let mut by_lib: HashMap<Uuid, ([u64; LADDER.len()], u64)> = HashMap::new();
    for r in rows {
        let n = u64::try_from(r.n).unwrap_or(0);
        let entry = by_lib.entry(r.library_id).or_insert(([0; LADDER.len()], 0));
        match r.rank.and_then(|k| usize::try_from(k).ok()) {
            Some(k) if k < LADDER.len() => entry.0[k] += n,
            _ => entry.1 += n,
        }
    }

    let views: Vec<LibraryAgeRatingsView> = libs
        .iter()
        .map(|lib| {
            let (rungs, unrated) = by_lib
                .get(&lib.id)
                .copied()
                .unwrap_or(([0; LADDER.len()], 0));
            LibraryAgeRatingsView {
                library_id: lib.id.to_string(),
                rungs: LADDER
                    .iter()
                    .zip(rungs)
                    .map(|(rating, series_count)| AgeRatingRungCount {
                        rating: (*rating).to_owned(),
                        series_count,
                    })
                    .collect(),
                unrated_count: unrated,
                series_count: rungs.iter().sum::<u64>() + unrated,
            }
        })
        .collect();
    Json(views).into_response()
}
