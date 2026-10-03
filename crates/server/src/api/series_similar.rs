//! "Similar series" endpoints (WP-7.4).
//!
//!   - `GET /series/{slug}/similar` — the series page rail: the top
//!     content-based neighbours of one series, each with a "because"
//!     list of the shared entities that earned the match.
//!   - `GET /me/similar-series` — the optional home rail (system saved
//!     view `similar_series`): neighbours of the series the caller read
//!     most recently, minus everything they've already started.
//!
//! Scoring + the neighbour cache live in [`crate::similarity`]. The
//! cache holds unfiltered lists; this module applies, per request, the
//! library ACL + age-rating cap (`VisibleLibraries::series_ok`), drops
//! series the caller has hidden (a still-current `rail_dismissals` row
//! of kind `series`, same auto-restore rule as On Deck), and paginates
//! with an opaque keyset cursor over `(score DESC, series_id ASC)`.
//! Removed series never enter the cached list.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::series;
use sea_orm::{ColumnTrait, DbBackend, EntityTrait, FromQueryResult, QueryFilter, Statement};
use serde::{Deserialize, Serialize};
use shared::pagination::{decode_cursor, encode_cursor};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::api::series::{SeriesView, find_by_slug, hydrate_series};
use crate::auth::CurrentUser;
use crate::library::access::{self, VisibleLibraries};
use crate::similarity::{self, Neighbor, Reason};
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(similar))
        .routes(routes!(home_rail))
}

const DEFAULT_LIMIT: u64 = 20;
const MAX_LIMIT: u64 = 50;
/// Recently-read series tried as the home rail's seed before giving up
/// (a seed whose neighbours are all read / hidden / invisible yields
/// nothing, so fall through to the next one).
const MAX_SEED_ATTEMPTS: usize = 3;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct SimilarQuery {
    /// Opaque continuation token from a previous page's `next_cursor`.
    pub cursor: Option<String>,
    /// Page size, 1–50 (default 20).
    pub limit: Option<u64>,
}

/// One neighbour: the series card plus why it matched.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SimilarSeriesItem {
    pub series: SeriesView,
    /// Similarity score (sum of capped per-kind overlap; higher = closer).
    pub score: f64,
    /// The largest shared-entity contributions, strongest first.
    pub because: Vec<Reason>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SimilarSeriesListView {
    pub items: Vec<SimilarSeriesItem>,
    pub next_cursor: Option<String>,
    /// Visible neighbours across all pages — first page only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
}

/// The series the home rail's neighbours are drawn from.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SimilarSeed {
    pub id: String,
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SimilarSeriesRailView {
    /// `None` when the caller hasn't read anything with visible,
    /// unread neighbours yet (the rail hides itself).
    pub seed: Option<SimilarSeed>,
    pub items: Vec<SimilarSeriesItem>,
    pub next_cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
}

/// Keyset position: last returned `(score, id)`, plus the home rail's
/// seed so later pages don't switch seeds under the reader.
#[derive(Debug, Serialize, Deserialize)]
struct SimilarCursor {
    s: f64,
    id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    seed: Option<Uuid>,
}

fn internal(e: impl std::fmt::Display, what: &str) -> Response {
    tracing::error!(error = %e, "series_similar: {what} failed");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
}

fn bad_cursor() -> Response {
    error(StatusCode::BAD_REQUEST, "bad_cursor", "invalid cursor")
}

fn parse_cursor(raw: Option<&str>) -> Result<Option<SimilarCursor>, Response> {
    match raw {
        None => Ok(None),
        Some(r) => decode_cursor::<SimilarCursor>(r)
            .map(Some)
            .map_err(|_| bad_cursor()),
    }
}

/// `GET /series/{slug}/similar`
#[utoipa::path(
    operation_id = "series_similar",    get,
    path = "/series/{slug}/similar",
    params(("slug" = String, Path,), SimilarQuery),
    responses(
        (status = 200, body = SimilarSeriesListView),
        (status = 400, description = "Malformed cursor"),
        (status = 404)
    )
)]
#[handler]
pub async fn similar(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Query(q): Query<SimilarQuery>,
) -> Response {
    let cursor = match parse_cursor(q.cursor.as_deref()) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let row = match find_by_slug(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let acl = access::for_user(&app, &user).await;
    if !acl.series_ok(row.library_id, row.age_rating.as_deref()) {
        return error(StatusCode::NOT_FOUND, "not_found", "series not found");
    }
    let neighbors = match similarity::neighbors(&app, row.id).await {
        Ok(n) => n,
        Err(e) => return internal(e, "neighbour compute"),
    };
    let hidden = match hidden_series(&app, user.id).await {
        Ok(h) => h,
        Err(e) => return internal(e, "hidden lookup"),
    };
    let visible = filter_visible(&neighbors, &acl, &hidden, &HashSet::new());
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    match build_page(&app, visible, cursor.as_ref(), limit, None).await {
        Ok((items, next_cursor, total)) => Json(SimilarSeriesListView {
            items,
            next_cursor,
            total,
        })
        .into_response(),
        Err(e) => internal(e, "hydrate"),
    }
}

/// `GET /me/similar-series` — "Because you read …" home rail.
#[utoipa::path(
    operation_id = "rails_similar_series",    get,
    path = "/me/similar-series",
    params(SimilarQuery),
    responses(
        (status = 200, body = SimilarSeriesRailView),
        (status = 400, description = "Malformed cursor")
    )
)]
#[handler]
pub async fn home_rail(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<SimilarQuery>,
) -> Response {
    let cursor = match parse_cursor(q.cursor.as_deref()) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let acl = access::for_user(&app, &user).await;
    let hidden = match hidden_series(&app, user.id).await {
        Ok(h) => h,
        Err(e) => return internal(e, "hidden lookup"),
    };
    let started = match started_series(&app, user.id).await {
        Ok(s) => s,
        Err(e) => return internal(e, "started lookup"),
    };
    let empty = || {
        Json(SimilarSeriesRailView {
            seed: None,
            items: Vec::new(),
            next_cursor: None,
            total: cursor.is_none().then_some(0),
        })
        .into_response()
    };
    let exclude: HashSet<Uuid> = started.iter().map(|s| s.series_id).collect();

    // Seed candidates: the cursor's seed on later pages, else the most
    // recently read visible + unhidden series, in recency order.
    let seeds: Vec<&StartedRow> = match cursor.as_ref().and_then(|c| c.seed) {
        Some(seed) => started.iter().filter(|s| s.series_id == seed).collect(),
        None => started
            .iter()
            .filter(|s| {
                !hidden.contains(&s.series_id)
                    && acl.series_ok(s.library_id, s.age_rating.as_deref())
            })
            .take(MAX_SEED_ATTEMPTS)
            .collect(),
    };
    for seed in seeds {
        let neighbors = match similarity::neighbors(&app, seed.series_id).await {
            Ok(n) => n,
            Err(e) => return internal(e, "neighbour compute"),
        };
        let visible = filter_visible(&neighbors, &acl, &hidden, &exclude);
        if visible.is_empty() {
            continue;
        }
        return match build_page(&app, visible, cursor.as_ref(), limit, Some(seed.series_id)).await {
            Ok((items, next_cursor, total)) => Json(SimilarSeriesRailView {
                seed: Some(SimilarSeed {
                    id: seed.series_id.to_string(),
                    name: seed.name.clone(),
                    slug: seed.slug.clone(),
                }),
                items,
                next_cursor,
                total,
            })
            .into_response(),
            Err(e) => internal(e, "hydrate"),
        };
    }
    empty()
}

/// Drop neighbours the caller can't see (ACL + age cap), has hidden, or
/// that the caller-specific `exclude` set names. Order is preserved.
fn filter_visible<'a>(
    neighbors: &'a Arc<Vec<Neighbor>>,
    acl: &VisibleLibraries,
    hidden: &HashSet<Uuid>,
    exclude: &HashSet<Uuid>,
) -> Vec<&'a Neighbor> {
    neighbors
        .iter()
        .filter(|n| {
            acl.series_ok(n.library_id, n.age_rating.as_deref())
                && !hidden.contains(&n.series_id)
                && !exclude.contains(&n.series_id)
        })
        .collect()
}

/// Slice one keyset page out of the (already ranked) visible list and
/// hydrate it into series cards.
async fn build_page(
    app: &AppState,
    visible: Vec<&Neighbor>,
    cursor: Option<&SimilarCursor>,
    limit: u64,
    seed: Option<Uuid>,
) -> Result<(Vec<SimilarSeriesItem>, Option<String>, Option<i64>), sea_orm::DbErr> {
    let total = cursor.is_none().then_some(visible.len() as i64);
    let after: Vec<&Neighbor> = match cursor {
        None => visible,
        Some(c) => visible
            .into_iter()
            .skip_while(|n| n.score > c.s || (n.score == c.s && n.series_id <= c.id))
            .collect(),
    };
    let limit = limit as usize;
    let page: Vec<&Neighbor> = after.iter().take(limit).copied().collect();
    let next_cursor = (after.len() > limit)
        .then(|| page.last())
        .flatten()
        .and_then(|last| {
            encode_cursor(&SimilarCursor {
                s: last.score,
                id: last.series_id,
                seed,
            })
            .ok()
        });
    if page.is_empty() {
        return Ok((Vec::new(), None, total));
    }
    let ids: Vec<Uuid> = page.iter().map(|n| n.series_id).collect();
    let rows = series::Entity::find()
        .filter(series::Column::Id.is_in(ids))
        .all(&app.db)
        .await?;
    // `hydrate_series` keeps input order; feed it rank order.
    let mut by_id: HashMap<Uuid, series::Model> = rows.into_iter().map(|r| (r.id, r)).collect();
    let ordered: Vec<series::Model> = page
        .iter()
        .filter_map(|n| by_id.remove(&n.series_id))
        .collect();
    let views = hydrate_series(app, ordered).await;
    let mut view_by_id: HashMap<String, SeriesView> =
        views.into_iter().map(|v| (v.id.clone(), v)).collect();
    let items = page
        .into_iter()
        .filter_map(|n| {
            view_by_id
                .remove(&n.series_id.to_string())
                .map(|series| SimilarSeriesItem {
                    series,
                    score: n.score,
                    because: n.because.clone(),
                })
        })
        .collect();
    Ok((items, next_cursor, total))
}

#[derive(Debug, FromQueryResult)]
struct HiddenRow {
    target_id: String,
}

/// Series the caller has hidden from rails and not touched since
/// (`rail_dismissals`, kind `series`, with On Deck's auto-restore rule:
/// a newer progress write on the series un-hides it).
async fn hidden_series(app: &AppState, user_id: Uuid) -> Result<HashSet<Uuid>, sea_orm::DbErr> {
    let rows = HiddenRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
        SELECT d.target_id
          FROM rail_dismissals d
         WHERE d.user_id = $1
           AND d.target_kind = 'series'
           AND NOT EXISTS (
                 SELECT 1 FROM progress_records p
                   JOIN issues i ON i.id = p.issue_id
                  WHERE p.user_id = $1
                    AND i.series_id::text = d.target_id
                    AND p.updated_at > d.dismissed_at)
        "#,
        [user_id.into()],
    ))
    .all(&app.db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| Uuid::parse_str(&r.target_id).ok())
        .collect())
}

#[derive(Debug, FromQueryResult)]
struct StartedRow {
    series_id: Uuid,
    library_id: Uuid,
    age_rating: Option<String>,
    name: String,
    slug: String,
}

/// Every non-removed series the caller has opened, most recent activity
/// first. Seeds the home rail and is excluded from it.
///
/// Progress counts on live (active, non-removed) issues only — the On Deck
/// rail's rule. The predicate also lets the progress → issue join use the
/// covering `issues_active_id_series_idx` (index-only); without it the
/// planner hashed a seq scan of the whole `issues` heap on every home-page
/// load (WP-8.3, 50k-issue measurement).
async fn started_series(app: &AppState, user_id: Uuid) -> Result<Vec<StartedRow>, sea_orm::DbErr> {
    StartedRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
        SELECT s.id AS series_id, s.library_id, s.age_rating, s.name, s.slug
          FROM (
                SELECT i.series_id, max(p.updated_at) AS last_activity
                  FROM progress_records p
                  JOIN issues i ON i.id = p.issue_id
                   AND i.state = 'active' AND i.removed_at IS NULL
                 WHERE p.user_id = $1
                   AND (p.last_page > 0 OR p.finished)
                 GROUP BY i.series_id
               ) started
          JOIN series s ON s.id = started.series_id AND s.removed_at IS NULL
         ORDER BY started.last_activity DESC, s.id
        "#,
        [user_id.into()],
    ))
    .all(&app.db)
    .await
}
