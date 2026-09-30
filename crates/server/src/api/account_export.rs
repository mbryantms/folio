//! `GET /me/export` — one JSON document carrying everything the calling
//! user owns (roadmap WP-2.1, audit R6).
//!
//! The export exists so a library rebuild or a host move never loses
//! user data, and so the M5 notes export can reuse the same shape. It is
//! deliberately **not** keyed on Folio's internal ids alone: every issue
//! reference carries the `content_hash` plus `(series_name, series_year,
//! issue_number)`, and every series reference carries `(series_name,
//! series_year, library_slug)`, so the document stays resolvable after
//! `issues.id` changes.
//!
//! Query shape: one bounded query per section plus three IN-batched
//! hydration queries (issues → series → libraries). No per-row lookups;
//! `perf_regressions.rs` pins the count. The document is built in memory —
//! every section is owned by a single user, so the size is bounded by that
//! user's activity, not by the library.
//!
//! The wire format is documented in `docs/dev/export-format.md`. Bump
//! [`EXPORT_VERSION`] on any breaking change to the envelope or a section.

use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use entity::{
    collection_entry, issue, library, marker, progress_record, rail_dismissal, reading_session,
    saved_view, series, user, user_page, user_rating, user_sidebar_entry, user_view_pin,
};

use crate::api::respond;
use crate::auth::CurrentUser;
use crate::middleware::rate_limit;
use crate::state::AppState;
use server_macros::handler;
use shared::error::ApiErrorCode;

/// `format` discriminator on the envelope.
pub const EXPORT_FORMAT: &str = "folio-user-export";
/// Envelope version. Bump on any breaking change to a section's shape.
pub const EXPORT_VERSION: u32 = 1;

/// Rows per `IN (...)` hydration batch. Bounds the bind-parameter count
/// on a large export while keeping the query count O(rows / CHUNK).
const HYDRATE_CHUNK: usize = 1000;

const KIND_COLLECTION: &str = "collection";
const SYSTEM_KEY_WANT_TO_READ: &str = "want_to_read";

pub fn routes() -> OpenApiRouter<AppState> {
    // Own sub-router so the rate-limit `route_layer` doesn't leak onto
    // sibling routes when this is merged into the `api` group.
    OpenApiRouter::new()
        .routes(routes!(export))
        .route_layer(rate_limit::USER_EXPORT.build())
}

// ───────── wire types ─────────

/// Top-level export envelope.
#[derive(Debug, Serialize, ToSchema)]
pub struct UserExport {
    /// Always `"folio-user-export"`.
    pub format: String,
    /// Envelope version. See `docs/dev/export-format.md` for the changelog.
    pub version: u32,
    /// RFC 3339 timestamp of when the document was produced.
    pub exported_at: String,
    pub user: ExportUser,
    pub sections: ExportSections,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExportUser {
    pub id: Uuid,
    pub email: Option<String>,
    pub display_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExportSections {
    pub progress: Vec<ExportProgress>,
    pub markers: Vec<ExportMarker>,
    /// User-authored collections (`saved_views.kind = 'collection'`,
    /// no `system_key`). Want to Read is split out below.
    pub collections: Vec<ExportCollection>,
    /// The per-user Want to Read system collection, or `null` when the
    /// user has never touched a surface that seeds it.
    pub want_to_read: Option<ExportCollection>,
    /// Filter and CBL-backed saved views (every user-owned `saved_views`
    /// row whose `kind != 'collection'`).
    pub saved_views: Vec<ExportSavedView>,
    pub ratings: Vec<ExportRating>,
    /// Every `user_page` row including the system Home page, each with
    /// its pinned rails.
    pub custom_pages: Vec<ExportPage>,
    pub sidebar: Vec<ExportSidebarEntry>,
    pub rail_dismissals: Vec<ExportRailDismissal>,
    /// `reading_sessions` rows. Hidden sessions are included with
    /// `hidden_from_log = true` so the flag survives a restore.
    pub reading_log: Vec<ExportReadingSession>,
    /// Per-user preferences from the `users` row, including reader
    /// `keybinds`.
    pub preferences: ExportPreferences,
}

/// Portable identity for an issue. `issue_id` is Folio's BLAKE3 id;
/// `content_hash` plus `(series_name, series_year, issue_number)` are
/// the keys that survive a rebuild. The hydrated fields are `null` only
/// when the referenced row no longer exists.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct IssueRef {
    pub issue_id: String,
    pub content_hash: Option<String>,
    pub series_id: Option<Uuid>,
    pub series_name: Option<String>,
    pub series_year: Option<i32>,
    /// `issues.number_raw` — the number exactly as tagged.
    pub issue_number: Option<String>,
    pub library_slug: Option<String>,
}

/// Portable identity for a series.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SeriesRef {
    pub series_id: Uuid,
    pub series_name: Option<String>,
    pub series_year: Option<i32>,
    pub library_slug: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExportProgress {
    pub issue: IssueRef,
    /// Reading-run counter: 0 is the first read, each explicit re-read
    /// opens the next run (`docs/dev/reading-progress.md`).
    pub run: i32,
    pub last_page: i32,
    pub percent: f64,
    pub finished: bool,
    pub finished_at: Option<String>,
    pub is_backfill: bool,
    pub device: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExportMarker {
    pub id: Uuid,
    pub issue: IssueRef,
    pub page_index: i32,
    pub kind: String,
    pub is_favorite: bool,
    pub tags: Vec<String>,
    #[schema(value_type = Option<Object>)]
    pub region: Option<serde_json::Value>,
    #[schema(value_type = Option<Object>)]
    pub selection: Option<serde_json::Value>,
    pub body: Option<String>,
    pub color: Option<String>,
    pub hidden_from_log: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExportCollection {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    /// `"want_to_read"` for the system collection, `null` otherwise.
    pub system_key: Option<String>,
    pub custom_tags: Vec<String>,
    pub preserve_canonical_order: bool,
    pub created_at: String,
    pub updated_at: String,
    /// Entries in `position` order.
    pub entries: Vec<ExportCollectionEntry>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExportCollectionEntry {
    pub position: i32,
    /// `"issue"` or `"series"` — exactly one of the refs below is set.
    pub entry_kind: String,
    pub issue: Option<IssueRef>,
    pub series: Option<SeriesRef>,
    pub added_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExportSavedView {
    pub id: Uuid,
    /// `"filter_series"`, `"filter_issues"` or `"cbl"`.
    pub kind: String,
    pub name: String,
    pub description: Option<String>,
    pub custom_year_start: Option<i32>,
    pub custom_year_end: Option<i32>,
    pub custom_tags: Vec<String>,
    pub match_mode: Option<String>,
    /// Filter DSL — a JSON array of `{group_id, field, op, value}`.
    #[schema(value_type = Option<Object>)]
    pub conditions: Option<serde_json::Value>,
    pub sort_field: Option<String>,
    pub sort_order: Option<String>,
    pub result_limit: Option<i32>,
    /// For `kind = "cbl"`: the backing `cbl_lists` row. CBL lists have
    /// their own XML export (`GET /me/cbl-lists/{id}/export`).
    pub cbl_list_id: Option<Uuid>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExportRating {
    /// `"issue"` or `"series"` — exactly one of the refs below is set.
    pub target_type: String,
    pub issue: Option<IssueRef>,
    pub series: Option<SeriesRef>,
    pub rating: f64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExportPage {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub is_system: bool,
    pub position: i32,
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Pinned rails on this page in `position` order.
    pub pins: Vec<ExportViewPin>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExportViewPin {
    pub view_id: Uuid,
    /// The pinned view's `kind` (`filter_series` / `cbl` / `system` /
    /// `collection`), hydrated so a system rail is recognisable after a
    /// rebuild. `null` when the view no longer exists.
    pub view_kind: Option<String>,
    pub view_name: Option<String>,
    /// `system_key` of the pinned view (e.g. `continue_reading`), when it
    /// is a built-in rail or the Want to Read collection.
    pub view_system_key: Option<String>,
    pub position: i32,
    pub pinned: bool,
    pub show_in_sidebar: bool,
    pub icon: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExportSidebarEntry {
    /// `builtin` / `library` / `view` / `header` / `spacer`.
    pub kind: String,
    pub ref_id: String,
    pub visible: bool,
    pub position: i32,
    pub label: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExportRailDismissal {
    /// `issue` / `series` / `cbl`.
    pub target_kind: String,
    pub target_id: String,
    pub dismissed_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExportReadingSession {
    pub id: Uuid,
    pub issue: IssueRef,
    pub client_session_id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub last_heartbeat_at: String,
    pub active_ms: i64,
    pub distinct_pages_read: i32,
    pub page_turns: i32,
    pub start_page: i32,
    pub end_page: i32,
    pub furthest_page: i32,
    pub device: Option<String>,
    pub view_mode: Option<String>,
    #[schema(value_type = Object)]
    pub client_meta: serde_json::Value,
    pub hidden_from_log: bool,
}

/// Raw preference columns off the `users` row. Token columns are exported
/// as stored (`null` = "no preference").
#[derive(Debug, Serialize, ToSchema)]
pub struct ExportPreferences {
    pub default_reading_direction: Option<String>,
    pub default_fit_mode: Option<String>,
    pub default_view_mode: Option<String>,
    pub default_page_strip: bool,
    pub default_page_animation: Option<String>,
    pub default_cover_solo: bool,
    pub theme: Option<String>,
    pub accent_color: Option<String>,
    pub density: Option<String>,
    /// Reader key overrides — `{ action_name: key_string }`.
    #[schema(value_type = Object)]
    pub keybinds: serde_json::Value,
    pub activity_tracking_enabled: bool,
    pub timezone: String,
    pub reading_min_active_ms: i32,
    pub reading_min_pages: i32,
    pub reading_idle_ms: i32,
    pub language: String,
    pub exclude_from_aggregates: bool,
    pub show_marker_count: bool,
    pub opds_wtr_reorder: bool,
    pub opds_progress_glyphs: bool,
    pub max_rails_per_page: i32,
}

// ───────── handler ─────────

#[utoipa::path(
    operation_id = "account_export",
    get,
    path = "/me/export",
    responses(
        (status = 200, body = UserExport, description = "Attachment; `Content-Disposition: attachment; filename=\"folio-export-<date>.json\"`"),
        (status = 401),
        (status = 429),
    )
)]
#[handler]
pub async fn export(State(app): State<AppState>, user: CurrentUser) -> Response {
    match build_export(&app.db, user.id).await {
        Ok(doc) => {
            let filename = format!("folio-export-{}.json", Utc::now().format("%Y-%m-%d"));
            (
                StatusCode::OK,
                [(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{filename}\""),
                )],
                Json(doc),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "account export failed");
            respond(
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::Internal,
                "failed to build export",
            )
        }
    }
}

// ───────── assembly ─────────

async fn build_export(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Result<UserExport, sea_orm::DbErr> {
    let Some(user_row) = user::Entity::find_by_id(user_id).one(db).await? else {
        return Err(sea_orm::DbErr::RecordNotFound("user".into()));
    };

    // ── one query per section ──
    let progress_rows = progress_record::Entity::find()
        .filter(progress_record::Column::UserId.eq(user_id))
        .order_by_asc(progress_record::Column::UpdatedAt)
        .all(db)
        .await?;
    let marker_rows = marker::Entity::find()
        .filter(marker::Column::UserId.eq(user_id))
        .order_by_asc(marker::Column::CreatedAt)
        .all(db)
        .await?;
    let view_rows = saved_view::Entity::find()
        .filter(saved_view::Column::UserId.eq(user_id))
        .order_by_asc(saved_view::Column::CreatedAt)
        .all(db)
        .await?;
    let rating_rows = user_rating::Entity::find()
        .filter(user_rating::Column::UserId.eq(user_id))
        .order_by_asc(user_rating::Column::CreatedAt)
        .all(db)
        .await?;
    let page_rows = user_page::Entity::find()
        .filter(user_page::Column::UserId.eq(user_id))
        .order_by_asc(user_page::Column::Position)
        .all(db)
        .await?;
    let pin_rows = user_view_pin::Entity::find()
        .filter(user_view_pin::Column::UserId.eq(user_id))
        .order_by_asc(user_view_pin::Column::Position)
        .all(db)
        .await?;
    let sidebar_rows = user_sidebar_entry::Entity::find()
        .filter(user_sidebar_entry::Column::UserId.eq(user_id))
        .order_by_asc(user_sidebar_entry::Column::Position)
        .all(db)
        .await?;
    let dismissal_rows = rail_dismissal::Entity::find()
        .filter(rail_dismissal::Column::UserId.eq(user_id))
        .order_by_asc(rail_dismissal::Column::DismissedAt)
        .all(db)
        .await?;
    let session_rows = reading_session::Entity::find()
        .filter(reading_session::Column::UserId.eq(user_id))
        .order_by_asc(reading_session::Column::StartedAt)
        .all(db)
        .await?;

    let (collection_views, other_views): (Vec<_>, Vec<_>) = view_rows
        .into_iter()
        .partition(|v| v.kind == KIND_COLLECTION);
    let collection_ids: Vec<Uuid> = collection_views.iter().map(|v| v.id).collect();
    let entry_rows = if collection_ids.is_empty() {
        Vec::new()
    } else {
        collection_entry::Entity::find()
            .filter(collection_entry::Column::SavedViewId.is_in(collection_ids))
            .order_by_asc(collection_entry::Column::Position)
            .all(db)
            .await?
    };

    // Pinned views may be system rails the user doesn't own — hydrate
    // their kind/name/system_key so the pin is meaningful after a rebuild.
    let pinned_view_ids: Vec<Uuid> = pin_rows
        .iter()
        .map(|p| p.view_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let pinned_views: HashMap<Uuid, (String, String, Option<String>)> =
        if pinned_view_ids.is_empty() {
            HashMap::new()
        } else {
            saved_view::Entity::find()
                .select_only()
                .column(saved_view::Column::Id)
                .column(saved_view::Column::Kind)
                .column(saved_view::Column::Name)
                .column(saved_view::Column::SystemKey)
                .filter(saved_view::Column::Id.is_in(pinned_view_ids))
                .into_tuple::<(Uuid, String, String, Option<String>)>()
                .all(db)
                .await?
                .into_iter()
                .map(|(id, kind, name, key)| (id, (kind, name, key)))
                .collect()
        };

    // ── collect every issue / series reference, then hydrate in batches ──
    let mut issue_ids: HashSet<String> = HashSet::new();
    let mut series_ids: HashSet<Uuid> = HashSet::new();
    for p in &progress_rows {
        issue_ids.insert(p.issue_id.clone());
    }
    for m in &marker_rows {
        issue_ids.insert(m.issue_id.clone());
        series_ids.insert(m.series_id);
    }
    for e in &entry_rows {
        if let Some(id) = &e.issue_id {
            issue_ids.insert(id.clone());
        }
        if let Some(id) = e.series_id {
            series_ids.insert(id);
        }
    }
    for r in &rating_rows {
        match r.target_type.as_str() {
            "issue" => {
                issue_ids.insert(r.target_id.clone());
            }
            "series" => {
                if let Ok(id) = r.target_id.parse::<Uuid>() {
                    series_ids.insert(id);
                }
            }
            _ => {}
        }
    }
    for s in &session_rows {
        issue_ids.insert(s.issue_id.clone());
        series_ids.insert(s.series_id);
    }

    let refs = Refs::load(db, issue_ids, series_ids).await?;

    // ── shape sections ──
    let progress = progress_rows
        .into_iter()
        .map(|p| ExportProgress {
            issue: refs.issue(&p.issue_id),
            run: p.run,
            last_page: p.last_page,
            percent: p.percent,
            finished: p.finished,
            finished_at: p.finished_at.map(|t| t.to_rfc3339()),
            is_backfill: p.is_backfill,
            device: p.device,
            updated_at: p.updated_at.to_rfc3339(),
        })
        .collect();

    let markers = marker_rows
        .into_iter()
        .map(|m| export_marker(m, &refs))
        .collect();

    let mut entries_by_view: HashMap<Uuid, Vec<ExportCollectionEntry>> = HashMap::new();
    for e in entry_rows {
        entries_by_view
            .entry(e.saved_view_id)
            .or_default()
            .push(ExportCollectionEntry {
                position: e.position,
                entry_kind: e.entry_kind,
                issue: e.issue_id.as_deref().map(|id| refs.issue(id)),
                series: e.series_id.map(|id| refs.series(id)),
                added_at: e.added_at.to_rfc3339(),
            });
    }
    let mut collections = Vec::new();
    let mut want_to_read = None;
    for v in collection_views {
        let entries = entries_by_view.remove(&v.id).unwrap_or_default();
        let is_wtr = v.system_key.as_deref() == Some(SYSTEM_KEY_WANT_TO_READ);
        let col = ExportCollection {
            id: v.id,
            name: v.name,
            description: v.description,
            system_key: v.system_key,
            custom_tags: v.custom_tags,
            preserve_canonical_order: v.preserve_canonical_order,
            created_at: v.created_at.to_rfc3339(),
            updated_at: v.updated_at.to_rfc3339(),
            entries,
        };
        if is_wtr && want_to_read.is_none() {
            want_to_read = Some(col);
        } else {
            collections.push(col);
        }
    }

    let saved_views = other_views
        .into_iter()
        .map(|v| ExportSavedView {
            id: v.id,
            kind: v.kind,
            name: v.name,
            description: v.description,
            custom_year_start: v.custom_year_start,
            custom_year_end: v.custom_year_end,
            custom_tags: v.custom_tags,
            match_mode: v.match_mode,
            conditions: v.conditions,
            sort_field: v.sort_field,
            sort_order: v.sort_order,
            result_limit: v.result_limit,
            cbl_list_id: v.cbl_list_id,
            created_at: v.created_at.to_rfc3339(),
            updated_at: v.updated_at.to_rfc3339(),
        })
        .collect();

    let ratings = rating_rows
        .into_iter()
        .map(|r| {
            let (issue_ref, series_ref) = match r.target_type.as_str() {
                "issue" => (Some(refs.issue(&r.target_id)), None),
                "series" => (
                    None,
                    r.target_id.parse::<Uuid>().ok().map(|id| refs.series(id)),
                ),
                _ => (None, None),
            };
            ExportRating {
                target_type: r.target_type,
                issue: issue_ref,
                series: series_ref,
                rating: r.rating,
                created_at: r.created_at.to_rfc3339(),
                updated_at: r.updated_at.to_rfc3339(),
            }
        })
        .collect();

    let mut pins_by_page: HashMap<Uuid, Vec<ExportViewPin>> = HashMap::new();
    for p in pin_rows {
        let hydrated = pinned_views.get(&p.view_id);
        pins_by_page
            .entry(p.page_id)
            .or_default()
            .push(ExportViewPin {
                view_id: p.view_id,
                view_kind: hydrated.map(|(k, _, _)| k.clone()),
                view_name: hydrated.map(|(_, n, _)| n.clone()),
                view_system_key: hydrated.and_then(|(_, _, key)| key.clone()),
                position: p.position,
                pinned: p.pinned,
                show_in_sidebar: p.show_in_sidebar,
                icon: p.icon,
            });
    }
    let custom_pages = page_rows
        .into_iter()
        .map(|pg| ExportPage {
            pins: pins_by_page.remove(&pg.id).unwrap_or_default(),
            id: pg.id,
            name: pg.name,
            slug: pg.slug,
            is_system: pg.is_system,
            position: pg.position,
            description: pg.description,
            created_at: pg.created_at.to_rfc3339(),
            updated_at: pg.updated_at.to_rfc3339(),
        })
        .collect();

    let sidebar = sidebar_rows
        .into_iter()
        .map(|s| ExportSidebarEntry {
            kind: s.kind,
            ref_id: s.ref_id,
            visible: s.visible,
            position: s.position,
            label: s.label,
        })
        .collect();

    let rail_dismissals = dismissal_rows
        .into_iter()
        .map(|d| ExportRailDismissal {
            target_kind: d.target_kind,
            target_id: d.target_id,
            dismissed_at: d.dismissed_at.to_rfc3339(),
        })
        .collect();

    let reading_log = session_rows
        .into_iter()
        .map(|s| ExportReadingSession {
            id: s.id,
            issue: refs.issue(&s.issue_id),
            client_session_id: s.client_session_id,
            started_at: s.started_at.to_rfc3339(),
            ended_at: s.ended_at.map(|t| t.to_rfc3339()),
            last_heartbeat_at: s.last_heartbeat_at.to_rfc3339(),
            active_ms: s.active_ms,
            distinct_pages_read: s.distinct_pages_read,
            page_turns: s.page_turns,
            start_page: s.start_page,
            end_page: s.end_page,
            furthest_page: s.furthest_page,
            device: s.device,
            view_mode: s.view_mode,
            client_meta: s.client_meta,
            hidden_from_log: s.hidden_from_log,
        })
        .collect();

    let preferences = ExportPreferences {
        default_reading_direction: user_row.default_reading_direction.clone(),
        default_fit_mode: user_row.default_fit_mode.clone(),
        default_view_mode: user_row.default_view_mode.clone(),
        default_page_strip: user_row.default_page_strip,
        default_page_animation: user_row.default_page_animation.clone(),
        default_cover_solo: user_row.default_cover_solo,
        theme: user_row.theme.clone(),
        accent_color: user_row.accent_color.clone(),
        density: user_row.density.clone(),
        keybinds: user_row.keybinds.clone(),
        activity_tracking_enabled: user_row.activity_tracking_enabled,
        timezone: user_row.timezone.clone(),
        reading_min_active_ms: user_row.reading_min_active_ms,
        reading_min_pages: user_row.reading_min_pages,
        reading_idle_ms: user_row.reading_idle_ms,
        language: user_row.language.clone(),
        exclude_from_aggregates: user_row.exclude_from_aggregates,
        show_marker_count: user_row.show_marker_count,
        opds_wtr_reorder: user_row.opds_wtr_reorder,
        opds_progress_glyphs: user_row.opds_progress_glyphs,
        max_rails_per_page: user_row.max_rails_per_page,
    };

    Ok(UserExport {
        format: EXPORT_FORMAT.to_string(),
        version: EXPORT_VERSION,
        exported_at: Utc::now().to_rfc3339(),
        user: ExportUser {
            id: user_row.id,
            email: user_row.email,
            display_name: user_row.display_name,
        },
        sections: ExportSections {
            progress,
            markers,
            collections,
            want_to_read,
            saved_views,
            ratings,
            custom_pages,
            sidebar,
            rail_dismissals,
            reading_log,
            preferences,
        },
    })
}

/// One marker in the shared export shape. The notes export
/// (`api::markers_export`) wraps the same struct, so a marker is described
/// once across both documents.
pub(crate) fn export_marker(m: marker::Model, refs: &Refs) -> ExportMarker {
    ExportMarker {
        id: m.id,
        issue: refs.issue(&m.issue_id),
        page_index: m.page_index,
        kind: m.kind,
        is_favorite: m.is_favorite,
        tags: m.tags,
        region: m.region,
        selection: m.selection,
        body: m.body,
        color: m.color,
        hidden_from_log: m.hidden_from_log,
        created_at: m.created_at.to_rfc3339(),
        updated_at: m.updated_at.to_rfc3339(),
    }
}

// ───────── identity hydration ─────────

pub(crate) struct IssueRow {
    content_hash: String,
    pub(crate) series_id: Uuid,
    number_raw: Option<String>,
    library_id: Uuid,
    /// Only read by the notes export (`markers_export`), which orders
    /// issues by `sort_number` and prints the title.
    pub(crate) sort_number: Option<f64>,
    pub(crate) title: Option<String>,
}

pub(crate) struct SeriesRow {
    pub(crate) name: String,
    pub(crate) year: Option<i32>,
    library_id: Uuid,
}

/// `(id, content_hash, series_id, number_raw, library_id, sort_number, title)`.
type IssueTuple = (
    String,
    String,
    Uuid,
    Option<String>,
    Uuid,
    Option<f64>,
    Option<String>,
);

/// IN-batched lookup tables for issue / series / library identity. Loads
/// issues first (their `series_id` widens the series set), then series
/// (their `library_id` widens the library set), then libraries.
///
/// Shared with the notes export (`api::markers_export`) so a marker is
/// described by the same identity keys in both documents.
pub(crate) struct Refs {
    pub(crate) issues: HashMap<String, IssueRow>,
    pub(crate) series: HashMap<Uuid, SeriesRow>,
    libraries: HashMap<Uuid, String>,
}

impl Refs {
    pub(crate) async fn load(
        db: &DatabaseConnection,
        issue_ids: HashSet<String>,
        mut series_ids: HashSet<Uuid>,
    ) -> Result<Self, sea_orm::DbErr> {
        let mut issues = HashMap::with_capacity(issue_ids.len());
        let issue_ids: Vec<String> = issue_ids.into_iter().collect();
        for chunk in issue_ids.chunks(HYDRATE_CHUNK) {
            let rows = issue::Entity::find()
                .select_only()
                .column(issue::Column::Id)
                .column(issue::Column::ContentHash)
                .column(issue::Column::SeriesId)
                .column(issue::Column::NumberRaw)
                .column(issue::Column::LibraryId)
                .column(issue::Column::SortNumber)
                .column(issue::Column::Title)
                .filter(issue::Column::Id.is_in(chunk.to_vec()))
                .into_tuple::<IssueTuple>()
                .all(db)
                .await?;
            for (id, content_hash, series_id, number_raw, library_id, sort_number, title) in rows {
                series_ids.insert(series_id);
                issues.insert(
                    id,
                    IssueRow {
                        content_hash,
                        series_id,
                        number_raw,
                        library_id,
                        sort_number,
                        title,
                    },
                );
            }
        }

        let mut series = HashMap::with_capacity(series_ids.len());
        let mut library_ids: HashSet<Uuid> = issues.values().map(|i| i.library_id).collect();
        let series_ids: Vec<Uuid> = series_ids.into_iter().collect();
        for chunk in series_ids.chunks(HYDRATE_CHUNK) {
            let rows = series::Entity::find()
                .select_only()
                .column(series::Column::Id)
                .column(series::Column::Name)
                .column(series::Column::Year)
                .column(series::Column::LibraryId)
                .filter(series::Column::Id.is_in(chunk.to_vec()))
                .into_tuple::<(Uuid, String, Option<i32>, Uuid)>()
                .all(db)
                .await?;
            for (id, name, year, library_id) in rows {
                library_ids.insert(library_id);
                series.insert(
                    id,
                    SeriesRow {
                        name,
                        year,
                        library_id,
                    },
                );
            }
        }

        let mut libraries = HashMap::with_capacity(library_ids.len());
        let library_ids: Vec<Uuid> = library_ids.into_iter().collect();
        for chunk in library_ids.chunks(HYDRATE_CHUNK) {
            let rows = library::Entity::find()
                .select_only()
                .column(library::Column::Id)
                .column(library::Column::Slug)
                .filter(library::Column::Id.is_in(chunk.to_vec()))
                .into_tuple::<(Uuid, String)>()
                .all(db)
                .await?;
            libraries.extend(rows);
        }

        Ok(Self {
            issues,
            series,
            libraries,
        })
    }

    pub(crate) fn issue(&self, issue_id: &str) -> IssueRef {
        let Some(i) = self.issues.get(issue_id) else {
            return IssueRef {
                issue_id: issue_id.to_owned(),
                content_hash: None,
                series_id: None,
                series_name: None,
                series_year: None,
                issue_number: None,
                library_slug: None,
            };
        };
        let s = self.series.get(&i.series_id);
        IssueRef {
            issue_id: issue_id.to_owned(),
            content_hash: Some(i.content_hash.clone()),
            series_id: Some(i.series_id),
            series_name: s.map(|s| s.name.clone()),
            series_year: s.and_then(|s| s.year),
            issue_number: i.number_raw.clone(),
            library_slug: self.libraries.get(&i.library_id).cloned(),
        }
    }

    pub(crate) fn series(&self, series_id: Uuid) -> SeriesRef {
        let s = self.series.get(&series_id);
        SeriesRef {
            series_id,
            series_name: s.map(|s| s.name.clone()),
            series_year: s.and_then(|s| s.year),
            library_slug: s.and_then(|s| self.libraries.get(&s.library_id).cloned()),
        }
    }
}
