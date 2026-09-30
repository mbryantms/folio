//! WP-5.5 — entity navigation feeds.
//!
//! `/opds/v1/{characters,teams,arcs,publishers}` are navigation feeds
//! (one subsection entry per entity with a visible appearance, name
//! order, numbered pages). Each entry drills into
//! `/opds/v1/<kind>/{slug}`: an acquisition feed of the entity's issues
//! for characters / teams / arcs (arcs in reading order, with PSE
//! prev/next links), and a series listing for publishers. Membership,
//! ACL and age-rating caps come from the same
//! [`crate::api::entity_pages`] core the web landing pages use.

use axum::{
    extract::{Path as AxPath, Query, State},
    response::Response,
};
use entity::{issue, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use std::collections::HashMap;
use uuid::Uuid;

use super::{
    ACQ_CT, AcquisitionFeedArgs, NAV_CT, PAGE_SIZE, PageQuery, atom, build_acquisition_feed,
    fetch_cover_issues, fetch_series_facets, paginate_links, render_nav_entry,
    render_series_subsection_entry, server_error, url_escape, wrap_nav_feed, xml_escape,
};
use crate::api::entity_pages::{self as ep, EntityKind, ListFilter, Paging};
use crate::auth::CurrentUser;
use crate::library::access;
use crate::state::AppState;

/// Root-feed entries linking the four entity navigation feeds.
pub(super) fn root_entries(base: &str, now: &str) -> String {
    let mut out = String::new();
    for kind in [
        EntityKind::Character,
        EntityKind::Team,
        EntityKind::Arc,
        EntityKind::Publisher,
    ] {
        let path = kind.path();
        out.push_str(&format!(
            "  <entry>\n    <id>{base}/opds/v1/{path}</id>\n    <title>{title}</title>\n    <updated>{now}</updated>\n    <link rel=\"subsection\" href=\"/opds/v1/{path}\" type=\"{NAV_CT}\"/>\n  </entry>\n",
            title = kind.plural_title(),
        ));
    }
    out
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
    let now = chrono::Utc::now().to_rfc3339();
    let path = kind.path();
    let base_href = format!("/opds/v1/{path}");
    let mut entries = paginate_links(&base_href, page, total.div_ceil(PAGE_SIZE).max(1));
    for (item, _) in &rows {
        let summary = format!(
            "{} {} · {} series",
            item.issue_count,
            if item.issue_count == 1 {
                "issue"
            } else {
                "issues"
            },
            item.series_count,
        );
        entries.push_str(&render_nav_entry(
            &format!("urn:folio:{path}:{}", item.id),
            &item.name,
            Some(&summary),
            &now,
            &format!("/opds/v1/{path}/{}", url_escape(&item.slug)),
        ));
    }
    let self_href = if page > 1 {
        format!("{base_href}?page={page}")
    } else {
        base_href
    };
    atom(wrap_nav_feed(
        &format!("urn:opds:{path}"),
        kind.plural_title(),
        &self_href,
        &entries,
    ))
}

/// Issues acquisition feed for a character / team / story arc.
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
    // OPDS entries render dc:* metadata from the full row (same as every
    // other acquisition feed); one page is capped at PAGE_SIZE rows.
    let mut by_id: HashMap<String, issue::Model> = match issue::Entity::find()
        .filter(issue::Column::Id.is_in(ids.clone()))
        .all(&app.db)
        .await
    {
        Ok(rows) => rows.into_iter().map(|r| (r.id.clone(), r)).collect(),
        Err(e) => return server_error(e.to_string()),
    };
    let issues: Vec<issue::Model> = ids.iter().filter_map(|id| by_id.remove(id)).collect();
    let path = kind.path();
    let base_href = format!("/opds/v1/{path}/{}", url_escape(&row.slug));
    let self_href = if page > 1 {
        format!("{base_href}?page={page}")
    } else {
        base_href.clone()
    };
    let total_pages = (issue_count.max(0) as u64).div_ceil(PAGE_SIZE).max(1);
    let pagination = paginate_links(&base_href, page, total_pages);
    let feed_id = format!("urn:folio:{path}:{}", row.id);
    let body = build_acquisition_feed(
        app,
        AcquisitionFeedArgs {
            feed_id: &feed_id,
            title: &row.name,
            self_href: &self_href,
            issues: &issues,
            pagination: &pagination,
            user_id: user.id,
            // A story arc is a reading order; cast feeds are discovery.
            sequential_nav: kind == EntityKind::Arc,
            up_next_issue_id: None,
            feed_last_read_date: None,
            entry_positions: None,
        },
    )
    .await;
    atom(body)
}

/// Series listing for a publisher — same entry shape as `by_creator`.
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
    let covers = fetch_cover_issues(&app.db, &ids).await;
    let facets = fetch_series_facets(&app.db, &ids).await;
    let mut entries = String::new();
    for s in &rows {
        let cover = covers.get(&s.id).map(String::as_str);
        entries.push_str(&render_series_subsection_entry(s, cover, facets.get(&s.id)));
    }
    let base_href = format!("/opds/v1/publishers/{}", url_escape(&row.slug));
    let self_href = if page > 1 {
        format!("{base_href}?page={page}")
    } else {
        base_href.clone()
    };
    let total_pages = (series_count.max(0) as u64).div_ceil(PAGE_SIZE).max(1);
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:dc="http://purl.org/dc/terms/">
  <id>urn:folio:publishers:{id}</id>
  <title>{title}</title>
  <updated>{now}</updated>
  <link rel="self" href="{self_href}" type="{acq}"/>
  <link rel="up" href="/opds/v1/publishers" type="{nav}"/>
{pagination}{entries}</feed>
"#,
        id = row.id,
        title = xml_escape(&row.name),
        now = chrono::Utc::now().to_rfc3339(),
        self_href = xml_escape(&self_href),
        acq = ACQ_CT,
        nav = NAV_CT,
        pagination = paginate_links(&base_href, page, total_pages),
    );
    atom(body)
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

pub(super) async fn character_acq(
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

pub(super) async fn team_acq(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Query(q): Query<PageQuery>,
) -> Response {
    entity_issue_feed(&app, &user, EntityKind::Team, &slug, q.page.unwrap_or(1)).await
}

pub(super) async fn arc_acq(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Query(q): Query<PageQuery>,
) -> Response {
    entity_issue_feed(&app, &user, EntityKind::Arc, &slug, q.page.unwrap_or(1)).await
}

pub(super) async fn publisher_acq(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(slug): AxPath<String>,
    Query(q): Query<PageQuery>,
) -> Response {
    publisher_series_feed(&app, &user, &slug, q.page.unwrap_or(1)).await
}
