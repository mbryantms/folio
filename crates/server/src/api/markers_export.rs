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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use axum::{
    Json,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::{issue, marker, user_issue_note, user_rating};
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
    /// Number of issue-level closing notes in the document.
    #[serde(default)]
    pub closing_notes: u64,
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
    /// The caller's issue-level closing note, when one exists. An issue
    /// with a closing note but no markers still appears, with `pages`
    /// empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closing_note: Option<NotesExportClosingNote>,
    /// Pages in reading order; only pages that carry a marker appear.
    pub pages: Vec<NotesExportPage>,
}

/// Issue-level closing note (`user_issue_notes`) — written from the
/// reader's end-of-issue Wrap up pane; not anchored to a page.
#[derive(Debug, Serialize, ToSchema)]
pub struct NotesExportClosingNote {
    pub body: String,
    /// The caller's 0..=5 rating of the issue at export time, if any.
    pub rating: Option<f64>,
    pub created_at: String,
    pub updated_at: String,
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
    // Issue-level closing notes ride along: an issue may have a note and
    // no markers, so its id joins the hydration set too.
    let note_rows = user_issue_note::Entity::find()
        .filter(user_issue_note::Column::UserId.eq(user_id))
        .order_by_asc(user_issue_note::Column::CreatedAt)
        .all(db)
        .await?;
    let closing_notes = note_rows.len() as u64;
    let mut issue_ids: HashSet<String> = rows.iter().map(|m| m.issue_id.clone()).collect();
    issue_ids.extend(note_rows.iter().map(|n| n.issue_id.clone()));
    let visible = visible_issue_ids(db, acl, &issue_ids).await?;
    let mut series_ids: HashSet<Uuid> = rows.iter().map(|m| m.series_id).collect();
    if !note_rows.is_empty() {
        // Series ids for note-only issues come from the issue rows.
        let note_issue_ids: Vec<String> = note_rows.iter().map(|n| n.issue_id.clone()).collect();
        series_ids.extend(
            issue::Entity::find()
                .select_only()
                .column(issue::Column::SeriesId)
                .filter(issue::Column::Id.is_in(note_issue_ids))
                .into_tuple::<Uuid>()
                .all(db)
                .await?,
        );
    }
    let refs = Refs::load(db, issue_ids, series_ids).await?;
    let ratings: HashMap<String, f64> = if note_rows.is_empty() {
        HashMap::new()
    } else {
        user_rating::Entity::find()
            .filter(user_rating::Column::UserId.eq(user_id))
            .filter(user_rating::Column::TargetType.eq("issue"))
            .filter(
                user_rating::Column::TargetId.is_in(
                    note_rows
                        .iter()
                        .map(|n| n.issue_id.clone())
                        .collect::<Vec<_>>(),
                ),
            )
            .all(db)
            .await?
            .into_iter()
            .map(|r| (r.target_id, r.rating))
            .collect()
    };

    // series → issue → (closing note, page → markers), each level in
    // display order.
    type Pages = BTreeMap<i32, Vec<NotesExportMarker>>;
    struct IssueBucket {
        issue_id: String,
        closing_note: Option<NotesExportClosingNote>,
        pages: Pages,
    }
    type Issues = BTreeMap<IssueKey, IssueBucket>;
    let mut tree: BTreeMap<SeriesKey, Issues> = BTreeMap::new();
    // Group keys for an issue. The issue's current series wins over a
    // marker's stored `series_id` (they only differ after a re-home).
    let keys_for = |issue_id: &str, fallback_series: Option<Uuid>| {
        let issue_row = refs.issues.get(issue_id);
        let series_id = issue_row
            .map(|i| i.series_id)
            .or(fallback_series)
            .unwrap_or_default();
        let series_row = refs.series.get(&series_id);
        let series_key: SeriesKey = (
            series_row
                .map(|s| s.name.to_lowercase())
                .unwrap_or_default(),
            series_row.and_then(|s| s.year),
            series_id,
        );
        let sort = issue_row.and_then(|i| i.sort_number);
        let issue_key: IssueKey = (
            sort.is_none(),
            OrdF64(sort.unwrap_or(0.0)),
            refs.issue(issue_id).issue_number.unwrap_or_default(),
            issue_id.to_owned(),
        );
        (series_key, issue_key)
    };
    for m in rows {
        let issue_id = m.issue_id.clone();
        let page_index = m.page_index;
        let (series_key, issue_key) = keys_for(&issue_id, Some(m.series_id));
        let available = visible.contains(&issue_id);
        let jump_url = available.then(|| format!("{base_url}/markers/{}", m.id));
        let marker = export_marker(m, &refs);
        tree.entry(series_key)
            .or_default()
            .entry(issue_key)
            .or_insert_with(|| IssueBucket {
                issue_id,
                closing_note: None,
                pages: BTreeMap::new(),
            })
            .pages
            .entry(page_index)
            .or_default()
            .push(NotesExportMarker {
                marker,
                available,
                jump_url,
            });
    }
    for n in note_rows {
        let (series_key, issue_key) = keys_for(&n.issue_id, None);
        let bucket = tree
            .entry(series_key)
            .or_default()
            .entry(issue_key)
            .or_insert_with(|| IssueBucket {
                issue_id: n.issue_id.clone(),
                closing_note: None,
                pages: BTreeMap::new(),
            });
        bucket.closing_note = Some(NotesExportClosingNote {
            rating: ratings.get(&n.issue_id).copied(),
            body: n.body,
            created_at: n.created_at.to_rfc3339(),
            updated_at: n.updated_at.to_rfc3339(),
        });
    }

    let series = tree
        .into_iter()
        .map(|((_, _, series_id), issues)| NotesExportSeries {
            series: refs.series(series_id),
            issues: issues
                .into_values()
                .map(|bucket| NotesExportIssue {
                    issue: refs.issue(&bucket.issue_id),
                    issue_title: refs
                        .issues
                        .get(&bucket.issue_id)
                        .and_then(|i| i.title.clone()),
                    closing_note: bucket.closing_note,
                    pages: bucket
                        .pages
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
        closing_notes,
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
    let _ = write!(out, "Exported {} · {} {noun}", doc.exported_at, doc.total);
    if doc.closing_notes > 0 {
        let n = doc.closing_notes;
        let noun = if n == 1 {
            "closing note"
        } else {
            "closing notes"
        };
        let _ = write!(out, " · {n} {noun}");
    }
    out.push('\n');
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
            if let Some(note) = &i.closing_note {
                render_closing_note(&mut out, note);
            }
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

/// `**Closing note** · ★★★★☆ (4/5)` then the body verbatim. Sits under
/// the issue heading, before any page.
fn render_closing_note(out: &mut String, note: &NotesExportClosingNote) {
    out.push_str("\n**Closing note**");
    if let Some(r) = note.rating {
        let full = r.floor() as usize;
        let half = r - r.floor() >= 0.5;
        let mut stars = "★".repeat(full.min(5));
        if half && full < 5 {
            stars.push('½');
        }
        let _ = write!(out, " · {stars} ({r}/5)");
    }
    out.push_str("\n\n");
    out.push_str(note.body.trim());
    out.push('\n');
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
