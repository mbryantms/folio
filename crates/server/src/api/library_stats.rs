//! Per-library counts and disk usage for the admin libraries pages
//! (audit 2026-11, §5 item 16).
//!
//! - `GET /admin/libraries/stats` — one row per library, for the compact
//!   figures on the libraries list.
//! - `GET /admin/libraries/{slug}/stats` — the same row for one library,
//!   for the overview tab.
//!
//! Every figure comes from the database; neither handler touches the
//! filesystem. Archive bytes are the scanner's stored `issues.file_size`
//! and backup bytes are the `library_backup_stats` tally the daily
//! `backup_prune` sweep stores ([`crate::library::backup_stats`]). A
//! library the sweep has not counted yet reports `null` backup figures
//! rather than walking its root on request. Thumbnail-cache bytes are not
//! reported: the thumbnail cache is one tree under the data dir with one
//! global budget sweep, and nothing tallies it per library.
//!
//! The query count is fixed whatever the number of libraries: the admin
//! check, the library rows, then one grouped statement per figure family
//! (series, issues, backups). Guarded by `tests/library_stats.rs`.

use axum::{
    Json,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::IntoResponse,
};
use entity::{library, library_backup_stats};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, FromQueryResult, QueryFilter, QueryOrder,
    Statement,
};
use serde::Serialize;
use std::collections::HashMap;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::error;
use crate::auth::RequireAdmin;
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(library_stats_all))
        .routes(routes!(library_stats_one))
}

/// Counts and disk usage for one library.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LibraryStatsView {
    pub library_id: String,
    pub slug: String,
    /// Series still on disk (`removed_at IS NULL`).
    pub series_count: u64,
    /// Readable issues still on disk (`state = 'active'`, not removed).
    /// Encrypted or malformed archives count in neither this nor
    /// `removed_issue_count`.
    pub active_issue_count: u64,
    /// Soft-removed issues (`removed_at` set, confirmed or not): the file
    /// left the disk, or a duplicate copy was removed from the catalogue.
    pub removed_issue_count: u64,
    /// Sum of the stored file size of the active issues, in bytes. Removed
    /// and unreadable issues are left out, so this is the readable
    /// collection's size, not every byte under the root.
    pub archive_bytes: u64,
    /// Bytes of `.bak` / `.bak.N` archive backups from the stored tally;
    /// `null` until the daily backup sweep (or the health page) has
    /// counted this library.
    pub backup_bytes: Option<u64>,
    pub backup_file_count: Option<u64>,
    /// RFC 3339 — when the backup tally was counted.
    pub backup_counted_at: Option<String>,
    /// A scan changed the library since the tally was counted, so the
    /// backup figures may be behind until the next recount (the health
    /// page or the daily sweep). `false` when there is no tally.
    pub backup_stale: bool,
    /// RFC 3339 — when the library's last full, completed scan finished
    /// (`libraries.last_scan_at`; scoped and cancelled scans leave it).
    pub last_scan_at: Option<String>,
}

#[utoipa::path(
    operation_id = "admin_library_stats_all",    get,
    path = "/admin/libraries/stats",
    responses(
        (status = 200, body = Vec<LibraryStatsView>),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn library_stats_all(
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
            tracing::error!(error = %e, "library stats: list libraries failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    match Figures::load(&app.db, None).await {
        Ok(f) => Json(libs.iter().map(|l| f.view(l)).collect::<Vec<_>>()).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "library stats: aggregate failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

#[utoipa::path(
    operation_id = "admin_library_stats_one",    get,
    path = "/admin/libraries/{slug}/stats",
    params(("slug" = String, Path,)),
    responses(
        (status = 200, body = LibraryStatsView),
        (status = 403, description = "admin only"),
        (status = 404, description = "library not found"),
    )
)]
#[handler]
pub async fn library_stats_one(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    AxPath(slug): AxPath<String>,
) -> impl IntoResponse {
    let lib = match super::libraries::find_by_slug_or_id(&app.db, &slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let id = lib.id;
    match Figures::load(&app.db, Some(id)).await {
        Ok(f) => Json(f.view(&lib)).into_response(),
        Err(e) => {
            tracing::error!(error = %e, library_id = %id, "library stats: aggregate failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
        }
    }
}

#[derive(FromQueryResult)]
struct SeriesRow {
    library_id: Uuid,
    n: i64,
}

#[derive(FromQueryResult)]
struct IssueRow {
    library_id: Uuid,
    active: i64,
    removed: i64,
    bytes: i64,
}

/// The grouped figures, keyed by library id.
struct Figures {
    series: HashMap<Uuid, i64>,
    issues: HashMap<Uuid, IssueRow>,
    backups: HashMap<Uuid, library_backup_stats::Model>,
}

impl Figures {
    /// One grouped statement per figure family, narrowed to `only` when set.
    async fn load(db: &DatabaseConnection, only: Option<Uuid>) -> Result<Self, sea_orm::DbErr> {
        let backend = db.get_database_backend();
        // `$1` in the issue / series SQL, an `eq` filter on the backup rows.
        let (and_scope, where_scope, values): (&str, &str, Vec<sea_orm::Value>) = match only {
            Some(id) => (
                "AND library_id = $1",
                "WHERE library_id = $1",
                vec![id.into()],
            ),
            None => ("", "", vec![]),
        };

        // The scoped form narrows on the leading `library_id` column of
        // `series_library_normalized_uniq`.
        let series_sql = format!(
            "SELECT library_id, COUNT(*)::bigint AS n FROM series \
             WHERE removed_at IS NULL {and_scope} GROUP BY library_id"
        );
        let series = SeriesRow::find_by_statement(Statement::from_sql_and_values(
            backend,
            series_sql,
            values.clone(),
        ))
        .all(db)
        .await?
        .into_iter()
        .map(|r| (r.library_id, r.n))
        .collect();

        // One pass over the issue rows for all three issue figures
        // (`issues_library_idx` for the scoped form; `file_size` is in no
        // index, so the rows are read once either way).
        let issues_sql = format!(
            "SELECT library_id, \
               COUNT(*) FILTER (WHERE removed_at IS NULL AND state = 'active')::bigint AS active, \
               COUNT(*) FILTER (WHERE removed_at IS NOT NULL)::bigint AS removed, \
               COALESCE(SUM(file_size) FILTER (WHERE removed_at IS NULL AND state = 'active'), 0)::bigint AS bytes \
             FROM issues {where_scope} GROUP BY library_id"
        );
        let issues = IssueRow::find_by_statement(Statement::from_sql_and_values(
            backend, issues_sql, values,
        ))
        .all(db)
        .await?
        .into_iter()
        .map(|r| (r.library_id, r))
        .collect();

        let mut backups_q = library_backup_stats::Entity::find();
        if let Some(id) = only {
            backups_q = backups_q.filter(library_backup_stats::Column::LibraryId.eq(id));
        }
        let backups = backups_q
            .all(db)
            .await?
            .into_iter()
            .map(|r| (r.library_id, r))
            .collect();

        Ok(Self {
            series,
            issues,
            backups,
        })
    }

    /// The view for `lib`; a library with no series, issues or tally gets
    /// zeros / `null`s.
    fn view(&self, lib: &library::Model) -> LibraryStatsView {
        let to_u64 = |n: i64| u64::try_from(n).unwrap_or(0);
        let issue = self.issues.get(&lib.id);
        let backup = self.backups.get(&lib.id);
        LibraryStatsView {
            library_id: lib.id.to_string(),
            slug: lib.slug.clone(),
            series_count: self.series.get(&lib.id).copied().map_or(0, to_u64),
            active_issue_count: issue.map_or(0, |r| to_u64(r.active)),
            removed_issue_count: issue.map_or(0, |r| to_u64(r.removed)),
            archive_bytes: issue.map_or(0, |r| to_u64(r.bytes)),
            backup_bytes: backup.map(|b| to_u64(b.total_bytes)),
            backup_file_count: backup.map(|b| to_u64(b.file_count)),
            backup_counted_at: backup.map(|b| b.computed_at.to_rfc3339()),
            backup_stale: backup.is_some_and(|b| b.stale),
            last_scan_at: lib.last_scan_at.map(|t| t.to_rfc3339()),
        }
    }
}
