//! Basic OPDS 1.2 catalog (§8 — minimal subset for Phase 2).
//!
//! - Root navigation feed → links to Series, Recent, Search.
//! - Series feed → one entry per series; each links to a per-series feed.
//! - Per-series feed → acquisition entries with download links.
//! - Recent feed → newest issues across the library, acquisition entries.
//! - Search → wraps the existing per-series search.
//! - Download → streams the raw archive file with an `application/zip` MIME.
//!
//! Auth: JWT only (Bearer or `__Host-comic_session` cookie). No app passwords
//! and no OPDS-PSE — Phase 6 introduces both. Every feed query filters via
//! `library_user_access` (admins see all).
//!
//! XML is emitted as escaped strings rather than via a serialization library.
//! The structure is fixed and small; the escaping helper below is the only
//! place where untrusted text is interpolated.

use axum::{
    Router,
    body::Body,
    extract::{Path as AxPath, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use entity::{issue, library_user_access, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::Deserialize;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::auth::CurrentUser;
use crate::state::AppState;

const PAGE_SIZE: u64 = 50;
const ATOM_CT: &str = "application/atom+xml; charset=utf-8";
const NAV_CT: &str = "application/atom+xml;profile=opds-catalog;kind=navigation";
const ACQ_CT: &str = "application/atom+xml;profile=opds-catalog;kind=acquisition";
const ZIP_CT: &str = "application/zip";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/opds/v1", get(root))
        .route("/opds/v1/series", get(series_list))
        .route("/opds/v1/series/{id}", get(series_one))
        .route("/opds/v1/recent", get(recent))
        .route("/opds/v1/search", get(search))
        .route("/opds/v1/issues/{id}/file", get(download))
}

#[derive(Debug, Deserialize)]
pub struct PageQuery {
    pub page: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
}

async fn root(State(app): State<AppState>, _user: CurrentUser) -> Response {
    let now = chrono::Utc::now().to_rfc3339();
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:opds="http://opds-spec.org/2010/catalog">
  <id>{base}/opds/v1</id>
  <title>{name}</title>
  <updated>{now}</updated>
  <link rel="self" href="/opds/v1" type="{nav}"/>
  <link rel="start" href="/opds/v1" type="{nav}"/>
  <link rel="search" href="/opds/v1/search?q={{searchTerms}}" type="application/opensearchdescription+xml"/>
  <entry>
    <id>{base}/opds/v1/series</id>
    <title>All series</title>
    <updated>{now}</updated>
    <link rel="subsection" href="/opds/v1/series" type="{acq}"/>
  </entry>
  <entry>
    <id>{base}/opds/v1/recent</id>
    <title>Recently added</title>
    <updated>{now}</updated>
    <link rel="http://opds-spec.org/sort/new" href="/opds/v1/recent" type="{acq}"/>
  </entry>
</feed>
"#,
        base = xml_escape(&app.cfg.public_url),
        name = xml_escape("Comic Reader"),
        now = now,
        nav = NAV_CT,
        acq = ACQ_CT,
    );
    atom(body)
}

async fn series_list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<PageQuery>,
) -> Response {
    let page = q.page.unwrap_or(1).max(1);
    let offset = (page - 1) * PAGE_SIZE;

    let allowed = match allowed_libraries(&app, &user).await {
        Ok(v) => v,
        Err(e) => return server_error(e),
    };
    let mut sel = series::Entity::find().order_by_asc(series::Column::Name);
    if let Some(ids) = allowed.as_ref() {
        sel = sel.filter(series::Column::LibraryId.is_in(ids.clone()));
    }
    let rows = match sel.offset(offset).limit(PAGE_SIZE).all(&app.db).await {
        Ok(r) => r,
        Err(e) => return server_error(e.to_string()),
    };

    let now = chrono::Utc::now().to_rfc3339();
    let mut entries = String::new();
    for s in &rows {
        entries.push_str(&format!(
            r#"  <entry>
    <id>urn:series:{id}</id>
    <title>{name}</title>
    <updated>{updated}</updated>
    {summary}
    <link rel="subsection" href="/opds/v1/series/{id}" type="{acq}"/>
  </entry>
"#,
            id = s.id,
            name = xml_escape(&s.name),
            updated = s.updated_at.to_rfc3339(),
            summary = s
                .summary
                .as_ref()
                .map(|s| format!("<summary>{}</summary>", xml_escape(s)))
                .unwrap_or_default(),
            acq = ACQ_CT,
        ));
    }

    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>{base}/opds/v1/series?page={page}</id>
  <title>All series</title>
  <updated>{now}</updated>
  <link rel="self" href="/opds/v1/series?page={page}" type="{acq}"/>
  <link rel="up" href="/opds/v1" type="{nav}"/>
  {next_link}
{entries}</feed>
"#,
        base = xml_escape(&app.cfg.public_url),
        now = now,
        acq = ACQ_CT,
        nav = NAV_CT,
        next_link = if rows.len() as u64 == PAGE_SIZE {
            format!(
                r#"<link rel="next" href="/opds/v1/series?page={}" type="{}"/>"#,
                page + 1,
                ACQ_CT
            )
        } else {
            String::new()
        },
        page = page,
    );
    atom(body)
}

async fn series_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> Response {
    let s = match series::Entity::find_by_id(id).one(&app.db).await {
        Ok(Some(s)) => s,
        Ok(None) => return not_found(),
        Err(e) => return server_error(e.to_string()),
    };
    if !visible(&app, &user, s.library_id).await {
        return not_found();
    }
    let issues = match issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(id))
        .order_by_asc(issue::Column::SortNumber)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => return server_error(e.to_string()),
    };
    let body = build_acquisition_feed(
        &app,
        &format!("urn:series:{id}"),
        &format!("Series — {}", s.name),
        &format!("/opds/v1/series/{id}"),
        &issues,
    );
    atom(body)
}

async fn recent(State(app): State<AppState>, user: CurrentUser) -> Response {
    let allowed = match allowed_libraries(&app, &user).await {
        Ok(v) => v,
        Err(e) => return server_error(e),
    };
    let mut sel = issue::Entity::find()
        .order_by_desc(issue::Column::CreatedAt)
        .limit(50);
    if let Some(ids) = allowed.as_ref() {
        sel = sel.filter(issue::Column::LibraryId.is_in(ids.clone()));
    }
    let rows = match sel.all(&app.db).await {
        Ok(r) => r,
        Err(e) => return server_error(e.to_string()),
    };
    let body = build_acquisition_feed(
        &app,
        "urn:recent",
        "Recently added",
        "/opds/v1/recent",
        &rows,
    );
    atom(body)
}

async fn search(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<SearchQuery>,
) -> Response {
    let needle = q.q.unwrap_or_default();
    let needle = needle.trim();
    if needle.is_empty() {
        return atom(build_acquisition_feed(
            &app,
            "urn:search",
            "Search",
            "/opds/v1/search",
            &[],
        ));
    }
    if needle.len() > 200 {
        return error(StatusCode::BAD_REQUEST, "validation", "query too long");
    }
    let allowed = match allowed_libraries(&app, &user).await {
        Ok(v) => v,
        Err(e) => return server_error(e),
    };
    // Series search via tsvector — kept simple in OPDS to match the existing
    // per-context `/series?q=` endpoint. Returns matched series as acquisition
    // links pointing at the per-series feed.
    let pattern = format!("%{}%", needle.replace('\\', "\\\\").replace('%', "\\%"));
    let mut sel = series::Entity::find()
        .filter(series::Column::Name.like(&pattern))
        .order_by_asc(series::Column::Name)
        .limit(PAGE_SIZE);
    if let Some(ids) = allowed.as_ref() {
        sel = sel.filter(series::Column::LibraryId.is_in(ids.clone()));
    }
    let rows = match sel.all(&app.db).await {
        Ok(r) => r,
        Err(e) => return server_error(e.to_string()),
    };

    let now = chrono::Utc::now().to_rfc3339();
    let mut entries = String::new();
    for s in &rows {
        entries.push_str(&format!(
            r#"  <entry>
    <id>urn:series:{id}</id>
    <title>{name}</title>
    <updated>{updated}</updated>
    <link rel="subsection" href="/opds/v1/series/{id}" type="{acq}"/>
  </entry>
"#,
            id = s.id,
            name = xml_escape(&s.name),
            updated = s.updated_at.to_rfc3339(),
            acq = ACQ_CT,
        ));
    }

    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>urn:search:{needle_escaped}</id>
  <title>Search — {needle_escaped}</title>
  <updated>{now}</updated>
  <link rel="self" href="/opds/v1/search?q={needle_url}" type="{acq}"/>
  <link rel="up" href="/opds/v1" type="{nav}"/>
{entries}</feed>
"#,
        needle_escaped = xml_escape(needle),
        needle_url = url_escape(needle),
        now = now,
        acq = ACQ_CT,
        nav = NAV_CT,
    );
    atom(body)
}

async fn download(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<String>,
) -> Response {
    let row = match issue::Entity::find_by_id(id).one(&app.db).await {
        Ok(Some(r)) => r,
        _ => return not_found(),
    };
    if !visible(&app, &user, row.library_id).await {
        return not_found();
    }
    let f = match tokio::fs::File::open(&row.file_path).await {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(error = %e, path = %row.file_path, "opds download open failed");
            return not_found();
        }
    };
    let len = f.metadata().await.ok().map(|m| m.len()).unwrap_or(0);
    let leaf = std::path::Path::new(&row.file_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("comic.cbz")
        .to_owned();
    let stream = ReaderStream::new(f);
    let mut hdrs = HeaderMap::new();
    hdrs.insert(header::CONTENT_TYPE, HeaderValue::from_static(ZIP_CT));
    hdrs.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{leaf}\"")).unwrap(),
    );
    hdrs.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    hdrs.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    (StatusCode::OK, hdrs, Body::from_stream(stream)).into_response()
}

// ────────────── helpers ──────────────

fn build_acquisition_feed(
    app: &AppState,
    feed_id: &str,
    title: &str,
    self_href: &str,
    issues: &[issue::Model],
) -> String {
    let now = chrono::Utc::now().to_rfc3339();
    let mut entries = String::new();
    for i in issues {
        let label = i.title.clone().unwrap_or_else(|| {
            i.number_raw
                .clone()
                .map(|n| format!("Issue #{n}"))
                .unwrap_or_else(|| "Issue".into())
        });
        entries.push_str(&format!(
            r#"  <entry>
    <id>urn:issue:{id}</id>
    <title>{title}</title>
    <updated>{updated}</updated>
    {summary}
    <link rel="http://opds-spec.org/image/thumbnail" href="/issues/{id}/pages/0/thumb" type="image/webp"/>
    <link rel="http://opds-spec.org/acquisition" href="/opds/v1/issues/{id}/file" type="application/zip"/>
  </entry>
"#,
            id = i.id,
            title = xml_escape(&label),
            updated = i.updated_at.to_rfc3339(),
            summary = i
                .summary
                .as_ref()
                .map(|s| format!("<summary>{}</summary>", xml_escape(s)))
                .unwrap_or_default(),
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>{id}</id>
  <title>{title}</title>
  <updated>{now}</updated>
  <link rel="self" href="{self_href}" type="{acq}"/>
  <link rel="up" href="/opds/v1" type="{nav}"/>
{entries}</feed>
"#,
        id = xml_escape(feed_id),
        title = xml_escape(title),
        self_href = xml_escape(self_href),
        now = now,
        acq = ACQ_CT,
        nav = NAV_CT,
        // unused but suppresses dead-code on `app`
        // — public_url is consumed in root() only.
    )
    .replace("{base}", &xml_escape(&app.cfg.public_url))
}

/// Returns the libraries the user is allowed to read. `None` for admins
/// (no filter applied).
async fn allowed_libraries(
    app: &AppState,
    user: &CurrentUser,
) -> Result<Option<Vec<Uuid>>, String> {
    if user.role == "admin" {
        return Ok(None);
    }
    let rows = library_user_access::Entity::find()
        .filter(library_user_access::Column::UserId.eq(user.id))
        .all(&app.db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Some(rows.into_iter().map(|r| r.library_id).collect()))
}

async fn visible(app: &AppState, user: &CurrentUser, lib_id: Uuid) -> bool {
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

fn atom(body: String) -> Response {
    let mut hdrs = HeaderMap::new();
    hdrs.insert(header::CONTENT_TYPE, HeaderValue::from_static(ATOM_CT));
    hdrs.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    (StatusCode::OK, hdrs, body).into_response()
}

fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", "not found")
}

fn server_error<E: std::fmt::Display>(e: E) -> Response {
    tracing::warn!(error = %e, "opds error");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        axum::Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

fn xml_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '&' => "&amp;".chars().collect::<Vec<_>>(),
            '<' => "&lt;".chars().collect(),
            '>' => "&gt;".chars().collect(),
            '"' => "&quot;".chars().collect(),
            '\'' => "&#39;".chars().collect(),
            _ => vec![c],
        })
        .collect()
}

fn url_escape(s: &str) -> String {
    s.bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect()
}
