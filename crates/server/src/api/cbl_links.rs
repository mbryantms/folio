//! Linked reading lists: the links API (plan `linked-reading-lists-1.0`,
//! PR 2a).
//!
//! A master list links each event list at the issue it is read after
//! (`kind = event`) and the next part at its end (`kind = continues`); see
//! [`crate::cbl::links`] for how a link keeps its place across refreshes.
//!
//! - `GET    /me/cbl-lists/{id}/links` — the list's events (with each
//!   event's progress for the caller and whether they skipped it), its
//!   next part, and the chapters its event anchors divide it into.
//! - `POST   /me/cbl-lists/{id}/links` — link a list after an entry, or as
//!   the next part.
//! - `PATCH  /me/cbl-lists/{id}/links/{link_id}` — move an event.
//! - `DELETE /me/cbl-lists/{id}/links/{link_id}`.
//! - `PUT` / `DELETE /me/cbl-lists/{id}/links/{link_id}/skip` — the
//!   caller's own "skip this event".
//! - `POST   /me/cbl-lists/{id}/progress/through` — mark every issue up to
//!   a position read (with `dry_run` to count first).
//! - `POST   /me/cbl-lists/{id}/view` — the caller's saved view of a list,
//!   created when they have none, so an event band always opens its list.
//!   Linking an event also gives the linker a view of it.
//!
//! Chapter breaks (PR 3a, [`super::cbl_breaks`]) come back in the same
//! view, and chapters start at them as well as ending at event anchors.
//!
//! Writes use the list's own rules ([`super::cbl_lists::ensure_can_manage`]):
//! the owner of a personal list, an admin for a shared one, audited as
//! `admin.cbl_list.link.*` when an admin changes a shared list. Two rules
//! keep links sane:
//! - **visibility** — a link may only point at a list everyone who can see
//!   the parent can see: a shared list links shared lists only;
//! - **one level** — an event list has no events of its own, and a list
//!   that is an event somewhere cannot take events.

use axum::{
    Extension, Json,
    extract::{Path as AxPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::reading_list_link::{self, AnchorStatus, LinkKind, LinkPlacement};
use entity::{cbl_entry, cbl_list, reading_list_break, reading_list_link_skip, saved_view};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DbBackend, EntityTrait, FromQueryResult,
    ModelTrait, PaginatorTrait, QueryFilter, Statement, sea_query::OnConflict, sea_query::Value,
};
use serde::{Deserialize, Serialize};
use server_macros::handler;
use shared::error::{ApiErrorCode, FieldError};
use std::collections::{BTreeMap, HashMap, HashSet};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;

use super::cbl_lists::{audit_admin_list_write, ensure_can_manage, ensure_owner, fetch_list};
use super::error;
use crate::api::extractors::Validated;
use crate::auth::CurrentUser;
use crate::cbl::links::{AnchorKey, EntryRef};
use crate::library::access::{self, VisibleLibraries};
use crate::middleware::RequestContext;
use crate::state::AppState;

/// Most links one list may hold. A master part has a handful of events;
/// the cap keeps the links GET a bounded, unpaginated read.
pub const MAX_LINKS_PER_LIST: u64 = 500;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_links, create_link))
        .routes(routes!(update_link, delete_link))
        .routes(routes!(skip_link, unskip_link))
        .routes(routes!(mark_read_through))
        .routes(routes!(open_list_view))
}

// ───── wire types ─────

/// Where an event sits in its parent list.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LinkAnchorView {
    /// 0-based position of the issue it is read after (the last known one
    /// when `status = lost`).
    pub position: Option<i32>,
    pub status: AnchorStatus,
    pub series: Option<String>,
    pub number: Option<String>,
}

/// Enough of an issue to open it in the reader.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LinkIssueRefView {
    pub issue_id: String,
    pub issue_slug: String,
    pub series_slug: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ReadingListLinkView {
    pub id: String,
    pub kind: LinkKind,
    pub placement: LinkPlacement,
    pub child_list_id: String,
    pub child_name: String,
    /// `None` for a `continues` link.
    pub anchor: Option<LinkAnchorView>,
    /// Order among several events at the same anchor.
    pub sort_within_anchor: i32,
    /// Entries in the linked list.
    pub total: i64,
    /// Entries the caller has in their library.
    pub in_library: i64,
    /// Of those, the ones the caller has finished.
    pub read: i64,
    /// The first unfinished issue the caller can open, in list order.
    pub first_unread: Option<LinkIssueRefView>,
    /// The caller's saved view of the linked list (their own, else a
    /// shared one), for opening it and reading in its order. `None` when
    /// they have no view of it.
    pub child_view_id: Option<String>,
    /// The caller chose to skip this event.
    pub skipped: bool,
}

/// A chapter break: a new chapter starts at the anchor issue (PR 3a).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ReadingListBreakView {
    pub id: String,
    /// The chapter's name; `None` = named after its first issue.
    pub label: Option<String>,
    /// The issue the chapter starts at.
    pub anchor: LinkAnchorView,
}

/// A stretch of the parent list. Chapters end at event anchors and start
/// at chapter breaks.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ReadingListChapterView {
    pub index: i32,
    /// 0-based, inclusive.
    pub start_position: i32,
    /// 0-based, inclusive.
    pub end_position: i32,
    pub label: String,
    /// The events read right after this chapter, in order (empty for the
    /// last chapter).
    pub event_link_ids: Vec<String>,
    /// The chapter break that starts this chapter, if any (for renaming or
    /// removing it).
    pub break_id: Option<String>,
    pub total: i64,
    pub in_library: i64,
    pub read: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ReadingListLinksView {
    /// Event links in reading order (anchor position, then
    /// `sort_within_anchor`).
    pub events: Vec<ReadingListLinkView>,
    /// The next part, if linked.
    pub continues: Option<ReadingListLinkView>,
    /// Chapter breaks in position order.
    pub breaks: Vec<ReadingListBreakView>,
    /// The parent list split at its event anchors. One chapter covering
    /// the whole list when it has no events; empty when it has no entries.
    pub chapters: Vec<ReadingListChapterView>,
    /// How many lists the caller can see link to this one.
    pub linked_from: i64,
    /// Those lists, most recently read first (the event page's "Part of"
    /// back link picks the one the reader came from, else the first).
    pub parents: Vec<super::cbl_link_nav::ReadingListParentView>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateLinkReq {
    pub child_list_id: Uuid,
    pub kind: LinkKind,
    /// Required for `event` (the entry it is read after); refused for
    /// `continues`.
    #[serde(default)]
    pub after_entry_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct UpdateLinkReq {
    /// Move the event after this entry.
    #[serde(default)]
    #[garde(skip)]
    pub after_entry_id: Option<Uuid>,
    /// 0 or more: the event's order among the events after one issue.
    #[serde(default)]
    #[garde(custom(not_negative))]
    pub sort_within_anchor: Option<i32>,
}

fn not_negative(value: &Option<i32>, _: &()) -> garde::Result {
    match value {
        Some(v) if *v < 0 => Err(garde::Error::new("must be 0 or more")),
        _ => Ok(()),
    }
}

fn position_not_negative(value: &i32, _: &()) -> garde::Result {
    not_negative(&Some(*value), &())
}

#[derive(Debug, Deserialize, garde::Validate, utoipa::ToSchema)]
pub struct MarkThroughReq {
    /// 0-based position: everything at or before it is marked read.
    #[garde(custom(position_not_negative))]
    pub position: i32,
    /// Count only; write nothing.
    #[serde(default)]
    #[garde(skip)]
    pub dry_run: bool,
}

/// The caller's saved view of a reading list.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListViewRef {
    pub view_id: String,
    /// The view was created by this call.
    pub created: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MarkThroughResp {
    /// Issues marked read (or that would be, on a dry run).
    pub marked: u32,
    /// Writes that failed.
    pub failed: u32,
}

// ───── helpers ─────

pub(super) fn internal() -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal")
}

pub(super) fn field_error(field: &str, message: &str) -> Response {
    super::respond_with_field_errors(
        StatusCode::UNPROCESSABLE_ENTITY,
        ApiErrorCode::Validation,
        format!("{field}: {message}"),
        vec![FieldError {
            field: field.to_owned(),
            message: message.to_owned(),
        }],
    )
}

/// The caller can read `list` (their own, or shared).
fn visible_to(list: &cbl_list::Model, user: &CurrentUser) -> bool {
    list.owner_user_id.is_none_or(|o| o == user.id)
}

#[derive(Debug, FromQueryResult)]
pub(super) struct EntryStatRow {
    pub cbl_list_id: Uuid,
    position: i32,
    matched_issue_id: Option<String>,
    issue_slug: Option<String>,
    series_slug: Option<String>,
    in_library: bool,
    is_read: bool,
}

#[derive(Debug, Default, Clone)]
pub(super) struct ListStats {
    pub total: i64,
    pub in_library: i64,
    pub read: i64,
    pub first_unread: Option<LinkIssueRefView>,
}

/// One row per entry of every list in `list_ids`, in position order, as
/// the caller sees it: `in_library` only for an issue they can open,
/// `is_read` only for one of those they finished. One statement.
pub(super) async fn entry_stats(
    app: &AppState,
    acl: &VisibleLibraries,
    user_id: Uuid,
    list_ids: &[Uuid],
) -> Result<Vec<EntryStatRow>, sea_orm::DbErr> {
    let mut params = vec![Value::from(list_ids.to_vec()), Value::from(user_id)];
    let vis = access::visible_issues_sql(acl, "i", &mut params);
    EntryStatRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r#"
            SELECT e.cbl_list_id, e.position, e.matched_issue_id,
                   i.slug AS issue_slug, s.slug AS series_slug,
                   (i.id IS NOT NULL) AS in_library,
                   (i.id IS NOT NULL AND COALESCE(p.finished, FALSE)) AS is_read
            FROM cbl_entries e
            LEFT JOIN issues i ON i.id = e.matched_issue_id{vis}
            LEFT JOIN series s ON s.id = i.series_id
            LEFT JOIN progress_records p ON p.issue_id = i.id AND p.user_id = $2
            WHERE e.cbl_list_id = ANY($1)
            ORDER BY e.cbl_list_id, e.position
            "#
        ),
        params,
    ))
    .all(&app.db)
    .await
}

pub(super) fn fold_stats<'a>(rows: impl Iterator<Item = &'a EntryStatRow>) -> ListStats {
    let mut s = ListStats::default();
    for r in rows {
        s.total += 1;
        if r.in_library {
            s.in_library += 1;
            if r.is_read {
                s.read += 1;
            } else if s.first_unread.is_none()
                && let (Some(id), Some(issue_slug), Some(series_slug)) =
                    (&r.matched_issue_id, &r.issue_slug, &r.series_slug)
            {
                s.first_unread = Some(LinkIssueRefView {
                    issue_id: id.clone(),
                    issue_slug: issue_slug.clone(),
                    series_slug: series_slug.clone(),
                });
            }
        }
    }
    s
}

/// The caller's saved view of each list in `list_ids`: their own first,
/// else a shared one. Lists they have no view of are absent.
pub(super) async fn caller_views(
    app: &AppState,
    user_id: Uuid,
    list_ids: &[Uuid],
) -> Result<HashMap<Uuid, Uuid>, sea_orm::DbErr> {
    let mut views: HashMap<Uuid, Uuid> = HashMap::new();
    if list_ids.is_empty() {
        return Ok(views);
    }
    let rows = saved_view::Entity::find()
        .filter(saved_view::Column::Kind.eq(saved_view::SavedViewKind::Cbl))
        .filter(saved_view::Column::CblListId.is_in(list_ids.to_vec()))
        .filter(
            sea_orm::Condition::any()
                .add(saved_view::Column::UserId.eq(user_id))
                .add(saved_view::Column::UserId.is_null()),
        )
        .all(&app.db)
        .await?;
    for v in rows {
        let Some(list_id) = v.cbl_list_id else {
            continue;
        };
        let own = v.user_id == Some(user_id);
        if own || !views.contains_key(&list_id) {
            views.insert(list_id, v.id);
        }
    }
    Ok(views)
}

/// The whole links view of `parent` for `user`. A fixed number of
/// statements however many links the list has.
pub(super) async fn build_view(
    app: &AppState,
    user: &CurrentUser,
    parent: &cbl_list::Model,
) -> Result<ReadingListLinksView, sea_orm::DbErr> {
    let links = reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ParentListId.eq(parent.id))
        .all(&app.db)
        .await?;
    let child_ids: Vec<Uuid> = links.iter().map(|l| l.child_list_id).collect();
    let children: HashMap<Uuid, cbl_list::Model> = if child_ids.is_empty() {
        HashMap::new()
    } else {
        cbl_list::Entity::find()
            .filter(cbl_list::Column::Id.is_in(child_ids))
            .all(&app.db)
            .await?
            .into_iter()
            .filter(|c| visible_to(c, user))
            .map(|c| (c.id, c))
            .collect()
    };
    let links: Vec<_> = links
        .into_iter()
        .filter(|l| children.contains_key(&l.child_list_id))
        .collect();

    let skipped: HashSet<Uuid> = if links.is_empty() {
        HashSet::new()
    } else {
        reading_list_link_skip::Entity::find()
            .filter(reading_list_link_skip::Column::UserId.eq(user.id))
            .filter(
                reading_list_link_skip::Column::LinkId
                    .is_in(links.iter().map(|l| l.id).collect::<Vec<_>>()),
            )
            .all(&app.db)
            .await?
            .into_iter()
            .map(|s| s.link_id)
            .collect()
    };

    // The caller's view of each child list: their own first, else shared.
    let views = caller_views(app, user.id, &children.keys().copied().collect::<Vec<_>>()).await?;

    let acl = access::for_user(app, user).await;
    let mut ids = vec![parent.id];
    ids.extend(children.keys().copied());
    let rows = entry_stats(app, &acl, user.id, &ids).await?;
    let mut by_list: HashMap<Uuid, Vec<&EntryStatRow>> = HashMap::new();
    for r in &rows {
        by_list.entry(r.cbl_list_id).or_default().push(r);
    }
    let stats_of = |id: Uuid| -> ListStats {
        by_list
            .get(&id)
            .map(|v| fold_stats(v.iter().copied()))
            .unwrap_or_default()
    };

    // One order for events everywhere (the page, Up Next, feeds).
    let mut links = links;
    links.sort_by_key(super::cbl_link_nav::event_order);
    let mut events = Vec::new();
    let mut continues = None;
    for link in &links {
        let child = &children[&link.child_list_id];
        let st = stats_of(child.id);
        let view = ReadingListLinkView {
            id: link.id.to_string(),
            kind: link.kind,
            placement: link.placement,
            child_list_id: child.id.to_string(),
            child_name: child.parsed_name.clone(),
            anchor: (link.kind == LinkKind::Event).then(|| LinkAnchorView {
                position: link.anchor_position_hint,
                status: link.anchor_status,
                series: link.anchor_series.clone(),
                number: link.anchor_number.clone(),
            }),
            sort_within_anchor: link.sort_within_anchor,
            total: st.total,
            in_library: st.in_library,
            read: st.read,
            first_unread: st.first_unread,
            child_view_id: views.get(&child.id).map(Uuid::to_string),
            skipped: skipped.contains(&link.id),
        };
        match link.kind {
            LinkKind::Event => events.push(view),
            LinkKind::Continues => continues = Some(view),
        }
    }

    let mut breaks: Vec<ReadingListBreakView> = reading_list_break::Entity::find()
        .filter(reading_list_break::Column::ListId.eq(parent.id))
        .all(&app.db)
        .await?
        .into_iter()
        .map(|b| ReadingListBreakView {
            id: b.id.to_string(),
            label: b.label,
            anchor: LinkAnchorView {
                position: b.anchor_position_hint,
                status: b.anchor_status,
                series: b.anchor_series,
                number: b.anchor_number,
            },
        })
        .collect();
    breaks.sort_by_key(|b| b.anchor.position.unwrap_or(i32::MAX));

    let parent_rows = by_list.get(&parent.id).cloned().unwrap_or_default();
    let chapters = chapters_of(&parent.parsed_name, &parent_rows, &events, &breaks);

    let parents = super::cbl_link_nav::parents_of(app, user.id, parent.id).await?;
    let linked_from = parents.len() as i64;

    Ok(ReadingListLinksView {
        events,
        continues,
        breaks,
        chapters,
        linked_from,
        parents,
    })
}

/// Split the parent's entries into chapters. A chapter **ends** at an event
/// anchor (its events are read right after it) and **starts** at a chapter
/// break. Labels, first match wins: the break's own label, "Up to <first
/// event at its end>", "After <last event before it>", "From <series>
/// #<number>" (an unnamed break), the list's name.
fn chapters_of(
    list_name: &str,
    rows: &[&EntryStatRow],
    events: &[ReadingListLinkView],
    breaks: &[ReadingListBreakView],
) -> Vec<ReadingListChapterView> {
    let (Some(first), Some(last)) = (
        rows.first().map(|r| r.position),
        rows.last().map(|r| r.position),
    ) else {
        return Vec::new();
    };
    // anchor position → events there, in reading order.
    let mut ends: BTreeMap<i32, Vec<&ReadingListLinkView>> = BTreeMap::new();
    for ev in events {
        if let Some(pos) = ev.anchor.as_ref().and_then(|a| a.position) {
            ends.entry(pos.clamp(first, last)).or_default().push(ev);
        }
    }
    // chapter start → the break that starts it (first one wins).
    let mut starts: BTreeMap<i32, Option<&ReadingListBreakView>> = BTreeMap::new();
    starts.insert(first, None);
    for pos in ends.keys() {
        if *pos < last {
            starts.entry(pos + 1).or_insert(None);
        }
    }
    for b in breaks {
        // A break at the first issue starts nothing new.
        let Some(pos) = b.anchor.position.filter(|p| *p > first) else {
            continue;
        };
        let slot = starts.entry(pos.min(last)).or_insert(None);
        if slot.is_none() {
            *slot = Some(b);
        }
    }
    let starts: Vec<_> = starts.into_iter().collect();
    let mut chapters = Vec::with_capacity(starts.len());
    let mut previous_events: &[&ReadingListLinkView] = &[];
    for (i, (start, brk)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map_or(last, |(next, _)| next - 1);
        let evs: &[&ReadingListLinkView] = ends.get(&end).map_or(&[], Vec::as_slice);
        let label = brk
            .and_then(|b| b.label.clone())
            .or_else(|| evs.first().map(|e| format!("Up to {}", e.child_name)))
            .or_else(|| {
                previous_events
                    .last()
                    .map(|e| format!("After {}", e.child_name))
            })
            .or_else(|| {
                brk.and_then(|b| match (&b.anchor.series, &b.anchor.number) {
                    (Some(s), Some(n)) => Some(format!("From {s} #{n}")),
                    _ => None,
                })
            })
            .unwrap_or_else(|| list_name.to_owned());
        let st = fold_stats(
            rows.iter()
                .copied()
                .filter(|r| r.position >= *start && r.position <= end),
        );
        chapters.push(ReadingListChapterView {
            index: i32::try_from(i).unwrap_or(i32::MAX),
            start_position: *start,
            end_position: end,
            label,
            event_link_ids: evs.iter().map(|e| e.id.clone()).collect(),
            break_id: brk.map(|b| b.id.clone()),
            total: st.total,
            in_library: st.in_library,
            read: st.read,
        });
        previous_events = evs;
    }
    chapters
}

/// The entry `entry_id` of `list_id` as an anchor key + its position.
pub(super) async fn anchor_of_entry(
    app: &AppState,
    list_id: Uuid,
    entry_id: Uuid,
) -> Result<Option<(AnchorKey, i32)>, sea_orm::DbErr> {
    Ok(cbl_entry::Entity::find_by_id(entry_id)
        .filter(cbl_entry::Column::CblListId.eq(list_id))
        .one(&app.db)
        .await?
        .map(|e| {
            let r = EntryRef {
                position: e.position,
                series_name: e.series_name,
                issue_number: e.issue_number,
                volume: e.volume,
                year: e.year,
                cv_issue_id: e.cv_issue_id,
            };
            (AnchorKey::of_entry(&r), r.position)
        }))
}

/// Serializes writes that add a link (here and in `cbl_derive`). Holds a
/// transaction-scoped Postgres advisory lock: keep the returned
/// transaction until the insert is done, then commit (or drop) it.
pub(super) async fn link_write_lock(
    app: &AppState,
) -> Result<sea_orm::DatabaseTransaction, sea_orm::DbErr> {
    use sea_orm::{ConnectionTrait, TransactionTrait};
    let txn = app.db.begin().await?;
    // A fixed key for "reading-list link writes".
    txn.execute_unprepared("SELECT pg_advisory_xact_lock(8206100471)")
        .await?;
    Ok(txn)
}

/// Next `sort_within_anchor` for a new event at `position`.
pub(super) async fn next_sort_at(
    app: &AppState,
    parent: Uuid,
    position: i32,
    except: Option<Uuid>,
) -> Result<i32, sea_orm::DbErr> {
    let mut q = reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ParentListId.eq(parent))
        .filter(reading_list_link::Column::Kind.eq(LinkKind::Event))
        .filter(reading_list_link::Column::AnchorPositionHint.eq(position));
    if let Some(id) = except {
        q = q.filter(reading_list_link::Column::Id.ne(id));
    }
    Ok(q.all(&app.db)
        .await?
        .iter()
        .map(|l| l.sort_within_anchor + 1)
        .max()
        .unwrap_or(0))
}

pub(super) async fn has_events(app: &AppState, list_id: Uuid) -> Result<bool, sea_orm::DbErr> {
    Ok(reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ParentListId.eq(list_id))
        .filter(reading_list_link::Column::Kind.eq(LinkKind::Event))
        .count(&app.db)
        .await?
        > 0)
}

pub(super) async fn is_an_event(app: &AppState, list_id: Uuid) -> Result<bool, sea_orm::DbErr> {
    Ok(reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ChildListId.eq(list_id))
        .filter(reading_list_link::Column::Kind.eq(LinkKind::Event))
        .count(&app.db)
        .await?
        > 0)
}

/// True when following `continues` links from `from` reaches `target`.
pub(super) async fn continues_reaches(
    app: &AppState,
    from: Uuid,
    target: Uuid,
) -> Result<bool, sea_orm::DbErr> {
    let mut cur = from;
    let mut seen = HashSet::new();
    while seen.insert(cur) {
        if cur == target {
            return Ok(true);
        }
        match reading_list_link::Entity::find()
            .filter(reading_list_link::Column::ParentListId.eq(cur))
            .filter(reading_list_link::Column::Kind.eq(LinkKind::Continues))
            .one(&app.db)
            .await?
        {
            Some(next) => cur = next.child_list_id,
            None => return Ok(false),
        }
    }
    Ok(false)
}

async fn fetch_link(
    app: &AppState,
    parent: Uuid,
    link_id: Uuid,
) -> Result<reading_list_link::Model, Response> {
    reading_list_link::Entity::find_by_id(link_id)
        .filter(reading_list_link::Column::ParentListId.eq(parent))
        .one(&app.db)
        .await
        .map_err(|_| internal())?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "not_found", "link not found"))
}

pub(super) async fn view_response(
    app: &AppState,
    user: &CurrentUser,
    parent: &cbl_list::Model,
    status: StatusCode,
) -> Response {
    match build_view(app, user, parent).await {
        Ok(v) => (status, Json(v)).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "cbl links: view failed");
            internal()
        }
    }
}

// ───── handlers ─────

#[utoipa::path(
    operation_id = "cbl_links_list",
    get,
    path = "/me/cbl-lists/{id}/links",
    params(("id" = String, Path,)),
    responses(
        (status = 200, body = ReadingListLinksView),
        (status = 404, description = "list not found"),
    )
)]
#[handler]
pub async fn list_links(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> Response {
    let list = match fetch_list(&app.db, id).await {
        Ok(l) => l,
        Err(r) => return r,
    };
    if let Err(r) = ensure_owner(&list, &user).await {
        return r;
    }
    view_response(&app, &user, &list, StatusCode::OK).await
}

#[utoipa::path(
    operation_id = "cbl_links_create",
    post,
    path = "/me/cbl-lists/{id}/links",
    params(("id" = String, Path,)),
    request_body = CreateLinkReq,
    responses(
        (status = 201, body = ReadingListLinksView),
        (status = 403, description = "shared list, caller is not an admin"),
        (status = 404, description = "list not found"),
        (status = 409, description = "already linked, or the list already has a next part"),
        (status = 422, description = "validation"),
    )
)]
#[handler]
pub async fn create_link(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    AxPath(id): AxPath<Uuid>,
    Json(req): Json<CreateLinkReq>,
) -> Response {
    let parent = match fetch_list(&app.db, id).await {
        Ok(l) => l,
        Err(r) => return r,
    };
    if let Err(r) = ensure_can_manage(&parent, &user).await {
        return r;
    }
    if req.child_list_id == parent.id {
        return field_error("child_list_id", "a list can't link to itself");
    }
    let child = match cbl_list::Entity::find_by_id(req.child_list_id)
        .one(&app.db)
        .await
    {
        Ok(Some(c)) if visible_to(&c, &user) => c,
        Ok(_) => return field_error("child_list_id", "reading list not found"),
        Err(_) => return internal(),
    };
    if parent.owner_user_id.is_none() && child.owner_user_id.is_some() {
        return field_error("child_list_id", "a shared list can only link shared lists");
    }

    let (anchor, position) = match (req.kind, req.after_entry_id) {
        (LinkKind::Event, None) => {
            return field_error("after_entry_id", "an event needs the issue it comes after");
        }
        (LinkKind::Continues, Some(_)) => {
            return field_error("after_entry_id", "a next part comes after the whole list");
        }
        (LinkKind::Event, Some(entry_id)) => match anchor_of_entry(&app, parent.id, entry_id).await
        {
            Ok(Some((k, p))) => (Some(k), Some(p)),
            Ok(None) => return field_error("after_entry_id", "entry not in this list"),
            Err(_) => return internal(),
        },
        (LinkKind::Continues, None) => (None, None),
    };

    // One link write at a time: the rules below (one level of events, no
    // loop of parts, one next part) are read-then-write, and two
    // concurrent creates could each pass and together break them.
    let lock = match link_write_lock(&app).await {
        Ok(l) => l,
        Err(_) => return internal(),
    };
    let checks = async {
        let dup = reading_list_link::Entity::find()
            .filter(reading_list_link::Column::ParentListId.eq(parent.id))
            .filter(reading_list_link::Column::ChildListId.eq(child.id))
            .filter(reading_list_link::Column::Kind.eq(req.kind))
            .count(&app.db)
            .await?
            > 0;
        let total = reading_list_link::Entity::find()
            .filter(reading_list_link::Column::ParentListId.eq(parent.id))
            .count(&app.db)
            .await?;
        let rule = match req.kind {
            LinkKind::Event => {
                if has_events(&app, child.id).await? {
                    Some("an event list can't have events of its own")
                } else if is_an_event(&app, parent.id).await? {
                    Some("this list is an event in another list, so it can't take events")
                } else {
                    None
                }
            }
            LinkKind::Continues => {
                let taken = reading_list_link::Entity::find()
                    .filter(reading_list_link::Column::ParentListId.eq(parent.id))
                    .filter(reading_list_link::Column::Kind.eq(LinkKind::Continues))
                    .count(&app.db)
                    .await?
                    > 0;
                if taken {
                    return Ok::<_, sea_orm::DbErr>((dup, total, None, true));
                }
                continues_reaches(&app, child.id, parent.id)
                    .await?
                    .then_some("that would make the parts loop")
            }
        };
        Ok((dup, total, rule, false))
    };
    let (dup, total, rule, has_next) = match checks.await {
        Ok(v) => v,
        Err(_) => return internal(),
    };
    if dup {
        return error(StatusCode::CONFLICT, "conflict", "already linked");
    }
    if has_next {
        return error(
            StatusCode::CONFLICT,
            "conflict",
            "this list already has a next part",
        );
    }
    if let Some(msg) = rule {
        return field_error("child_list_id", msg);
    }
    if total >= MAX_LINKS_PER_LIST {
        return field_error(
            "child_list_id",
            &format!("a list can hold at most {MAX_LINKS_PER_LIST} links"),
        );
    }

    let sort = match position {
        Some(p) => match next_sort_at(&app, parent.id, p, None).await {
            Ok(n) => n,
            Err(_) => return internal(),
        },
        None => 0,
    };
    let now = Utc::now().fixed_offset();
    let anchor = anchor.as_ref();
    let row = reading_list_link::ActiveModel {
        id: Set(Uuid::now_v7()),
        parent_list_id: Set(parent.id),
        child_list_id: Set(child.id),
        kind: Set(req.kind),
        anchor_cv_issue_id: Set(anchor.and_then(|a| a.cv_issue_id)),
        anchor_series: Set(anchor.and_then(|a| a.series.clone())),
        anchor_number: Set(anchor.and_then(|a| a.number.clone())),
        anchor_volume: Set(anchor.and_then(|a| a.volume.clone())),
        anchor_year: Set(anchor.and_then(|a| a.year.clone())),
        anchor_position_hint: Set(position),
        anchor_status: Set(AnchorStatus::Placed),
        placement: Set(LinkPlacement::Manual),
        sort_within_anchor: Set(sort),
        created_by: Set(Some(user.id)),
        created_at: Set(now),
        updated_at: Set(now),
    };
    if let Err(e) = row.insert(&app.db).await {
        // The checks above answer the conflicts; a unique violation here
        // is a race with a writer outside the lock, anything else a fault.
        if matches!(
            e.sql_err(),
            Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
        ) {
            return error(StatusCode::CONFLICT, "conflict", "already linked");
        }
        tracing::error!(error = %e, "cbl links: insert failed");
        return internal();
    }
    if let Err(e) = lock.commit().await {
        tracing::warn!(error = %e, "cbl links: releasing the write lock failed");
    }
    // The linker can open the event from the band straight away.
    if req.kind == LinkKind::Event
        && let Err(e) = super::saved_views::ensure_cbl_view(&app.db, user.id, &child).await
    {
        tracing::warn!(error = %e, "cbl links: couldn't create the event's view");
    }
    audit_admin_list_write(
        &app,
        &ctx,
        &user,
        &parent,
        "admin.cbl_list.link.create",
        serde_json::json!({
            "child_list_id": child.id.to_string(),
            "kind": req.kind.as_str(),
        }),
    )
    .await;
    view_response(&app, &user, &parent, StatusCode::CREATED).await
}

#[utoipa::path(
    operation_id = "cbl_links_update",
    patch,
    path = "/me/cbl-lists/{id}/links/{link_id}",
    params(("id" = String, Path,), ("link_id" = String, Path,)),
    request_body = UpdateLinkReq,
    responses(
        (status = 200, body = ReadingListLinksView),
        (status = 404, description = "list or link not found"),
        (status = 422, description = "validation"),
    )
)]
#[handler]
pub async fn update_link(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    AxPath((id, link_id)): AxPath<(Uuid, Uuid)>,
    Validated(req): Validated<UpdateLinkReq>,
) -> Response {
    let parent = match fetch_list(&app.db, id).await {
        Ok(l) => l,
        Err(r) => return r,
    };
    if let Err(r) = ensure_can_manage(&parent, &user).await {
        return r;
    }
    let link = match fetch_link(&app, parent.id, link_id).await {
        Ok(l) => l,
        Err(r) => return r,
    };
    if link.kind == LinkKind::Continues
        && (req.after_entry_id.is_some() || req.sort_within_anchor.is_some())
    {
        return field_error("after_entry_id", "a next part comes after the whole list");
    }
    let mut am: reading_list_link::ActiveModel = link.clone().into();
    if let Some(entry_id) = req.after_entry_id {
        let (key, pos) = match anchor_of_entry(&app, parent.id, entry_id).await {
            Ok(Some(v)) => v,
            Ok(None) => return field_error("after_entry_id", "entry not in this list"),
            Err(_) => return internal(),
        };
        let sort = match req.sort_within_anchor {
            Some(s) => s,
            None => match next_sort_at(&app, parent.id, pos, Some(link.id)).await {
                Ok(n) => n,
                Err(_) => return internal(),
            },
        };
        am.anchor_cv_issue_id = Set(key.cv_issue_id);
        am.anchor_series = Set(key.series);
        am.anchor_number = Set(key.number);
        am.anchor_volume = Set(key.volume);
        am.anchor_year = Set(key.year);
        am.anchor_position_hint = Set(Some(pos));
        am.anchor_status = Set(AnchorStatus::Placed);
        am.placement = Set(LinkPlacement::Manual);
        am.sort_within_anchor = Set(sort);
    } else if let Some(s) = req.sort_within_anchor {
        am.sort_within_anchor = Set(s);
    }
    am.updated_at = Set(Utc::now().fixed_offset());
    if am.update(&app.db).await.is_err() {
        return internal();
    }
    audit_admin_list_write(
        &app,
        &ctx,
        &user,
        &parent,
        "admin.cbl_list.link.update",
        serde_json::json!({
            "link_id": link.id.to_string(),
            "child_list_id": link.child_list_id.to_string(),
        }),
    )
    .await;
    view_response(&app, &user, &parent, StatusCode::OK).await
}

#[utoipa::path(
    operation_id = "cbl_links_delete",
    delete,
    path = "/me/cbl-lists/{id}/links/{link_id}",
    params(("id" = String, Path,), ("link_id" = String, Path,)),
    responses(
        (status = 204),
        (status = 404, description = "list or link not found"),
    )
)]
#[handler]
pub async fn delete_link(
    State(app): State<AppState>,
    user: CurrentUser,
    Extension(ctx): Extension<RequestContext>,
    AxPath((id, link_id)): AxPath<(Uuid, Uuid)>,
) -> Response {
    let parent = match fetch_list(&app.db, id).await {
        Ok(l) => l,
        Err(r) => return r,
    };
    if let Err(r) = ensure_can_manage(&parent, &user).await {
        return r;
    }
    let link = match fetch_link(&app, parent.id, link_id).await {
        Ok(l) => l,
        Err(r) => return r,
    };
    let detail = serde_json::json!({
        "link_id": link.id.to_string(),
        "child_list_id": link.child_list_id.to_string(),
        "kind": link.kind.as_str(),
    });
    if link.delete(&app.db).await.is_err() {
        return internal();
    }
    audit_admin_list_write(
        &app,
        &ctx,
        &user,
        &parent,
        "admin.cbl_list.link.delete",
        detail,
    )
    .await;
    StatusCode::NO_CONTENT.into_response()
}

/// The event link `link_id` of a list the caller can read, for the
/// caller's own skip state.
async fn readable_event(
    app: &AppState,
    user: &CurrentUser,
    id: Uuid,
    link_id: Uuid,
) -> Result<reading_list_link::Model, Response> {
    let parent = fetch_list(&app.db, id).await?;
    ensure_owner(&parent, user).await?;
    let link = fetch_link(app, parent.id, link_id).await?;
    let child_visible = cbl_list::Entity::find_by_id(link.child_list_id)
        .one(&app.db)
        .await
        .map_err(|_| internal())?
        .is_some_and(|c| visible_to(&c, user));
    if !child_visible {
        return Err(error(StatusCode::NOT_FOUND, "not_found", "link not found"));
    }
    if link.kind != LinkKind::Event {
        return Err(field_error("link_id", "only an event can be skipped"));
    }
    Ok(link)
}

#[utoipa::path(
    operation_id = "cbl_links_skip",
    put,
    path = "/me/cbl-lists/{id}/links/{link_id}/skip",
    params(("id" = String, Path,), ("link_id" = String, Path,)),
    responses(
        (status = 204),
        (status = 404, description = "list or link not found"),
        (status = 422, description = "not an event"),
    )
)]
#[handler]
pub async fn skip_link(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((id, link_id)): AxPath<(Uuid, Uuid)>,
) -> Response {
    let link = match readable_event(&app, &user, id, link_id).await {
        Ok(l) => l,
        Err(r) => return r,
    };
    let row = reading_list_link_skip::ActiveModel {
        user_id: Set(user.id),
        link_id: Set(link.id),
        skipped_at: Set(Utc::now().fixed_offset()),
    };
    match reading_list_link_skip::Entity::insert(row)
        .on_conflict(
            OnConflict::columns([
                reading_list_link_skip::Column::UserId,
                reading_list_link_skip::Column::LinkId,
            ])
            .do_nothing()
            .to_owned(),
        )
        .try_insert()
        .exec(&app.db)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => internal(),
    }
}

#[utoipa::path(
    operation_id = "cbl_links_unskip",
    delete,
    path = "/me/cbl-lists/{id}/links/{link_id}/skip",
    params(("id" = String, Path,), ("link_id" = String, Path,)),
    responses(
        (status = 204),
        (status = 404, description = "list or link not found"),
    )
)]
#[handler]
pub async fn unskip_link(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath((id, link_id)): AxPath<(Uuid, Uuid)>,
) -> Response {
    let link = match readable_event(&app, &user, id, link_id).await {
        Ok(l) => l,
        Err(r) => return r,
    };
    match reading_list_link_skip::Entity::delete_many()
        .filter(reading_list_link_skip::Column::UserId.eq(user.id))
        .filter(reading_list_link_skip::Column::LinkId.eq(link.id))
        .exec(&app.db)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => internal(),
    }
}

#[derive(Debug, FromQueryResult)]
struct UnreadIssue {
    id: String,
    page_count: Option<i32>,
}

#[utoipa::path(
    operation_id = "cbl_links_mark_read_through",
    post,
    path = "/me/cbl-lists/{id}/progress/through",
    params(("id" = String, Path,)),
    request_body = MarkThroughReq,
    responses(
        (status = 200, body = MarkThroughResp),
        (status = 404, description = "list not found"),
        (status = 422, description = "validation"),
    )
)]
#[handler]
pub async fn mark_read_through(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
    Validated(req): Validated<MarkThroughReq>,
) -> Response {
    let list = match fetch_list(&app.db, id).await {
        Ok(l) => l,
        Err(r) => return r,
    };
    if let Err(r) = ensure_owner(&list, &user).await {
        return r;
    }
    let acl = access::for_user(&app, &user).await;
    // Every unfinished issue the caller can open, matched at or before the
    // position. An issue listed twice is marked once.
    let mut params = vec![
        Value::from(list.id),
        Value::from(user.id),
        Value::from(req.position),
    ];
    let vis = access::visible_issues_sql(&acl, "i", &mut params);
    let issues = match UnreadIssue::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r#"
            SELECT DISTINCT i.id, i.page_count
            FROM cbl_entries e
            JOIN issues i ON i.id = e.matched_issue_id{vis}
            LEFT JOIN progress_records p ON p.issue_id = i.id AND p.user_id = $2
            WHERE e.cbl_list_id = $1
              AND e.position <= $3
              AND i.state = 'active'
              AND COALESCE(p.finished, FALSE) = FALSE
            "#
        ),
        params,
    ))
    .all(&app.db)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "cbl links: mark-through lookup failed");
            return internal();
        }
    };
    if req.dry_run {
        return Json(MarkThroughResp {
            marked: u32::try_from(issues.len()).unwrap_or(u32::MAX),
            failed: 0,
        })
        .into_response();
    }
    let mut marked = 0u32;
    let mut failed = 0u32;
    for iss in issues {
        let last = iss.page_count.unwrap_or(1).max(1) - 1;
        // Marked read without being read here: backfill, so it stays out
        // of time-bound activity (same as the bulk mark).
        match super::progress::upsert_explicit(
            &app,
            user.id,
            &iss.id,
            iss.page_count,
            last,
            1.0,
            true,
            None,
            super::progress::resolve_is_backfill(true, true),
        )
        .await
        {
            Ok(_) => marked += 1,
            Err(e) => {
                tracing::warn!(error = %e, issue_id = %iss.id, "cbl links: mark-through write failed");
                failed += 1;
            }
        }
    }
    Json(MarkThroughResp { marked, failed }).into_response()
}

#[utoipa::path(
    operation_id = "cbl_lists_open_view",
    post,
    path = "/me/cbl-lists/{id}/view",
    params(("id" = String, Path,)),
    responses(
        (status = 200, body = ListViewRef, description = "the caller's existing view"),
        (status = 201, body = ListViewRef, description = "a view was created for the caller"),
        (status = 404, description = "list not found"),
    )
)]
#[handler]
pub async fn open_list_view(
    State(app): State<AppState>,
    user: CurrentUser,
    AxPath(id): AxPath<Uuid>,
) -> Response {
    let list = match fetch_list(&app.db, id).await {
        Ok(l) => l,
        Err(r) => return r,
    };
    if let Err(r) = ensure_owner(&list, &user).await {
        return r;
    }
    match super::saved_views::ensure_cbl_view(&app.db, user.id, &list).await {
        Ok((view_id, created)) => (
            if created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            },
            Json(ListViewRef {
                view_id: view_id.to_string(),
                created,
            }),
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "cbl links: open view failed");
            internal()
        }
    }
}
