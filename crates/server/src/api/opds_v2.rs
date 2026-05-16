//! OPDS 2.0 — JSON-LD parallel surface to the Atom XML feeds in
//! [`api::opds`](super::opds).
//!
//! Every route under `/opds/v2/*` mirrors its `/opds/v1/*` counterpart's
//! data exactly; only the wire format differs. Data fetching, library
//! ACL, audit, and rate-limiting reuse the v1 helpers (`allowed_libraries`,
//! `visible`, `fetch_series_slugs`, `fetch_visible_issues_preserving_order`,
//! `dsl_from_view`, `ensure_want_to_read_seeded`) so the two protocols
//! can't drift in business logic.
//!
//! Content type for nav + acquisition feeds is `application/opds+json`
//! per the OPDS 2.0 spec. Publication-detail responses don't exist in
//! this surface today — series detail uses the publications-list shape,
//! matching how the web app's series page lists issues directly.
//!
//! Acquisition link payload mirrors the Atom output: every issue gets
//! a download link with the per-extension MIME from `mime_for`, an
//! image-thumbnail link, a full-size image link, an optional related
//! link to the JSON API, and the PSE signed-URL stream link (rendered
//! by `crate::auth::url_signing::issue_query`).
//!
//! Personal surfaces (`/opds/v2/wtr`, `/opds/v2/lists`, `/opds/v2/collections`,
//! `/opds/v2/views`) mirror M4. The mixed-collection feed surfaces
//! series-kind entries as **navigation** entries (pointing into the
//! per-series feed) and issue-kind entries as **publications** — OPDS 2.0
//! permits both in the same document.

use axum::{
    Router,
    extract::{Path as AxPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use entity::{cbl_entry, cbl_list, collection_entry, issue, saved_view, series, user_view_pin};
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, EntityTrait, FromQueryResult, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Statement, sea_query::PostgresQueryBuilder,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::api::collections::ensure_want_to_read_seeded;
use crate::api::opds;
use crate::api::saved_views::{KIND_COLLECTION, KIND_FILTER_SERIES, SYSTEM_KEY_WANT_TO_READ};
use crate::auth::CurrentUser;
use crate::library::access;
use crate::middleware::rate_limit;
use crate::state::AppState;
use crate::views::{
    compile::{self, CompileInput},
    dsl::{SortField, SortOrder},
};

const NAV_CT: &str = "application/opds+json";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/opds/v2", get(root))
        .route("/opds/v2/series", get(series_list))
        .route("/opds/v2/series/{id}", get(series_one))
        .route("/opds/v2/recent", get(recent))
        .route("/opds/v2/search", get(search))
        .route("/opds/v2/issues/{id}/file", get(super::opds::download))
        // Personal surfaces (M4 parity)
        .route("/opds/v2/wtr", get(wtr))
        .route("/opds/v2/lists", get(cbl_lists_nav))
        .route("/opds/v2/lists/{id}", get(cbl_list_acq))
        .route("/opds/v2/collections", get(collections_nav))
        .route("/opds/v2/collections/{id}", get(collection_acq))
        .route("/opds/v2/views", get(views_nav))
        .route("/opds/v2/views/{id}", get(view_acq))
        // M7 — progress write. Same handler as v1; OPDS 2.0 clients
        // posting to either path land in the same audit row.
        .route(
            "/opds/v2/issues/{id}/progress",
            axum::routing::put(super::opds::progress_put),
        )
        .layer(rate_limit::OPDS.build())
}

// ────────────── handlers — catalog ──────────────

async fn root(State(_app): State<AppState>, _user: CurrentUser) -> Response {
    let body = json!({
        "metadata": {
            "title": "Folio OPDS 2.0",
        },
        "links": [
            { "rel": "self", "href": "/opds/v2", "type": NAV_CT },
            { "rel": "start", "href": "/opds/v2", "type": NAV_CT },
            { "rel": "search",
              "href": "/opds/v2/search{?query}",
              "type": NAV_CT,
              "templated": true },
        ],
        "navigation": [
            { "title": "All series", "href": "/opds/v2/series", "type": NAV_CT },
            { "title": "Recently added", "href": "/opds/v2/recent", "type": NAV_CT },
            { "title": "Want to Read", "href": "/opds/v2/wtr", "type": NAV_CT },
            { "title": "Reading lists", "href": "/opds/v2/lists", "type": NAV_CT },
            { "title": "Collections", "href": "/opds/v2/collections", "type": NAV_CT },
            { "title": "Saved views", "href": "/opds/v2/views", "type": NAV_CT },
        ],
    });
    json_response(body)
}

#[derive(Debug, Deserialize)]
struct PageQuery {
    page: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: Option<String>,
}

async fn series_list(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<PageQuery>,
) -> Response {
    let page = q.page.unwrap_or(1).max(1);
    let offset = (page - 1) * opds::PAGE_SIZE;

    let allowed = match opds::allowed_libraries(&app, &user).await {
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
    let total_pages = total.div_ceil(opds::PAGE_SIZE).max(1);

    let mut sel = series::Entity::find().order_by_asc(series::Column::Name);
    if let Some(ids) = allowed.as_ref() {
        sel = sel.filter(series::Column::LibraryId.is_in(ids.clone()));
    }
    let rows = match sel.offset(offset).limit(opds::PAGE_SIZE).all(&app.db).await {
        Ok(r) => r,
        Err(e) => return server_error(e.to_string()),
    };

    let navigation: Vec<Value> = rows.iter().map(series_nav_entry).collect();
    let mut links = vec![json!({
        "rel": "self",
        "href": format!("/opds/v2/series?page={page}"),
        "type": NAV_CT,
    })];
    paginate_links(
        &mut links,
        "/opds/v2/series",
        page,
        total_pages,
        opds::PAGE_SIZE,
    );

    let body = json!({
        "metadata": {
            "title": "All series",
            "itemsPerPage": opds::PAGE_SIZE,
            "numberOfItems": total,
            "currentPage": page,
        },
        "links": links,
        "navigation": navigation,
    });
    json_response(body)
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
    if !opds::visible(&app, &user, s.library_id).await {
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
    let total_pages = total.div_ceil(opds::PAGE_SIZE).max(1);
    let page = q.page.unwrap_or(1).max(1);
    let offset = (page - 1) * opds::PAGE_SIZE;
    let issues = match issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(id))
        .order_by_asc(issue::Column::SortNumber)
        .offset(offset)
        .limit(opds::PAGE_SIZE)
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => return server_error(e.to_string()),
    };
    let self_href = format!("/opds/v2/series/{id}");
    let publications = build_publications(&app, &user, &issues).await;
    let mut links = vec![json!({
        "rel": "self",
        "href": format!("{self_href}?page={page}"),
        "type": NAV_CT,
    })];
    paginate_links(&mut links, &self_href, page, total_pages, opds::PAGE_SIZE);

    let body = json!({
        "metadata": {
            "title": format!("Series — {}", s.name),
            "itemsPerPage": opds::PAGE_SIZE,
            "numberOfItems": total,
            "currentPage": page,
        },
        "links": links,
        "publications": publications,
    });
    json_response(body)
}

async fn recent(State(app): State<AppState>, user: CurrentUser) -> Response {
    let allowed = match opds::allowed_libraries(&app, &user).await {
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
    let publications = build_publications(&app, &user, &rows).await;
    let body = json!({
        "metadata": {
            "title": "Recently added",
            "numberOfItems": publications.len(),
        },
        "links": [
            { "rel": "self", "href": "/opds/v2/recent", "type": NAV_CT },
        ],
        "publications": publications,
    });
    json_response(body)
}

async fn search(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<SearchQuery>,
) -> Response {
    let needle = q.q.unwrap_or_default();
    let needle = needle.trim();
    if needle.is_empty() {
        let body = json!({
            "metadata": { "title": "Search" },
            "links": [
                { "rel": "self", "href": "/opds/v2/search", "type": NAV_CT },
                { "rel": "search",
                  "href": "/opds/v2/search{?query}",
                  "type": NAV_CT,
                  "templated": true },
            ],
            "navigation": [],
        });
        return json_response(body);
    }
    if needle.len() > 200 {
        return error_status(StatusCode::BAD_REQUEST, "validation");
    }
    let allowed = match opds::allowed_libraries(&app, &user).await {
        Ok(v) => v,
        Err(e) => return server_error(e),
    };
    let pattern = format!("%{}%", needle.replace('\\', "\\\\").replace('%', "\\%"));
    let mut sel = series::Entity::find()
        .filter(series::Column::Name.like(&pattern))
        .order_by_asc(series::Column::Name)
        .limit(opds::PAGE_SIZE);
    if let Some(ids) = allowed.as_ref() {
        sel = sel.filter(series::Column::LibraryId.is_in(ids.clone()));
    }
    let rows = match sel.all(&app.db).await {
        Ok(r) => r,
        Err(e) => return server_error(e.to_string()),
    };
    let navigation: Vec<Value> = rows.iter().map(series_nav_entry).collect();
    let body = json!({
        "metadata": {
            "title": format!("Search — {needle}"),
            "numberOfItems": navigation.len(),
        },
        "links": [
            { "rel": "self",
              "href": format!("/opds/v2/search?q={}", url_escape(needle)),
              "type": NAV_CT },
        ],
        "navigation": navigation,
    });
    json_response(body)
}

// ────────────── handlers — personal surfaces ──────────────

async fn wtr(State(app): State<AppState>, user: CurrentUser) -> Response {
    let wtr = match ensure_want_to_read_seeded(&app.db, user.id).await {
        Ok(v) => v,
        Err(e) => return server_error(e.to_string()),
    };
    render_collection_acq_v2(&app, &user, &wtr, "/opds/v2/wtr", "Want to Read").await
}

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
    let navigation: Vec<Value> = rows
        .iter()
        .map(|l| {
            json!({
                "title": l.parsed_name,
                "href": format!("/opds/v2/lists/{}", l.id),
                "type": NAV_CT,
                "metadata": {
                    "identifier": format!("urn:cbl:{}", l.id),
                    "description": l.description,
                    "modified": l.updated_at.to_rfc3339(),
                },
            })
        })
        .collect();
    let body = json!({
        "metadata": { "title": "Reading lists", "numberOfItems": navigation.len() },
        "links": [{ "rel": "self", "href": "/opds/v2/lists", "type": NAV_CT }],
        "navigation": navigation,
    });
    json_response(body)
}

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
    let issues = opds::fetch_visible_issues_preserving_order(&app, &issue_ids, &visible).await;
    let publications = build_publications(&app, &user, &issues).await;

    let body = json!({
        "metadata": {
            "title": format!("Reading list — {}", list.parsed_name),
            "identifier": format!("urn:cbl:{id}"),
            "numberOfItems": publications.len(),
        },
        "links": [{
            "rel": "self",
            "href": format!("/opds/v2/lists/{id}"),
            "type": NAV_CT,
        }],
        "publications": publications,
    });
    json_response(body)
}

async fn collections_nav(State(app): State<AppState>, user: CurrentUser) -> Response {
    if let Err(e) = ensure_want_to_read_seeded(&app.db, user.id).await {
        tracing::warn!(error = %e, "opds_v2: wtr seed failed; collections nav continuing");
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
    rows.sort_by(|a, b| {
        let a_wtr = a.system_key.as_deref() == Some(SYSTEM_KEY_WANT_TO_READ);
        let b_wtr = b.system_key.as_deref() == Some(SYSTEM_KEY_WANT_TO_READ);
        match (a_wtr, b_wtr) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });
    let navigation: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "title": r.name,
                "href": format!("/opds/v2/collections/{}", r.id),
                "type": NAV_CT,
                "metadata": {
                    "identifier": format!("urn:collection:{}", r.id),
                    "description": r.description,
                    "modified": r.updated_at.to_rfc3339(),
                },
            })
        })
        .collect();
    let body = json!({
        "metadata": { "title": "Collections", "numberOfItems": navigation.len() },
        "links": [{ "rel": "self", "href": "/opds/v2/collections", "type": NAV_CT }],
        "navigation": navigation,
    });
    json_response(body)
}

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
    if view.kind != KIND_COLLECTION || view.user_id != Some(user.id) {
        return not_found();
    }
    let self_href = format!("/opds/v2/collections/{id}");
    let title = view.name.clone();
    render_collection_acq_v2(&app, &user, &view, &self_href, &title).await
}

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
        let body = json!({
            "metadata": { "title": "Saved views", "numberOfItems": 0 },
            "links": [{ "rel": "self", "href": "/opds/v2/views", "type": NAV_CT }],
            "navigation": [],
        });
        return json_response(body);
    }
    let rows = match saved_view::Entity::find()
        .filter(saved_view::Column::Id.is_in(visible_ids.iter().copied().collect::<Vec<_>>()))
        .filter(saved_view::Column::Kind.eq(KIND_FILTER_SERIES))
        .filter(
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
    let navigation: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "title": r.name,
                "href": format!("/opds/v2/views/{}", r.id),
                "type": NAV_CT,
                "metadata": {
                    "identifier": format!("urn:view:{}", r.id),
                    "description": r.description,
                    "modified": r.updated_at.to_rfc3339(),
                },
            })
        })
        .collect();
    let body = json!({
        "metadata": { "title": "Saved views", "numberOfItems": navigation.len() },
        "links": [{ "rel": "self", "href": "/opds/v2/views", "type": NAV_CT }],
        "navigation": navigation,
    });
    json_response(body)
}

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
        return not_found();
    }
    if let Some(owner) = view.user_id
        && owner != user.id
    {
        return not_found();
    }
    let filter = match opds::dsl_from_view(&view) {
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
    if rows.len() as u64 > view_limit {
        rows.truncate(view_limit as usize);
    }
    let navigation: Vec<Value> = rows.iter().map(series_nav_entry).collect();
    let body = json!({
        "metadata": {
            "title": view.name,
            "identifier": format!("urn:view:{id}"),
            "numberOfItems": navigation.len(),
        },
        "links": [{
            "rel": "self",
            "href": format!("/opds/v2/views/{id}"),
            "type": NAV_CT,
        }],
        "navigation": navigation,
    });
    json_response(body)
}

// ────────────── builders / helpers ──────────────

/// Build an OPDS 2.0 publications array from a slice of issue rows.
/// Mirrors the per-entry shape the Atom feed emits via
/// `render_issue_acq_entry` — same link rels, same PSE template,
/// same Dublin Core metadata — but typed as JSON objects.
async fn build_publications(
    app: &AppState,
    user: &CurrentUser,
    issues: &[issue::Model],
) -> Vec<Value> {
    if issues.is_empty() {
        return Vec::new();
    }
    let slugs = opds::fetch_series_slugs(&app.db, issues).await;
    let key = app.secrets.url_signing_key.as_ref();
    issues
        .iter()
        .map(|i| publication_for(i, slugs.get(&i.series_id).map(String::as_str), user.id, key))
        .collect()
}

fn publication_for(
    i: &issue::Model,
    series_slug: Option<&str>,
    user_id: Uuid,
    url_signing_key: &[u8],
) -> Value {
    let label = i.title.clone().unwrap_or_else(|| {
        i.number_raw
            .clone()
            .map(|n| format!("Issue #{n}"))
            .unwrap_or_else(|| "Issue".into())
    });
    let mut metadata = serde_json::Map::new();
    metadata.insert("@type".into(), Value::from("http://schema.org/Periodical"));
    metadata.insert("title".into(), Value::from(label));
    metadata.insert(
        "identifier".into(),
        Value::from(format!("urn:folio:issue:{}", i.id)),
    );
    metadata.insert("modified".into(), Value::from(i.updated_at.to_rfc3339()));
    if let Some(s) = i.summary.as_deref().filter(|s| !s.is_empty()) {
        metadata.insert("description".into(), Value::from(s));
    }
    if let Some(lang) = i.language_code.as_deref().filter(|s| !s.is_empty()) {
        metadata.insert("language".into(), Value::from(lang));
    }
    if let Some(pub_) = i.publisher.as_deref().filter(|s| !s.is_empty()) {
        metadata.insert("publisher".into(), json!({ "name": pub_ }));
    }
    if let Some(d) = opds::iso_date_from_ymd(i.year, i.month, i.day) {
        metadata.insert("published".into(), Value::from(d));
    }
    if let Some(name) = opds::first_csv_field(i.writer.as_deref()) {
        metadata.insert("author".into(), json!([{ "name": name }]));
    }
    let mut subjects: Vec<Value> = Vec::new();
    for c in opds::csv_fields(i.genre.as_deref()).chain(opds::csv_fields(i.tags.as_deref())) {
        subjects.push(json!({ "name": c }));
    }
    if !subjects.is_empty() {
        metadata.insert("subject".into(), Value::from(subjects));
    }

    // Build links + images.
    let mut links = vec![json!({
        "rel": "http://opds-spec.org/acquisition",
        "href": format!("/opds/v1/issues/{}/file", i.id),
        "type": super::opds::mime_for(&i.file_path),
    })];
    if let Some(slug) = series_slug {
        links.push(json!({
            "rel": "related",
            "href": format!("/api/series/{slug}"),
            "type": "application/json",
        }));
    }
    // PSE stream link, only when the issue has a page count.
    let page_count = i.page_count.unwrap_or(0).max(0);
    if page_count > 0 {
        let query = crate::auth::url_signing::issue_query(&i.id, user_id, url_signing_key);
        links.push(json!({
            "rel": "http://vaemendis.net/opds-pse/stream",
            "href": format!("/opds/pse/{}/{{pageNumber}}?{query}", i.id),
            "type": "image/jpeg",
            "templated": true,
            "properties": { "numberOfItems": page_count },
        }));
    }

    let images = vec![
        json!({
            "rel": "http://opds-spec.org/image/thumbnail",
            "href": format!("/issues/{}/pages/0/thumb", i.id),
            "type": "image/webp",
        }),
        json!({
            "rel": "http://opds-spec.org/image",
            "href": format!("/issues/{}/pages/0", i.id),
            "type": "image/jpeg",
        }),
    ];

    json!({
        "metadata": Value::Object(metadata),
        "links": links,
        "images": images,
    })
}

fn series_nav_entry(s: &series::Model) -> Value {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "identifier".into(),
        Value::from(format!("urn:series:{}", s.id)),
    );
    metadata.insert("modified".into(), Value::from(s.updated_at.to_rfc3339()));
    if let Some(summary) = s.summary.as_deref().filter(|s| !s.is_empty()) {
        metadata.insert("description".into(), Value::from(summary));
    }
    json!({
        "title": s.name,
        "href": format!("/opds/v2/series/{}", s.id),
        "type": NAV_CT,
        "metadata": Value::Object(metadata),
    })
}

fn paginate_links(
    links: &mut Vec<Value>,
    base_href: &str,
    page: u64,
    total_pages: u64,
    per_page: u64,
) {
    if total_pages <= 1 {
        return;
    }
    let mut push = |rel: &str, p: u64| {
        links.push(json!({
            "rel": rel,
            "href": format!("{base_href}?page={p}"),
            "type": NAV_CT,
        }));
    };
    push("first", 1);
    if page > 1 {
        push("previous", page - 1);
    }
    if page < total_pages {
        push("next", page + 1);
    }
    push("last", total_pages);
    // Soak up the warning if `per_page` is unused — kept in the signature
    // so call sites self-document the page size they're using.
    let _ = per_page;
}

/// Mixed-collection acquisition feed. Series entries surface as
/// `navigation` objects (drill into the per-series feed); issue
/// entries surface as `publications`. Position order is preserved
/// across both arrays — clients render the navigation list first by
/// convention so visually it's "series, then issues".
async fn render_collection_acq_v2(
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
    let issue_models: Vec<issue::Model> = if issue_ids.is_empty() {
        Vec::new()
    } else {
        issue::Entity::find()
            .filter(issue::Column::Id.is_in(issue_ids))
            .all(&app.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|i| visible.contains(i.library_id))
            .collect()
    };
    let issue_by_id: HashMap<String, issue::Model> = issue_models
        .iter()
        .cloned()
        .map(|i| (i.id.clone(), i))
        .collect();

    // Walk rows in position order so series + issues retain the
    // collection's logical sequence.
    let mut navigation: Vec<Value> = Vec::new();
    let mut publication_models: Vec<issue::Model> = Vec::new();
    for row in &rows {
        if let Some(sid) = row.series_id
            && let Some(s) = series_by_id.get(&sid)
        {
            navigation.push(series_nav_entry(s));
        } else if let Some(iid) = row.issue_id.as_deref()
            && let Some(i) = issue_by_id.get(iid)
        {
            publication_models.push(i.clone());
        }
    }
    let publications = build_publications(app, user, &publication_models).await;

    let body = json!({
        "metadata": {
            "title": title,
            "identifier": format!("urn:collection:{}", view.id),
            "numberOfItems": navigation.len() + publications.len(),
        },
        "links": [{
            "rel": "self",
            "href": self_href,
            "type": NAV_CT,
        }],
        "navigation": navigation,
        "publications": publications,
    });
    json_response(body)
}

fn json_response(body: Value) -> Response {
    let mut hdrs = HeaderMap::new();
    hdrs.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/opds+json; charset=utf-8"),
    );
    (StatusCode::OK, hdrs, axum::Json(body)).into_response()
}

fn not_found() -> Response {
    error_status(StatusCode::NOT_FOUND, "not_found")
}

fn server_error<E: std::fmt::Display>(e: E) -> Response {
    tracing::warn!(error = %e, "opds_v2 error");
    error_status(StatusCode::INTERNAL_SERVER_ERROR, "internal")
}

fn error_status(status: StatusCode, code: &str) -> Response {
    (
        status,
        axum::Json(json!({"error": {"code": code, "message": code}})),
    )
        .into_response()
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
