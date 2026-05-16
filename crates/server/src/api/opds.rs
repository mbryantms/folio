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
use entity::{
    cbl_entry, cbl_list, collection_entry, issue, library_user_access, saved_view, series,
    series_credit, series_genre, user_page, user_view_pin,
};
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, EntityTrait, FromQueryResult, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Statement, sea_query::PostgresQueryBuilder,
};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::api::collections::ensure_want_to_read_seeded;
use crate::api::saved_views::{KIND_COLLECTION, KIND_FILTER_SERIES, SYSTEM_KEY_WANT_TO_READ};
use crate::audit::{self, AuditEntry};
use crate::auth::{CurrentUser, RequireProgressScope};
use crate::library::access;
use crate::middleware::{RequestContext, rate_limit};
use crate::state::AppState;
use crate::views::{
    compile::{self, CompileInput},
    dsl::{FilterDsl, MatchMode, SortField, SortOrder},
};

pub(crate) const PAGE_SIZE: u64 = 50;
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
        // M4 of opds-richer-feeds — faceted browse over the series
        // list. Facet links materialise as `<link rel="opds-spec.org/
        // facet">` elements that clients render as a filter sidebar.
        .route("/opds/v1/browse", get(browse))
        .route("/opds/v1/recent", get(recent))
        .route("/opds/v1/search", get(search))
        .route("/opds/v1/search.xml", get(search_description))
        .route("/opds/v1/issues/{id}/file", get(download))
        // M4 — personal surfaces
        .route("/opds/v1/wtr", get(wtr))
        .route("/opds/v1/lists", get(cbl_lists_nav))
        .route("/opds/v1/lists/{id}", get(cbl_list_acq))
        .route("/opds/v1/collections", get(collections_nav))
        .route("/opds/v1/collections/{id}", get(collection_acq))
        .route("/opds/v1/views", get(views_nav))
        .route("/opds/v1/views/{id}", get(view_acq))
        // M3 of opds-richer-feeds — user-curated Pages surfaced as OPDS
        // feeds. /opds/v1/pages lists the user's pages in position order;
        // /opds/v1/pages/{slug} expands one page into its pinned saved-
        // views, each linking back into the existing /opds/v1/views/{id}
        // handler.
        .route("/opds/v1/pages", get(pages_nav))
        .route("/opds/v1/pages/{slug}", get(page_acq))
        // M5 — PSE (Page Streaming Extension). Sig-auth only: no
        // CurrentUser extractor on the handler, the URL itself carries
        // the bearer (`?u=&exp=&sig=`). Shares the OPDS rate-limit
        // bucket since streaming clients are part of the same throughput
        // budget.
        .route("/opds/pse/{issue_id}/{n}", get(crate::api::opds_pse::stream))
        // M7 — progress sync. `read+progress`-scoped tokens (or cookie
        // users) can PUT progress; the KOReader sync shim accepts
        // KOReader's wire format and maps it onto the same upsert path.
        .route(
            "/opds/v1/issues/{id}/progress",
            axum::routing::put(progress_put),
        )
        .route(
            "/opds/v1/syncs/progress/{document_hash}",
            axum::routing::put(koreader_sync_put),
        )
        // M6 — content negotiation. Runs before the v1 handlers so a
        // client explicitly asking for OPDS 2.0 gets redirected before
        // we burn a DB roundtrip rendering atom they're going to
        // discard.
        .layer(middleware::from_fn(negotiate_opds_v2))
        .layer(middleware::from_fn(www_authenticate_on_401))
        .layer(rate_limit::OPDS.build())
}

/// Content negotiation between OPDS 1.x (Atom) and OPDS 2.0 (JSON-LD).
/// If a client hits a `/opds/v1/*` path while explicitly preferring
/// `application/opds+json`, we 308-redirect to the matching `/opds/v2/*`
/// route. 308 (not 302) preserves the request method — OPDS clients
/// only ever GET, so this is academic, but the spec calls for permanent
/// redirects in protocol-version negotiation. The download route at
/// `/opds/v1/issues/{id}/file` is left as canonical: byte content is
/// version-agnostic.
async fn negotiate_opds_v2(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let accept = req
        .headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if let Some(suffix) = path.strip_prefix("/opds/v1/")
        && accept.contains("application/opds+json")
        && suffix != "issues/{id}/file"
        && !suffix.starts_with("issues/")
    {
        let mut target = format!("/opds/v2/{suffix}");
        if let Some(q) = req.uri().query() {
            target.push('?');
            target.push_str(q);
        }
        return Response::builder()
            .status(StatusCode::PERMANENT_REDIRECT)
            .header(header::LOCATION, target)
            .body(axum::body::Body::empty())
            .unwrap();
    }
    next.run(req).await
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

/// Query parameters for `/opds/v1/browse` (M4 of opds-richer-feeds).
/// Each field is an optional **single value**. Stacking happens
/// through the AND-conjunction of multiple fields, not multi-value
/// per field — keeps the URL space sane for facet-link generation
/// (one `<link rel=facet>` per (group, value) pair, one active state
/// per group). A user wanting multi-publisher OR-filtering can build
/// that with the existing saved-views surface; the OPDS facet UI
/// commits to single-select per group.
#[derive(Debug, Default, Deserialize, Clone)]
pub struct BrowseQuery {
    pub status: Option<String>,
    pub publisher: Option<String>,
    pub page: Option<u64>,
}

/// Maximum number of distinct publishers surfaced in the publisher
/// facet group. Keeps the facet sidebar bounded on libraries with a
/// long tail of one-off publishers; users searching for a specific
/// rarely-used publisher still reach it via the existing search
/// surface.
pub(crate) const BROWSE_PUBLISHER_FACET_LIMIT: u64 = 20;

/// The fixed set of `series.status` values we'll surface as facet
/// links. Matches `crate::api::series::VALID_STATUSES`. The order
/// here drives the order the client renders the facet group in.
pub(crate) const BROWSE_STATUSES: &[&str] = &["continuing", "ended", "hiatus", "cancelled"];

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
    <id>{base}/opds/v1/browse</id>
    <title>Browse</title>
    <updated>{now}</updated>
    <link rel="subsection" href="/opds/v1/browse" type="{acq}"/>
  </entry>
  <entry>
    <id>{base}/opds/v1/recent</id>
    <title>Recently added</title>
    <updated>{now}</updated>
    <link rel="http://opds-spec.org/sort/new" href="/opds/v1/recent" type="{acq}"/>
  </entry>
  <entry>
    <id>{base}/opds/v1/wtr</id>
    <title>Want to Read</title>
    <updated>{now}</updated>
    <link rel="subsection" href="/opds/v1/wtr" type="{acq}"/>
  </entry>
  <entry>
    <id>{base}/opds/v1/lists</id>
    <title>Reading lists</title>
    <updated>{now}</updated>
    <link rel="subsection" href="/opds/v1/lists" type="{nav}"/>
  </entry>
  <entry>
    <id>{base}/opds/v1/collections</id>
    <title>Collections</title>
    <updated>{now}</updated>
    <link rel="subsection" href="/opds/v1/collections" type="{nav}"/>
  </entry>
  <entry>
    <id>{base}/opds/v1/views</id>
    <title>Saved views</title>
    <updated>{now}</updated>
    <link rel="subsection" href="/opds/v1/views" type="{nav}"/>
  </entry>
  <entry>
    <id>{base}/opds/v1/pages</id>
    <title>My pages</title>
    <updated>{now}</updated>
    <link rel="subsection" href="/opds/v1/pages" type="{nav}"/>
  </entry>
</feed>
"#,
        base = xml_escape(&app.cfg().public_url),
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

    let series_ids: Vec<Uuid> = rows.iter().map(|s| s.id).collect();
    let covers = fetch_cover_issues(&app.db, &series_ids).await;
    let facets = fetch_series_facets(&app.db, &series_ids).await;
    let now = chrono::Utc::now().to_rfc3339();
    let mut entries = String::new();
    for s in &rows {
        let cover = covers.get(&s.id).map(String::as_str);
        let f = facets.get(&s.id);
        entries.push_str(&render_series_subsection_entry(s, cover, f));
    }

    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:dc="http://purl.org/dc/terms/">
  <id>{base}/opds/v1/series?page={page}</id>
  <title>All series</title>
  <updated>{now}</updated>
  <link rel="self" href="/opds/v1/series?page={page}" type="{acq}"/>
  <link rel="up" href="/opds/v1" type="{nav}"/>
{pagination}{entries}</feed>
"#,
        base = xml_escape(&app.cfg().public_url),
        now = now,
        acq = ACQ_CT,
        nav = NAV_CT,
        pagination = paginate_links("/opds/v1/series", page, total_pages),
        page = page,
    );
    atom(body)
}

// ────────────── M4 of opds-richer-feeds — faceted browse ──────────────

/// Compute the distinct publishers across the user's visible
/// libraries, ranked by series count, truncated to
/// [`BROWSE_PUBLISHER_FACET_LIMIT`]. Used to populate the publisher
/// facet group. Returns `(value, count)` pairs sorted alphabetically
/// AFTER truncation so the sidebar reads naturally even on libraries
/// with hundreds of publishers.
pub(crate) async fn compute_publisher_facets(
    app: &AppState,
    allowed: Option<&Vec<Uuid>>,
) -> Vec<(String, u64)> {
    #[derive(FromQueryResult)]
    struct Row {
        publisher: String,
        n: i64,
    }
    let (sql, params) = if let Some(ids) = allowed {
        let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("${i}")).collect();
        (
            format!(
                "SELECT publisher, COUNT(*)::bigint AS n FROM series \
                 WHERE publisher IS NOT NULL AND publisher <> '' \
                   AND library_id IN ({}) \
                 GROUP BY publisher ORDER BY n DESC LIMIT {}",
                placeholders.join(","),
                BROWSE_PUBLISHER_FACET_LIMIT
            ),
            ids.iter().map(|id| (*id).into()).collect::<Vec<_>>(),
        )
    } else {
        (
            format!(
                "SELECT publisher, COUNT(*)::bigint AS n FROM series \
                 WHERE publisher IS NOT NULL AND publisher <> '' \
                 GROUP BY publisher ORDER BY n DESC LIMIT {}",
                BROWSE_PUBLISHER_FACET_LIMIT
            ),
            Vec::new(),
        )
    };
    let stmt = Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, params);
    let rows = match Row::find_by_statement(stmt).all(&app.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "opds: publisher-facet lookup failed");
            return Vec::new();
        }
    };
    let mut out: Vec<(String, u64)> = rows
        .into_iter()
        .map(|r| (r.publisher, r.n.max(0) as u64))
        .collect();
    // Alphabetical AFTER the top-N truncation so order is stable
    // regardless of count ties.
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Compute the count of series per status value within the user's
/// allowed library set. Statuses with zero series are still emitted
/// (count=0) so the facet group has a complete shape — clients can
/// either hide zero-count facets or render them disabled.
pub(crate) async fn compute_status_facets(
    app: &AppState,
    allowed: Option<&Vec<Uuid>>,
) -> Vec<(&'static str, u64)> {
    let mut out: Vec<(&'static str, u64)> = Vec::with_capacity(BROWSE_STATUSES.len());
    for status in BROWSE_STATUSES {
        let mut sel = series::Entity::find().filter(series::Column::Status.eq(*status));
        if let Some(ids) = allowed {
            sel = sel.filter(series::Column::LibraryId.is_in(ids.clone()));
        }
        let n = sel.count(&app.db).await.unwrap_or(0);
        out.push((*status, n));
    }
    out
}

/// Render facet `<link>` blocks for an OPDS Atom feed. The
/// `opds:facetGroup` attribute is what clients use to group
/// links into "Status", "Publisher" sidebars; `opds:activeFacet`
/// flags the currently-selected value so the client renders it
/// as the chosen state.
///
/// Each link's href encodes the FULL post-toggle state: clicking
/// "Status: continuing" while "Publisher: Marvel" is selected
/// produces `?status=continuing&publisher=Marvel`. Clicking the
/// already-active value cancels it (omitting the param entirely).
fn render_browse_facets(
    q: &BrowseQuery,
    status_counts: &[(&str, u64)],
    publisher_counts: &[(String, u64)],
) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    let status_active = q.status.as_deref();
    for (value, count) in status_counts {
        let active = status_active == Some(*value);
        let href = browse_href(
            // Toggle: if this value is already active, the link
            // points at the "clear status" URL.
            if active { None } else { Some(*value) },
            q.publisher.as_deref(),
        );
        let _ = writeln!(
            out,
            r#"  <link rel="http://opds-spec.org/facet" href="{href}" title="{title}" opds:facetGroup="Status" opds:activeFacet="{active}" thr:count="{count}"/>"#,
            href = xml_escape(&href),
            title = xml_escape(&capitalize(value)),
            active = active,
        );
    }

    let publisher_active = q.publisher.as_deref();
    for (value, count) in publisher_counts {
        let active = publisher_active == Some(value.as_str());
        let href = browse_href(
            q.status.as_deref(),
            if active { None } else { Some(value.as_str()) },
        );
        let _ = writeln!(
            out,
            r#"  <link rel="http://opds-spec.org/facet" href="{href}" title="{title}" opds:facetGroup="Publisher" opds:activeFacet="{active}" thr:count="{count}"/>"#,
            href = xml_escape(&href),
            title = xml_escape(value),
            active = active,
        );
    }
    out
}

/// Build a `/opds/v1/browse` URL with the given facet selection.
/// Returns the bare path when no facets are selected so the
/// "no facets active" facet links cleanly clear all filters.
fn browse_href(status: Option<&str>, publisher: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(2);
    if let Some(s) = status {
        parts.push(format!("status={}", url_escape(s)));
    }
    if let Some(p) = publisher {
        parts.push(format!("publisher={}", url_escape(p)));
    }
    if parts.is_empty() {
        "/opds/v1/browse".into()
    } else {
        format!("/opds/v1/browse?{}", parts.join("&"))
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

async fn browse(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<BrowseQuery>,
) -> Response {
    let page = q.page.unwrap_or(1).max(1);
    let offset = (page - 1) * PAGE_SIZE;

    // Validate status if supplied — silently drop unknown values
    // rather than 400ing so a stale facet link from a removed status
    // doesn't break navigation. Equivalent of OPDS's "unknown facet
    // value yields no facet applied" contract.
    let status_filter = q.status.as_deref().filter(|s| BROWSE_STATUSES.contains(s));

    let allowed = match allowed_libraries(&app, &user).await {
        Ok(v) => v,
        Err(e) => return server_error(e),
    };
    let allowed_vec = allowed.clone();

    // Count + fetch the filtered series. Mirrors series_list's two-
    // pass shape so total_pages / pagination linkery is identical.
    let mut count_sel = series::Entity::find();
    if let Some(ids) = allowed.as_ref() {
        count_sel = count_sel.filter(series::Column::LibraryId.is_in(ids.clone()));
    }
    if let Some(s) = status_filter {
        count_sel = count_sel.filter(series::Column::Status.eq(s.to_owned()));
    }
    if let Some(p) = q.publisher.as_deref() {
        count_sel = count_sel.filter(series::Column::Publisher.eq(p.to_owned()));
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
    if let Some(s) = status_filter {
        sel = sel.filter(series::Column::Status.eq(s.to_owned()));
    }
    if let Some(p) = q.publisher.as_deref() {
        sel = sel.filter(series::Column::Publisher.eq(p.to_owned()));
    }
    let rows = match sel.offset(offset).limit(PAGE_SIZE).all(&app.db).await {
        Ok(r) => r,
        Err(e) => return server_error(e.to_string()),
    };
    let series_ids: Vec<Uuid> = rows.iter().map(|s| s.id).collect();
    let covers = fetch_cover_issues(&app.db, &series_ids).await;
    let series_facets = fetch_series_facets(&app.db, &series_ids).await;
    let mut entries = String::new();
    for s in &rows {
        let cover = covers.get(&s.id).map(String::as_str);
        let f = series_facets.get(&s.id);
        entries.push_str(&render_series_subsection_entry(s, cover, f));
    }

    // Facet sidebar — same set on every page, computed against the
    // FULL library scope (not the post-filter slice) so users can
    // expand back out from a narrow filter without re-navigating to
    // /opds/v1/browse.
    let status_counts = compute_status_facets(&app, allowed_vec.as_ref()).await;
    let publisher_counts = compute_publisher_facets(&app, allowed_vec.as_ref()).await;
    let facet_links = render_browse_facets(&q, &status_counts, &publisher_counts);

    // Self / pagination href reflects the current facet state so
    // clients that bookmark a `<link rel=self>` land in the same
    // filtered view.
    let self_href = {
        let mut h = browse_href(status_filter, q.publisher.as_deref());
        if page > 1 {
            if h.contains('?') {
                h.push('&');
            } else {
                h.push('?');
            }
            h.push_str(&format!("page={page}"));
        }
        h
    };
    let now = chrono::Utc::now().to_rfc3339();
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:dc="http://purl.org/dc/terms/" xmlns:opds="http://opds-spec.org/2010/catalog" xmlns:thr="http://purl.org/syndication/thread/1.0">
  <id>urn:opds:browse</id>
  <title>Browse</title>
  <updated>{now}</updated>
  <link rel="self" href="{self_href}" type="{acq}"/>
  <link rel="up" href="/opds/v1" type="{nav}"/>
{facet_links}{pagination}{entries}</feed>
"#,
        self_href = xml_escape(&self_href),
        now = now,
        acq = ACQ_CT,
        nav = NAV_CT,
        // Pagination URLs preserve the facet selection so paging
        // through a filtered set stays filtered.
        pagination = paginate_links(
            &browse_href(status_filter, q.publisher.as_deref()),
            page,
            total_pages
        ),
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
        user.id,
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
        user.id,
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
            build_acquisition_feed(
                &app,
                "urn:search",
                "Search",
                "/opds/v1/search",
                &[],
                "",
                user.id,
            )
            .await,
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

    let series_ids: Vec<Uuid> = rows.iter().map(|s| s.id).collect();
    let covers = fetch_cover_issues(&app.db, &series_ids).await;
    let facets = fetch_series_facets(&app.db, &series_ids).await;
    let now = chrono::Utc::now().to_rfc3339();
    let mut entries = String::new();
    for s in &rows {
        let cover = covers.get(&s.id).map(String::as_str);
        let f = facets.get(&s.id);
        entries.push_str(&render_series_subsection_entry(s, cover, f));
    }

    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:dc="http://purl.org/dc/terms/">
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
        base = xml_escape(&app.cfg().public_url),
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

pub(crate) async fn download(
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
    user_id: Uuid,
) -> String {
    let now = chrono::Utc::now().to_rfc3339();
    let slugs = fetch_series_slugs(&app.db, issues).await;
    let key = app.secrets.url_signing_key.as_ref();
    let mut entries = String::new();
    for i in issues {
        let series_slug = slugs.get(&i.series_id).map(String::as_str);
        entries.push_str(&render_issue_acq_entry(
            i,
            series_slug,
            Some((user_id, key)),
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:dc="http://purl.org/dc/terms/" xmlns:pse="http://vaemendis.net/opds-pse/ns">
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
    )
}

/// Render a single `<entry>` for an acquisition feed. Pulled out so the
/// recent/search/series-detail feeds AND the mixed collection / Want to
/// Read feeds emit identical entry shapes (dc metadata, dual image rels,
/// download acquisition link, optional PSE stream link).
///
/// `user_id` + `url_signing_key` are required because the PSE stream
/// link is signed per (issue, user, exp): the link can't be rendered
/// without knowing the caller. Pass `None` for `pse_ctx` if the entry
/// is rendered for a non-authenticated surface (none today; reserved).
fn render_issue_acq_entry(
    i: &issue::Model,
    series_slug: Option<&str>,
    pse_ctx: Option<(Uuid, &[u8])>,
) -> String {
    let label = i.title.clone().unwrap_or_else(|| {
        i.number_raw
            .clone()
            .map(|n| format!("Issue #{n}"))
            .unwrap_or_else(|| "Issue".into())
    });
    let pse_link = match pse_ctx {
        Some((user_id, key)) => render_pse_stream_link(i, user_id, key),
        None => String::new(),
    };
    format!(
        r#"  <entry>
    <id>urn:issue:{id}</id>
    <title>{title}</title>
    <updated>{updated}</updated>
    {summary}
{metadata}    <link rel="http://opds-spec.org/image/thumbnail" href="/issues/{id}/pages/0/thumb" type="image/webp"/>
    <link rel="http://opds-spec.org/image" href="/issues/{id}/pages/0" type="image/jpeg"/>
{related}    <link rel="http://opds-spec.org/acquisition" href="/opds/v1/issues/{id}/file" type="{mime}"/>
{pse}  </entry>
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
                "    <link rel=\"related\" href=\"/api/series/{}\" type=\"application/json\"/>\n",
                xml_escape(s),
            ))
            .unwrap_or_default(),
        mime = mime_for(&i.file_path),
        pse = pse_link,
    )
}

/// Render the per-entry PSE stream link. The literal `{pageNumber}`
/// token is intentional — OPDS-PSE clients substitute it themselves
/// when fetching each page. `pse:count` advertises the page total so
/// clients can build a UI scrubber without a probe round-trip.
fn render_pse_stream_link(i: &issue::Model, user_id: Uuid, key: &[u8]) -> String {
    let count = i.page_count.unwrap_or(0).max(0);
    if count == 0 {
        return String::new();
    }
    let query = crate::auth::url_signing::issue_query(&i.id, user_id, key);
    // Ampersands in the href must be XML-escaped (`&amp;`) — the
    // `{pageNumber}` token stays literal, the rest of the query is just
    // ASCII alphanumerics and `=`.
    let escaped_query = query.replace('&', "&amp;");
    format!(
        "    <link rel=\"http://vaemendis.net/opds-pse/stream\" type=\"image/jpeg\" pse:count=\"{count}\" href=\"/opds/pse/{id}/{{pageNumber}}?{q}\"/>\n",
        id = i.id,
        q = escaped_query,
    )
}

/// Render a single series `<entry>` whose acquisition is "drill into the
/// per-series feed" (subsection link). Shared by `series_list`, mixed
/// collection feeds, and saved-view filter feeds.
///
/// `cover_issue_id`: when `Some`, emit OPDS `image/thumbnail` +
/// `image` rels pointing at that issue's page-0 art. Clients use
/// these to display series cover art in browse views; without them
/// they fall back to a generic folder icon. Resolved in bulk via
/// [`fetch_cover_issues`] one query per feed render. `None` means
/// the series has no active issues (in-progress import, fully-
/// removed library) — emit the entry without image links and let
/// the client render its placeholder.
fn render_series_subsection_entry(
    s: &series::Model,
    cover_issue_id: Option<&str>,
    facets: Option<&SeriesFacets>,
) -> String {
    let cover_links = match cover_issue_id {
        Some(id) => format!(
            r#"    <link rel="http://opds-spec.org/image/thumbnail" href="/issues/{id}/pages/0/thumb" type="image/webp"/>
    <link rel="http://opds-spec.org/image" href="/issues/{id}/pages/0" type="image/jpeg"/>
"#,
            id = xml_escape(id),
        ),
        None => String::new(),
    };
    format!(
        r#"  <entry>
    <id>urn:series:{id}</id>
    <title>{name}</title>
    <updated>{updated}</updated>
{description}{metadata}{cover_links}    <link rel="subsection" href="/opds/v1/series/{id}" type="{acq}"/>
  </entry>
"#,
        id = s.id,
        name = xml_escape(&s.name),
        updated = s.updated_at.to_rfc3339(),
        description = render_series_description(s.summary.as_deref()),
        metadata = entry_metadata_series(s, facets),
        acq = ACQ_CT,
    )
}

/// Render a series description as either `<summary>` (plain text) or
/// `<content type="html">` (rich markup). The latter lets OPDS clients
/// that support HTML render paragraphs / emphasis instead of jamming
/// the markup into a plain text node. Detection is intentionally
/// coarse: any leading angle-bracket tag *or* a recognisable Markdown
/// emphasis marker triggers the html branch. False positives are
/// cheap — `<content type="html">` requires the body be entity-
/// escaped just like `<summary>`, so worst case is the same plain-
/// text rendering with a different element name.
fn render_series_description(summary: Option<&str>) -> String {
    let Some(text) = summary.map(str::trim).filter(|s| !s.is_empty()) else {
        return String::new();
    };
    let looks_rich = text.starts_with('<')
        || text.contains("\n\n")
        || text.contains("**")
        || text.contains("__")
        || text.contains("[](");
    if looks_rich {
        format!(
            "    <content type=\"html\">{}</content>\n",
            xml_escape(text)
        )
    } else {
        format!("    <summary>{}</summary>\n", xml_escape(text))
    }
}

/// Per-entry metadata block for series rows: Dublin Core publisher /
/// issued / language, one `<author>` element per writer, one
/// `<category>` element per genre. Mirrors the existing
/// [`entry_metadata`] for issue rows; emits nothing when the source
/// fields are all empty so clients that don't speak DC don't see
/// noise.
///
/// **The feed containing series entries with this metadata MUST
/// declare `xmlns:dc="http://purl.org/dc/terms/"` on its `<feed>`
/// element.** Without that namespace the `<dc:*>` elements are not
/// valid XML and strict clients (KOReader's libxml-based parser, in
/// particular) will refuse the entire feed. Folio's series-emitting
/// feeds were updated to declare it in the same change that added
/// this function.
///
/// Output lines are 4-space-indented to nest cleanly inside `<entry>`.
fn entry_metadata_series(s: &series::Model, facets: Option<&SeriesFacets>) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    if let Some(pub_) = s.publisher.as_deref().filter(|v| !v.is_empty()) {
        let _ = writeln!(out, "    <dc:publisher>{}</dc:publisher>", xml_escape(pub_));
    }
    if let Some(year) = s.year {
        let _ = writeln!(out, "    <dc:issued>{year}</dc:issued>");
    }
    if !s.language_code.is_empty() {
        let _ = writeln!(
            out,
            "    <dc:language>{}</dc:language>",
            xml_escape(&s.language_code)
        );
    }
    if let Some(f) = facets {
        // OPDS spec allows multiple `<author>` elements per entry;
        // most clients display the first as the byline and surface
        // the rest in detail views. Emit all writers we know about.
        for person in &f.writers {
            let _ = writeln!(
                out,
                "    <author><name>{}</name></author>",
                xml_escape(person)
            );
        }
        // Genres become Atom `<category>` chips. The `scheme` is
        // Folio-namespaced rather than a generic vocabulary — the
        // genre values come from the rolled-up scanner taxonomy
        // (ComicInfo + user edits) and don't map cleanly to BISAC
        // or any other industry list. Clients that don't recognise
        // the scheme fall back to displaying the `term` as a chip,
        // which is exactly what we want.
        for genre in &f.genres {
            let escaped = xml_escape(genre);
            let _ = writeln!(
                out,
                "    <category term=\"{escaped}\" label=\"{escaped}\" scheme=\"urn:folio:genre\"/>",
            );
        }
    }
    out
}

/// Resolve a "cover issue" per series for OPDS feeds — the issue
/// whose page-0 thumbnail should represent the series in clients
/// like Panels, Chunky, KOReader. Without this, OPDS clients fall
/// back to a generic folder icon for every series entry.
///
/// Pick rule (M1 of opds-richer-feeds): first active, non-removed
/// issue ordered by `sort_number ASC, file_path ASC` — mirrors what
/// the web `api::series::get_one` handler already does for the
/// detail page's hero cover, so OPDS and web see the same image.
///
/// One DB round-trip regardless of input length via Postgres'
/// `DISTINCT ON`. Empty inputs short-circuit. Failures degrade
/// gracefully — return an empty map and let the renderer emit a
/// folder-icon fallback rather than 500ing the whole feed.
pub(crate) async fn fetch_cover_issues(
    db: &sea_orm::DatabaseConnection,
    series_ids: &[Uuid],
) -> HashMap<Uuid, String> {
    if series_ids.is_empty() {
        return HashMap::new();
    }
    let mut ids: Vec<Uuid> = series_ids.to_vec();
    ids.sort();
    ids.dedup();
    #[derive(FromQueryResult)]
    struct Row {
        series_id: Uuid,
        id: String,
    }
    // Postgres `DISTINCT ON (series_id)` keeps the first row per
    // group as ordered by the outer ORDER BY clause. sea-orm doesn't
    // model this directly so we drop to a raw statement; the
    // alternative (a LATERAL join or a window function) is more
    // code for the same plan.
    let stmt = Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        r#"
        SELECT DISTINCT ON (series_id) series_id, id
        FROM issues
        WHERE series_id = ANY($1)
          AND state = 'active'
          AND removed_at IS NULL
        ORDER BY series_id, sort_number ASC NULLS LAST, file_path ASC
        "#,
        [ids.into()],
    );
    match Row::find_by_statement(stmt).all(db).await {
        Ok(rows) => rows.into_iter().map(|r| (r.series_id, r.id)).collect(),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "opds: cover-issue lookup failed; series entries will fall back to folder icons"
            );
            HashMap::new()
        }
    }
}

/// Per-series metadata that OPDS clients display alongside the
/// cover art: writers (Atom `<author>`) and genres (Atom `<category>`).
/// Populated by [`fetch_series_facets`]; consumed by
/// [`entry_metadata_series`] when rendering series entries.
///
/// Both vectors are sorted alphabetically inside the helper so OPDS
/// output is stable across renders.
#[derive(Default, Debug, Clone)]
pub(crate) struct SeriesFacets {
    pub writers: Vec<String>,
    pub genres: Vec<String>,
}

/// Resolve writer credits + genres for every series in `series_ids`
/// with at most two DB round-trips total, regardless of feed size.
/// Sources are the pre-rolled `series_credits` (role='writer') and
/// `series_genres` tables — the scanner's metadata_rollup writes
/// distinct values per series so we don't need to re-aggregate from
/// issue rows here.
///
/// Empty inputs short-circuit. Lookup failures degrade gracefully —
/// return an empty map and let the renderer omit the metadata
/// rather than 500ing the feed.
pub(crate) async fn fetch_series_facets(
    db: &sea_orm::DatabaseConnection,
    series_ids: &[Uuid],
) -> HashMap<Uuid, SeriesFacets> {
    if series_ids.is_empty() {
        return HashMap::new();
    }
    let mut ids: Vec<Uuid> = series_ids.to_vec();
    ids.sort();
    ids.dedup();
    let mut out: HashMap<Uuid, SeriesFacets> = HashMap::new();

    let writer_rows = match series_credit::Entity::find()
        .filter(series_credit::Column::SeriesId.is_in(ids.clone()))
        .filter(series_credit::Column::Role.eq("writer"))
        .all(db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "opds: writer-credit lookup failed");
            Vec::new()
        }
    };
    for c in writer_rows {
        out.entry(c.series_id).or_default().writers.push(c.person);
    }

    let genre_rows = match series_genre::Entity::find()
        .filter(series_genre::Column::SeriesId.is_in(ids))
        .all(db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "opds: genre lookup failed");
            Vec::new()
        }
    };
    for g in genre_rows {
        out.entry(g.series_id).or_default().genres.push(g.genre);
    }

    // Stable output across renders. Junction rows have no inherent
    // order, so without this we'd churn the XML on every request and
    // burn through any CDN cache that fronts the feed.
    for f in out.values_mut() {
        f.writers.sort();
        f.writers.dedup();
        f.genres.sort();
        f.genres.dedup();
    }
    out
}

/// Bulk-fetch the slug for every distinct series referenced in `issues`.
/// Empty inputs short-circuit so the search-empty-state path doesn't query.
pub(crate) async fn fetch_series_slugs(
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
pub(crate) fn first_csv_field(raw: Option<&str>) -> Option<String> {
    raw?.split(',')
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Iterator over the unique trimmed non-empty fields of a CSV string.
/// Order preserved; later duplicates suppressed.
pub(crate) fn csv_fields(raw: Option<&str>) -> impl Iterator<Item = String> {
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
pub(crate) fn iso_date_from_ymd(
    year: Option<i32>,
    month: Option<i32>,
    day: Option<i32>,
) -> Option<String> {
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
    // Handle bases that already carry a query string (M4 browse
    // feeds pass `/opds/v1/browse?status=continuing` to preserve
    // the facet selection across pages). Append with `&` if a `?`
    // is already present, `?` otherwise.
    let sep = if base_href.contains('?') { '&' } else { '?' };
    let push = |out: &mut String, rel: &str, p: u64| {
        out.push_str(&format!(
            "  <link rel=\"{rel}\" href=\"{base_href}{sep}page={p}\" type=\"{ACQ_CT}\"/>\n",
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
/// (no filter applied). Shared with `opds_v2` so the two surfaces apply
/// identical ACLs without duplication.
pub(crate) async fn allowed_libraries(
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

pub(crate) async fn visible(app: &AppState, user: &CurrentUser, lib_id: Uuid) -> bool {
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

// ────────────── M4 — personal surfaces ──────────────
//
// Four nav + acq feed pairs surface the same content the web app shows in
// its sidebar: Want to Read (per-user auto-seeded collection), CBL reading
// lists, all user-owned collections, and saved filter views. Every feed
// runs through the standard `library_user_access` ACL — entries belonging
// to libraries the caller can't see are silently dropped (the existing
// web-side surfaces use the same model).

/// `GET /opds/v1/wtr` — direct shortcut into the auto-seeded Want-to-Read
/// collection. Seeds the row on first hit so a fresh user / fresh OPDS
/// client lands here without needing to visit the web app first.
async fn wtr(State(app): State<AppState>, user: CurrentUser) -> Response {
    let wtr = match ensure_want_to_read_seeded(&app.db, user.id).await {
        Ok(v) => v,
        Err(e) => return server_error(e.to_string()),
    };
    render_collection_acq(&app, &user, &wtr, "/opds/v1/wtr", "Want to Read").await
}

/// `GET /opds/v1/lists` — navigation feed of the user's CBL reading
/// lists (plus system-shared lists with `owner_user_id IS NULL`).
async fn cbl_lists_nav(State(app): State<AppState>, user: CurrentUser) -> Response {
    let rows = match cbl_list::Entity::find()
        .filter(
            Condition::any()
                .add(cbl_list::Column::OwnerUserId.is_null())
                .add(cbl_list::Column::OwnerUserId.eq(user.id)),
        )
        .order_by_asc(cbl_list::Column::ParsedName)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => return server_error(e.to_string()),
    };
    let entries: String = rows
        .iter()
        .map(|l| {
            render_nav_entry(
                &format!("urn:cbl:{}", l.id),
                &l.parsed_name,
                l.description.as_deref(),
                &l.updated_at.to_rfc3339(),
                &format!("/opds/v1/lists/{}", l.id),
            )
        })
        .collect();
    atom(wrap_nav_feed(
        "urn:opds:lists",
        "Reading lists",
        "/opds/v1/lists",
        &entries,
    ))
}

/// `GET /opds/v1/lists/{id}` — acquisition feed of resolved (matched)
/// issues for a CBL reading list, in CBL position order. Unmatched
/// entries are silently skipped — they have no downloadable file.
async fn cbl_list_acq(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> Response {
    let list = match cbl_list::Entity::find_by_id(id).one(&app.db).await {
        Ok(Some(l)) => l,
        Ok(None) => return not_found(),
        Err(e) => return server_error(e.to_string()),
    };
    if let Some(owner) = list.owner_user_id
        && owner != user.id
    {
        return not_found();
    }
    let entries = match cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(id))
        .filter(cbl_entry::Column::MatchedIssueId.is_not_null())
        .order_by_asc(cbl_entry::Column::Position)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => return server_error(e.to_string()),
    };
    let issue_ids: Vec<String> = entries
        .iter()
        .filter_map(|e| e.matched_issue_id.clone())
        .collect();
    let visible = access::for_user(&app, &user).await;
    let issues = fetch_visible_issues_preserving_order(&app, &issue_ids, &visible).await;
    let self_href = format!("/opds/v1/lists/{id}");
    let body = build_acquisition_feed(
        &app,
        &format!("urn:cbl:{id}"),
        &format!("Reading list — {}", list.parsed_name),
        &self_href,
        &issues,
        "",
        user.id,
    )
    .await;
    atom(body)
}

/// `GET /opds/v1/collections` — navigation feed of the caller's
/// collections (including the auto-seeded Want-to-Read). WTR is pulled
/// to the top to match the web sidebar ordering.
async fn collections_nav(State(app): State<AppState>, user: CurrentUser) -> Response {
    if let Err(e) = ensure_want_to_read_seeded(&app.db, user.id).await {
        // Non-fatal: log + carry on. The list call still surfaces any
        // existing rows, so a transient seed failure doesn't blank the
        // catalog.
        tracing::warn!(error = %e, "opds: wtr seed failed; collections nav continuing");
    }
    let mut rows = match saved_view::Entity::find()
        .filter(saved_view::Column::UserId.eq(user.id))
        .filter(saved_view::Column::Kind.eq(KIND_COLLECTION))
        .order_by_asc(saved_view::Column::Name)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => return server_error(e.to_string()),
    };
    // Stable secondary sort: WTR first, then alpha.
    rows.sort_by(|a, b| {
        let a_wtr = a.system_key.as_deref() == Some(SYSTEM_KEY_WANT_TO_READ);
        let b_wtr = b.system_key.as_deref() == Some(SYSTEM_KEY_WANT_TO_READ);
        match (a_wtr, b_wtr) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });
    let entries: String = rows
        .iter()
        .map(|r| {
            render_nav_entry(
                &format!("urn:collection:{}", r.id),
                &r.name,
                r.description.as_deref(),
                &r.updated_at.to_rfc3339(),
                &format!("/opds/v1/collections/{}", r.id),
            )
        })
        .collect();
    atom(wrap_nav_feed(
        "urn:opds:collections",
        "Collections",
        "/opds/v1/collections",
        &entries,
    ))
}

/// `GET /opds/v1/collections/{id}` — mixed acquisition feed (series
/// subsections + issue downloads) in collection-entry position order.
async fn collection_acq(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> Response {
    let view = match saved_view::Entity::find_by_id(id).one(&app.db).await {
        Ok(Some(v)) => v,
        Ok(None) => return not_found(),
        Err(e) => return server_error(e.to_string()),
    };
    // 404 on kind mismatch + 404 on cross-user (don't disclose existence).
    if view.kind != KIND_COLLECTION || view.user_id != Some(user.id) {
        return not_found();
    }
    let self_href = format!("/opds/v1/collections/{id}");
    let title = view.name.clone();
    render_collection_acq(&app, &user, &view, &self_href, &title).await
}

/// `GET /opds/v1/views` — navigation feed of the user's pinned or
/// sidebar-visible **filter** views (`kind = 'filter_series'`). CBL +
/// collection saved views are filtered out — they have dedicated routes.
async fn views_nav(State(app): State<AppState>, user: CurrentUser) -> Response {
    let pins = match user_view_pin::Entity::find()
        .filter(user_view_pin::Column::UserId.eq(user.id))
        .all(&app.db)
        .await
    {
        Ok(p) => p,
        Err(e) => return server_error(e.to_string()),
    };
    let visible_ids: HashSet<Uuid> = pins
        .iter()
        .filter(|p| p.pinned || p.show_in_sidebar)
        .map(|p| p.view_id)
        .collect();
    if visible_ids.is_empty() {
        return atom(wrap_nav_feed(
            "urn:opds:views",
            "Saved views",
            "/opds/v1/views",
            "",
        ));
    }
    let rows = match saved_view::Entity::find()
        .filter(saved_view::Column::Id.is_in(visible_ids.iter().copied().collect::<Vec<_>>()))
        .filter(saved_view::Column::Kind.eq(KIND_FILTER_SERIES))
        .filter(
            // Include system filter views (user_id IS NULL) AND the
            // caller's own. The pin row already gates visibility, but
            // belt-and-braces against another user's pinned-view id
            // leaking in if pin ownership ever drifts.
            Condition::any()
                .add(saved_view::Column::UserId.is_null())
                .add(saved_view::Column::UserId.eq(user.id)),
        )
        .order_by_asc(saved_view::Column::Name)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => return server_error(e.to_string()),
    };
    let entries: String = rows
        .iter()
        .map(|r| {
            render_nav_entry(
                &format!("urn:view:{}", r.id),
                &r.name,
                r.description.as_deref(),
                &r.updated_at.to_rfc3339(),
                &format!("/opds/v1/views/{}", r.id),
            )
        })
        .collect();
    atom(wrap_nav_feed(
        "urn:opds:views",
        "Saved views",
        "/opds/v1/views",
        &entries,
    ))
}

/// `GET /opds/v1/views/{id}` — acquisition feed (series-subsection
/// entries) of a filter view's results. Drives the same compile path as
/// `/me/saved-views/{id}/results` so OPDS sees identical data to the web
/// UI. Library ACL is enforced server-side by the compiler.
async fn view_acq(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> Response {
    let view = match saved_view::Entity::find_by_id(id).one(&app.db).await {
        Ok(Some(v)) => v,
        Ok(None) => return not_found(),
        Err(e) => return server_error(e.to_string()),
    };
    if view.kind != KIND_FILTER_SERIES {
        // 404 to avoid leaking whether the id exists in another kind.
        return not_found();
    }
    if let Some(owner) = view.user_id
        && owner != user.id
    {
        return not_found();
    }
    let filter = match dsl_from_view(&view) {
        Ok(f) => f,
        Err(e) => return server_error(e.to_string()),
    };
    let sort_field = view
        .sort_field
        .as_deref()
        .and_then(SortField::parse)
        .unwrap_or(SortField::CreatedAt);
    let sort_order = match view.sort_order.as_deref() {
        Some("asc") => SortOrder::Asc,
        _ => SortOrder::Desc,
    };
    let view_limit = view.result_limit.unwrap_or(50).clamp(1, 200) as u64;
    let visible = access::for_user(&app, &user).await;
    let input = CompileInput {
        dsl: &filter,
        sort_field,
        sort_order,
        limit: view_limit,
        cursor: None,
        user_id: user.id,
        visible_libraries: visible,
    };
    let stmt = match compile::compile(&input) {
        Ok(s) => s,
        Err(e) => return server_error(format!("{e}")),
    };
    let (sql, values) = stmt.build(PostgresQueryBuilder);
    let backend = app.db.get_database_backend();
    let raw = Statement::from_sql_and_values(backend, sql, values);
    let mut rows = match series::Model::find_by_statement(raw).all(&app.db).await {
        Ok(r) => r,
        Err(e) => return server_error(e.to_string()),
    };
    // Compile fetches limit+1 for cursor purposes; we drop the trailing
    // sentinel row so the feed advertises exactly `result_limit` entries.
    if rows.len() as u64 > view_limit {
        rows.truncate(view_limit as usize);
    }
    let series_ids: Vec<Uuid> = rows.iter().map(|s| s.id).collect();
    let covers = fetch_cover_issues(&app.db, &series_ids).await;
    let facets = fetch_series_facets(&app.db, &series_ids).await;
    let entries: String = rows
        .iter()
        .map(|s| {
            render_series_subsection_entry(
                s,
                covers.get(&s.id).map(String::as_str),
                facets.get(&s.id),
            )
        })
        .collect();
    let self_href = format!("/opds/v1/views/{id}");
    let now = chrono::Utc::now().to_rfc3339();
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:dc="http://purl.org/dc/terms/">
  <id>urn:view:{id}</id>
  <title>{title}</title>
  <updated>{now}</updated>
  <link rel="self" href="{self_href}" type="{acq}"/>
  <link rel="up" href="/opds/v1" type="{nav}"/>
{entries}</feed>
"#,
        id = id,
        title = xml_escape(&view.name),
        now = now,
        self_href = xml_escape(&self_href),
        acq = ACQ_CT,
        nav = NAV_CT,
    );
    atom(body)
}

// ────────────── M3 of opds-richer-feeds — user Pages ──────────────
//
// Cross-link the multi-page rails feature (memory: multi_page_rails_done)
// into OPDS. Each user's pages become a top-level browse hierarchy so the
// reader's "All series" / "Recently added" sections sit alongside the
// user's curated rails ("My horror reads", "Currently in progress", etc.)
// without needing the web UI to discover them.

/// `GET /opds/v1/pages` — navigation feed listing the calling user's
/// pages in `position` order. Each entry drills into the per-page acq
/// feed at `/opds/v1/pages/{slug}`. Returns the user's own pages only
/// — pages are private; this is also enforced by the per-row
/// `user_id` filter (no shared-page concept today).
async fn pages_nav(State(app): State<AppState>, user: CurrentUser) -> Response {
    let pages = match user_page::Entity::find()
        .filter(user_page::Column::UserId.eq(user.id))
        .order_by_asc(user_page::Column::Position)
        .all(&app.db)
        .await
    {
        Ok(p) => p,
        Err(e) => return server_error(e.to_string()),
    };
    let entries: String = pages
        .iter()
        .map(|p| {
            render_nav_entry(
                &format!("urn:page:{}", p.id),
                &p.name,
                p.description.as_deref(),
                &p.updated_at.to_rfc3339(),
                &format!("/opds/v1/pages/{}", xml_escape(&p.slug)),
            )
        })
        .collect();
    atom(wrap_nav_feed(
        "urn:opds:pages",
        "My pages",
        "/opds/v1/pages",
        &entries,
    ))
}

/// `GET /opds/v1/pages/{slug}` — navigation feed expanding one page
/// into its pinned saved-views. Each pin renders as a `subsection`
/// link into the existing `/opds/v1/views/{id}` handler, which
/// already knows how to render a view's results as series entries.
///
/// Ownership: 404 on a slug the calling user doesn't own. Bare
/// "page not found" is intentional — operator-grade leak guard,
/// same as `/opds/v1/views/{id}` for non-owned views.
///
/// Pin visibility: same logic as the existing /opds/v1/views nav
/// feed — surface pins where `pinned = true` (rail-visible) or
/// `show_in_sidebar = true`. A pin that's neither is unscoped state
/// the user has saved but isn't actively using; hide it.
///
/// Pin kinds: only filter-views are exposed today. The mixed
/// `collection` kind would also work but lives at /opds/v1/lists
/// already, so we defer the cross-link until M7 unifies the surface.
async fn page_acq(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
) -> Response {
    let page = match user_page::Entity::find()
        .filter(user_page::Column::UserId.eq(user.id))
        .filter(user_page::Column::Slug.eq(slug.clone()))
        .one(&app.db)
        .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return not_found(),
        Err(e) => return server_error(e.to_string()),
    };
    let pins = match user_view_pin::Entity::find()
        .filter(user_view_pin::Column::UserId.eq(user.id))
        .filter(user_view_pin::Column::PageId.eq(page.id))
        .order_by_asc(user_view_pin::Column::Position)
        .all(&app.db)
        .await
    {
        Ok(p) => p,
        Err(e) => return server_error(e.to_string()),
    };
    let visible_pins: Vec<&user_view_pin::Model> = pins
        .iter()
        .filter(|p| p.pinned || p.show_in_sidebar)
        .collect();
    let self_href = format!("/opds/v1/pages/{}", xml_escape(&page.slug));
    if visible_pins.is_empty() {
        return atom(wrap_nav_feed(
            &format!("urn:page:{}", page.id),
            &page.name,
            &self_href,
            "",
        ));
    }
    let view_ids: Vec<Uuid> = visible_pins.iter().map(|p| p.view_id).collect();
    let view_rows = match saved_view::Entity::find()
        .filter(saved_view::Column::Id.is_in(view_ids.clone()))
        .filter(saved_view::Column::Kind.eq(KIND_FILTER_SERIES))
        .filter(
            // Same belt-and-braces ownership check as `/opds/v1/views`.
            Condition::any()
                .add(saved_view::Column::UserId.is_null())
                .add(saved_view::Column::UserId.eq(user.id)),
        )
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => return server_error(e.to_string()),
    };
    let view_by_id: HashMap<Uuid, &saved_view::Model> =
        view_rows.iter().map(|v| (v.id, v)).collect();
    // Walk pins in pin-position order so the OPDS surface mirrors
    // the order the user sees in the web sidebar / rail grid.
    let mut entries = String::new();
    for pin in &visible_pins {
        if let Some(v) = view_by_id.get(&pin.view_id) {
            entries.push_str(&render_nav_entry(
                &format!("urn:view:{}", v.id),
                &v.name,
                v.description.as_deref(),
                &v.updated_at.to_rfc3339(),
                &format!("/opds/v1/views/{}", v.id),
            ));
        }
    }
    atom(wrap_nav_feed(
        &format!("urn:page:{}", page.id),
        &page.name,
        &self_href,
        &entries,
    ))
}

// ────────────── M4 helpers ──────────────

/// Render the body of a collection acquisition feed (Want to Read +
/// `/opds/v1/collections/{id}`). Collections carry mixed series + issue
/// entries: series surface as subsection links into the per-series feed;
/// issues surface as direct file acquisitions. Position order is
/// preserved.
async fn render_collection_acq(
    app: &AppState,
    user: &CurrentUser,
    view: &saved_view::Model,
    self_href: &str,
    title: &str,
) -> Response {
    let rows = match collection_entry::Entity::find()
        .filter(collection_entry::Column::SavedViewId.eq(view.id))
        .order_by_asc(collection_entry::Column::Position)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => return server_error(e.to_string()),
    };

    let series_ids: Vec<Uuid> = rows.iter().filter_map(|r| r.series_id).collect();
    let issue_ids: Vec<String> = rows.iter().filter_map(|r| r.issue_id.clone()).collect();

    let visible = access::for_user(app, user).await;

    let series_by_id: HashMap<Uuid, series::Model> = if series_ids.is_empty() {
        HashMap::new()
    } else {
        series::Entity::find()
            .filter(series::Column::Id.is_in(series_ids))
            .all(&app.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|s| visible.contains(s.library_id))
            .map(|s| (s.id, s))
            .collect()
    };
    let issue_by_id: HashMap<String, issue::Model> = if issue_ids.is_empty() {
        HashMap::new()
    } else {
        issue::Entity::find()
            .filter(issue::Column::Id.is_in(issue_ids))
            .all(&app.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|i| visible.contains(i.library_id))
            .map(|i| (i.id.clone(), i))
            .collect()
    };
    // Series slug lookup for `related` links on issue entries.
    let issue_series_ids: Vec<Uuid> = issue_by_id.values().map(|i| i.series_id).collect();
    let issue_slug_by_series: HashMap<Uuid, String> = if issue_series_ids.is_empty() {
        HashMap::new()
    } else {
        series::Entity::find()
            .filter(series::Column::Id.is_in(issue_series_ids))
            .all(&app.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.id, s.slug))
            .collect()
    };
    // Per-series cover so series entries in mixed collection feeds
    // render with art instead of folder icons (M1 of opds-richer-feeds).
    // Per-series facets so they also carry dc:publisher / author /
    // category metadata (M2).
    let collection_series_ids: Vec<Uuid> = series_by_id.keys().copied().collect();
    let series_covers = fetch_cover_issues(&app.db, &collection_series_ids).await;
    let series_facets = fetch_series_facets(&app.db, &collection_series_ids).await;

    let key = app.secrets.url_signing_key.as_ref();
    let mut entries = String::new();
    for row in &rows {
        if let Some(sid) = row.series_id
            && let Some(s) = series_by_id.get(&sid)
        {
            let cover = series_covers.get(&s.id).map(String::as_str);
            let f = series_facets.get(&s.id);
            entries.push_str(&render_series_subsection_entry(s, cover, f));
        } else if let Some(iid) = row.issue_id.as_deref()
            && let Some(i) = issue_by_id.get(iid)
        {
            let slug = issue_slug_by_series.get(&i.series_id).map(String::as_str);
            entries.push_str(&render_issue_acq_entry(i, slug, Some((user.id, key))));
        }
    }

    let now = chrono::Utc::now().to_rfc3339();
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:dc="http://purl.org/dc/terms/" xmlns:pse="http://vaemendis.net/opds-pse/ns">
  <id>urn:collection:{view_id}</id>
  <title>{title}</title>
  <updated>{now}</updated>
  <link rel="self" href="{self_href}" type="{acq}"/>
  <link rel="up" href="/opds/v1" type="{nav}"/>
{entries}</feed>
"#,
        view_id = view.id,
        title = xml_escape(title),
        now = now,
        self_href = xml_escape(self_href),
        acq = ACQ_CT,
        nav = NAV_CT,
    );
    atom(body)
}

/// Fetch issues whose ids are in `ids`, drop any in libraries the user
/// can't see, and return them **in the input order**. The CBL acq feed
/// preserves reading-list position; sea_orm's `is_in` doesn't.
pub(crate) async fn fetch_visible_issues_preserving_order(
    app: &AppState,
    ids: &[String],
    visible: &access::VisibleLibraries,
) -> Vec<issue::Model> {
    if ids.is_empty() {
        return Vec::new();
    }
    let rows = issue::Entity::find()
        .filter(issue::Column::Id.is_in(ids.to_vec()))
        .all(&app.db)
        .await
        .unwrap_or_default();
    let by_id: HashMap<String, issue::Model> = rows
        .into_iter()
        .filter(|i| visible.contains(i.library_id))
        .map(|i| (i.id.clone(), i))
        .collect();
    ids.iter().filter_map(|id| by_id.get(id).cloned()).collect()
}

/// Reconstruct a `FilterDsl` from a stored saved-view row. Mirrors the
/// logic in `saved_views::dsl_from_view` (private there) — duplicated
/// here rather than re-exported to keep the cross-module surface narrow.
pub(crate) fn dsl_from_view(view: &saved_view::Model) -> Result<FilterDsl, serde_json::Error> {
    let mode = match view.match_mode.as_deref() {
        Some("any") => MatchMode::Any,
        _ => MatchMode::All,
    };
    let conditions = match view.conditions.as_ref() {
        Some(j) => serde_json::from_value(j.clone())?,
        None => Vec::new(),
    };
    Ok(FilterDsl {
        match_mode: mode,
        conditions,
    })
}

/// Render a single `<entry>` for a navigation feed. Used by `/lists`,
/// `/collections`, and `/views` — same shape, just different titles +
/// links. Summary is optional (we omit the element entirely when None
/// rather than emit an empty `<summary/>`).
fn render_nav_entry(
    id_urn: &str,
    title: &str,
    summary: Option<&str>,
    updated_rfc3339: &str,
    detail_href: &str,
) -> String {
    let summary_xml = summary
        .filter(|s| !s.is_empty())
        .map(|s| format!("\n    <summary>{}</summary>", xml_escape(s)))
        .unwrap_or_default();
    format!(
        r#"  <entry>
    <id>{id}</id>
    <title>{title}</title>
    <updated>{updated}</updated>{summary}
    <link rel="subsection" href="{href}" type="{acq}"/>
  </entry>
"#,
        id = xml_escape(id_urn),
        title = xml_escape(title),
        updated = updated_rfc3339,
        summary = summary_xml,
        href = xml_escape(detail_href),
        acq = ACQ_CT,
    )
}

/// Wrap a pre-rendered string of `<entry>` blocks in the standard
/// navigation-feed envelope.
fn wrap_nav_feed(feed_id: &str, title: &str, self_href: &str, entries: &str) -> String {
    let now = chrono::Utc::now().to_rfc3339();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>{id}</id>
  <title>{title}</title>
  <updated>{now}</updated>
  <link rel="self" href="{self_href}" type="{nav}"/>
  <link rel="up" href="/opds/v1" type="{nav}"/>
{entries}</feed>
"#,
        id = xml_escape(feed_id),
        title = xml_escape(title),
        now = now,
        self_href = xml_escape(self_href),
        nav = NAV_CT,
    )
}

// ────────────── M7 — progress sync ──────────────

#[derive(Debug, Deserialize)]
pub(crate) struct ProgressPutReq {
    pub page: i32,
    /// Optional — when absent, server preserves the previous `finished`
    /// flag. Matches `POST /progress`'s semantics for cross-surface
    /// consistency.
    #[serde(default)]
    pub finished: Option<bool>,
    /// Free-form client identifier for tracing reads (e.g. "Chunky/iPad").
    /// Echoed back via the `device` column on the progress row.
    #[serde(default)]
    pub device: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct ProgressPutResp {
    issue_id: String,
    page: i32,
    percent: f64,
    finished: bool,
    updated_at: String,
}

/// `PUT /opds/v1/issues/{id}/progress` — write reading progress from an
/// OPDS client. Requires either a cookie session or an app-password
/// scoped `read+progress`. Audit row `opds.progress.write` per call.
///
/// Exposed `pub(crate)` so the v2 router can re-use the exact same
/// handler at `PUT /opds/v2/issues/{id}/progress`; the body + ACL +
/// audit shape are identical across protocol versions.
pub(crate) async fn progress_put(
    State(app): State<AppState>,
    user: RequireProgressScope,
    Extension(ctx): Extension<RequestContext>,
    AxPath(issue_id): AxPath<String>,
    axum::Json(req): axum::Json<ProgressPutReq>,
) -> Response {
    let user = user.0;
    if req.page < 0 {
        return error(StatusCode::BAD_REQUEST, "validation", "page must be >= 0");
    }
    let row = match issue::Entity::find_by_id(issue_id.clone())
        .one(&app.db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return not_found(),
        Err(e) => return server_error(e.to_string()),
    };
    if !visible(&app, &user, row.library_id).await {
        return not_found();
    }
    let model = match crate::api::progress::upsert_for(
        &app,
        user.id,
        &row,
        req.page,
        req.finished,
        req.device.clone(),
    )
    .await
    {
        Ok(m) => m,
        Err(e) => return server_error(e.to_string()),
    };

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "opds.progress.write",
            target_type: Some("issue"),
            target_id: Some(row.id.clone()),
            payload: serde_json::json!({ "page": model.last_page, "finished": model.finished }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    axum::Json(ProgressPutResp {
        issue_id: model.issue_id,
        page: model.last_page,
        percent: model.percent,
        finished: model.finished,
        updated_at: model.updated_at.to_rfc3339(),
    })
    .into_response()
}

#[derive(Debug, Deserialize)]
pub(crate) struct KoreaderSyncReq {
    /// Optional in the body when present in the URL path. Tolerated so
    /// older KOReader builds that include it still work.
    #[serde(default)]
    pub document: Option<String>,
    /// Opaque KOReader state string. Persisted verbatim into `device`
    /// so a future GET roundtrip can echo it back — KOReader treats it
    /// as a black-box marker.
    #[serde(default)]
    pub progress: Option<String>,
    /// 0-1 float. Multiplied by `issue.page_count` to derive the
    /// integer `last_page` Folio stores.
    pub percentage: f64,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct KoreaderSyncResp {
    document: String,
    timestamp: i64,
}

/// `PUT /opds/v1/syncs/progress/{document_hash}` — KOReader Sync.app
/// wire-format shim. KOReader's stock setup expects a server that
/// accepts a percentage-based update and replies with `{document,
/// timestamp}`. The shim maps `document_hash` to `issue.id` (both are
/// BLAKE3-hex of the file bytes — same value), converts percentage →
/// integer page, then routes through the standard upsert path so the
/// row shows up in the web reader's progress list.
async fn koreader_sync_put(
    State(app): State<AppState>,
    user: RequireProgressScope,
    Extension(ctx): Extension<RequestContext>,
    AxPath(document_hash): AxPath<String>,
    axum::Json(req): axum::Json<KoreaderSyncReq>,
) -> Response {
    let user = user.0;
    if !(0.0..=1.0).contains(&req.percentage) {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "percentage must be between 0 and 1",
        );
    }
    // Tolerate KOReader builds that send `document` in the body — if
    // both are present, the URL wins (it's already past the router) but
    // they must agree.
    if let Some(body_doc) = req.document.as_deref()
        && body_doc != document_hash
    {
        return error(
            StatusCode::BAD_REQUEST,
            "validation",
            "URL document_hash and body.document disagree",
        );
    }
    let row = match issue::Entity::find_by_id(document_hash.clone())
        .one(&app.db)
        .await
    {
        Ok(Some(r)) => r,
        // KOReader expects 401 (not 404) when the hash is unknown to
        // the syncing user — keeps the client's retry behavior sane on
        // device-side cache drift.
        Ok(None) => {
            return error(
                StatusCode::UNAUTHORIZED,
                "document_unknown",
                "document_unknown",
            );
        }
        Err(e) => return server_error(e.to_string()),
    };
    if !visible(&app, &user, row.library_id).await {
        return error(
            StatusCode::UNAUTHORIZED,
            "document_unknown",
            "document_unknown",
        );
    }
    let page_count = row.page_count.unwrap_or(0).max(0);
    let page = if page_count > 0 {
        ((req.percentage * page_count as f64).round() as i32).clamp(0, page_count - 1)
    } else {
        0
    };
    let finished_hint = if req.percentage >= 1.0 {
        Some(true)
    } else {
        None
    };
    // Roll the KOReader marker string into the `device` column so a
    // later GET can echo it back. `device` is the only free-form
    // string column on `progress_records`, hence the double duty.
    let device = req.progress.clone().or(req.device.clone());
    let model =
        match crate::api::progress::upsert_for(&app, user.id, &row, page, finished_hint, device)
            .await
        {
            Ok(m) => m,
            Err(e) => return server_error(e.to_string()),
        };

    audit::record(
        &app.db,
        AuditEntry {
            actor_id: user.id,
            action: "opds.progress.write",
            target_type: Some("issue"),
            target_id: Some(row.id.clone()),
            payload: serde_json::json!({
                "source": "koreader",
                "percentage": req.percentage,
                "page": model.last_page,
                "device_id": req.device_id,
            }),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;

    axum::Json(KoreaderSyncResp {
        document: document_hash,
        timestamp: model.updated_at.timestamp(),
    })
    .into_response()
}
