//! OPDS 1.2 catalog (§8).
//!
//! - Root navigation feed → links to Series, Recent, Search.
//! - Series feed → one entry per series; each links to a per-series feed.
//! - Per-series feed → acquisition entries with download links; paginated.
//! - Recent feed → newest issues across the library, acquisition entries.
//! - Search → series-name LIKE match.
//! - Search description (`/opds/v1/search.xml`) → OpenSearch document; what
//!   KOReader and Chunky 3 fetch to discover the query template.
//! - Download → streams the raw archive file, MIME picked per extension,
//!   honours `Range: bytes=N-M` / `bytes=N-` so resumable clients work.
//!
//! Auth: cookie session, `Authorization: Bearer <jwt|app_…>`, or
//! `Authorization: Basic <b64(user:app_…)>` (Basic restricted to
//! `app_…` tokens by the extractor — JWT-via-Basic is a footgun guard).
//! Every feed query filters via `library_user_access` (admins see all).
//!
//! XML is emitted as escaped strings rather than via a serialization library.
//! The structure is fixed and small; the escaping helper below is the only
//! place where untrusted text is interpolated.

use axum::{
    Extension, Router,
    body::Body,
    extract::{Path as AxPath, Query, Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use entity::{issue, library_user_access, series};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::Deserialize;
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::audit::{self, AuditEntry};
use crate::auth::CurrentUser;
use crate::middleware::{RequestContext, rate_limit};
use crate::state::AppState;

const PAGE_SIZE: u64 = 50;
const ATOM_CT: &str = "application/atom+xml; charset=utf-8";
const NAV_CT: &str = "application/atom+xml;profile=opds-catalog;kind=navigation";
const ACQ_CT: &str = "application/atom+xml;profile=opds-catalog;kind=acquisition";
const DEFAULT_ACQ_MIME: &str = "application/zip";
const WWW_AUTHENTICATE_OPDS: &str = r#"Basic realm="Folio OPDS""#;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/opds/v1", get(root))
        .route("/opds/v1/series", get(series_list))
        .route("/opds/v1/series/{id}", get(series_one))
        .route("/opds/v1/recent", get(recent))
        .route("/opds/v1/search", get(search))
        .route("/opds/v1/search.xml", get(search_description))
        .route("/opds/v1/issues/{id}/file", get(download))
        .layer(middleware::from_fn(www_authenticate_on_401))
        .layer(rate_limit::OPDS.build())
}

/// Adds `WWW-Authenticate: Basic realm="Folio OPDS"` to any 401 produced by
/// downstream OPDS handlers (today, only the auth extractor). Without this,
/// Chunky / KyBook / Panels silently fail to prompt for credentials.
async fn www_authenticate_on_401(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    if resp.status() == StatusCode::UNAUTHORIZED
        && !resp.headers().contains_key(header::WWW_AUTHENTICATE)
    {
        resp.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static(WWW_AUTHENTICATE_OPDS),
        );
    }
    resp
}

/// MIME type to advertise for a comic file by extension. Shared by the
/// acquisition-link `type` attribute and the download response
/// `Content-Type` so the two never drift.
pub(crate) fn mime_for(path: &str) -> &'static str {
    let lower = path.rsplit('.').next().map(str::to_ascii_lowercase);
    match lower.as_deref() {
        Some("cbz") => "application/vnd.comicbook+zip",
        Some("cbr") => "application/vnd.comicbook-rar",
        Some("cb7") => "application/x-cb7",
        Some("cbt") => "application/x-cbt",
        Some("pdf") => "application/pdf",
        Some("epub") => "application/epub+zip",
        _ => DEFAULT_ACQ_MIME,
    }
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
  <link rel="search" href="/opds/v1/search.xml" type="application/opensearchdescription+xml"/>
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
    let mut count_sel = series::Entity::find();
    if let Some(ids) = allowed.as_ref() {
        count_sel = count_sel.filter(series::Column::LibraryId.is_in(ids.clone()));
    }
    let total = match count_sel.count(&app.db).await {
        Ok(n) => n,
        Err(e) => return server_error(e.to_string()),
    };
    let total_pages = total.div_ceil(PAGE_SIZE).max(1);

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
{pagination}{entries}</feed>
"#,
        base = xml_escape(&app.cfg.public_url),
        now = now,
        acq = ACQ_CT,
        nav = NAV_CT,
        pagination = paginate_links("/opds/v1/series", page, total_pages),
        page = page,
    );
    atom(body)
}

async fn series_one(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Query(q): Query<PageQuery>,
) -> Response {
    let s = match series::Entity::find_by_id(id).one(&app.db).await {
        Ok(Some(s)) => s,
        Ok(None) => return not_found(),
        Err(e) => return server_error(e.to_string()),
    };
    if !visible(&app, &user, s.library_id).await {
        return not_found();
    }
    let total = match issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(id))
        .count(&app.db)
        .await
    {
        Ok(n) => n,
        Err(e) => return server_error(e.to_string()),
    };
    let total_pages = total.div_ceil(PAGE_SIZE).max(1);
    let page = q.page.unwrap_or(1).max(1);
    let offset = (page - 1) * PAGE_SIZE;
    let issues = match issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(id))
        .order_by_asc(issue::Column::SortNumber)
        .offset(offset)
        .limit(PAGE_SIZE)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => return server_error(e.to_string()),
    };
    let self_href = format!("/opds/v1/series/{id}");
    let body = build_acquisition_feed(
        &app,
        &format!("urn:series:{id}"),
        &format!("Series — {}", s.name),
        &format!("{self_href}?page={page}"),
        &issues,
        &paginate_links(&self_href, page, total_pages),
    )
    .await;
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
        "",
    )
    .await;
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
        return atom(
            build_acquisition_feed(&app, "urn:search", "Search", "/opds/v1/search", &[], "").await,
        );
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

/// OpenSearch description document. KOReader and Chunky 3 fetch this to
/// discover the query template; clients substitute `{searchTerms}`
/// themselves before issuing the request.
async fn search_description(State(app): State<AppState>, _user: CurrentUser) -> Response {
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">
  <ShortName>{name}</ShortName>
  <Description>Folio OPDS catalog search</Description>
  <InputEncoding>UTF-8</InputEncoding>
  <Url type="{acq}" template="{base}/opds/v1/search?q={{searchTerms}}"/>
</OpenSearchDescription>
"#,
        name = xml_escape("Folio"),
        acq = ACQ_CT,
        base = xml_escape(&app.cfg.public_url),
    );
    let mut hdrs = HeaderMap::new();
    hdrs.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/opensearchdescription+xml; charset=utf-8"),
    );
    hdrs.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    (StatusCode::OK, hdrs, body).into_response()
}

async fn download(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<String>,
    headers: HeaderMap,
) -> Response {
    let row = match issue::Entity::find_by_id(id).one(&app.db).await {
        Ok(Some(r)) => r,
        _ => return not_found(),
    };
    if !visible(&app, &user, row.library_id).await {
        return not_found();
    }
    let mut f = match tokio::fs::File::open(&row.file_path).await {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(error = %e, path = %row.file_path, "opds download open failed");
            return not_found();
        }
    };
    let total = f.metadata().await.ok().map(|m| m.len()).unwrap_or(0);
    let leaf = std::path::Path::new(&row.file_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("comic.cbz")
        .to_owned();
    let mime = mime_for(&row.file_path);

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "opds.download",
            target_type: Some("issue"),
            target_id: Some(row.id.clone()),
            payload: serde_json::json!({ "file_path": row.file_path }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_byte_range(v, total));

    match range {
        Some(Ok((start, end))) => {
            if let Err(e) = f.seek(std::io::SeekFrom::Start(start)).await {
                tracing::warn!(error = %e, path = %row.file_path, "opds range seek failed");
                return server_error(e.to_string());
            }
            let len = end - start + 1;
            let body = Body::from_stream(ReaderStream::new(f.take(len)));
            let mut hdrs = HeaderMap::new();
            hdrs.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
            hdrs.insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!("attachment; filename=\"{leaf}\"")).unwrap(),
            );
            hdrs.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
            hdrs.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            hdrs.insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes {start}-{end}/{total}")).unwrap(),
            );
            (StatusCode::PARTIAL_CONTENT, hdrs, body).into_response()
        }
        Some(Err(())) => {
            let mut hdrs = HeaderMap::new();
            hdrs.insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{total}")).unwrap(),
            );
            (StatusCode::RANGE_NOT_SATISFIABLE, hdrs, Body::empty()).into_response()
        }
        None => {
            let stream = ReaderStream::new(f);
            let mut hdrs = HeaderMap::new();
            hdrs.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
            hdrs.insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!("attachment; filename=\"{leaf}\"")).unwrap(),
            );
            hdrs.insert(header::CONTENT_LENGTH, HeaderValue::from(total));
            hdrs.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            (StatusCode::OK, hdrs, Body::from_stream(stream)).into_response()
        }
    }
}

/// Parse a single `Range: bytes=N-M` or `bytes=N-` header against a known
/// resource size. Returns:
/// - `Some(Ok((start, end)))` for a valid, satisfiable range (inclusive).
/// - `Some(Err(()))` for a malformed or unsatisfiable range — the caller
///   should answer 416.
/// - `None` if the header isn't a `bytes=` range at all (caller falls back
///   to a full 200 response).
///
/// Multi-range (`bytes=0-100,200-300`) is intentionally unsupported — OPDS
/// clients never request it and the multipart/byteranges body shape isn't
/// worth the code.
fn parse_byte_range(header: &str, total: u64) -> Option<Result<(u64, u64), ()>> {
    let rest = header.trim().strip_prefix("bytes=")?;
    if rest.contains(',') {
        return Some(Err(()));
    }
    let (lhs, rhs) = rest.split_once('-')?;
    let lhs = lhs.trim();
    let rhs = rhs.trim();
    // `bytes=-N` (suffix range, last N bytes) is part of the spec but
    // OPDS clients don't use it — answer 416 rather than silently fall
    // through.
    if lhs.is_empty() {
        return Some(Err(()));
    }
    let start: u64 = lhs.parse().map_err(|_| ()).ok()?;
    let end: u64 = if rhs.is_empty() {
        if total == 0 {
            return Some(Err(()));
        }
        total - 1
    } else {
        rhs.parse().map_err(|_| ()).ok()?
    };
    if start > end || end >= total {
        return Some(Err(()));
    }
    Some(Ok((start, end)))
}

// ────────────── helpers ──────────────

async fn build_acquisition_feed(
    app: &AppState,
    feed_id: &str,
    title: &str,
    self_href: &str,
    issues: &[issue::Model],
    pagination: &str,
) -> String {
    let now = chrono::Utc::now().to_rfc3339();
    let slugs = fetch_series_slugs(&app.db, issues).await;
    let mut entries = String::new();
    for i in issues {
        let label = i.title.clone().unwrap_or_else(|| {
            i.number_raw
                .clone()
                .map(|n| format!("Issue #{n}"))
                .unwrap_or_else(|| "Issue".into())
        });
        let series_slug = slugs.get(&i.series_id).map(String::as_str);
        entries.push_str(&format!(
            r#"  <entry>
    <id>urn:issue:{id}</id>
    <title>{title}</title>
    <updated>{updated}</updated>
    {summary}
{metadata}    <link rel="http://opds-spec.org/image/thumbnail" href="/issues/{id}/pages/0/thumb" type="image/webp"/>
    <link rel="http://opds-spec.org/image" href="/issues/{id}/pages/0" type="image/jpeg"/>
{related}    <link rel="http://opds-spec.org/acquisition" href="/opds/v1/issues/{id}/file" type="{mime}"/>
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
            metadata = entry_metadata(i),
            related = series_slug
                .map(|s| format!(
                    "    <link rel=\"related\" href=\"/series/{}\" type=\"application/json\"/>\n",
                    xml_escape(s),
                ))
                .unwrap_or_default(),
            mime = mime_for(&i.file_path),
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:dc="http://purl.org/dc/terms/">
  <id>{id}</id>
  <title>{title}</title>
  <updated>{now}</updated>
  <link rel="self" href="{self_href}" type="{acq}"/>
  <link rel="up" href="/opds/v1" type="{nav}"/>
{pagination}{entries}</feed>
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

/// Bulk-fetch the slug for every distinct series referenced in `issues`.
/// Empty inputs short-circuit so the search-empty-state path doesn't query.
async fn fetch_series_slugs(
    db: &sea_orm::DatabaseConnection,
    issues: &[issue::Model],
) -> HashMap<Uuid, String> {
    if issues.is_empty() {
        return HashMap::new();
    }
    let mut ids: Vec<Uuid> = issues.iter().map(|i| i.series_id).collect();
    ids.sort();
    ids.dedup();
    match series::Entity::find()
        .filter(series::Column::Id.is_in(ids))
        .all(db)
        .await
    {
        Ok(rows) => rows.into_iter().map(|s| (s.id, s.slug)).collect(),
        Err(e) => {
            tracing::warn!(error = %e, "opds: series-slug lookup failed; related rels will be omitted");
            HashMap::new()
        }
    }
}

/// Emit Dublin Core, author, and category elements for an issue entry.
/// Each line is indented 4 spaces to match the surrounding `<entry>` body.
/// Empty / null fields are skipped — never emit a `<dc:foo/>` with no text.
fn entry_metadata(i: &issue::Model) -> String {
    let mut out = String::new();
    let _ = std::fmt::Write::write_fmt(
        &mut out,
        format_args!(
            "    <dc:identifier>urn:folio:issue:{}</dc:identifier>\n",
            xml_escape(&i.id),
        ),
    );
    if let Some(lang) = i.language_code.as_deref().filter(|s| !s.is_empty()) {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!("    <dc:language>{}</dc:language>\n", xml_escape(lang)),
        );
    }
    if let Some(pub_) = i.publisher.as_deref().filter(|s| !s.is_empty()) {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!("    <dc:publisher>{}</dc:publisher>\n", xml_escape(pub_)),
        );
    }
    if let Some(issued) = iso_date_from_ymd(i.year, i.month, i.day) {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!("    <dc:issued>{issued}</dc:issued>\n"),
        );
    }
    if let Some(name) = first_csv_field(i.writer.as_deref()) {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!("    <author><name>{}</name></author>\n", xml_escape(&name)),
        );
    }
    for category in csv_fields(i.genre.as_deref()).chain(csv_fields(i.tags.as_deref())) {
        let escaped = xml_escape(&category);
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!("    <category term=\"{escaped}\" label=\"{escaped}\"/>\n"),
        );
    }
    out
}

/// First non-empty trimmed CSV field, or `None` if the input is empty / all
/// whitespace. ComicInfo stores credit lists as `"Stan Lee, Steve Ditko"`.
fn first_csv_field(raw: Option<&str>) -> Option<String> {
    raw?.split(',')
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Iterator over the unique trimmed non-empty fields of a CSV string.
/// Order preserved; later duplicates suppressed.
fn csv_fields(raw: Option<&str>) -> impl Iterator<Item = String> {
    let mut seen: Vec<String> = Vec::new();
    raw.unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .filter_map(move |s| {
            if seen.iter().any(|prev| prev == &s) {
                None
            } else {
                seen.push(s.clone());
                Some(s)
            }
        })
}

/// Render a partial date as ISO 8601: `2020`, `2020-05`, or `2020-05-04`.
/// Returns `None` when no usable year is present — month/day without year
/// is degenerate and not emitted.
fn iso_date_from_ymd(year: Option<i32>, month: Option<i32>, day: Option<i32>) -> Option<String> {
    let year = year?;
    if year <= 0 {
        return None;
    }
    let m = month.filter(|m| (1..=12).contains(m));
    let d = day.filter(|d| (1..=31).contains(d));
    Some(match (m, d) {
        (Some(m), Some(d)) => format!("{year:04}-{m:02}-{d:02}"),
        (Some(m), None) => format!("{year:04}-{m:02}"),
        _ => format!("{year:04}"),
    })
}

/// Render first/previous/next/last acquisition-feed link rels for a paged
/// resource. `base_href` is the path without the `page` query (`/opds/v1/series`
/// or `/opds/v1/series/{uuid}`). Emits only the rels that apply at the
/// current page so clients don't follow a dangling `next` past the end.
fn paginate_links(base_href: &str, page: u64, total_pages: u64) -> String {
    let mut out = String::new();
    let push = |out: &mut String, rel: &str, p: u64| {
        out.push_str(&format!(
            "  <link rel=\"{rel}\" href=\"{base_href}?page={p}\" type=\"{ACQ_CT}\"/>\n",
        ));
    };
    if total_pages > 1 {
        push(&mut out, "first", 1);
        if page > 1 {
            push(&mut out, "previous", page - 1);
        }
        if page < total_pages {
            push(&mut out, "next", page + 1);
        }
        push(&mut out, "last", total_pages);
    }
    out
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
