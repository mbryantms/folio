//! Linked reading lists 2.0: find the events a list reads **inline** and
//! propose a named chapter for each.
//!
//! A CMRO part, a CBRO "With Events" master or a "with events" character
//! list already has every event's issues in place, so there is nothing to
//! link. What they lack is structure: a 1,000-issue part is one unbroken
//! run. `POST /me/cbl-lists/{id}/chapters/detect` compares the list with
//! known event lists and reports, for each event read here as one
//! stretch, where it starts and ends ([`crate::cbl::derive::detect_event_runs`]).
//!
//! It is a **preview**: nothing is written. The reader picks which to
//! keep and the web creates ordinary chapter breaks for them (the breaks
//! API), so a detected chapter is renamed, moved and removed like any
//! other. Chapters are never applied on import, and a reader who wants
//! none uses the list's Plain view.
//!
//! Candidates, in this order:
//! 1. the lists the caller can already see (for a shared list: shared
//!    lists only, so its chapter names don't come from someone's private
//!    list), except the parts of an order;
//! 2. for a catalog list, the catalog's event lists for the years the
//!    list covers ([`crate::cbl::sources::event_files_for_years`]),
//!    downloaded for the comparison and not stored.

use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use entity::reading_list_link::{self, AnchorStatus, LinkKind};
use entity::{cbl_entry, cbl_list, reading_list_break};
use sea_orm::{
    ColumnTrait, Condition, EntityTrait, FromQueryResult, QueryFilter, QueryOrder, QuerySelect,
};
use serde::Serialize;
use server_macros::handler;
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

use super::cbl_derive::{catalog_failure, catalog_origin, download_parsed};
use super::cbl_lists::{ensure_can_manage, fetch_list};
use super::error;
use crate::auth::CurrentUser;
use crate::cbl::derive::{self, BookKey, Candidate};
use crate::cbl::{catalog, sources};
use crate::state::AppState;

/// Lists of the caller's compared at most.
const MAX_OWN_CANDIDATES: u64 = 300;
/// A list this long is a master, not an event.
const MAX_EVENT_ENTRIES: usize = 400;
/// Catalog event lists one detection downloads at most.
const MAX_CATALOG_CANDIDATES: usize = 60;
/// The index refresh and the downloads share this.
const CATALOG_BUDGET: std::time::Duration = std::time::Duration::from_secs(40);
const _: () = assert!(CATALOG_BUDGET.as_secs() + 10 < crate::app::JSON_ROUTE_TIMEOUT.as_secs());

pub fn routes() -> OpenApiRouter<AppState> {
    // Up to 60 catalog downloads per call, uncached: the same per-user
    // budget as the "Find events and parts" preview.
    OpenApiRouter::new()
        .routes(routes!(detect))
        .route_layer(crate::middleware::rate_limit::CBL_DERIVE_PREVIEW.build_per_user())
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProposedChapter {
    /// The event's name, as its list is named.
    pub name: String,
    /// The entry the chapter would start at.
    pub entry_id: String,
    /// 0-based position of that entry.
    pub position: i32,
    /// "Fantastic Four #8": the issue the event starts at here.
    pub starts_at: String,
    /// Issues of the event in the stretch, and in the event's own list.
    pub issues: i32,
    pub total: i32,
    /// 0-based position of the stretch's last entry.
    pub end_position: i32,
    /// A chapter already starts at `position` (nothing to create there).
    pub already_chapter: bool,
    /// The entry right after the stretch, where the list goes on: a break
    /// there ends the event's chapter. `None` at the end of the list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_entry_id: Option<String>,
    /// A chapter already starts right after the stretch.
    pub next_already_chapter: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DetectChaptersPreview {
    /// In reading order.
    pub proposals: Vec<ProposedChapter>,
    /// Event lists the list was compared with.
    pub compared: i32,
    /// Some of those came from the catalog (the list is a catalog list).
    pub used_catalog: bool,
}

fn internal() -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
}

#[derive(Debug, FromQueryResult)]
struct EntryRow {
    id: Uuid,
    position: i32,
    series_name: String,
    issue_number: String,
    cv_issue_id: Option<i32>,
    year: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct CandidateEntry {
    cbl_list_id: Uuid,
    series_name: String,
    issue_number: String,
    cv_issue_id: Option<i32>,
}

#[derive(Debug, FromQueryResult)]
struct CandidateList {
    id: Uuid,
    parsed_name: String,
    catalog_path: Option<String>,
}

#[utoipa::path(
    operation_id = "cbl_lists_detect_chapters",
    post,
    path = "/me/cbl-lists/{id}/chapters/detect",
    params(("id" = String, Path,)),
    responses(
        (status = 200, body = DetectChaptersPreview),
        (status = 403, description = "not the list's manager"),
        (status = 404, description = "list not found"),
        (status = 429, description = "too many detections"),
        (status = 502, description = "the catalog could not be read"),
    )
)]
#[handler]
pub async fn detect(
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
    let entries = match cbl_entry::Entity::find()
        .select_only()
        .columns([
            cbl_entry::Column::Id,
            cbl_entry::Column::Position,
            cbl_entry::Column::SeriesName,
            cbl_entry::Column::IssueNumber,
            cbl_entry::Column::CvIssueId,
            cbl_entry::Column::Year,
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
    let mut out = DetectChaptersPreview {
        proposals: Vec::new(),
        compared: 0,
        used_catalog: false,
    };
    if entries.is_empty() {
        return Json(out).into_response();
    }
    let keys: Vec<BookKey> = entries
        .iter()
        .map(|e| BookKey::new(e.cv_issue_id, &e.series_name, &e.issue_number))
        .collect();

    // 1. Lists the caller can see.
    let visible = match list.owner_user_id {
        // A shared list takes its chapter names from shared lists only.
        None => Condition::all().add(cbl_list::Column::OwnerUserId.is_null()),
        Some(_) => Condition::any()
            .add(cbl_list::Column::OwnerUserId.eq(user.id))
            .add(cbl_list::Column::OwnerUserId.is_null()),
    };
    let own: Vec<CandidateList> = match cbl_list::Entity::find()
        .select_only()
        .columns([
            cbl_list::Column::Id,
            cbl_list::Column::ParsedName,
            cbl_list::Column::CatalogPath,
        ])
        .filter(visible)
        .filter(cbl_list::Column::Id.ne(list.id))
        .limit(MAX_OWN_CANDIDATES)
        .into_model::<CandidateList>()
        .all(&app.db)
        .await
    {
        Ok(rows) => rows
            .into_iter()
            // A part of an order is a master, never an event.
            .filter(|l| {
                l.catalog_path
                    .as_deref()
                    .is_none_or(|p| sources::profile(p).order_key.is_none())
            })
            .collect(),
        Err(_) => return internal(),
    };
    let mut names: Vec<String> = Vec::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut have_paths: HashSet<String> = HashSet::new();
    if !own.is_empty() {
        let rows = match cbl_entry::Entity::find()
            .select_only()
            .columns([
                cbl_entry::Column::CblListId,
                cbl_entry::Column::SeriesName,
                cbl_entry::Column::IssueNumber,
                cbl_entry::Column::CvIssueId,
            ])
            .filter(
                cbl_entry::Column::CblListId.is_in(own.iter().map(|l| l.id).collect::<Vec<_>>()),
            )
            .into_model::<CandidateEntry>()
            .all(&app.db)
            .await
        {
            Ok(r) => r,
            Err(_) => return internal(),
        };
        let mut by_list: HashMap<Uuid, Vec<BookKey>> = HashMap::new();
        for r in rows {
            by_list.entry(r.cbl_list_id).or_default().push(BookKey::new(
                r.cv_issue_id,
                &r.series_name,
                &r.issue_number,
            ));
        }
        for l in own {
            let Some(k) = by_list.remove(&l.id) else {
                continue;
            };
            if k.len() > MAX_EVENT_ENTRIES {
                continue;
            }
            if let Some(p) = l.catalog_path {
                have_paths.insert(p);
            }
            names.push(l.parsed_name);
            candidates.push(Candidate { keys: k });
        }
    }

    // 2. The catalog's event lists for the years the list covers.
    match catalog_origin(&app, &list).await {
        Ok(Some((source, path))) => {
            let years: Vec<i32> = entries
                .iter()
                .filter_map(|e| e.year.as_deref()?.trim().parse().ok())
                .filter(|y| (1900..=2200).contains(y))
                .collect();
            if let (Some(lo), Some(hi)) = (years.iter().min(), years.iter().max()) {
                let deadline = tokio::time::Instant::now() + CATALOG_BUDGET;
                let index = match tokio::time::timeout_at(
                    deadline,
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
                let index_paths: Vec<&str> =
                    index.entries.iter().map(|e| e.path.as_str()).collect();
                let wanted: HashSet<&str> =
                    sources::event_files_for_years(&path, *lo, *hi, &index_paths)
                        .into_iter()
                        .filter(|p| !have_paths.contains(*p) && *p != path)
                        .take(MAX_CATALOG_CANDIDATES)
                        .collect();
                let files: Vec<catalog::CatalogEntry> = index
                    .entries
                    .iter()
                    .filter(|e| wanted.contains(e.path.as_str()))
                    .cloned()
                    .collect();
                if !files.is_empty() {
                    // A slow catalog costs the catalog candidates, not the
                    // whole detection.
                    if let Ok(mut parsed) =
                        tokio::time::timeout_at(deadline, download_parsed(&source, files)).await
                    {
                        let mut paths: Vec<String> = parsed.keys().cloned().collect();
                        paths.sort_unstable();
                        for p in paths {
                            let Some(cbl) = parsed.remove(&p) else {
                                continue;
                            };
                            if cbl.books.len() > MAX_EVENT_ENTRIES {
                                continue;
                            }
                            names.push(if cbl.name.trim().is_empty() {
                                sources::file_stem(&p).to_owned()
                            } else {
                                cbl.name.clone()
                            });
                            candidates.push(Candidate {
                                keys: cbl
                                    .books
                                    .iter()
                                    .map(|b| {
                                        BookKey::new(b.comicvine_issue_id(), &b.series, &b.number)
                                    })
                                    .collect(),
                            });
                            out.used_catalog = true;
                        }
                    }
                }
            }
        }
        Ok(None) => {}
        Err(_) => return internal(),
    }
    out.compared = i32::try_from(candidates.len()).unwrap_or(i32::MAX);
    if candidates.is_empty() {
        return Json(out).into_response();
    }

    let sizes: Vec<usize> = candidates.iter().map(|c| c.keys.len()).collect();
    // CPU work over thousands of books: off the async workers.
    let runs =
        match tokio::task::spawn_blocking(move || derive::detect_event_runs(&keys, &candidates))
            .await
        {
            Ok(r) => r,
            Err(_) => return internal(),
        };

    // Where a chapter already starts: the top, a placed break, and the
    // issue after a placed event's anchor.
    let mut starts: HashSet<i32> = HashSet::from([entries[0].position]);
    match reading_list_break::Entity::find()
        .filter(reading_list_break::Column::ListId.eq(list.id))
        .filter(reading_list_break::Column::AnchorStatus.eq(AnchorStatus::Placed))
        .all(&app.db)
        .await
    {
        Ok(rows) => starts.extend(rows.into_iter().filter_map(|b| b.anchor_position_hint)),
        Err(_) => return internal(),
    }
    match reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ParentListId.eq(list.id))
        .filter(reading_list_link::Column::Kind.eq(LinkKind::Event))
        .filter(reading_list_link::Column::AnchorStatus.eq(AnchorStatus::Placed))
        .all(&app.db)
        .await
    {
        Ok(rows) => starts.extend(
            rows.into_iter()
                .filter_map(|l| l.anchor_position_hint)
                .map(|p| p + 1),
        ),
        Err(_) => return internal(),
    }

    for run in runs {
        let first = &entries[run.start];
        let last = &entries[run.end];
        let next = entries.get(run.end + 1);
        out.proposals.push(ProposedChapter {
            name: names[run.candidate].clone(),
            entry_id: first.id.to_string(),
            position: first.position,
            starts_at: format!("{} #{}", first.series_name, first.issue_number),
            issues: i32::try_from(run.issues).unwrap_or(i32::MAX),
            total: i32::try_from(sizes[run.candidate]).unwrap_or(i32::MAX),
            end_position: last.position,
            already_chapter: starts.contains(&first.position),
            next_entry_id: next.map(|n| n.id.to_string()),
            next_already_chapter: next.is_some_and(|n| starts.contains(&n.position)),
        });
    }
    Json(out).into_response()
}
