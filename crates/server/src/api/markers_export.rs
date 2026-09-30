//! `GET /me/markers/export` — the caller's markers as one Markdown or
//! JSON document, grouped series → issue → page (roadmap WP-5.1, audit
//! R11 / UX-12).
//!
//! The export is the durable, portable form of a user's notes: the
//! Markdown variant is meant to drop straight into a notes app, the JSON
//! variant is the machine-readable twin. Each marker is described with
//! the same [`ExportMarker`] shape as the full account export
//! (`GET /me/export`, [`super::account_export`]), so identity keys
//! (`content_hash`, `(series_name, series_year, issue_number)`) survive a
//! library rebuild. Each marker also carries a `jump_url` — the absolute
//! marker permalink `{public_url}/markers/{id}`
//! ([`super::issue_permalink::marker_permalink`]), which 303s to the
//! reader at the marker's page in peek mode. The permalink goes through
//! the marker id rather than baking in slugs, so a series rename doesn't
//! break links already pasted into another app.
//!
//! Scope: every marker the caller owns, every kind (bookmark, note,
//! favorite, highlight), including markers on issues since removed from
//! the library or no longer visible to the caller — the export is the
//! user's own data, same posture as `GET /me/export`. Those entries are
//! flagged `available: false`, carry no `jump_url` (the permalink would
//! 404), and read "(no longer available)" in Markdown (owner decision
//! 2026-09-30). Query shape: one marker query, the account export's
//! IN-batched identity hydration (issues → series → libraries), and one
//! IN-batched visibility probe (live + library grant + age cap).
//!
//! Wire format is documented in `docs/dev/export-format.md` ("Notes
//! export").

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

use axum::{
    Json,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::{issue, marker};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::account_export::{ExportMarker, IssueRef, Refs, SeriesRef, export_marker};
use crate::api::respond;
use crate::auth::CurrentUser;
use crate::library::access::{self, VisibleLibraries};
use crate::middleware::rate_limit;
use crate::state::AppState;
use server_macros::handler;
use shared::error::ApiErrorCode;

/// `format` discriminator on the JSON envelope.
pub const NOTES_EXPORT_FORMAT: &str = "folio-notes-export";
/// Envelope version. Bump on any breaking change.
pub const NOTES_EXPORT_VERSION: u32 = 1;

pub fn routes() -> OpenApiRouter<AppState> {
    // Own sub-router so the rate-limit `route_layer` stays on this route.
    OpenApiRouter::new()
        .routes(routes!(export))
        .route_layer(rate_limit::NOTES_EXPORT.build())
}

/// `?format=` for the notes export.
#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotesExportFormat {
    /// `text/markdown` (default).
    #[default]
    Md,
    /// `application/json` — [`NotesExport`].
    Json,
}

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    #[serde(default)]
    pub format: Option<NotesExportFormat>,
}

// ───────── wire types ─────────

#[derive(Debug, Serialize, ToSchema)]
pub struct NotesExport {
    /// Always `"folio-notes-export"`.
    pub format: String,
    pub version: u32,
    /// RFC 3339.
    pub exported_at: String,
    /// Number of markers in the document.
    pub total: u64,
    /// Series ordered by name, then year.
    pub series: Vec<NotesExportSeries>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NotesExportSeries {
    pub series: SeriesRef,
    /// Issues ordered by sort number.
    pub issues: Vec<NotesExportIssue>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NotesExportIssue {
    pub issue: IssueRef,
    pub issue_title: Option<String>,
    /// Pages in reading order; only pages that carry a marker appear.
    pub pages: Vec<NotesExportPage>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NotesExportPage {
    /// Zero-based page index (the reader's `?page=` value).
    pub page_index: i32,
    /// Markers on this page, oldest first.
    pub markers: Vec<NotesExportMarker>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NotesExportMarker {
    #[serde(flatten)]
    pub marker: ExportMarker,
    /// `false` when the marker's issue was removed from the library or is
    /// no longer visible to the caller (library grant / age cap). The
    /// marker is still exported; only the link is withheld.
    pub available: bool,
    /// Absolute marker permalink (`{public_url}/markers/{id}`); 303s to
    /// the reader at this page in peek mode. Omitted when `available` is
    /// `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jump_url: Option<String>,
}

// ───────── handler ─────────

#[utoipa::path(
    operation_id = "markers_export",
    get,
    path = "/me/markers/export",
    params(("format" = Option<String>, Query, description = "`md` (default) or `json`")),
    responses(
        (status = 200, body = NotesExport, description = "Attachment. `format=json` returns this body; `format=md` returns `text/markdown` (`folio-notes-<date>.md`)."),
        (status = 400, description = "unknown format"),
        (status = 401),
        (status = 429),
    )
)]
#[handler]
pub async fn export(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ExportQuery>,
) -> Response {
    let base_url = app.cfg().public_url.trim_end_matches('/').to_owned();
    let acl = access::for_user(&app, &user).await;
    let doc = match build(&app.db, user.id, &acl, &base_url).await {
        Ok(doc) => doc,
        Err(e) => {
            tracing::error!(error = %e, "notes export failed");
            return respond(
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::Internal,
                "failed to build notes export",
            );
        }
    };
    let date = Utc::now().format("%Y-%m-%d");
    match q.format.unwrap_or_default() {
        NotesExportFormat::Json => (
            StatusCode::OK,
            [(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"folio-notes-{date}.json\""),
            )],
            Json(doc),
        )
            .into_response(),
        NotesExportFormat::Md => (
            StatusCode::OK,
            [
                (
                    header::CONTENT_TYPE,
                    "text/markdown; charset=utf-8".to_owned(),
                ),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"folio-notes-{date}.md\""),
                ),
            ],
            render_markdown(&doc),
        )
            .into_response(),
    }
}

// ───────── assembly ─────────

/// Sort key for a series group: case-folded name, year, id.
type SeriesKey = (String, Option<i32>, Uuid);
/// Sort key for an issue group: sort number (NULLs last), number, id.
type IssueKey = (bool, OrdF64, String, String);

/// `f64` with a total order for `BTreeMap` keys (`sort_number` is never
/// NaN in practice; `total_cmp` keeps it well-defined anyway).
#[derive(Debug, Clone, Copy, PartialEq)]
struct OrdF64(f64);
impl Eq for OrdF64 {}
impl PartialOrd for OrdF64 {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for OrdF64 {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// Rows per `IN (...)` visibility batch (same bound as the account
/// export's hydration).
const VISIBLE_CHUNK: usize = 1000;

/// The subset of `issue_ids` that is live (`removed_at IS NULL`) and
/// visible to the caller under `acl` (library grant + age cap).
async fn visible_issue_ids(
    db: &DatabaseConnection,
    acl: &VisibleLibraries,
    issue_ids: &HashSet<String>,
) -> Result<HashSet<String>, sea_orm::DbErr> {
    let ids: Vec<String> = issue_ids.iter().cloned().collect();
    let mut out = HashSet::with_capacity(ids.len());
    for chunk in ids.chunks(VISIBLE_CHUNK) {
        let mut q = issue::Entity::find()
            .select_only()
            .column(issue::Column::Id)
            .filter(issue::Column::Id.is_in(chunk.to_vec()))
            .filter(issue::Column::RemovedAt.is_null());
        if let Some(cond) = acl.issue_filter() {
            q = q.filter(cond);
        }
        out.extend(q.into_tuple::<String>().all(db).await?);
    }
    Ok(out)
}

async fn build(
    db: &DatabaseConnection,
    user_id: Uuid,
    acl: &VisibleLibraries,
    base_url: &str,
) -> Result<NotesExport, sea_orm::DbErr> {
    let rows = marker::Entity::find()
        .filter(marker::Column::UserId.eq(user_id))
        .order_by_asc(marker::Column::CreatedAt)
        .order_by_asc(marker::Column::Id)
        .all(db)
        .await?;
    let total = rows.len() as u64;
    let issue_ids: HashSet<String> = rows.iter().map(|m| m.issue_id.clone()).collect();
    let visible = visible_issue_ids(db, acl, &issue_ids).await?;
    let series_ids = rows.iter().map(|m| m.series_id).collect();
    let refs = Refs::load(db, issue_ids, series_ids).await?;

    // series → issue → page → markers, each level in display order.
    type Pages = BTreeMap<i32, Vec<NotesExportMarker>>;
    type Issues = BTreeMap<IssueKey, (String, Pages)>;
    let mut tree: BTreeMap<SeriesKey, Issues> = BTreeMap::new();
    for m in rows {
        let issue_id = m.issue_id.clone();
        let page_index = m.page_index;
        let issue_row = refs.issues.get(&issue_id);
        // The issue's current series wins over the marker's stored
        // `series_id` (they only differ if the issue was re-homed).
        let series_id = issue_row.map_or(m.series_id, |i| i.series_id);
        let series_row = refs.series.get(&series_id);
        let series_key = (
            series_row
                .map(|s| s.name.to_lowercase())
                .unwrap_or_default(),
            series_row.and_then(|s| s.year),
            series_id,
        );
        let sort = issue_row.and_then(|i| i.sort_number);
        let issue_key = (
            sort.is_none(),
            OrdF64(sort.unwrap_or(0.0)),
            refs.issue(&issue_id).issue_number.unwrap_or_default(),
            issue_id.clone(),
        );
        let available = visible.contains(&issue_id);
        let jump_url = available.then(|| format!("{base_url}/markers/{}", m.id));
        let marker = export_marker(m, &refs);
        tree.entry(series_key)
            .or_default()
            .entry(issue_key)
            .or_insert_with(|| (issue_id, BTreeMap::new()))
            .1
            .entry(page_index)
            .or_default()
            .push(NotesExportMarker {
                marker,
                available,
                jump_url,
            });
    }

    let series = tree
        .into_iter()
        .map(|((_, _, series_id), issues)| NotesExportSeries {
            series: refs.series(series_id),
            issues: issues
                .into_values()
                .map(|(issue_id, pages)| NotesExportIssue {
                    issue: refs.issue(&issue_id),
                    issue_title: refs.issues.get(&issue_id).and_then(|i| i.title.clone()),
                    pages: pages
                        .into_iter()
                        .map(|(page_index, markers)| NotesExportPage {
                            page_index,
                            markers,
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();

    Ok(NotesExport {
        format: NOTES_EXPORT_FORMAT.to_owned(),
        version: NOTES_EXPORT_VERSION,
        exported_at: Utc::now().to_rfc3339(),
        total,
        series,
    })
}

// ───────── Markdown ─────────

fn kind_label(kind: &str) -> &str {
    match kind {
        "bookmark" => "Bookmark",
        "note" => "Note",
        "favorite" => "Favorite",
        "highlight" => "Highlight",
        other => other,
    }
}

/// Render the export as Markdown: `##` series, `###` issue, `####` page,
/// then one block per marker (kind line with the Jump link and tags, the
/// captured text as a blockquote, the note body verbatim). Pure function
/// of the document so the snapshot test pins the exact layout.
pub fn render_markdown(doc: &NotesExport) -> String {
    let mut out = String::new();
    let noun = if doc.total == 1 { "marker" } else { "markers" };
    let _ = writeln!(out, "# Folio notes\n");
    let _ = writeln!(out, "Exported {} · {} {noun}", doc.exported_at, doc.total);
    for s in &doc.series {
        let name = s.series.series_name.as_deref().unwrap_or("Unknown series");
        match s.series.series_year {
            Some(y) => {
                let _ = write!(out, "\n## {name} ({y})\n");
            }
            None => {
                let _ = write!(out, "\n## {name}\n");
            }
        }
        for i in &s.issues {
            let heading = match (i.issue.issue_number.as_deref(), i.issue_title.as_deref()) {
                (Some(n), Some(t)) => format!("#{n} · {t}"),
                (Some(n), None) => format!("#{n}"),
                (None, Some(t)) => t.to_owned(),
                (None, None) => "Issue".to_owned(),
            };
            let _ = write!(out, "\n### {heading}\n");
            for p in &i.pages {
                let _ = write!(out, "\n#### Page {}\n", p.page_index + 1);
                for nm in &p.markers {
                    render_marker(&mut out, nm);
                }
            }
        }
    }
    out
}

fn render_marker(out: &mut String, nm: &NotesExportMarker) {
    let m = &nm.marker;
    let star = if m.is_favorite { " ★" } else { "" };
    let _ = write!(out, "\n**{}**{star} · ", kind_label(&m.kind));
    match (&nm.jump_url, nm.available) {
        (Some(url), true) => {
            let _ = write!(out, "[Jump to page]({url})");
        }
        _ => out.push_str("(no longer available)"),
    }
    if !m.tags.is_empty() {
        let tags: Vec<String> = m.tags.iter().map(|t| format!("`{t}`")).collect();
        let _ = write!(out, " · tags: {}", tags.join(", "));
    }
    out.push('\n');
    let captured = m
        .selection
        .as_ref()
        .and_then(|s| s.get("text"))
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty());
    if let Some(text) = captured {
        out.push('\n');
        for line in text.lines() {
            if line.trim().is_empty() {
                out.push_str(">\n");
            } else {
                let _ = writeln!(out, "> {line}");
            }
        }
    }
    if let Some(body) = m.body.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        let _ = write!(out, "\n{body}\n");
    }
}
