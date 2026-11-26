//! Linked reading lists, PR 3b: find a catalog master's events and parts
//! automatically ([`crate::cbl::derive`] holds the pure rules).
//!
//! - `POST /me/cbl-lists/{id}/links/derive` — a **preview**: for a CBRO
//!   master, download its with-events twin and the candidate event lists
//!   (transiently; nothing is stored), diff, and propose each event's
//!   place; for any catalog list, propose the previous / next part when
//!   it is in the caller's lists. Says which event lists would be
//!   imported. POST because it fetches from the catalog.
//! - `POST /me/cbl-lists/{id}/links/derive/apply` — commit the proposals
//!   the person kept: import missing event lists (with the master's
//!   ownership: a shared master's events are imported shared, D5), link
//!   them (`placement = auto_twin`) and chain parts (`auto_chain`).
//!
//! Both need the right to manage the master (`ensure_can_manage`). An
//! apply that imports anything spends **one** hourly import token however
//! many lists it imports, and each personal import is checked against the
//! per-user remote-list cap; a list that can't be imported is reported in
//! `failed` and the rest still apply.

use axum::{
    Extension, Json,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::reading_list_link::{self, AnchorStatus, LinkKind, LinkPlacement};
use entity::{catalog_source, cbl_entry, cbl_list};
use futures::StreamExt;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, FromQueryResult, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use serde::{Deserialize, Serialize};
use server_macros::handler;
use std::collections::{HashMap, HashSet};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

use super::cbl_links::{
    MAX_LINKS_PER_LIST, ReadingListLinksView, anchor_of_entry, continues_reaches, has_events,
    internal, is_an_event, next_sort_at,
};
use super::cbl_lists::{
    SourceMeta, audit_admin_list_write, ensure_can_manage, fetch_list, insert_list_from_parsed,
};
use super::error;
use super::extractors::Validated;
use crate::auth::CurrentUser;
use crate::cbl::catalog::{self, CatalogEntry, CatalogError};
use crate::cbl::derive::{self, BookKey, Candidate};
use crate::middleware::RequestContext;
use crate::state::AppState;

/// Event lists one preview downloads at most (an era folder holds ≤ 30).
const MAX_CANDIDATES: usize = 60;
/// Parallel catalog downloads.
const DOWNLOAD_CONCURRENCY: usize = 6;
/// Imports an apply starts before it stops and reports the rest (each
/// import matches its entries inline; the JSON route times out at 60 s).
/// Running the apply again finishes: imported lists are reused.
const APPLY_IMPORT_BUDGET: std::time::Duration = std::time::Duration::from_secs(35);

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(preview))
        .routes(routes!(apply))
}

// ───── wire types ─────

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProposedEventLink {
    pub catalog_path: String,
    pub name: String,
    /// Issues in the event list.
    pub total: i32,
    /// Of those, inserted at this spot by the with-events version.
    pub issues_here: i32,
    pub after_entry_id: String,
    pub after_position: i32,
    pub after_label: String,
    /// Order among events at the same spot.
    pub order_at_anchor: i32,
    /// The caller's (or a shared) copy of the event list, if they have one.
    pub existing_list_id: Option<String>,
    /// Already linked into this list.
    pub already_linked: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UnplacedRunView {
    /// The entry the run follows; `None` = before the first entry.
    pub after_label: Option<String>,
    pub issues: i32,
    pub first_label: String,
    pub last_label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PartDirection {
    /// This list continues in the other one.
    Next,
    /// The other list continues in this one.
    Previous,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProposedPartLink {
    pub direction: PartDirection,
    pub list_id: String,
    pub name: String,
    pub already_linked: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DerivePreview {
    /// The list is a CBRO master whose with-events version is in the
    /// catalog (events can be found).
    pub twin_found: bool,
    pub events: Vec<ProposedEventLink>,
    /// Inserted issues no event list covers — shown, never guessed.
    pub unplaced: Vec<UnplacedRunView>,
    pub parts: Vec<ProposedPartLink>,
    /// Proposed event lists the caller doesn't have yet (imported on apply).
    pub imports_needed: i32,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ApplyEvent {
    pub catalog_path: String,
    pub after_entry_id: Uuid,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ApplyPart {
    pub direction: PartDirection,
    pub list_id: Uuid,
}

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct ApplyDeriveReq {
    /// In reading order; several at one entry keep this order.
    #[serde(default)]
    #[garde(length(max = 100))]
    pub events: Vec<ApplyEvent>,
    #[serde(default)]
    #[garde(length(max = 2))]
    pub parts: Vec<ApplyPart>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ApplyFailure {
    pub name: String,
    pub message: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ApplyDeriveResp {
    /// Event lists imported.
    pub imported: i32,
    /// Links created (events + parts).
    pub linked: i32,
    pub failed: Vec<ApplyFailure>,
    pub links: ReadingListLinksView,
}

// ───── helpers ─────

#[derive(Debug, FromQueryResult)]
struct EntryRow {
    id: Uuid,
    position: i32,
    series_name: String,
    issue_number: String,
    cv_issue_id: Option<i32>,
}

fn label(series: &str, number: &str) -> String {
    format!("{series} #{number}")
}

fn book_keys(parsed: &parsers::cbl::ParsedCbl) -> Vec<BookKey> {
    parsed
        .books
        .iter()
        .map(|b| BookKey::new(b.comicvine_issue_id(), &b.series, &b.number))
        .collect()
}

fn catalog_failure(e: &CatalogError) -> Response {
    match e {
        CatalogError::RateLimited => error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "GitHub rate-limited the catalog",
        ),
        e if e.not_found_message().is_some() => error(
            StatusCode::NOT_FOUND,
            "not_found",
            &e.not_found_message().unwrap_or_default(),
        ),
        e => error(StatusCode::BAD_GATEWAY, "fetch_failed", &e.public_message()),
    }
}

/// Download and parse `entries` in parallel; failures are dropped (logged).
async fn download_parsed(
    source: &catalog_source::Model,
    entries: Vec<CatalogEntry>,
) -> HashMap<String, parsers::cbl::ParsedCbl> {
    futures::stream::iter(entries)
        .map(|entry| async move {
            let blob = catalog::download_blob(source, &entry).await;
            let parsed = blob
                .map_err(|e| e.to_string())
                .and_then(|b| parsers::cbl::parse(&b.bytes).map_err(|e| e.to_string()));
            match parsed {
                Ok(p) => Some((entry.path, p)),
                Err(e) => {
                    tracing::warn!(path = %entry.path, error = %e, "cbl derive: candidate skipped");
                    None
                }
            }
        })
        .buffer_unordered(DOWNLOAD_CONCURRENCY)
        .filter_map(|x| async move { x })
        .collect()
        .await
}

/// Lists of `source` at `paths` the parent may link: for a shared parent
/// only shared lists, else the caller's own (preferred) or shared ones.
async fn usable_lists_at(
    app: &AppState,
    parent: &cbl_list::Model,
    user: &CurrentUser,
    source_id: Uuid,
    paths: &[String],
) -> Result<HashMap<String, cbl_list::Model>, sea_orm::DbErr> {
    if paths.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = cbl_list::Entity::find()
        .filter(cbl_list::Column::CatalogSourceId.eq(source_id))
        .filter(cbl_list::Column::CatalogPath.is_in(paths.to_vec()))
        .all(&app.db)
        .await?;
    let mut out: HashMap<String, cbl_list::Model> = HashMap::new();
    for r in rows {
        let ok = match parent.owner_user_id {
            None => r.owner_user_id.is_none(),
            Some(_) => r.owner_user_id.is_none() || r.owner_user_id == Some(user.id),
        };
        let Some(path) = r.catalog_path.clone() else {
            continue;
        };
        if !ok {
            continue;
        }
        let own = r.owner_user_id == Some(user.id);
        if own || !out.contains_key(&path) {
            out.insert(path, r);
        }
    }
    Ok(out)
}

/// The previous / next part of `parent` among lists with its owner, by the
/// catalog folder's part ordinals.
async fn part_neighbours(
    app: &AppState,
    parent: &cbl_list::Model,
    source_id: Uuid,
    path: &str,
    index_paths: &[&str],
) -> Result<Vec<(PartDirection, cbl_list::Model)>, sea_orm::DbErr> {
    let Some((prefix, n)) = derive::part_ordinal(derive::file_stem(path)) else {
        return Ok(Vec::new());
    };
    let folder = path.rsplit_once('/').map_or("", |(f, _)| f);
    let siblings: Vec<(f64, &str)> = index_paths
        .iter()
        .filter(|p| p.rsplit_once('/').is_some_and(|(f, _)| f == folder))
        .filter_map(|p| {
            let (pre, m) = derive::part_ordinal(derive::file_stem(p))?;
            (pre == prefix && m != n).then_some((m, *p))
        })
        .collect();
    let next = siblings
        .iter()
        .filter(|(m, _)| *m > n)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let prev = siblings
        .iter()
        .filter(|(m, _)| *m < n)
        .max_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = Vec::new();
    for (dir, hit) in [(PartDirection::Next, next), (PartDirection::Previous, prev)] {
        let Some((_, p)) = hit else {
            continue;
        };
        let mut q = cbl_list::Entity::find()
            .filter(cbl_list::Column::CatalogSourceId.eq(source_id))
            .filter(cbl_list::Column::CatalogPath.eq(*p));
        q = match parent.owner_user_id {
            Some(owner) => q.filter(cbl_list::Column::OwnerUserId.eq(owner)),
            None => q.filter(cbl_list::Column::OwnerUserId.is_null()),
        };
        if let Some(l) = q.one(&app.db).await? {
            out.push((dir, l));
        }
    }
    Ok(out)
}

async fn has_continues(app: &AppState, list_id: Uuid) -> Result<bool, sea_orm::DbErr> {
    Ok(reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ParentListId.eq(list_id))
        .filter(reading_list_link::Column::Kind.eq(LinkKind::Continues))
        .count(&app.db)
        .await?
        > 0)
}

async fn link_exists(
    app: &AppState,
    parent: Uuid,
    child: Uuid,
    kind: LinkKind,
) -> Result<bool, sea_orm::DbErr> {
    Ok(reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ParentListId.eq(parent))
        .filter(reading_list_link::Column::ChildListId.eq(child))
        .filter(reading_list_link::Column::Kind.eq(kind))
        .count(&app.db)
        .await?
        > 0)
}

/// The list's catalog source and path, when it came from a catalog.
async fn catalog_origin(
    app: &AppState,
    list: &cbl_list::Model,
) -> Result<Option<(catalog_source::Model, String)>, sea_orm::DbErr> {
    let (Some(source_id), Some(path)) = (list.catalog_source_id, list.catalog_path.clone()) else {
        return Ok(None);
    };
    Ok(catalog_source::Entity::find_by_id(source_id)
        .one(&app.db)
        .await?
        .map(|s| (s, path)))
}

// ───── handlers ─────

#[utoipa::path(
    operation_id = "cbl_links_derive_preview",
    post,
    path = "/me/cbl-lists/{id}/links/derive",
    params(("id" = String, Path,)),
    responses(
        (status = 200, body = DerivePreview),
        (status = 404, description = "list not found"),
        (status = 429, description = "GitHub rate-limited the catalog"),
        (status = 502, description = "the catalog could not be read"),
    )
)]
#[handler]
pub async fn preview(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> Response {
    let list = match fetch_list(&app.db, id).await {
        Ok(l) => l,
        Err(r) => return r,
    };
    if let Err(r) = ensure_can_manage(&list, &user).await {
        return r;
    }
    let mut out = DerivePreview {
        twin_found: false,
        events: Vec::new(),
        unplaced: Vec::new(),
        parts: Vec::new(),
        imports_needed: 0,
    };
    let (source, path) = match catalog_origin(&app, &list).await {
        Ok(Some(v)) => v,
        Ok(None) => return Json(out).into_response(),
        Err(_) => return internal(),
    };
    let index = match tokio::time::timeout(
        crate::cbl::fetch::REQUEST_BUDGET.list_budget,
        catalog::refresh_index(&app.db, &source, false),
    )
    .await
    {
        Ok(Ok(i)) => i,
        Ok(Err(e)) => return catalog_failure(&e),
        Err(_) => {
            return error(
                StatusCode::BAD_GATEWAY,
                "fetch_failed",
                "the catalog took too long to answer",
            );
        }
    };
    let index_paths: Vec<&str> = index.entries.iter().map(|e| e.path.as_str()).collect();

    // Parts.
    match part_neighbours(&app, &list, source.id, &path, &index_paths).await {
        Ok(found) => {
            for (dir, other) in found {
                let (from, to) = match dir {
                    PartDirection::Next => (list.id, other.id),
                    PartDirection::Previous => (other.id, list.id),
                };
                let already = link_exists(&app, from, to, LinkKind::Continues)
                    .await
                    .unwrap_or(false);
                out.parts.push(ProposedPartLink {
                    direction: dir,
                    list_id: other.id.to_string(),
                    name: other.parsed_name.clone(),
                    already_linked: already,
                });
            }
        }
        Err(_) => return internal(),
    }

    // Events, for a CBRO master with a twin.
    let Some(twin_path) = derive::twin_path(&path) else {
        return Json(out).into_response();
    };
    let Some(twin_entry) = index.entries.iter().find(|e| e.path == twin_path).cloned() else {
        return Json(out).into_response();
    };
    out.twin_found = true;
    let candidate_paths: Vec<String> = derive::event_candidate_paths(&path, &index_paths)
        .into_iter()
        .take(MAX_CANDIDATES)
        .map(str::to_owned)
        .collect();
    let mut wanted: Vec<CatalogEntry> = vec![twin_entry];
    wanted.extend(
        index
            .entries
            .iter()
            .filter(|e| candidate_paths.contains(&e.path))
            .cloned(),
    );
    let downloaded = match tokio::time::timeout(
        crate::cbl::fetch::REQUEST_BUDGET.list_budget,
        download_parsed(&source, wanted),
    )
    .await
    {
        Ok(d) => d,
        Err(_) => {
            return error(
                StatusCode::BAD_GATEWAY,
                "fetch_failed",
                "the catalog took too long to answer",
            );
        }
    };
    let Some(twin) = downloaded.get(&twin_path) else {
        return error(
            StatusCode::BAD_GATEWAY,
            "fetch_failed",
            "couldn't read the with-events version of this list",
        );
    };

    let entries = match cbl_entry::Entity::find()
        .select_only()
        .columns([
            cbl_entry::Column::Id,
            cbl_entry::Column::Position,
            cbl_entry::Column::SeriesName,
            cbl_entry::Column::IssueNumber,
            cbl_entry::Column::CvIssueId,
        ])
        .filter(cbl_entry::Column::CblListId.eq(list.id))
        .order_by_asc(cbl_entry::Column::Position)
        .into_model::<EntryRow>()
        .all(&app.db)
        .await
    {
        Ok(e) => e,
        Err(_) => return internal(),
    };
    let plain: Vec<BookKey> = entries
        .iter()
        .map(|e| BookKey::new(e.cv_issue_id, &e.series_name, &e.issue_number))
        .collect();
    let cand_order: Vec<&String> = candidate_paths
        .iter()
        .filter(|p| downloaded.contains_key(*p))
        .collect();
    let candidates: Vec<Candidate> = cand_order
        .iter()
        .map(|p| Candidate {
            keys: book_keys(&downloaded[*p]),
        })
        .collect();
    let d = derive::twin_diff(&plain, &book_keys(twin), &candidates);

    let placed_paths: Vec<String> = d
        .placements
        .iter()
        .map(|p| cand_order[p.candidate].clone())
        .collect();
    let existing = match usable_lists_at(&app, &list, &user, source.id, &placed_paths).await {
        Ok(m) => m,
        Err(_) => return internal(),
    };
    let linked: HashSet<Uuid> = match reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ParentListId.eq(list.id))
        .filter(reading_list_link::Column::Kind.eq(LinkKind::Event))
        .all(&app.db)
        .await
    {
        Ok(rows) => rows.into_iter().map(|l| l.child_list_id).collect(),
        Err(_) => return internal(),
    };
    for p in &d.placements {
        let cpath = cand_order[p.candidate];
        let parsed = &downloaded[cpath];
        let anchor = &entries[p.after];
        let have = existing.get(cpath);
        if have.is_none() {
            out.imports_needed += 1;
        }
        out.events.push(ProposedEventLink {
            catalog_path: cpath.clone(),
            name: if parsed.name.trim().is_empty() {
                derive::file_stem(cpath).to_owned()
            } else {
                parsed.name.clone()
            },
            total: i32::try_from(parsed.books.len()).unwrap_or(i32::MAX),
            issues_here: i32::try_from(p.issues).unwrap_or(i32::MAX),
            after_entry_id: anchor.id.to_string(),
            after_position: anchor.position,
            after_label: label(&anchor.series_name, &anchor.issue_number),
            order_at_anchor: i32::try_from(p.order).unwrap_or(0),
            existing_list_id: have.map(|l| l.id.to_string()),
            already_linked: have.is_some_and(|l| linked.contains(&l.id)),
        });
    }
    let twin_label = |i: usize| {
        twin.books
            .get(i)
            .map(|b| label(&b.series, &b.number))
            .unwrap_or_default()
    };
    for u in &d.unplaced {
        out.unplaced.push(UnplacedRunView {
            after_label: u
                .after
                .map(|a| label(&entries[a].series_name, &entries[a].issue_number)),
            issues: i32::try_from(u.issues).unwrap_or(i32::MAX),
            first_label: twin_label(u.first),
            last_label: twin_label(u.last),
        });
    }
    Json(out).into_response()
}

#[utoipa::path(
    operation_id = "cbl_links_derive_apply",
    post,
    path = "/me/cbl-lists/{id}/links/derive/apply",
    params(("id" = String, Path,)),
    request_body = ApplyDeriveReq,
    responses(
        (status = 200, body = ApplyDeriveResp),
        (status = 404, description = "list not found"),
        (status = 422, description = "validation"),
        (status = 429, description = "import quota exceeded"),
    )
)]
#[handler]
pub async fn apply(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<Uuid>,
    Validated(req): Validated<ApplyDeriveReq>,
) -> Response {
    let parent = match fetch_list(&app.db, id).await {
        Ok(l) => l,
        Err(r) => return r,
    };
    if let Err(r) = ensure_can_manage(&parent, &user).await {
        return r;
    }
    let mut failed: Vec<ApplyFailure> = Vec::new();
    let (mut imported, mut linked) = (0i32, 0i32);
    let fail = |failed: &mut Vec<ApplyFailure>, name: &str, message: &str| {
        failed.push(ApplyFailure {
            name: name.to_owned(),
            message: message.to_owned(),
        });
    };

    if !req.events.is_empty() {
        let (source, _) = match catalog_origin(&app, &parent).await {
            Ok(Some(v)) => v,
            Ok(None) => {
                return super::cbl_links::field_error(
                    "events",
                    "only a list from a catalog can find events",
                );
            }
            Err(_) => return internal(),
        };
        match is_an_event(&app, parent.id).await {
            Ok(true) => {
                return super::cbl_links::field_error(
                    "events",
                    "this list is an event in another list, so it can't take events",
                );
            }
            Ok(false) => {}
            Err(_) => return internal(),
        }
        let paths: Vec<String> = req.events.iter().map(|e| e.catalog_path.clone()).collect();
        let mut have = match usable_lists_at(&app, &parent, &user, source.id, &paths).await {
            Ok(m) => m,
            Err(_) => return internal(),
        };
        // One import token for the whole apply.
        if paths.iter().any(|p| !have.contains_key(p))
            && let Err(resp) = crate::cbl::quota::consume_import(&app, &user).await
        {
            return resp;
        }
        // Download every list to import up front, in parallel.
        let missing: Vec<CatalogEntry> = if paths.iter().any(|p| !have.contains_key(p)) {
            let index = match catalog::refresh_index(&app.db, &source, false).await {
                Ok(i) => i,
                Err(e) => return catalog_failure(&e),
            };
            index
                .entries
                .into_iter()
                .filter(|e| paths.contains(&e.path) && !have.contains_key(&e.path))
                .collect()
        } else {
            Vec::new()
        };
        let source_ref = &source;
        let mut blobs: HashMap<String, Result<catalog::FetchedBlob, String>> =
            futures::stream::iter(missing)
                .map(|entry| async move {
                    let blob = catalog::download_blob(source_ref, &entry)
                        .await
                        .map_err(|e| e.public_message());
                    (entry.path, blob)
                })
                .buffer_unordered(DOWNLOAD_CONCURRENCY)
                .collect()
                .await;
        let deadline = tokio::time::Instant::now() + APPLY_IMPORT_BUDGET;

        for ev in &req.events {
            let name = derive::file_stem(&ev.catalog_path).to_owned();
            // Import when missing.
            let child = if let Some(l) = have.get(&ev.catalog_path) {
                l.clone()
            } else {
                let Some(blob) = blobs.remove(&ev.catalog_path) else {
                    fail(&mut failed, &name, "not in the catalog");
                    continue;
                };
                if tokio::time::Instant::now() >= deadline {
                    fail(
                        &mut failed,
                        &name,
                        "not imported in time; run Find events again to finish",
                    );
                    continue;
                }
                if parent.owner_user_id.is_some()
                    && crate::cbl::quota::check_remote_list_cap(&app, &user)
                        .await
                        .is_err()
                {
                    fail(&mut failed, &name, "you've reached your reading-list limit");
                    continue;
                }
                let blob = match blob {
                    Ok(b) => b,
                    Err(msg) => {
                        fail(&mut failed, &name, &msg);
                        continue;
                    }
                };
                let xml = String::from_utf8_lossy(&blob.bytes).into_owned();
                let parsed = match parsers::cbl::parse(xml.as_bytes()) {
                    Ok(p) if p.books.len() <= crate::cbl::MAX_BOOKS_PER_LIST => p,
                    Ok(_) => {
                        fail(&mut failed, &name, &crate::cbl::too_many_entries_message());
                        continue;
                    }
                    Err(e) => {
                        fail(&mut failed, &name, &e.to_string());
                        continue;
                    }
                };
                match insert_list_from_parsed(
                    &app,
                    parent.owner_user_id,
                    &parsed,
                    &xml,
                    SourceMeta::Catalog {
                        catalog_source_id: source.id,
                        catalog_path: ev.catalog_path.clone(),
                        blob_sha: blob.blob_sha,
                        description: None,
                        refresh_schedule: None,
                    },
                )
                .await
                {
                    Ok(l) => {
                        imported += 1;
                        have.insert(ev.catalog_path.clone(), l.clone());
                        l
                    }
                    Err(_) => {
                        fail(&mut failed, &name, "import failed");
                        continue;
                    }
                }
            };
            let child_name = child.parsed_name.clone();
            // Everyone who reads the master can open the event.
            let view = match child.owner_user_id {
                None => super::saved_views::ensure_shared_cbl_view(&app.db, &child).await,
                Some(_) => super::saved_views::ensure_cbl_view(&app.db, user.id, &child).await,
            };
            if let Err(e) = view {
                tracing::warn!(error = %e, "cbl derive: couldn't create the event's view");
            }
            match link_event(&app, &user, &parent, &child, ev.after_entry_id).await {
                Ok(true) => linked += 1,
                Ok(false) => {}
                Err(msg) => fail(&mut failed, &child_name, &msg),
            }
        }
    }

    for part in &req.parts {
        let other = match cbl_list::Entity::find_by_id(part.list_id)
            .one(&app.db)
            .await
        {
            Ok(Some(o)) if o.owner_user_id == parent.owner_user_id => o,
            Ok(_) => {
                fail(&mut failed, "part", "reading list not found");
                continue;
            }
            Err(_) => return internal(),
        };
        let (from, to) = match part.direction {
            PartDirection::Next => (&parent, &other),
            PartDirection::Previous => (&other, &parent),
        };
        match link_part(&app, from, to).await {
            Ok(true) => linked += 1,
            Ok(false) => {}
            Err(msg) => fail(&mut failed, &other.parsed_name, &msg),
        }
    }

    audit_admin_list_write(
        &app,
        &ctx,
        &user,
        &parent,
        "admin.cbl_list.links.derive",
        serde_json::json!({ "imported": imported, "linked": linked }),
    )
    .await;
    let links = match super::cbl_links::build_view(&app, &user, &parent).await {
        Ok(v) => v,
        Err(_) => return internal(),
    };
    Json(ApplyDeriveResp {
        imported,
        linked,
        failed,
        links,
    })
    .into_response()
}

/// Link `child` as an event after `entry_id`; `Ok(false)` = already linked.
async fn link_event(
    app: &AppState,
    user: &CurrentUser,
    parent: &cbl_list::Model,
    child: &cbl_list::Model,
    entry_id: Uuid,
) -> Result<bool, String> {
    let _lock = super::cbl_links::link_write_lock(app)
        .await
        .map_err(|_| "couldn't link it".to_owned())?;
    let db_err = |_| "couldn't link it".to_owned();
    if child.id == parent.id {
        return Err("a list can't link to itself".into());
    }
    if link_exists(app, parent.id, child.id, LinkKind::Event)
        .await
        .map_err(db_err)?
    {
        return Ok(false);
    }
    if has_events(app, child.id).await.map_err(db_err)? {
        return Err("it has events of its own".into());
    }
    let total = reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ParentListId.eq(parent.id))
        .count(&app.db)
        .await
        .map_err(db_err)?;
    if total >= MAX_LINKS_PER_LIST {
        return Err(format!(
            "a list can hold at most {MAX_LINKS_PER_LIST} links"
        ));
    }
    let Some((key, position)) = anchor_of_entry(app, parent.id, entry_id)
        .await
        .map_err(db_err)?
    else {
        return Err("its place is no longer in this list".into());
    };
    let sort = next_sort_at(app, parent.id, position, None)
        .await
        .map_err(db_err)?;
    let now = Utc::now().fixed_offset();
    reading_list_link::ActiveModel {
        id: Set(Uuid::now_v7()),
        parent_list_id: Set(parent.id),
        child_list_id: Set(child.id),
        kind: Set(LinkKind::Event),
        anchor_cv_issue_id: Set(key.cv_issue_id),
        anchor_series: Set(key.series),
        anchor_number: Set(key.number),
        anchor_volume: Set(key.volume),
        anchor_year: Set(key.year),
        anchor_position_hint: Set(Some(position)),
        anchor_status: Set(AnchorStatus::Placed),
        placement: Set(LinkPlacement::AutoTwin),
        sort_within_anchor: Set(sort),
        created_by: Set(Some(user.id)),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&app.db)
    .await
    .map_err(db_err)?;
    Ok(true)
}

/// Chain `from` → `to` (`continues`); `Ok(false)` = already chained.
async fn link_part(
    app: &AppState,
    from: &cbl_list::Model,
    to: &cbl_list::Model,
) -> Result<bool, String> {
    let _lock = super::cbl_links::link_write_lock(app)
        .await
        .map_err(|_| "couldn't link it".to_owned())?;
    let db_err = |_| "couldn't link it".to_owned();
    if link_exists(app, from.id, to.id, LinkKind::Continues)
        .await
        .map_err(db_err)?
    {
        return Ok(false);
    }
    if has_continues(app, from.id).await.map_err(db_err)? {
        return Err(format!("{} already has a next part", from.parsed_name));
    }
    if continues_reaches(app, to.id, from.id)
        .await
        .map_err(db_err)?
    {
        return Err("that would make the parts loop".into());
    }
    let now = Utc::now().fixed_offset();
    reading_list_link::ActiveModel {
        id: Set(Uuid::now_v7()),
        parent_list_id: Set(from.id),
        child_list_id: Set(to.id),
        kind: Set(LinkKind::Continues),
        anchor_cv_issue_id: Set(None),
        anchor_series: Set(None),
        anchor_number: Set(None),
        anchor_volume: Set(None),
        anchor_year: Set(None),
        anchor_position_hint: Set(None),
        anchor_status: Set(AnchorStatus::Placed),
        placement: Set(LinkPlacement::AutoChain),
        sort_within_anchor: Set(0),
        created_by: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&app.db)
    .await
    .map_err(db_err)?;
    Ok(true)
}
