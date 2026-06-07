//! `/issues/{id}` — read, edit, and refresh-metadata endpoints.
//!
//! The DB schema column-set on `issues` is shared with the scanner, so a
//! `PATCH /issues/{id}` records its writes in `user_edited` to flag those
//! columns as sticky. The scanner's update path checks the flag set and
//! skips matching columns, preserving user edits across rescans.

use axum::{
    Extension, Json,
    extract::{Path as AxPath, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use entity::{issue, library, library_health_issue, library_user_access, series};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DbBackend, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    Set, Statement, Value, sea_query::Expr,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::library::access;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use uuid::Uuid;

use crate::api::libraries::{ScanMode, ScanResp};
use crate::audit::{self, AuditEntry};
use crate::auth::{CurrentUser, RequireAdmin};
use crate::middleware::RequestContext;
use crate::state::AppState;

use super::error;
use super::series::{IssueDetailView, IssueLink, IssueSummaryView};
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(search))
        .routes(routes!(bulk_metadata))
        .routes(routes!(get_one))
        .routes(routes!(metadata_overview))
        .routes(routes!(update))
        .routes(routes!(clear_field_pin))
        .routes(routes!(scan_issue))
        .routes(routes!(next_in_series))
        .routes(routes!(prev_in_series))
        .routes(routes!(list_issue_health))
}

/// Resolve `(series_slug, issue_slug)` to the canonical issue row. Returns
/// the standard 404 envelope on miss for either slug. Visibility-by-library
/// is the caller's responsibility.
pub(crate) async fn find_by_slugs(
    db: &sea_orm::DatabaseConnection,
    series_slug: &str,
    issue_slug: &str,
) -> Result<issue::Model, axum::response::Response> {
    let s = match series::Entity::find()
        .filter(series::Column::Slug.eq(series_slug))
        .one(db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return Err(error(
                StatusCode::NOT_FOUND,
                "not_found",
                "series not found",
            ));
        }
        Err(e) => {
            tracing::error!(error = %e, series_slug, "series slug lookup failed");
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ));
        }
    };
    match issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(s.id))
        .filter(issue::Column::Slug.eq(issue_slug))
        .one(db)
        .await
    {
        Ok(Some(r)) => Ok(r),
        Ok(None) => Err(error(StatusCode::NOT_FOUND, "not_found", "issue not found")),
        Err(e) => {
            tracing::error!(error = %e, issue_slug, "issue slug lookup failed");
            Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal",
            ))
        }
    }
}

// ───── GET /issues/{id} ─────

#[utoipa::path(
    operation_id = "issues_get_one",    get,
    path = "/series/{series_slug}/issues/{issue_slug}",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    responses(
        (status = 200, body = IssueDetailView),
        (status = 404)
    )
)]
#[handler]
pub async fn get_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
) -> impl IntoResponse {
    let row = match find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !visible_in_library(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }
    let rating = crate::api::series::lookup_user_rating(&app, user.id, "issue", &row.id).await;
    // Pull the parent series' and library's reading-direction overrides
    // so the reader can consult them in the resolution chain below
    // ComicInfo `<Manga>` but above the hard-coded LTR default.
    // See `manga-and-bulk-metadata-1.0` M1 + M2.
    let series_dir = series::Entity::find_by_id(row.series_id)
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .and_then(|s| s.reading_direction);
    let library_row = library::Entity::find_by_id(row.library_id)
        .one(&app.db)
        .await
        .ok()
        .flatten();
    let library_default_dir = library_row
        .as_ref()
        .map(|lib| lib.default_reading_direction.clone());
    let allow_archive_writeback = library_row
        .as_ref()
        .is_some_and(|lib| lib.allow_archive_writeback);
    let library_cbr_convert_confirmed = library_row
        .as_ref()
        .is_some_and(|lib| lib.cbr_convert_confirmed_at.is_some());
    // Creator-slug map for this issue's credits. One JOIN against
    // `person` — the FK populated by the scanner's series rollup. The
    // UI uses this so credit chips link to /creators/<slug> directly
    // (matching how every other detail-page chip resolves).
    let creator_slugs = build_issue_creator_slugs(&app, &row.id).await;
    let issue_id = row.id.clone();
    let mut view = IssueDetailView::from_model(row, &series_slug);
    view.user_rating = rating;
    view.series_reading_direction = series_dir;
    view.library_default_reading_direction = library_default_dir;
    view.allow_archive_writeback = allow_archive_writeback;
    view.library_cbr_convert_confirmed = library_cbr_convert_confirmed;
    view.creator_slugs = creator_slugs;
    crate::api::series::enrich_issue_detail_legacy_ids(&app.db, &mut view, &issue_id).await;
    view.metadata_completeness = Some(crate::api::series::assess_issue_view(&view));
    Json(view).into_response()
}

// ───── GET /series/{slug}/issues/{slug}/metadata-overview ─────

/// One field's provenance: which source set it, when, and (for provider
/// sources) which external record it came from.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FieldProvenanceRow {
    /// `MetadataField::key()` value (e.g. `"summary"`, `"credits"`).
    pub field: String,
    /// Raw provenance code (`"user"`, `"comicinfo"`, `"metron"`, …).
    pub set_by: String,
    /// Human label for `set_by` (e.g. `"ComicInfo.xml"`, `"You"`).
    pub source_label: String,
    pub set_at: String,
    pub source_external_id: Option<String>,
}

/// Which metadata source files the scanner found for this issue. ComicInfo
/// presence comes from `comic_info_raw`; MetronInfo from
/// `issue.metroninfo_present`; series.json (a per-folder file) from the parent
/// `series.series_json_present`. Each is `"present"` | `"absent"` |
/// `"unknown"` (the last meaning the row was scanned before the tracking
/// column existed — a rescan resolves it).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SourceFilesView {
    pub comicinfo: String,
    pub metroninfo: String,
    pub series_json: String,
}

/// Total metadata overview for an issue: completeness, source files,
/// freshness, per-field provenance, external IDs, and user-pinned fields.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MetadataOverviewView {
    pub completeness: Option<crate::metadata::completeness::CompletenessReport>,
    pub source_files: SourceFilesView,
    pub external_ids: Vec<crate::api::external_ids::ExternalIdRow>,
    pub provenance: Vec<FieldProvenanceRow>,
    pub user_edited: Vec<String>,
    pub last_metadata_sync_at: Option<String>,
    pub last_rewrite_at: Option<String>,
    pub last_rewrite_kind: Option<String>,
}

/// Human label for a `field_provenance.set_by` code.
fn provenance_source_label(set_by: &str) -> &'static str {
    match set_by {
        "user" => "You",
        "comicinfo" => "ComicInfo.xml",
        "metroninfo" => "MetronInfo.xml",
        "comicvine" => "ComicVine",
        "metron" => "Metron",
        "scanner_inference" => "Scanner (filename)",
        "scanner_folder_tag" => "Scanner (folder)",
        "cross_reference" => "Cross-reference",
        "migration_v1" => "Migration",
        _ => "Other",
    }
}

#[utoipa::path(
    operation_id = "issues_metadata_overview",    get,
    path = "/series/{series_slug}/issues/{issue_slug}/metadata-overview",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    responses(
        (status = 200, body = MetadataOverviewView),
        (status = 404)
    )
)]
#[handler]
pub async fn metadata_overview(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
) -> impl IntoResponse {
    let row = match find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !visible_in_library(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }

    let issue_id = row.id.clone();
    let series_id = row.series_id;
    let has_comicinfo = !row.comic_info_raw.is_null();
    let metroninfo_present = row.metroninfo_present;
    let last_metadata_sync_at = row.last_metadata_sync_at.map(|t| t.to_rfc3339());
    let last_rewrite_at = row.last_rewrite_at.map(|t| t.to_rfc3339());
    let last_rewrite_kind = row.last_rewrite_kind.clone();
    let user_edited: Vec<String> =
        serde_json::from_value(row.user_edited.clone()).unwrap_or_default();

    // series.json is a per-folder file → its presence lives on the parent
    // series row.
    let series_json_present = series::Entity::find_by_id(series_id)
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .and_then(|s| s.series_json_present);

    // Completeness via the shared scorer — build the view the same way
    // `get_one` does so the provider-match signal sees the legacy CV/Metron ids.
    let mut view = IssueDetailView::from_model(row, &series_slug);
    crate::api::series::enrich_issue_detail_legacy_ids(&app.db, &mut view, &issue_id).await;
    let completeness = Some(crate::api::series::assess_issue_view(&view));

    // Provenance rows (field → source → when), most-recent first.
    let provenance: Vec<FieldProvenanceRow> =
        crate::metadata::apply::fetch_field_provenance_rows(&app.db, "issue", &issue_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|p| FieldProvenanceRow {
                source_label: provenance_source_label(&p.set_by).to_owned(),
                field: p.field,
                set_by: p.set_by,
                set_at: p.set_at.to_rfc3339(),
                source_external_id: p.source_external_id,
            })
            .collect();

    let external_ids = crate::api::external_ids::fetch_rows(&app, "issue", &issue_id).await;

    Json(MetadataOverviewView {
        completeness,
        source_files: SourceFilesView {
            comicinfo: if has_comicinfo { "present" } else { "absent" }.to_owned(),
            metroninfo: presence_str(metroninfo_present).to_owned(),
            series_json: presence_str(series_json_present).to_owned(),
        },
        external_ids,
        provenance,
        user_edited,
        last_metadata_sync_at,
        last_rewrite_at,
        last_rewrite_kind,
    })
    .into_response()
}

/// Map a nullable scanner presence flag to the wire string. `None` = scanned
/// before the column existed (rescan to learn), distinct from a definite
/// `"absent"`.
fn presence_str(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "present",
        Some(false) => "absent",
        None => "unknown",
    }
}

async fn build_issue_creator_slugs(
    app: &AppState,
    issue_id: &str,
) -> std::collections::HashMap<String, String> {
    use sea_orm::{ConnectionTrait, FromQueryResult, Statement};
    #[derive(Debug, FromQueryResult)]
    struct Row {
        person: String,
        slug: String,
    }
    Row::find_by_statement(Statement::from_sql_and_values(
        app.db.get_database_backend(),
        "SELECT DISTINCT ic.person AS person, p.slug AS slug \
         FROM issue_credits ic \
         JOIN person p ON p.id = ic.person_id \
         WHERE ic.issue_id = $1",
        [issue_id.into()],
    ))
    .all(&app.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| (r.person, r.slug))
    .collect()
}

// ───── PATCH /issues/{id} ─────

/// Body for `PATCH /series/{series_slug}/issues/{issue_slug}`.
///
/// Every field is optional. For nullable columns the body distinguishes:
///   - field absent     → leave column untouched
///   - field present, null → clear column, mark as user-edited
///   - field present, set  → write column, mark as user-edited
///
/// `additional_links` is replace-all: send the full desired array, or `[]`
/// to clear. Empty / whitespace-only `url` entries are rejected.
///
/// Mirrors the editable subset of ComicInfo.xml — fields the scanner reads
/// from the file. The scanner consults `user_edited` on rescan and skips
/// matching columns, so DB edits are sticky and the source file is never
/// rewritten.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct UpdateIssueReq {
    // Identity / publication
    #[serde(default, deserialize_with = "deserialize_some")]
    pub title: Option<Option<String>>,
    /// Maps to the entity's `number_raw` column (e.g. "1", "1.5", "Annual 2").
    #[serde(default, deserialize_with = "deserialize_some")]
    pub number: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub volume: Option<Option<i32>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub year: Option<Option<i32>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub month: Option<Option<i32>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub day: Option<Option<i32>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub summary: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub notes: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub publisher: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub imprint: Option<Option<String>>,

    // Credits
    #[serde(default, deserialize_with = "deserialize_some")]
    pub writer: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub penciller: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub inker: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub colorist: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub letterer: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub cover_artist: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub editor: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub translator: Option<Option<String>>,

    // Cast / setting / story
    #[serde(default, deserialize_with = "deserialize_some")]
    pub characters: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub teams: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub locations: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub alternate_series: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub story_arc: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub story_arc_number: Option<Option<String>>,

    // Classification
    #[serde(default, deserialize_with = "deserialize_some")]
    pub genre: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub tags: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub language_code: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub age_rating: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub format: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub black_and_white: Option<Option<bool>>,
    /// One of `Yes`, `YesAndRightToLeft`, `No`, or null.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub manga: Option<Option<String>>,

    // Ordering / external
    #[serde(default, deserialize_with = "deserialize_some")]
    pub sort_number: Option<Option<f64>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub web_url: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub gtin: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub comicvine_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub metron_id: Option<Option<i64>>,

    /// Replace-all. Each link must have a non-empty `url`.
    pub additional_links: Option<Vec<IssueLink>>,
}

fn deserialize_some<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Trim, then collapse empty strings to `None`. Matches scanner behavior so
/// the DB never contains whitespace-only / empty CSV strings.
fn norm_str(v: Option<String>) -> Option<String> {
    v.and_then(|s| {
        let t = s.trim().to_owned();
        if t.is_empty() { None } else { Some(t) }
    })
}

/// Log a user external-ID edit that was skipped because the ID is
/// already assigned to another item (the cross-entity unique). Maps the
/// write outcome to `()` so it composes with the sibling `delete`
/// arm. The user-facing `<ExternalIdsCard>` add path returns a 409
/// instead; these legacy PATCH fields stay best-effort + logged.
fn log_external_id_skip(
    issue_id: &str,
    field: &str,
    outcome: crate::metadata::writers::SetExternalIdOutcome,
) {
    if let crate::metadata::writers::SetExternalIdOutcome::SkippedConflict { owner } = outcome {
        tracing::warn!(
            issue_id,
            field,
            owner,
            "issue external_id edit skipped: identifier already assigned to another item"
        );
    }
}

#[utoipa::path(
    operation_id = "issues_update",    patch,
    path = "/series/{series_slug}/issues/{issue_slug}",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    request_body = UpdateIssueReq,
    responses(
        (status = 200, body = IssueDetailView),
        (status = 400, description = "validation error"),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
    )
)]
#[handler]
pub async fn update(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
    Json(req): Json<UpdateIssueReq>,
) -> impl IntoResponse {
    let row = match find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let id = row.id.clone();
    if !visible_in_library(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }

    // ── pre-write validation (everything that can reject up front, before
    // any active model writes) ──
    if let Some(links) = req.additional_links.as_ref() {
        for l in links {
            if l.url.trim().is_empty() {
                return error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation.additional_links",
                    "each link needs a non-empty url",
                );
            }
            // No URL parsing — accept anything non-empty so the user can
            // store internal notes like "wiki:foo". Downstream renderers
            // treat the value as plain text if it's not a valid href.
        }
    }
    if let Some(Some(f)) = req.sort_number
        && !f.is_finite()
    {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.sort_number",
            "sort_number must be finite",
        );
    }
    if let Some(Some(y)) = req.year
        && !(1800..=2999).contains(&y)
    {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.year",
            "year out of range",
        );
    }
    if let Some(Some(m)) = req.month
        && !(1..=12).contains(&m)
    {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.month",
            "month must be 1..=12",
        );
    }
    if let Some(Some(d)) = req.day
        && !(1..=31).contains(&d)
    {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.day",
            "day must be 1..=31",
        );
    }
    if let Some(Some(v)) = req.volume
        && !(0..=9999).contains(&v)
    {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.volume",
            "volume out of range",
        );
    }
    if let Some(Some(ref s)) = req.manga {
        let t = s.trim();
        if !matches!(t, "Yes" | "YesAndRightToLeft" | "No") {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation.manga",
                "manga must be Yes, YesAndRightToLeft, or No",
            );
        }
    }
    if let Some(Some(ref s)) = req.language_code
        && s.len() > 16
    {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.language_code",
            "language_code too long",
        );
    }

    // Carry forward existing edited-flag set; new writes append to it.
    let mut edited: BTreeSet<String> = match row.user_edited.as_array() {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        None => BTreeSet::new(),
    };

    // Track what changed so the audit payload reflects the actual diff.
    let mut changes = serde_json::Map::new();

    let mut am: issue::ActiveModel = row.clone().into();
    let mut touched = false;

    // ── nullable string columns ──
    macro_rules! apply_str {
        ($req_field:ident, $col:ident, $name:literal) => {
            if let Some(v) = req.$req_field {
                let normalized = norm_str(v);
                am.$col = Set(normalized.clone());
                edited.insert($name.into());
                changes.insert($name.into(), serde_json::json!(normalized));
                touched = true;
            }
        };
    }
    apply_str!(title, title, "title");
    apply_str!(number, number_raw, "number_raw");
    apply_str!(summary, summary, "summary");
    apply_str!(notes, notes, "notes");
    apply_str!(publisher, publisher, "publisher");
    apply_str!(imprint, imprint, "imprint");
    apply_str!(writer, writer, "writer");
    apply_str!(penciller, penciller, "penciller");
    apply_str!(inker, inker, "inker");
    apply_str!(colorist, colorist, "colorist");
    apply_str!(letterer, letterer, "letterer");
    apply_str!(cover_artist, cover_artist, "cover_artist");
    apply_str!(editor, editor, "editor");
    apply_str!(translator, translator, "translator");
    apply_str!(characters, characters, "characters");
    apply_str!(teams, teams, "teams");
    apply_str!(locations, locations, "locations");
    apply_str!(alternate_series, alternate_series, "alternate_series");
    apply_str!(story_arc, story_arc, "story_arc");
    apply_str!(story_arc_number, story_arc_number, "story_arc_number");
    apply_str!(genre, genre, "genre");
    apply_str!(tags, tags, "tags");
    apply_str!(language_code, language_code, "language_code");
    apply_str!(age_rating, age_rating, "age_rating");
    apply_str!(format, format, "format");
    apply_str!(manga, manga, "manga");
    apply_str!(web_url, web_url, "web_url");
    // gtin / comicvine_id / metron_id are on external_ids now, not
    // the issue row. Track what the user touched here so we can
    // apply via writers::set_external_id after the row update commits.
    // None = field absent from request; Some(None) = explicit clear;
    // Some(Some(v)) = set-to-v. SetBy::User skips writers's user-
    // precedence gate (a user write always overrides a prior user
    // write, which is exactly what we want).
    let pending_gtin = req.gtin.clone();
    let pending_cv = req.comicvine_id;
    let pending_metron = req.metron_id;
    if pending_gtin.is_some() {
        edited.insert("gtin".into());
        changes.insert(
            "gtin".into(),
            serde_json::json!(pending_gtin.as_ref().and_then(|o| o.clone())),
        );
        touched = true;
    }

    // ── nullable scalar columns ──
    if let Some(v) = req.volume {
        am.volume = Set(v);
        edited.insert("volume".into());
        changes.insert("volume".into(), serde_json::json!(v));
        touched = true;
    }
    if let Some(v) = req.year {
        am.year = Set(v);
        edited.insert("year".into());
        changes.insert("year".into(), serde_json::json!(v));
        touched = true;
    }
    if let Some(v) = req.month {
        am.month = Set(v);
        edited.insert("month".into());
        changes.insert("month".into(), serde_json::json!(v));
        touched = true;
    }
    if let Some(v) = req.day {
        am.day = Set(v);
        edited.insert("day".into());
        changes.insert("day".into(), serde_json::json!(v));
        touched = true;
    }
    if let Some(v) = req.black_and_white {
        am.black_and_white = Set(v);
        edited.insert("black_and_white".into());
        changes.insert("black_and_white".into(), serde_json::json!(v));
        touched = true;
    }
    if let Some(v) = req.sort_number {
        am.sort_number = Set(v);
        edited.insert("sort_number".into());
        changes.insert("sort_number".into(), serde_json::json!(v));
        touched = true;
    }
    if let Some(opt) = pending_cv {
        edited.insert("comicvine_id".into());
        changes.insert("comicvine_id".into(), serde_json::json!(opt));
        touched = true;
    }
    if let Some(opt) = pending_metron {
        edited.insert("metron_id".into());
        changes.insert("metron_id".into(), serde_json::json!(opt));
        touched = true;
    }

    if let Some(links) = req.additional_links {
        let normalized: Vec<IssueLink> = links
            .into_iter()
            .map(|l| IssueLink {
                label: norm_str(l.label),
                url: l.url.trim().to_owned(),
            })
            .collect();
        let json = serde_json::to_value(&normalized).unwrap_or(serde_json::json!([]));
        am.additional_links = Set(json.clone());
        // additional_links has no scanner counterpart so we don't add it to
        // `user_edited`; the scanner never touches it.
        changes.insert("additional_links".into(), json);
        touched = true;
    }

    if !touched {
        let row_id = row.id.clone();
        let mut view = IssueDetailView::from_model(row, &series_slug);
        crate::api::series::enrich_issue_detail_legacy_ids(&app.db, &mut view, &row_id).await;
        return Json(view).into_response();
    }

    let edited_arr: Vec<String> = edited.into_iter().collect();
    am.user_edited = Set(serde_json::json!(edited_arr));
    am.updated_at = Set(chrono::Utc::now().fixed_offset());

    let updated = match am.update(&app.db).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(issue_id = %id, error = %e, "update issue failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    // Dual-write to field_provenance for every touched scalar that
    // maps to a typed MetadataField. The legacy user_edited JSON
    // column stays in place as the scanner's user-precedence source
    // — this is the de-risking work for the upcoming metadata-
    // sidecar-writeback plan, whose composer reads field_provenance
    // to preserve user pins across provider applies.
    //
    // Failures here are logged but don't fail the PATCH — the row
    // already updated, and the next provider apply will overwrite
    // the field_provenance row anyway. Don't double-roll-back.
    for key in &edited_arr {
        if let Some(field) = patch_field_key_to_metadata_field(key)
            && let Err(e) = crate::metadata::writers::write_field_provenance(
                &app.db,
                "issue",
                &updated.id,
                field,
                crate::metadata::writers::SetBy::User,
                None,
            )
            .await
        {
            tracing::warn!(
                issue_id = %updated.id,
                field = %key,
                error = %e,
                "issue PATCH: field_provenance dual-write failed (non-fatal)"
            );
        }
    }

    // Apply external-ID edits the user touched. Set-to-value writes
    // route through writers::set_external_id (set_by='user');
    // explicit clears delete the row outright.
    use crate::metadata::writers::{self as writers, SetBy};
    use crate::metadata::{Identifier, Source};
    if let Some(opt) = pending_gtin {
        let res = match opt {
            Some(v) if !v.is_empty() => writers::set_external_id(
                &app.db,
                "issue",
                &updated.id,
                &Identifier::new(Source::Gtin, v),
                SetBy::User,
            )
            .await
            .map(|o| log_external_id_skip(&updated.id, "gtin", o)),
            _ => writers::delete_external_id(&app.db, "issue", &updated.id, Source::Gtin).await,
        };
        if let Err(e) = res {
            tracing::error!(issue_id = %updated.id, error = %e, "issue external_id (gtin) write failed");
        }
    }
    if let Some(opt) = pending_cv {
        let res = match opt {
            Some(v) => writers::set_external_id(
                &app.db,
                "issue",
                &updated.id,
                &Identifier::new(Source::ComicVine, v.to_string()),
                SetBy::User,
            )
            .await
            .map(|o| log_external_id_skip(&updated.id, "comicvine", o)),
            None => {
                writers::delete_external_id(&app.db, "issue", &updated.id, Source::ComicVine).await
            }
        };
        if let Err(e) = res {
            tracing::error!(issue_id = %updated.id, error = %e, "issue external_id (comicvine) write failed");
        }
    }
    if let Some(opt) = pending_metron {
        let res = match opt {
            Some(v) => writers::set_external_id(
                &app.db,
                "issue",
                &updated.id,
                &Identifier::new(Source::Metron, v.to_string()),
                SetBy::User,
            )
            .await
            .map(|o| log_external_id_skip(&updated.id, "metron", o)),
            None => {
                writers::delete_external_id(&app.db, "issue", &updated.id, Source::Metron).await
            }
        };
        if let Err(e) = res {
            tracing::error!(issue_id = %updated.id, error = %e, "issue external_id (metron) write failed");
        }
    }

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "admin.issue.update",
            target_type: Some("issue"),
            target_id: Some(updated.id.clone()),
            payload: serde_json::json!({
                "changes": changes,
                "user_edited": edited_arr,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    let updated_id = updated.id.clone();
    let mut view = IssueDetailView::from_model(updated, &series_slug);
    crate::api::series::enrich_issue_detail_legacy_ids(&app.db, &mut view, &updated_id).await;
    Json(view).into_response()
}

// ───── POST /issues/{id}/scan ─────

/// Optional query params for the scan-issue endpoint.
#[derive(Debug, Default, Deserialize)]
pub struct ScanIssueQuery {
    /// Defaults to `true` — clicking "Scan issue" is an explicit user
    /// request, so re-parse the file even if its mtime hasn't moved. The
    /// query string can opt back into the cheap fast path with `?force=false`
    /// (mostly useful for the file-watch trigger, not the UI).
    #[serde(default = "default_true")]
    pub force: bool,
}

fn default_true() -> bool {
    true
}

#[utoipa::path(
    operation_id = "issues_clear_field_pin",
    delete,
    path = "/series/{series_slug}/issues/{issue_slug}/field-provenance/{field}",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
        ("field" = String, Path, description = "MetadataField::key() — e.g. `title`, `credits`, `cover.variants`"),
    ),
    responses(
        (status = 200, description = "pin cleared (or no-op when none existed)"),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
    )
)]
#[handler]
pub async fn clear_field_pin(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath((series_slug, issue_slug, field)): AxPath<(String, String, String)>,
) -> impl IntoResponse {
    let row = match find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !visible_in_library(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }
    // Also drop the field from `issue.user_edited` — the JSON list the
    // scanner consults to skip user-pinned columns on rescan. Without
    // this the next scan would still treat the field as user-pinned
    // (the list and the field_provenance row are paired bookkeeping).
    let cleared =
        match crate::metadata::writers::clear_user_pin(&app.db, "issue", &row.id, &field).await {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(issue_id = row.id, field, error = %e, "clear_field_pin failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };
    // user_edited JSON list — best-effort sync.
    if cleared {
        let mut edited: Vec<String> =
            serde_json::from_value(row.user_edited.clone()).unwrap_or_default();
        let before = edited.len();
        edited.retain(|f| f != &field);
        if edited.len() != before {
            let mut am: issue::ActiveModel = row.clone().into();
            am.user_edited = sea_orm::Set(serde_json::to_value(&edited).unwrap_or_default());
            am.updated_at = sea_orm::Set(chrono::Utc::now().fixed_offset());
            if let Err(e) = am.update(&app.db).await {
                tracing::warn!(issue_id = row.id, field, error = %e, "user_edited sync failed");
            }
        }
    }

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "admin.issue.field_pin_clear",
            target_type: Some("issue"),
            target_id: Some(row.id.clone()),
            payload: serde_json::json!({"field": field, "cleared": cleared}),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    (
        StatusCode::OK,
        Json(serde_json::json!({"cleared": cleared})),
    )
        .into_response()
}

#[utoipa::path(
    operation_id = "issues_scan_issue",    post,
    path = "/series/{series_slug}/issues/{issue_slug}/scan",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
        ("force" = Option<bool>, Query, description = "Bypass the size+mtime fast path. Defaults to true."),
    ),
    responses(
        (status = 202, description = "issue scan job enqueued"),
        (status = 403, description = "admin only"),
        (status = 404, description = "issue not found"),
    )
)]
#[handler]
pub async fn scan_issue(
    State(app): State<AppState>,
    RequireAdmin(user): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
    Query(q): Query<ScanIssueQuery>,
) -> impl IntoResponse {
    let row = match find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let id = row.id.clone();
    let outcome = match app
        .jobs
        .coalesce_scoped_scan(
            row.library_id,
            row.series_id,
            None,
            crate::jobs::scan_series::JobKind::Issue,
            Some(id.clone()),
            q.force,
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(e) => {
            tracing::error!(issue_id = %id, error = %e, "scan_issue enqueue failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "admin.issue.scan",
            target_type: Some("issue"),
            target_id: Some(id.clone()),
            payload: serde_json::json!({
                "series_id": row.series_id.to_string(),
                "force": q.force,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    let mode = if q.force {
        ScanMode::ContentVerify
    } else {
        ScanMode::Normal
    };
    (
        StatusCode::ACCEPTED,
        Json(ScanResp {
            scan_id: outcome.scan_id().to_string(),
            state: if outcome.was_coalesced() {
                "coalesced"
            } else {
                "queued"
            },
            coalesced: outcome.was_coalesced(),
            kind: "issue",
            library_id: row.library_id.to_string(),
            mode: mode.as_str(),
            coalesced_into: outcome
                .was_coalesced()
                .then(|| outcome.scan_id().to_string()),
            queued_followup: false,
            reason: mode.reason().to_owned(),
            series_id: Some(row.series_id.to_string()),
            issue_id: Some(id),
        }),
    )
        .into_response()
}

// ───── GET /issues/{id}/next ─────

#[derive(Debug, Default, Deserialize)]
pub struct NextInSeriesQuery {
    /// Number of upcoming issues to return. Clamped to 1..=20, default 5.
    pub limit: Option<u64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct NextInSeriesView {
    pub items: Vec<IssueSummaryView>,
}

/// Returns the next N issues in the same series, ordered by `sort_number`
/// ASC (NULLS LAST) with `id` as a stable tie-breaker. Removed / soft-deleted
/// issues are filtered out so the list mirrors the series page. The current
/// issue is excluded from the result.
#[utoipa::path(
    operation_id = "issues_next_in_series",    get,
    path = "/series/{series_slug}/issues/{issue_slug}/next",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
        ("limit" = Option<u64>, Query, description = "Max upcoming issues (1..=20, default 5)"),
    ),
    responses(
        (status = 200, body = NextInSeriesView),
        (status = 404, description = "issue not found"),
    )
)]
#[handler]
pub async fn next_in_series(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
    Query(q): Query<NextInSeriesQuery>,
) -> impl IntoResponse {
    let row = match find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !visible_in_library(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }
    let limit = q.limit.unwrap_or(5).clamp(1, 20);

    // Match the series-page sort: sort_number ASC NULLS LAST, then id.
    // The "next" cursor is the (sort_number, id) tuple of the current row;
    // anything strictly after is a candidate.
    let mut select = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(row.series_id))
        .filter(issue::Column::RemovedAt.is_null())
        .filter(issue::Column::Id.ne(row.id.clone()));

    // Sort handling — emulate "NULLS LAST" via a synthesized ASC bool.
    let nulls_last = Expr::cust("sort_number IS NULL");
    select = select
        .order_by_asc(nulls_last)
        .order_by_asc(issue::Column::SortNumber)
        .order_by_asc(issue::Column::Id);

    // Cursor: only rows that come *after* the current row in the same sort.
    // (sort_number IS NULL) covers the "current row has a number, NULL rows
    // come after"; the > / = clauses cover the strict-greater + tiebreak.
    select = match row.sort_number {
        Some(curr) => select.filter(Expr::cust_with_values(
            "(sort_number IS NULL) OR (sort_number > $1) OR (sort_number = $1 AND id > $2)",
            vec![Value::from(curr), Value::from(row.id.clone())],
        )),
        // Current row has no sort_number — NULLS LAST means the only "after"
        // candidates are other NULL rows with a larger id.
        None => select
            .filter(Expr::cust("sort_number IS NULL"))
            .filter(issue::Column::Id.gt(row.id.clone())),
    };

    let rows: Vec<issue::Model> = match select.limit(limit).all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "next_in_series query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let items = rows
        .into_iter()
        .map(|m| IssueSummaryView::from_model(m, &series_slug))
        .collect();
    Json(NextInSeriesView { items }).into_response()
}

// ───── GET /series/{series_slug}/issues/{issue_slug}/prev ─────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PrevInSeriesView {
    /// The issue immediately preceding the current one in `sort_number`
    /// order, or `null` when the current issue is the first in the series.
    pub item: Option<IssueSummaryView>,
}

/// Returns the single issue immediately *before* the current one in the same
/// series — the mirror of [`next_in_series`], using the same ordering
/// (`sort_number` ASC NULLS LAST, `id` tie-breaker) reversed so the immediate
/// predecessor is selected. Removed / soft-deleted issues are filtered out.
/// `item` is `null` when the current issue is the first in the series.
#[utoipa::path(
    operation_id = "issues_prev_in_series",
    get,
    path = "/series/{series_slug}/issues/{issue_slug}/prev",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    responses(
        (status = 200, body = PrevInSeriesView),
        (status = 404, description = "issue not found"),
    )
)]
#[handler]
pub async fn prev_in_series(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
) -> impl IntoResponse {
    let row = match find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !visible_in_library(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }

    let mut select = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(row.series_id))
        .filter(issue::Column::RemovedAt.is_null())
        .filter(issue::Column::Id.ne(row.id.clone()));

    // Reverse of next_in_series' sort so `.one()` yields the *immediate*
    // predecessor: (sort_number IS NULL) DESC, sort_number DESC, id DESC.
    let nulls_first = Expr::cust("sort_number IS NULL");
    select = select
        .order_by_desc(nulls_first)
        .order_by_desc(issue::Column::SortNumber)
        .order_by_desc(issue::Column::Id);

    // Candidates strictly *before* the current row in the ASC ordering — the
    // complement of next_in_series' "after" predicate.
    select = match row.sort_number {
        Some(curr) => select.filter(Expr::cust_with_values(
            "(sort_number IS NOT NULL) AND ((sort_number < $1) OR (sort_number = $1 AND id < $2))",
            vec![Value::from(curr), Value::from(row.id.clone())],
        )),
        // Current row is in the NULLS-LAST bucket: every non-null row precedes
        // it, as do NULL rows with a smaller id.
        None => select.filter(Expr::cust_with_values(
            "(sort_number IS NOT NULL) OR (sort_number IS NULL AND id < $1)",
            vec![Value::from(row.id.clone())],
        )),
    };

    let prev: Option<issue::Model> = match select.one(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "prev_in_series query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let item = prev.map(|m| IssueSummaryView::from_model(m, &series_slug));
    Json(PrevInSeriesView { item }).into_response()
}

// ───── GET /series/{series_slug}/issues/{issue_slug}/health-issues ─────

/// Tranche B of recovery-visibility (`~/.claude/plans/recovery-visibility-1.0.md`).
/// Returns the open `library_health_issues` rows whose payload `path`
/// matches this issue's file. The issue detail page renders a small
/// badge when this list is non-empty, and the reader fires a one-time
/// toast on issue open. Resolved + dismissed rows are excluded — the
/// badge represents "things you can still act on right now."
///
/// Auth: regular `CurrentUser` (NOT admin-only). Any user with library
/// access can see whether the file they're about to read is partial /
/// recovered. The admin Health tab is still the place to dismiss or
/// triage.
#[utoipa::path(
    operation_id = "issues_list_issue_health",    get,
    path = "/series/{series_slug}/issues/{issue_slug}/health-issues",
    params(
        ("series_slug" = String, Path,),
        ("issue_slug" = String, Path,),
    ),
    responses(
        (status = 200, body = Vec<crate::api::health_issues::HealthIssueView>),
        (status = 404, description = "issue not found"),
    )
)]
#[handler]
pub async fn list_issue_health(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((series_slug, issue_slug)): AxPath<(String, String)>,
) -> impl IntoResponse {
    let row = match find_by_slugs(&app.db, &series_slug, &issue_slug).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if !visible_in_library(&app, &user, row.library_id).await {
        return error(StatusCode::NOT_FOUND, "not_found", "issue not found");
    }

    // Health-issue payloads carry the issue's file path under
    // `data.path` (per `IssueKind`'s serde tag/content layout). Match
    // on that JSON expression. Postgres-only; the workspace targets
    // Postgres so no portability concern.
    let rows = match library_health_issue::Entity::find()
        .from_raw_sql(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT * FROM library_health_issues
                WHERE library_id = $1
                  AND resolved_at IS NULL
                  AND dismissed_at IS NULL
                  AND payload->'data'->>'path' = $2
                ORDER BY severity DESC, last_seen_at DESC"#,
            [row.library_id.into(), row.file_path.clone().into()],
        ))
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "list issue health failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    Json(
        rows.into_iter()
            .map(crate::api::health_issues::HealthIssueView::from)
            .collect::<Vec<_>>(),
    )
    .into_response()
}

// ───── PATCH /me/issues/bulk-metadata ─────────────────────────────────────

/// Per-field patch surface for the bulk-edit dialog
/// (`manga-and-bulk-metadata-1.0` M4). Every field is independently
/// optional. Sending `null` for a nullable field clears it; omitting
/// leaves it untouched.
///
/// **Credit fields are deliberately excluded** (writer, penciller,
/// cover_artist, editor, translator, inker, colorist, letterer):
/// these vary issue-to-issue in real series (guest artists, variant
/// covers, mid-series translator changes) and bulk-editing them
/// risks clobbering accurate per-issue credits. Continue using the
/// per-issue drawer for credits.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct BulkMetadataPatch {
    /// ISO-639-1 language code (`"ja"`, `"en"`, …). `null` clears.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub language_code: Option<Option<String>>,
    /// ComicInfo Manga: `"No"` / `"Yes"` / `"YesAndRightToLeft"`.
    /// `null` clears.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub manga: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub publisher: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub imprint: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub age_rating: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub format: Option<Option<String>>,
    /// CSV. Replaces the field wholesale; for additive tag operations
    /// the M5 dialog assembles the union client-side and sends it
    /// here.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub genre: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub tags: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub story_arc: Option<Option<String>>,
}

impl BulkMetadataPatch {
    /// `true` when no fields were set (all `None`). The handler
    /// returns an empty-counts response without touching the DB in
    /// that case.
    fn is_empty(&self) -> bool {
        self.language_code.is_none()
            && self.manga.is_none()
            && self.publisher.is_none()
            && self.imprint.is_none()
            && self.age_rating.is_none()
            && self.format.is_none()
            && self.genre.is_none()
            && self.tags.is_none()
            && self.story_arc.is_none()
    }

    /// Names of fields the caller actually included in the patch.
    /// Used to populate `issue.user_edited` so the scanner skips
    /// them on rescan and the audit-log row stays concise.
    fn touched_field_names(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.language_code.is_some() {
            out.push("language_code");
        }
        if self.manga.is_some() {
            out.push("manga");
        }
        if self.publisher.is_some() {
            out.push("publisher");
        }
        if self.imprint.is_some() {
            out.push("imprint");
        }
        if self.age_rating.is_some() {
            out.push("age_rating");
        }
        if self.format.is_some() {
            out.push("format");
        }
        if self.genre.is_some() {
            out.push("genre");
        }
        if self.tags.is_some() {
            out.push("tags");
        }
        if self.story_arc.is_some() {
            out.push("story_arc");
        }
        out
    }
}

#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BulkMode {
    /// Update only issues where the targeted column is currently
    /// `NULL`. Default — destructive overwrites require an explicit
    /// `replace` opt-in.
    #[default]
    SkipIfSet,
    /// Update unconditionally. Caller has confirmed they want to
    /// clobber per-issue values.
    Replace,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct BulkMetadataReq {
    pub issue_ids: Vec<String>,
    pub patch: BulkMetadataPatch,
    #[serde(default)]
    pub mode: BulkMode,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BulkMetadataResp {
    /// Issues whose row was updated.
    pub updated: u32,
    /// Issues skipped because every targeted column was already set
    /// AND `mode = "skip_if_set"`. (Counted once per issue, not per
    /// field.)
    pub skipped: u32,
    /// Issues the caller doesn't have library access to. Filtered
    /// silently; surfaced in the response for admin debugging.
    pub forbidden: u32,
    /// Issues whose id didn't resolve to an active row.
    pub not_found: u32,
}

/// Bulk-update a per-field patch across a list of issue ids.
///
/// One transaction; batched by issue across the patch's fields. ACL
/// is per-issue via the library access check. Skips per-field +
/// per-issue when `mode = skip_if_set` AND the column is non-NULL.
/// Emits one `admin.issue.bulk_metadata_update` audit row per call
/// with `{ patch_keys, mode, updated_count, requested_count }`.
#[utoipa::path(
    operation_id = "issues_bulk_metadata",    patch,
    path = "/me/issues/bulk-metadata",
    request_body = BulkMetadataReq,
    responses(
        (status = 200, body = BulkMetadataResp),
        (status = 400, description = "validation"),
    )
)]
#[handler]
pub async fn bulk_metadata(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<BulkMetadataReq>,
) -> impl IntoResponse {
    const MAX_IDS: usize = 500;
    if req.issue_ids.len() > MAX_IDS {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            &format!("issue_ids cap is {MAX_IDS}"),
        );
    }
    if req.patch.is_empty() {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.empty_patch",
            "patch must include at least one field",
        );
    }

    // Validate enum-valued fields up-front so a bad value doesn't
    // partially apply.
    if let Some(Some(v)) = req.patch.manga.as_ref()
        && !matches!(v.as_str(), "Yes" | "No" | "YesAndRightToLeft")
    {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation.manga",
            "manga must be Yes, No, YesAndRightToLeft, or null",
        );
    }

    if req.issue_ids.is_empty() {
        return Json(BulkMetadataResp {
            updated: 0,
            skipped: 0,
            forbidden: 0,
            not_found: 0,
        })
        .into_response();
    }

    // Dedup ids — a noisy client shouldn't double-bill.
    let mut seen = std::collections::HashSet::with_capacity(req.issue_ids.len());
    let ids: Vec<String> = req
        .issue_ids
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect();
    let requested = ids.len() as u32;

    let rows: Vec<issue::Model> = match issue::Entity::find()
        .filter(issue::Column::Id.is_in(ids.clone()))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "bulk-metadata issue lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let not_found = requested.saturating_sub(rows.len() as u32);

    let mut updated: u32 = 0;
    let mut skipped: u32 = 0;
    let mut forbidden: u32 = 0;
    let mode_skip_if_set = matches!(req.mode, BulkMode::SkipIfSet);

    for row in rows {
        if !visible_in_library(&app, &user, row.library_id).await {
            forbidden += 1;
            continue;
        }

        // Build the per-row active model, applying patch fields
        // conditionally on `mode`. Use helper closures so the
        // skip_if_set logic stays readable; the per-field touch
        // count drives the `skipped` counter when nothing changed.
        let mut am: issue::ActiveModel = row.clone().into();
        let mut row_touched = false;
        macro_rules! apply {
            ($patch:expr, $current:expr, $field:ident) => {
                if let Some(v) = $patch.as_ref() {
                    let should_apply = !mode_skip_if_set || $current.is_none();
                    if should_apply {
                        am.$field = Set(v.clone());
                        row_touched = true;
                    }
                }
            };
        }
        apply!(req.patch.language_code, row.language_code, language_code);
        apply!(req.patch.manga, row.manga, manga);
        apply!(req.patch.publisher, row.publisher, publisher);
        apply!(req.patch.imprint, row.imprint, imprint);
        apply!(req.patch.age_rating, row.age_rating, age_rating);
        apply!(req.patch.format, row.format, format);
        apply!(req.patch.genre, row.genre, genre);
        apply!(req.patch.tags, row.tags, tags);
        apply!(req.patch.story_arc, row.story_arc, story_arc);

        if !row_touched {
            skipped += 1;
            continue;
        }

        // Stamp user_edited with every field this call touched so
        // the scanner skips them on rescan. We add to the existing
        // set rather than replace so prior PATCH /issues edits stay
        // sticky.
        let touched_names = req.patch.touched_field_names();
        let mut user_edited: BTreeSet<String> =
            serde_json::from_value(row.user_edited.clone()).unwrap_or_default();
        for name in &touched_names {
            user_edited.insert((*name).to_owned());
        }
        am.user_edited = Set(serde_json::json!(
            user_edited.into_iter().collect::<Vec<_>>()
        ));
        am.updated_at = Set(chrono::Utc::now().fixed_offset());

        let row_id = row.id.clone();
        match am.update(&app.db).await {
            Ok(_) => {
                updated += 1;
                // Dual-write to field_provenance — same de-risking
                // as the per-issue PATCH handler. Failures non-fatal.
                for name in &touched_names {
                    if let Some(field) = patch_field_key_to_metadata_field(name)
                        && let Err(e) = crate::metadata::writers::write_field_provenance(
                            &app.db,
                            "issue",
                            &row_id,
                            field,
                            crate::metadata::writers::SetBy::User,
                            None,
                        )
                        .await
                    {
                        tracing::warn!(
                            issue_id = %row_id,
                            field = %name,
                            error = %e,
                            "bulk-metadata: field_provenance dual-write failed (non-fatal)"
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, issue_id = %row.id, "bulk-metadata update failed");
                // Surface as not_found in the response — caller's
                // perspective is "didn't apply". The error log lets
                // operators investigate.
                forbidden += 1;
            }
        }
    }

    // Single audit row per call. Payload includes the *names* of the
    // touched fields (not the values — those can contain large
    // free-form strings) plus the counts.
    let _ = audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "admin.issue.bulk_metadata_update",
            target_type: Some("issue"),
            // Multi-target audit row — no single id. The patch_keys
            // payload + count is the trail.
            target_id: None,
            payload: serde_json::json!({
                "patch_keys": req.patch.touched_field_names(),
                "mode": match req.mode {
                    BulkMode::SkipIfSet => "skip_if_set",
                    BulkMode::Replace => "replace",
                },
                "requested": requested,
                "updated": updated,
                "skipped": skipped,
                "forbidden": forbidden,
                "not_found": not_found,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    Json(BulkMetadataResp {
        updated,
        skipped,
        forbidden,
        not_found,
    })
    .into_response()
}

// ───── GET /issues/search ─────

const SEARCH_MAX_QUERY_LEN: usize = 200;
const SEARCH_DEFAULT_LIMIT: u64 = 20;
const SEARCH_MAX_LIMIT: u64 = 50;

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    /// Optional series-id constraint; useful when a CBL entry's series
    /// already resolves but the issue number is missing/ambiguous.
    pub series_id: Option<Uuid>,
    #[serde(default)]
    pub limit: Option<u64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IssueSearchView {
    pub items: Vec<IssueSearchHit>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct IssueSearchHit {
    #[serde(flatten)]
    pub issue: IssueSummaryView,
    pub series_name: String,
    /// Search-result excerpt with `<mark>…</mark>` tags around matched
    /// terms — same shape as `SeriesView.snippet`. Omitted when the
    /// issue's free-text fields (summary / story_arc / characters)
    /// contain no highlightable fragment for the query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

/// Cross-library issue search backed by `issues.search_doc`. Used by
/// the CBL Resolution UI to pick a manual match for ambiguous /
/// missing entries. Visibility-filtered to the caller's libraries.
/// Cross-library issues listing with the same metadata-facet surface
/// the library grid offers for series. Hooks into `series.rs`'s shared
/// cursor helpers so pagination encoding stays consistent.
#[derive(Debug, Deserialize)]
pub struct ListIssuesCrossQuery {
    pub library: Option<Uuid>,
    pub q: Option<String>,
    #[serde(default = "default_cross_limit")]
    pub limit: u64,
    #[serde(default)]
    pub sort: Option<super::series::IssueSort>,
    #[serde(default)]
    pub order: Option<super::series::SortOrder>,
    #[serde(default)]
    pub cursor: Option<String>,
    /// Inclusive bounds on `issue.year`. NULLs are excluded when
    /// either bound is set.
    #[serde(default)]
    pub year_from: Option<i32>,
    #[serde(default)]
    pub year_to: Option<i32>,
    /// CSV facets — server splits and applies as IN-set or
    /// includes-any against the issues' own metadata columns.
    #[serde(default)]
    pub publisher: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub age_rating: Option<String>,
    #[serde(default)]
    pub genres: Option<String>,
    #[serde(default)]
    pub tags: Option<String>,
    #[serde(default)]
    pub writers: Option<String>,
    #[serde(default)]
    pub pencillers: Option<String>,
    #[serde(default)]
    pub inkers: Option<String>,
    #[serde(default)]
    pub colorists: Option<String>,
    #[serde(default)]
    pub letterers: Option<String>,
    #[serde(default)]
    pub cover_artists: Option<String>,
    #[serde(default)]
    pub editors: Option<String>,
    #[serde(default)]
    pub translators: Option<String>,
    #[serde(default)]
    pub characters: Option<String>,
    #[serde(default)]
    pub teams: Option<String>,
    #[serde(default)]
    pub locations: Option<String>,
    /// Inclusive bounds on the calling user's per-issue rating
    /// (0..=5). Issues the user hasn't rated are excluded when set.
    #[serde(default)]
    pub user_rating_min: Option<f64>,
    #[serde(default)]
    pub user_rating_max: Option<f64>,
}

fn default_cross_limit() -> u64 {
    50
}

const MAX_QUERY_LEN: usize = 200;

// ───── /issues list helpers ─────
//
// `list` orchestrates these: validate → visibility → static filters →
// search-mode early-return → count → cursor → sort → fetch → hydrate.
// Each helper threads a sea_orm::Select<issue::Entity> through; the
// validation-only helpers return `Result<(), Response>` so the
// handler can surface the 4xx without unwinding the select pipeline.

/// Reject pathological inputs before any DB work. Returns a static
/// 400-validation message on failure — caller wraps with the canonical
/// `error()` builder. Keeps the helper's `Result` variant small
/// (clippy::result_large_err).
fn validate_list_query_params(q: &ListIssuesCrossQuery) -> Result<(), &'static str> {
    if let Some(s) = q.q.as_ref()
        && s.len() > MAX_QUERY_LEN
    {
        return Err("q too long");
    }
    if q.user_rating_min.is_some() || q.user_rating_max.is_some() {
        let min = q.user_rating_min.unwrap_or(0.0);
        let max = q.user_rating_max.unwrap_or(5.0);
        if !(0.0..=5.0).contains(&min) || !(0.0..=5.0).contains(&max) || min > max {
            return Err("user_rating bounds must be 0..=5 with min <= max");
        }
    }
    Ok(())
}

/// Apply the per-user library visibility filter. Returns `None` when
/// the caller is restricted and has no overlap with the requested
/// scope — the handler should short-circuit to an empty page.
fn apply_issue_visibility(
    mut select: sea_orm::Select<issue::Entity>,
    visible: &access::VisibleLibraries,
    library: Option<Uuid>,
) -> Option<sea_orm::Select<issue::Entity>> {
    if let Some(lib) = library {
        if !visible.contains(lib) {
            return None;
        }
        select = select.filter(issue::Column::LibraryId.eq(lib));
    } else if !visible.unrestricted {
        if visible.allowed.is_empty() {
            return None;
        }
        select = select.filter(
            issue::Column::LibraryId.is_in(visible.allowed.iter().copied().collect::<Vec<_>>()),
        );
    }
    Some(select)
}

/// Year-range bounds + IN-set facets on direct `issues.*` columns.
fn apply_issue_direct_column_filters(
    mut select: sea_orm::Select<issue::Entity>,
    q: &ListIssuesCrossQuery,
) -> sea_orm::Select<issue::Entity> {
    if let Some(y) = q.year_from {
        select = select.filter(issue::Column::Year.gte(y));
    }
    if let Some(y) = q.year_to {
        select = select.filter(issue::Column::Year.lte(y));
    }
    let direct_facets: [(Option<&str>, issue::Column); 3] = [
        (q.publisher.as_deref(), issue::Column::Publisher),
        (q.language.as_deref(), issue::Column::LanguageCode),
        (q.age_rating.as_deref(), issue::Column::AgeRating),
    ];
    for (raw, column) in direct_facets {
        let Some(raw) = raw else { continue };
        let v = super::series::split_csv(raw);
        if v.is_empty() {
            continue;
        }
        select = select.filter(column.is_in(v));
    }
    select
}

/// CSV-includes-any against the issues' CSV-shaped metadata columns
/// (`genre`, `tags`, credits, characters/teams/locations). Same
/// per-value split as `aggregate_csv` / `split_csv`: prefer `;` when
/// the column value contains one, otherwise split on `,`. Keeps chip
/// values aligned between the picker and the facet filter.
fn apply_issue_csv_facet_filters(
    mut select: sea_orm::Select<issue::Entity>,
    q: &ListIssuesCrossQuery,
) -> sea_orm::Select<issue::Entity> {
    let csv_facets: [(Option<&str>, &'static str); 13] = [
        (q.genres.as_deref(), "genre"),
        (q.tags.as_deref(), "tags"),
        (q.writers.as_deref(), "writer"),
        (q.pencillers.as_deref(), "penciller"),
        (q.inkers.as_deref(), "inker"),
        (q.colorists.as_deref(), "colorist"),
        (q.letterers.as_deref(), "letterer"),
        (q.cover_artists.as_deref(), "cover_artist"),
        (q.editors.as_deref(), "editor"),
        (q.translators.as_deref(), "translator"),
        (q.characters.as_deref(), "characters"),
        (q.teams.as_deref(), "teams"),
        (q.locations.as_deref(), "locations"),
    ];
    for (raw, column) in csv_facets {
        let Some(raw) = raw else { continue };
        let values = super::series::split_csv(raw);
        if values.is_empty() {
            continue;
        }
        let lowered: Vec<String> = values.iter().map(|s| s.to_lowercase()).collect();
        let sql = format!(
            "EXISTS (SELECT 1 FROM unnest( \
               regexp_split_to_array( \
                 coalesce(issues.{column}, ''), \
                 CASE WHEN coalesce(issues.{column}, '') LIKE '%;%' THEN ';' ELSE ',' END \
               ) \
             ) AS piece WHERE lower(trim(piece)) = ANY($1))",
        );
        select = select.filter(Expr::cust_with_values(&sql, [Value::from(lowered)]));
    }
    select
}

/// EXISTS-subquery filter on the calling user's per-issue rating.
/// Caller must call `validate_list_query_params` first — this helper
/// trusts the bounds.
fn apply_issue_user_rating_filter(
    mut select: sea_orm::Select<issue::Entity>,
    q: &ListIssuesCrossQuery,
    user_id: Uuid,
) -> sea_orm::Select<issue::Entity> {
    if q.user_rating_min.is_some() || q.user_rating_max.is_some() {
        let min = q.user_rating_min.unwrap_or(0.0);
        let max = q.user_rating_max.unwrap_or(5.0);
        select = select.filter(Expr::cust_with_values(
            "EXISTS (SELECT 1 FROM user_ratings ur \
             WHERE ur.user_id = $1 \
               AND ur.target_type = 'issue' \
               AND ur.target_id = issues.id \
               AND ur.rating BETWEEN $2 AND $3)",
            [Value::from(user_id), Value::from(min), Value::from(max)],
        ));
    }
    select
}

/// Decode the opaque cursor and dispatch to the per-sort
/// `apply_*_cursor` helper. Returns the canonical "invalid cursor"
/// message on any decode failure; caller maps to 400 `validation`.
/// Static `Err` keeps the `Result` variant small
/// (clippy::result_large_err).
fn apply_issue_cursor(
    select: sea_orm::Select<issue::Entity>,
    cursor: &str,
    sort: super::series::IssueSort,
    asc: bool,
    user_id: Uuid,
) -> Result<sea_orm::Select<issue::Entity>, &'static str> {
    use super::series::{
        IssueSort, apply_f64_cursor, apply_i32_cursor, apply_ts_cursor, parse_cursor,
    };
    let (c_value, c_id) = parse_cursor(cursor).map_err(|_| "invalid cursor")?;
    let parse_f64 = || -> Result<Option<f64>, &'static str> {
        if c_value.is_empty() {
            Ok(None)
        } else {
            c_value
                .parse::<f64>()
                .map(Some)
                .map_err(|_| "invalid cursor")
        }
    };
    let parse_i32 = || -> Result<Option<i32>, &'static str> {
        if c_value.is_empty() {
            Ok(None)
        } else {
            c_value
                .parse::<i32>()
                .map(Some)
                .map_err(|_| "invalid cursor")
        }
    };
    Ok(match sort {
        IssueSort::Number => apply_f64_cursor(
            select,
            issue::Column::SortNumber,
            issue::Column::Id,
            parse_f64()?,
            c_id,
            asc,
        ),
        IssueSort::Year => apply_i32_cursor(
            select,
            issue::Column::Year,
            issue::Column::Id,
            parse_i32()?,
            c_id,
            asc,
        ),
        IssueSort::PageCount => apply_i32_cursor(
            select,
            issue::Column::PageCount,
            issue::Column::Id,
            parse_i32()?,
            c_id,
            asc,
        ),
        IssueSort::CreatedAt => {
            let ts =
                chrono::DateTime::parse_from_rfc3339(&c_value).map_err(|_| "invalid cursor")?;
            apply_ts_cursor(
                select,
                issue::Column::CreatedAt,
                issue::Column::Id,
                ts,
                c_id,
                asc,
            )
        }
        IssueSort::UpdatedAt => {
            let ts =
                chrono::DateTime::parse_from_rfc3339(&c_value).map_err(|_| "invalid cursor")?;
            apply_ts_cursor(
                select,
                issue::Column::UpdatedAt,
                issue::Column::Id,
                ts,
                c_id,
                asc,
            )
        }
        IssueSort::UserRating => apply_user_rating_cursor(select, user_id, parse_f64()?, c_id, asc),
    })
}

/// Final `ORDER BY` chain. Each sort mode keeps `(NULLs LAST, value,
/// id)` ordering so cursor pagination tiebreaks cleanly.
fn apply_issue_sort_ordering(
    select: sea_orm::Select<issue::Entity>,
    sort: super::series::IssueSort,
    asc: bool,
    user_id: Uuid,
) -> sea_orm::Select<issue::Entity> {
    use super::series::IssueSort;
    match sort {
        IssueSort::Number => {
            let nulls_last = Expr::cust("sort_number IS NULL");
            let s = select.order_by_asc(nulls_last);
            if asc {
                s.order_by_asc(issue::Column::SortNumber)
                    .order_by_asc(issue::Column::Id)
            } else {
                s.order_by_desc(issue::Column::SortNumber)
                    .order_by_desc(issue::Column::Id)
            }
        }
        IssueSort::Year => {
            let nulls_last = Expr::cust("year IS NULL");
            let s = select.order_by_asc(nulls_last);
            if asc {
                s.order_by_asc(issue::Column::Year)
                    .order_by_asc(issue::Column::Id)
            } else {
                s.order_by_desc(issue::Column::Year)
                    .order_by_desc(issue::Column::Id)
            }
        }
        IssueSort::PageCount => {
            let nulls_last = Expr::cust("page_count IS NULL");
            let s = select.order_by_asc(nulls_last);
            if asc {
                s.order_by_asc(issue::Column::PageCount)
                    .order_by_asc(issue::Column::Id)
            } else {
                s.order_by_desc(issue::Column::PageCount)
                    .order_by_desc(issue::Column::Id)
            }
        }
        IssueSort::CreatedAt => {
            if asc {
                select
                    .order_by_asc(issue::Column::CreatedAt)
                    .order_by_asc(issue::Column::Id)
            } else {
                select
                    .order_by_desc(issue::Column::CreatedAt)
                    .order_by_desc(issue::Column::Id)
            }
        }
        IssueSort::UpdatedAt => {
            if asc {
                select
                    .order_by_asc(issue::Column::UpdatedAt)
                    .order_by_asc(issue::Column::Id)
            } else {
                select
                    .order_by_desc(issue::Column::UpdatedAt)
                    .order_by_desc(issue::Column::Id)
            }
        }
        IssueSort::UserRating => {
            let rating_expr = Expr::cust_with_values(
                "(SELECT ur.rating FROM user_ratings ur \
                  WHERE ur.user_id = $1 AND ur.target_type = 'issue' \
                    AND ur.target_id = issues.id)",
                [Value::from(user_id)],
            );
            let nulls_last_expr = Expr::cust_with_values(
                "(SELECT ur.rating FROM user_ratings ur \
                  WHERE ur.user_id = $1 AND ur.target_type = 'issue' \
                    AND ur.target_id = issues.id) IS NULL",
                [Value::from(user_id)],
            );
            let s = select.order_by_asc(nulls_last_expr);
            if asc {
                s.order_by_asc(rating_expr).order_by_asc(issue::Column::Id)
            } else {
                s.order_by_desc(rating_expr)
                    .order_by_desc(issue::Column::Id)
            }
        }
    }
}

/// Compute the opaque cursor encoding for the boundary row, when the
/// fetched window overflows the page limit. Pulls the per-user rating
/// out of band for the `UserRating` sort so the cursor encodes the
/// correlated subquery's value.
async fn compute_next_issue_cursor(
    app: &AppState,
    rows: &[issue::Model],
    limit: u64,
    sort: super::series::IssueSort,
    user_id: Uuid,
) -> Option<String> {
    use super::series::{IssueSort, encode_cursor};
    if rows.len() as u64 <= limit {
        return None;
    }
    let r = rows.get(limit as usize - 1)?;
    let value = match sort {
        IssueSort::Number => r.sort_number.map(|n| n.to_string()).unwrap_or_default(),
        IssueSort::Year => r.year.map(|y| y.to_string()).unwrap_or_default(),
        IssueSort::PageCount => r.page_count.map(|p| p.to_string()).unwrap_or_default(),
        IssueSort::CreatedAt => r.created_at.to_rfc3339(),
        IssueSort::UpdatedAt => r.updated_at.to_rfc3339(),
        IssueSort::UserRating => fetch_user_rating(app, user_id, &r.id)
            .await
            .map(|v| v.to_string())
            .unwrap_or_default(),
    };
    Some(encode_cursor(&value, &r.id))
}

#[utoipa::path(
    operation_id = "issues_list",    get,
    path = "/issues",
    responses((status = 200, body = super::series::IssueListView))
)]
#[handler]
pub async fn list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListIssuesCrossQuery>,
) -> impl IntoResponse {
    use super::series::{IssueListView, IssueSort, SortOrder, clamp_limit};

    if let Err(msg) = validate_list_query_params(&q) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "validation", msg);
    }

    let visible = access::for_user(&app, &user).await;
    let empty = || {
        Json(IssueListView {
            items: Vec::new(),
            next_cursor: None,
            total: Some(0),
        })
        .into_response()
    };

    let base = issue::Entity::find()
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null());
    let Some(mut select) = apply_issue_visibility(base, &visible, q.library) else {
        return empty();
    };
    select = apply_issue_direct_column_filters(select, &q);
    select = apply_issue_csv_facet_filters(select, &q);
    select = apply_issue_user_rating_filter(select, &q, user.id);

    let limit = clamp_limit(q.limit);
    let q_text = q.q.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty());

    // Search mode: rank by ts_rank_cd by default and paginate with an
    // opaque offset cursor. If the caller passes an explicit sort, use
    // the same deterministic issue ordering as the non-search branch.
    if let Some(text) = q_text {
        let offset = match q.cursor.as_deref() {
            Some(cursor) => match super::series::parse_offset_cursor(cursor) {
                Ok(v) => v,
                Err(_) => {
                    return error(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "validation",
                        "invalid cursor",
                    );
                }
            },
            None => 0,
        };
        let filtered = select.filter(Expr::cust_with_values(
            "search_doc @@ websearch_to_tsquery('simple', $1)",
            [text],
        ));
        let total = if q.cursor.is_none() {
            use sea_orm::PaginatorTrait;
            match filtered.clone().count(&app.db).await {
                Ok(n) => Some(n as i64),
                Err(e) => {
                    tracing::error!(error = %e, "list issues cross search count failed");
                    return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
                }
            }
        } else {
            None
        };
        let ranked = if let Some(sort) = q.sort {
            let order = q.order.unwrap_or(match sort {
                IssueSort::Number => SortOrder::Asc,
                IssueSort::CreatedAt
                | IssueSort::UpdatedAt
                | IssueSort::Year
                | IssueSort::PageCount
                | IssueSort::UserRating => SortOrder::Desc,
            });
            apply_issue_sort_ordering(filtered, sort, matches!(order, SortOrder::Asc), user.id)
        } else {
            filtered
                .order_by_desc(Expr::cust_with_values(
                    "ts_rank_cd(search_doc, websearch_to_tsquery('simple', $1), 32)",
                    [text],
                ))
                .order_by_asc(issue::Column::Id)
        }
        .offset(offset)
        .limit(limit + 1);
        let rows = match ranked.all(&app.db).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "list issues cross search failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        };
        let has_more = rows.len() as u64 > limit;
        let next_cursor = has_more.then(|| super::series::encode_offset_cursor(offset + limit));
        let page: Vec<issue::Model> = rows.into_iter().take(limit as usize).collect();
        return hydrate_and_respond(&app, page, next_cursor, total).await;
    }

    let sort = q.sort.unwrap_or_default();
    let order = q.order.unwrap_or(match sort {
        IssueSort::Number => SortOrder::Asc,
        IssueSort::CreatedAt
        | IssueSort::UpdatedAt
        | IssueSort::Year
        | IssueSort::PageCount
        | IssueSort::UserRating => SortOrder::Desc,
    });
    let asc = matches!(order, SortOrder::Asc);

    // First-page-only count — see `series::list` for the rationale.
    use sea_orm::PaginatorTrait;
    let total: Option<i64> = if q.cursor.is_none() {
        match select.clone().count(&app.db).await {
            Ok(n) => Some(n as i64),
            Err(e) => {
                tracing::error!(error = %e, "list issues cross count failed");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
            }
        }
    } else {
        None
    };

    if let Some(cursor) = q.cursor.as_deref() {
        select = match apply_issue_cursor(select, cursor, sort, asc, user.id) {
            Ok(s) => s,
            Err(msg) => return error(StatusCode::UNPROCESSABLE_ENTITY, "validation", msg),
        };
    }
    select = apply_issue_sort_ordering(select, sort, asc, user.id);

    let rows: Vec<issue::Model> = match select.limit(limit + 1).all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "list issues cross failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let next_cursor = compute_next_issue_cursor(&app, &rows, limit, sort, user.id).await;
    let page: Vec<issue::Model> = rows.into_iter().take(limit as usize).collect();
    hydrate_and_respond(&app, page, next_cursor, total).await
}

/// Look up the calling user's rating for one issue by id; used to
/// compute the cursor sort_value for the `user_rating` sort.
async fn fetch_user_rating(app: &AppState, user_id: Uuid, issue_id: &str) -> Option<f64> {
    use entity::user_rating;
    use sea_orm::ColumnTrait;
    user_rating::Entity::find()
        .filter(user_rating::Column::UserId.eq(user_id))
        .filter(user_rating::Column::TargetType.eq("issue"))
        .filter(user_rating::Column::TargetId.eq(issue_id))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .map(|m| m.rating)
}

/// Rating cursor: filter on `(rating > c) OR (rating = c AND id > id)`
/// using a correlated subquery for the join. NULL rating boundary
/// keeps within the no-rating bucket and paginates by id alone.
fn apply_user_rating_cursor(
    select: sea_orm::Select<issue::Entity>,
    user_id: Uuid,
    c_value: Option<f64>,
    c_id: String,
    asc: bool,
) -> sea_orm::Select<issue::Entity> {
    use sea_orm::ColumnTrait;
    let rating_sq = "(SELECT ur.rating FROM user_ratings ur \
                       WHERE ur.user_id = $1 AND ur.target_type = 'issue' \
                         AND ur.target_id = issues.id)";
    match c_value {
        Some(v) => {
            // Two-arm OR: strictly past the boundary value, OR equal value with id past boundary.
            let cmp = if asc { ">" } else { "<" };
            let sql =
                format!("({rating_sq} {cmp} $2 OR ({rating_sq} = $2 AND issues.id {cmp} $3))",);
            select.filter(Expr::cust_with_values(
                &sql,
                [Value::from(user_id), Value::from(v), Value::from(c_id)],
            ))
        }
        None => {
            // No-rating boundary: stay in the NULL bucket, paginate by id.
            let sql = format!("{rating_sq} IS NULL");
            let s = select.filter(Expr::cust_with_values(&sql, [Value::from(user_id)]));
            if asc {
                s.filter(issue::Column::Id.gt(c_id))
            } else {
                s.filter(issue::Column::Id.lt(c_id))
            }
        }
    }
}

/// Hydrate `issue::Model`s into `IssueSummaryView`s with their parent
/// series slug. One batched series fetch keeps it O(1) round-trips.
async fn hydrate_and_respond(
    app: &AppState,
    rows: Vec<issue::Model>,
    next_cursor: Option<String>,
    total: Option<i64>,
) -> axum::response::Response {
    use super::series::IssueListView;
    if rows.is_empty() {
        return Json(IssueListView {
            items: Vec::new(),
            next_cursor,
            total,
        })
        .into_response();
    }
    let series_ids: BTreeSet<Uuid> = rows.iter().map(|r| r.series_id).collect();
    let series_rows: Vec<series::Model> = match series::Entity::find()
        .filter(series::Column::Id.is_in(series_ids.into_iter().collect::<Vec<_>>()))
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "issues hydrate (series lookup) failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let series_lookup: std::collections::HashMap<Uuid, series::Model> =
        series_rows.into_iter().map(|s| (s.id, s)).collect();
    let items: Vec<IssueSummaryView> = rows
        .into_iter()
        .filter_map(|i| {
            let s = series_lookup.get(&i.series_id)?;
            let series_slug = s.slug.clone();
            Some(IssueSummaryView::from_model(i, &series_slug))
        })
        .collect();
    Json(IssueListView {
        items,
        next_cursor,
        total,
    })
    .into_response()
}

#[utoipa::path(
    operation_id = "issues_search",    get,
    path = "/issues/search",
    params(
        ("q" = String, Query,),
        ("series_id" = Option<String>, Query,),
        ("limit" = Option<u64>, Query,),
    ),
    responses((status = 200, body = IssueSearchView))
)]
#[handler]
pub async fn search(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<SearchQuery>,
) -> impl IntoResponse {
    let text = q.q.trim();
    if text.is_empty() {
        return Json(IssueSearchView { items: Vec::new() }).into_response();
    }
    if text.len() > SEARCH_MAX_QUERY_LEN {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "validation", "q too long");
    }
    let limit = q
        .limit
        .unwrap_or(SEARCH_DEFAULT_LIMIT)
        .clamp(1, SEARCH_MAX_LIMIT);

    let visible = access::for_user(&app, &user).await;
    let mut sel = issue::Entity::find()
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .filter(Expr::cust_with_values(
            "search_doc @@ websearch_to_tsquery('simple', $1)",
            [text],
        ))
        .order_by_desc(Expr::cust_with_values(
            "ts_rank_cd(search_doc, websearch_to_tsquery('simple', $1), 32)",
            [text],
        ))
        .limit(limit);
    if let Some(sid) = q.series_id {
        sel = sel.filter(issue::Column::SeriesId.eq(sid));
    }
    if !visible.unrestricted {
        if visible.allowed.is_empty() {
            return Json(IssueSearchView { items: Vec::new() }).into_response();
        }
        let ids: Vec<Uuid> = visible.allowed.iter().copied().collect();
        sel = sel.filter(issue::Column::LibraryId.is_in(ids));
    }
    let rows = match sel.all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "issue search failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    if rows.is_empty() {
        return Json(IssueSearchView { items: Vec::new() }).into_response();
    }
    let series_ids: BTreeSet<Uuid> = rows.iter().map(|r| r.series_id).collect();
    let series_rows: Vec<series::Model> = match series::Entity::find()
        .filter(series::Column::Id.is_in(series_ids.into_iter().collect::<Vec<_>>()))
        .all(&app.db)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "series hydrate failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };
    let series_lookup: std::collections::HashMap<Uuid, series::Model> =
        series_rows.into_iter().map(|s| (s.id, s)).collect();
    // Second pass for ts_headline excerpts. Failures degrade silently —
    // a hit without a snippet still renders, just without the "why
    // it matched" callout.
    let snippets = fetch_issue_snippets(&app, &rows, text)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "issue snippet fetch failed; continuing without");
            std::collections::HashMap::new()
        });
    let items = rows
        .into_iter()
        .filter_map(|i| {
            let s = series_lookup.get(&i.series_id)?;
            let series_slug = s.slug.clone();
            let series_name = s.name.clone();
            let snippet = snippets.get(&i.id).cloned();
            Some(IssueSearchHit {
                issue: IssueSummaryView::from_model(i, &series_slug),
                series_name,
                snippet,
            })
        })
        .collect();
    Json(IssueSearchView { items }).into_response()
}

/// Issue-level companion to `series::fetch_series_snippets`. Highlights
/// against `summary` (highest-signal free-text field on an issue). We
/// could also dig into story_arc / characters / locations later, but
/// summary covers ~all real-world matches and keeps the SQL simple.
async fn fetch_issue_snippets(
    app: &AppState,
    rows: &[issue::Model],
    q_text: &str,
) -> Result<HashMap<String, String>, sea_orm::DbErr> {
    use sea_orm::{ConnectionTrait, FromQueryResult, Statement, Value};
    if rows.is_empty() {
        return Ok(HashMap::new());
    }

    #[derive(Debug, FromQueryResult)]
    struct SnippetRow {
        id: String,
        snippet: Option<String>,
    }

    let mut params: Vec<Value> = Vec::with_capacity(rows.len() + 1);
    params.push(Value::from(q_text.to_string()));
    let id_placeholders: Vec<String> = rows
        .iter()
        .map(|r| {
            params.push(Value::from(r.id.clone()));
            format!("${}", params.len())
        })
        .collect();
    let sql = format!(
        r#"SELECT id,
                  ts_headline(
                    'simple',
                    COALESCE(summary, ''),
                    websearch_to_tsquery('simple', $1),
                    'MaxFragments=1, MaxWords=18, MinWords=5, ShortWord=2, StartSel=<mark>, StopSel=</mark>, HighlightAll=false'
                  ) AS snippet
             FROM issues
             WHERE id IN ({})"#,
        id_placeholders.join(",")
    );

    let backend = app.db.get_database_backend();
    let stmt = Statement::from_sql_and_values(backend, sql, params);
    let rows: Vec<SnippetRow> = SnippetRow::find_by_statement(stmt).all(&app.db).await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let s = r.snippet?;
            if s.contains("<mark>") {
                Some((r.id, s))
            } else {
                None
            }
        })
        .collect())
}

// ───── helpers ─────

async fn visible_in_library(app: &AppState, user: &CurrentUser, lib_id: Uuid) -> bool {
    if user.role == "admin" {
        return true;
    }
    library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user.id))
        .filter(library_user_access::Column::LibraryId.eq(lib_id))
        .one(&app.db)
        .await
        .ok()
        .flatten()
        .is_some()
}

/// Map a string key from `issue.user_edited` JSON to its
/// corresponding [`MetadataField`] variant. Returns `None` for keys
/// the typed enum doesn't cover yet — the legacy column still
/// tracks them; only fields the matcher / composer / apply pipeline
/// understand land in `field_provenance`.
///
/// Coverage today includes everything the upcoming sidecar-writeback
/// plan's composer reads. Keys outside this set (e.g. `web_url`,
/// `additional_links`, `alternate_series`, the per-role credit
/// columns) stay in `user_edited` only — the scanner's legacy
/// user-precedence check still respects them; the composer just
/// doesn't have a slot for them.
///
/// metadata-providers-1.0 M10 dual-write.
fn patch_field_key_to_metadata_field(key: &str) -> Option<crate::metadata::MetadataField> {
    use crate::metadata::MetadataField;
    use crate::metadata::identifier::Source;
    match key {
        "title" => Some(MetadataField::Title),
        "summary" => Some(MetadataField::Summary),
        "notes" => Some(MetadataField::Notes),
        "publisher" => Some(MetadataField::Publisher),
        "imprint" => Some(MetadataField::Imprint),
        "language_code" => Some(MetadataField::LanguageCode),
        "age_rating" => Some(MetadataField::AgeRating),
        "format" => Some(MetadataField::Format),
        "manga" => Some(MetadataField::Manga),
        "volume" => Some(MetadataField::Volume),
        // The user edits credit roles + character/team/location CSV
        // strings directly today; the composer reads the junction-
        // shaped MetadataField variants. Map each to its junction.
        "writer" | "penciller" | "inker" | "colorist" | "letterer" | "cover_artist" | "editor"
        | "translator" => Some(MetadataField::Credits),
        "characters" => Some(MetadataField::Characters),
        "teams" => Some(MetadataField::Teams),
        "locations" => Some(MetadataField::Locations),
        "story_arc" | "story_arc_number" => Some(MetadataField::StoryArcs),
        "genre" => Some(MetadataField::Genres),
        "tags" => Some(MetadataField::Tags),
        // External-IDs already write field_provenance via
        // writers::set_external_id's own provenance row (set_by='user'
        // on the external_ids row itself, which the composer reads
        // through a separate channel). The MetadataField::ExternalId
        // arm is here for completeness in case a caller switches
        // to the typed field path.
        "gtin" => Some(MetadataField::ExternalId(Source::Gtin)),
        "comicvine_id" => Some(MetadataField::ExternalId(Source::ComicVine)),
        "metron_id" => Some(MetadataField::ExternalId(Source::Metron)),
        // Year + cover-date split: PATCH writes y/m/d separately;
        // the composer reads CoverDate. Map year-the-issue-field to
        // CoverDate so a user edit on the cover year survives a
        // provider apply.
        "year" | "month" | "day" => Some(MetadataField::CoverDate),
        // Fields with no MetadataField slot: web_url, additional_links,
        // alternate_series, black_and_white, sort_number, number_raw.
        // Stay in user_edited only.
        _ => None,
    }
}
