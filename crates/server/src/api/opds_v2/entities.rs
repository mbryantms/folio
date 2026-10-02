//! WP-8.4 — OPDS 2.0 entity feeds, the JSON mirror of
//! [`crate::api::opds`]'s WP-5.5 entity feeds.
//!
//! `/opds/v2/{characters,teams,arcs,publishers}` are navigation feeds (one
//! entry per entity with a visible appearance, name order, numbered pages
//! of [`opds::PAGE_SIZE`]). Each entry drills into
//! `/opds/v2/<kind>/{slug}`: a publications feed of the entity's issues
//! for characters / teams / arcs (arcs in reading order, with per-
//! publication `previous` / `next` links like the v1 PSE nav), and a
//! series navigation feed for publishers. Membership, ACL and age-rating
//! caps come from the same [`crate::api::entity_pages`] core the web
//! landing pages and the v1 feeds use, so the two protocols can't drift.

use axum::{
    extract::{Path as AxPath, Query, State},
    response::Response,
};
use entity::{issue, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use std::collections::HashMap;
use uuid::Uuid;

use super::{
    NAV_CT, PageQuery, build_publications, build_publications_sequential, json_response,
    paginate_links, series_nav_entry, server_error, url_escape,
};
use crate::api::entity_pages::{self as ep, EntityKind, ListFilter, Paging};
use crate::api::opds::{self, PAGE_SIZE};
use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;

/// Root-catalog navigation entries for the four entity feeds.
pub(super) fn root_navigation() -> Vec<Value> {
    [
        EntityKind::Character,
        EntityKind::Team,
        EntityKind::Arc,
        EntityKind::Publisher,
    ]
    .into_iter()
    .map(|kind| {
        json!({
            "title": kind.plural_title(),
            "href": format!("/opds/v2/{}", kind.path()),
            "type": NAV_CT,
        })
    })
    .collect()
}

fn self_href(base: &str, page: u64) -> String {
    if page > 1 {
        format!("{base}?page={page}")
    } else {
        base.to_owned()
    }
}

async fn entity_index(app: &AppState, user: &CurrentUser, kind: EntityKind, page: u64) -> Response {
    let page = page.max(1);
    let visible = access::for_user(app, user).await;
    let filter = ListFilter {
        starts_with: None,
        q: None,
    };
    let total = match ep::list_count(app, kind, &visible, &filter).await {
        Ok(n) => n,
        Err(e) => return server_error(e.to_string()),
    };
    let paging = Paging {
        after: None,
        offset: (page - 1) * PAGE_SIZE,
        limit: PAGE_SIZE,
    };
    let rows = match ep::list_page(app, kind, &visible, &filter, &paging).await {
        Ok((rows, _)) => rows,
        Err(e) => return server_error(e.to_string()),
    };
    let path = kind.path();
    let base_href = format!("/opds/v2/{path}");
    let navigation: Vec<Value> = rows
        .iter()
        .map(|(item, _)| {
            let noun = if item.issue_count == 1 {
                "issue"
            } else {
                "issues"
            };
            json!({
                "title": item.name,
                "href": format!("/opds/v2/{path}/{}", url_escape(&item.slug)),
                "type": NAV_CT,
                "metadata": {
                    "identifier": format!("urn:folio:{path}:{}", item.id),
                    "description": format!(
                        "{} {noun} · {} series",
                        item.issue_count, item.series_count
                    ),
                    "numberOfItems": item.issue_count,
                },
            })
        })
        .collect();
    let total_pages = total.div_ceil(PAGE_SIZE).max(1);
    let mut links = vec![
        json!({ "rel": "self", "href": self_href(&base_href, page), "type": NAV_CT }),
        json!({ "rel": "up", "href": "/opds/v2", "type": NAV_CT }),
    ];
    paginate_links(&mut links, &base_href, page, total_pages, PAGE_SIZE);
    json_response(json!({
        "metadata": {
            "title": kind.plural_title(),
            "identifier": format!("urn:opds:{path}"),
            "itemsPerPage": PAGE_SIZE,
            "numberOfItems": total,
            "currentPage": page,
        },
        "links": links,
        "navigation": navigation,
    }))
}

/// Feed-level metadata shared by the entity detail feeds.
fn entity_metadata(
    kind: EntityKind,
    row: &ep::EntityRow,
    total: i64,
    page: u64,
) -> serde_json::Map<String, Value> {
    let mut metadata = serde_json::Map::new();
    metadata.insert("title".into(), Value::from(row.name.clone()));
    metadata.insert(
        "identifier".into(),
        Value::from(format!("urn:folio:{}:{}", kind.path(), row.id)),
    );
    if let Some(d) = row.description.as_deref().filter(|d| !d.is_empty()) {
        metadata.insert("description".into(), Value::from(d));
    }
    metadata.insert("itemsPerPage".into(), Value::from(PAGE_SIZE));
    metadata.insert("numberOfItems".into(), Value::from(total.max(0)));
    metadata.insert("currentPage".into(), Value::from(page));
    metadata
}

/// Publications feed for a character / team / story arc.
async fn entity_issue_feed(
    app: &AppState,
    user: &CurrentUser,
    kind: EntityKind,
    slug: &str,
    page: u64,
) -> Response {
    let page = page.max(1);
    let (row, visible, _, issue_count) =
        match ep::resolve_visible_for_user(app, user, kind, slug).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let paging = Paging {
        after: None,
        offset: (page - 1) * PAGE_SIZE,
        limit: PAGE_SIZE,
    };
    let ids: Vec<String> = match ep::issue_page(app, kind, &row, &visible, &paging).await {
        Ok((keys, _)) => keys.into_iter().map(|(id, _)| id).collect(),
        Err(e) => return server_error(e.to_string()),
    };
    let mut by_id: HashMap<String, issue::Model> = match issue::Entity::find()
        .filter(issue::Column::Id.is_in(ids.clone()))
        .all(&app.db)
        .await
    {
        Ok(rows) => rows.into_iter().map(|r| (r.id.clone(), r)).collect(),
        Err(e) => return server_error(e.to_string()),
    };
    let issues: Vec<issue::Model> = ids.iter().filter_map(|id| by_id.remove(id)).collect();
    // A story arc is a reading order; cast feeds are discovery.
    let publications = if kind == EntityKind::Arc {
        build_publications_sequential(app, user, &issues, None, None).await
    } else {
        build_publications(app, user, &issues).await
    };
    let path = kind.path();
    let base_href = format!("/opds/v2/{path}/{}", url_escape(&row.slug));
    let total_pages = (issue_count.max(0) as u64).div_ceil(PAGE_SIZE).max(1);
    let mut links = vec![
        json!({ "rel": "self", "href": self_href(&base_href, page), "type": NAV_CT }),
        json!({ "rel": "up", "href": format!("/opds/v2/{path}"), "type": NAV_CT }),
    ];
    paginate_links(&mut links, &base_href, page, total_pages, PAGE_SIZE);
    let mut body = serde_json::Map::new();
    body.insert(
        "metadata".into(),
        Value::Object(entity_metadata(kind, &row, issue_count, page)),
    );
    body.insert("links".into(), Value::Array(links));
    if let Some(img) = row.image_url.as_deref().filter(|u| !u.is_empty()) {
        body.insert("images".into(), json!([{ "href": img }]));
    }
    body.insert("publications".into(), Value::Array(publications));
    json_response(Value::Object(body))
}

/// Series navigation feed for a publisher.
async fn publisher_series_feed(
    app: &AppState,
    user: &CurrentUser,
    slug: &str,
    page: u64,
) -> Response {
    let kind = EntityKind::Publisher;
    let page = page.max(1);
    let (row, visible, series_count, _) =
        match ep::resolve_visible_for_user(app, user, kind, slug).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let paging = Paging {
        after: None,
        offset: (page - 1) * PAGE_SIZE,
        limit: PAGE_SIZE,
    };
    let ids: Vec<Uuid> = match ep::series_page(app, kind, &row, &visible, &paging).await {
        Ok((keys, _)) => keys.into_iter().map(|(id, _)| id).collect(),
        Err(e) => return server_error(e.to_string()),
    };
    let mut by_id: HashMap<Uuid, series::Model> = match series::Entity::find()
        .filter(series::Column::Id.is_in(ids.clone()))
        .all(&app.db)
        .await
    {
        Ok(rows) => rows.into_iter().map(|r| (r.id, r)).collect(),
        Err(e) => return server_error(e.to_string()),
    };
    let rows: Vec<series::Model> = ids.iter().filter_map(|id| by_id.remove(id)).collect();
    let covers = opds::fetch_cover_issues(&app.db, &ids).await;
    let facets = opds::fetch_series_facets(&app.db, &ids).await;
    let navigation: Vec<Value> = rows
        .iter()
        .map(|s| series_nav_entry(s, covers.get(&s.id).map(String::as_str), facets.get(&s.id)))
        .collect();
    let base_href = format!("/opds/v2/publishers/{}", url_escape(&row.slug));
    let total_pages = (series_count.max(0) as u64).div_ceil(PAGE_SIZE).max(1);
    let mut links = vec![
        json!({ "rel": "self", "href": self_href(&base_href, page), "type": NAV_CT }),
        json!({ "rel": "up", "href": "/opds/v2/publishers", "type": NAV_CT }),
    ];
    paginate_links(&mut links, &base_href, page, total_pages, PAGE_SIZE);
    json_response(json!({
        "metadata": Value::Object(entity_metadata(kind, &row, series_count, page)),
        "links": links,
        "navigation": navigation,
    }))
}

pub(super) async fn characters_nav(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<PageQuery>,
) -> Response {
    entity_index(&app, &user, EntityKind::Character, q.page.unwrap_or(1)).await
}

pub(super) async fn teams_nav(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<PageQuery>,
) -> Response {
    entity_index(&app, &user, EntityKind::Team, q.page.unwrap_or(1)).await
}

pub(super) async fn arcs_nav(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<PageQuery>,
) -> Response {
    entity_index(&app, &user, EntityKind::Arc, q.page.unwrap_or(1)).await
}

pub(super) async fn publishers_nav(
    State(app): State<AppState>,
    user: CurrentUser,
    Query(q): Query<PageQuery>,
) -> Response {
    entity_index(&app, &user, EntityKind::Publisher, q.page.unwrap_or(1)).await
}

pub(super) async fn character_feed(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Query(q): Query<PageQuery>,
) -> Response {
    entity_issue_feed(
        &app,
        &user,
        EntityKind::Character,
        &slug,
        q.page.unwrap_or(1),
    )
    .await
}

pub(super) async fn team_feed(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Query(q): Query<PageQuery>,
) -> Response {
    entity_issue_feed(&app, &user, EntityKind::Team, &slug, q.page.unwrap_or(1)).await
}

pub(super) async fn arc_feed(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Query(q): Query<PageQuery>,
) -> Response {
    entity_issue_feed(&app, &user, EntityKind::Arc, &slug, q.page.unwrap_or(1)).await
}

pub(super) async fn publisher_feed(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Query(q): Query<PageQuery>,
) -> Response {
    publisher_series_feed(&app, &user, &slug, q.page.unwrap_or(1)).await
}
