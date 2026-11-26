//! Linked reading lists, PR 4: where reading goes next across linked lists.
//!
//! - **Detours** (D2): at an issue an event is anchored after, Up Next
//!   still points at the main line and also offers each event there the
//!   reader hasn't skipped and has something unread in, in
//!   `sort_within_anchor` order.
//! - **Continues**: at the end of a list with a next part, Up Next moves
//!   into the next part's first unread issue.
//! - **Return** (D3): at the end of an event, Up Next goes back to the
//!   parent — the one the reader came from (`?via=` on the reader), else
//!   the one with the most recent reading on its own issues — at the first
//!   unread issue after the event's anchor, offering that anchor's other
//!   unread events as detours.
//!
//! [`parents_of`] is the one "which lists is this one part of, most
//! recently read first" resolver: the reader's return and the event list
//! page's back link both use it.

use chrono::{DateTime, FixedOffset};
use entity::reading_list_link::{self, AnchorStatus, LinkKind};
use entity::{cbl_list, issue, reading_list_link_skip};
use sea_orm::{
    ColumnTrait, DbBackend, EntityTrait, FromQueryResult, QueryFilter, Statement, Value,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use super::cbl_links::{LinkIssueRefView, caller_views, entry_stats, fold_stats};
use crate::library::access::VisibleLibraries;
use crate::state::AppState;

/// An event the reader can read now, offered beside the main line.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct NextUpDetour {
    pub link_id: String,
    pub list_id: String,
    pub name: String,
    /// The reader's view of the event list (`None` = they have none yet;
    /// `POST /me/cbl-lists/{list_id}/view` makes one).
    pub view_id: Option<String>,
    pub total: i64,
    pub in_library: i64,
    pub read: i64,
    /// Where to start: the first unread issue the reader can open.
    pub first_unread: LinkIssueRefView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NextUpSwitchReason {
    /// The list ended; its next part continues it.
    Continues,
    /// The event ended; back to the list it is part of.
    Return,
}

/// Up Next left the list the reader was in: read the target in this one.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct NextUpListSwitch {
    pub reason: NextUpSwitchReason,
    /// The reading list; `None` when `collection` is set.
    pub list_id: Option<String>,
    /// Back to a collection (the list was read as one of its detours):
    /// read the target with `?collection=<view_id>`.
    pub collection: bool,
    pub name: String,
    /// The reader's view of that list, or the collection's id (`None` =
    /// they have no view of the list yet).
    pub view_id: Option<String>,
}

/// A list the given one is linked into.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ReadingListParentView {
    pub link_id: String,
    pub kind: LinkKind,
    pub list_id: String,
    pub name: String,
    /// The caller's view of the parent list.
    pub view_id: Option<String>,
    /// For an event: 0-based position of the issue it is read after.
    pub anchor_position: Option<i32>,
    /// For an event: "Excalibur #12".
    pub anchor_label: Option<String>,
    /// The caller's latest reading on the parent's own issues.
    pub last_read_at: Option<DateTime<FixedOffset>>,
}

fn visible(list: &cbl_list::Model, user_id: Uuid) -> bool {
    list.owner_user_id.is_none_or(|o| o == user_id)
}

#[derive(Debug, FromQueryResult)]
struct LastRead {
    cbl_list_id: Uuid,
    last_read_at: Option<DateTime<FixedOffset>>,
}

/// Lists `list_id` is linked into that the caller can see, most recently
/// read first (then oldest link first). Three statements.
pub(crate) async fn parents_of(
    app: &AppState,
    user_id: Uuid,
    list_id: Uuid,
) -> Result<Vec<ReadingListParentView>, sea_orm::DbErr> {
    Ok(parents_with_status(app, user_id, list_id)
        .await?
        .into_iter()
        .map(|(p, _)| p)
        .collect())
}

/// [`parents_of`] with each link's anchor status: Up Next only returns
/// into a parent whose anchor is still placed.
async fn parents_with_status(
    app: &AppState,
    user_id: Uuid,
    list_id: Uuid,
) -> Result<Vec<(ReadingListParentView, AnchorStatus)>, sea_orm::DbErr> {
    let rows: Vec<_> = reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ChildListId.eq(list_id))
        .find_also_related(cbl_list::Entity)
        .all(&app.db)
        .await?
        .into_iter()
        .filter_map(|(l, p)| p.filter(|p| visible(p, user_id)).map(|p| (l, p)))
        .collect();
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Uuid> = rows.iter().map(|(_, p)| p.id).collect();
    let views = caller_views(app, user_id, &ids).await?;
    let last: HashMap<Uuid, Option<DateTime<FixedOffset>>> =
        LastRead::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
            SELECT e.cbl_list_id, MAX(p.updated_at) AS last_read_at
            FROM cbl_entries e
            JOIN progress_records p ON p.issue_id = e.matched_issue_id AND p.user_id = $2
            WHERE e.cbl_list_id = ANY($1)
            GROUP BY e.cbl_list_id
            "#,
            [Value::from(ids), Value::from(user_id)],
        ))
        .all(&app.db)
        .await?
        .into_iter()
        .map(|r| (r.cbl_list_id, r.last_read_at))
        .collect();
    let mut out: Vec<(DateTime<FixedOffset>, AnchorStatus, ReadingListParentView)> = rows
        .into_iter()
        .map(|(l, p)| {
            let anchored = l.kind == LinkKind::Event;
            let label = match (&l.anchor_series, &l.anchor_number) {
                (Some(s), Some(n)) if anchored => Some(format!("{s} #{n}")),
                _ => None,
            };
            (
                l.created_at,
                l.anchor_status,
                ReadingListParentView {
                    link_id: l.id.to_string(),
                    kind: l.kind,
                    list_id: p.id.to_string(),
                    name: p.parsed_name,
                    view_id: views.get(&p.id).map(Uuid::to_string),
                    anchor_position: if anchored {
                        l.anchor_position_hint
                    } else {
                        None
                    },
                    anchor_label: label,
                    last_read_at: last.get(&p.id).copied().flatten(),
                },
            )
        })
        .collect();
    out.sort_by(|(ca, _, a), (cb, _, b)| {
        b.last_read_at
            .cmp(&a.last_read_at)
            .then_with(|| ca.cmp(cb))
            .then_with(|| a.link_id.cmp(&b.link_id))
    });
    Ok(out.into_iter().map(|(_, st, v)| (v, st)).collect())
}

/// The one order events are read in, everywhere they are listed: by the
/// issue they follow, then their order at that issue, then age, then id
/// (so a tie never depends on the order rows came back in).
pub(crate) fn event_order(l: &reading_list_link::Model) -> (i32, i32, DateTime<FixedOffset>, Uuid) {
    (
        l.anchor_position_hint.unwrap_or(i32::MAX),
        l.sort_within_anchor,
        l.created_at,
        l.id,
    )
}

/// Events of `list_id` anchored at a position in `from..to` (`to = None`:
/// to the end of the list) that the reader can see, hasn't skipped and
/// has an unread issue in, in reading order. `except` leaves out one list
/// (the event being returned from).
///
/// A range, not one position: Up Next steps over entries that are
/// finished, not in the library or hidden, and an event anchored after
/// one of those is still due before the next issue the reader opens.
pub(crate) async fn detours_between(
    app: &AppState,
    user_id: Uuid,
    acl: &VisibleLibraries,
    list_id: Uuid,
    from: i32,
    to: Option<i32>,
    except: Option<Uuid>,
) -> Result<Vec<NextUpDetour>, sea_orm::DbErr> {
    let mut q = reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ParentListId.eq(list_id))
        .filter(reading_list_link::Column::Kind.eq(LinkKind::Event))
        .filter(reading_list_link::Column::AnchorStatus.eq(AnchorStatus::Placed))
        .filter(reading_list_link::Column::AnchorPositionHint.gte(from));
    if let Some(to) = to {
        q = q.filter(reading_list_link::Column::AnchorPositionHint.lt(to));
    }
    let mut links = q.all(&app.db).await?;
    links.retain(|l| Some(l.child_list_id) != except);
    if links.is_empty() {
        return Ok(Vec::new());
    }
    links.sort_by_key(event_order);
    let skipped: HashSet<Uuid> = reading_list_link_skip::Entity::find()
        .filter(reading_list_link_skip::Column::UserId.eq(user_id))
        .filter(
            reading_list_link_skip::Column::LinkId
                .is_in(links.iter().map(|l| l.id).collect::<Vec<_>>()),
        )
        .all(&app.db)
        .await?
        .into_iter()
        .map(|s| s.link_id)
        .collect();
    links.retain(|l| !skipped.contains(&l.id));
    let pairs: Vec<(Uuid, Uuid)> = links.iter().map(|l| (l.id, l.child_list_id)).collect();
    detours_for(app, user_id, acl, &pairs).await
}

/// A reading list as the caller sees it: name, their view, progress and
/// where to start. Used for event detours and for a collection's list
/// entries.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ReadingListCardView {
    pub list_id: String,
    pub name: String,
    /// The caller's view of the list (`None` = they have none yet).
    pub view_id: Option<String>,
    pub total: i64,
    pub in_library: i64,
    pub read: i64,
    /// The first unfinished issue the caller can open, in list order.
    pub first_unread: Option<LinkIssueRefView>,
}

/// Cards for the lists in `list_ids` the caller can see (own or shared);
/// others are absent. A fixed number of statements.
pub(crate) async fn list_cards(
    app: &AppState,
    user_id: Uuid,
    acl: &VisibleLibraries,
    list_ids: &[Uuid],
) -> Result<HashMap<Uuid, ReadingListCardView>, sea_orm::DbErr> {
    if list_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let lists: Vec<cbl_list::Model> = cbl_list::Entity::find()
        .filter(cbl_list::Column::Id.is_in(list_ids.to_vec()))
        .all(&app.db)
        .await?
        .into_iter()
        .filter(|c| visible(c, user_id))
        .collect();
    let ids: Vec<Uuid> = lists.iter().map(|l| l.id).collect();
    let views = caller_views(app, user_id, &ids).await?;
    let rows = entry_stats(app, acl, user_id, &ids).await?;
    Ok(lists
        .into_iter()
        .map(|l| {
            let st = fold_stats(rows.iter().filter(|r| r.cbl_list_id == l.id));
            (
                l.id,
                ReadingListCardView {
                    list_id: l.id.to_string(),
                    name: l.parsed_name,
                    view_id: views.get(&l.id).map(Uuid::to_string),
                    total: st.total,
                    in_library: st.in_library,
                    read: st.read,
                    first_unread: st.first_unread,
                },
            )
        })
        .collect())
}

/// Detours for `(id, list)` pairs in order: lists the caller can see and
/// has an unread issue in. `id` is the event link's (or collection
/// entry's) id.
pub(crate) async fn detours_for(
    app: &AppState,
    user_id: Uuid,
    acl: &VisibleLibraries,
    pairs: &[(Uuid, Uuid)],
) -> Result<Vec<NextUpDetour>, sea_orm::DbErr> {
    let ids: Vec<Uuid> = pairs.iter().map(|(_, l)| *l).collect();
    let cards = list_cards(app, user_id, acl, &ids).await?;
    Ok(pairs
        .iter()
        .filter_map(|(id, list)| {
            let c = cards.get(list)?;
            let first_unread = c.first_unread.clone()?;
            Some(NextUpDetour {
                link_id: id.to_string(),
                list_id: c.list_id.clone(),
                name: c.name.clone(),
                view_id: c.view_id.clone(),
                total: c.total,
                in_library: c.in_library,
                read: c.read,
                first_unread,
            })
        })
        .collect())
}

/// Where Up Next goes when `list_id` has nothing unread left after the
/// current issue: its next part, else (for an event) back to a parent.
pub(crate) struct LinkedNext {
    pub list_id: Uuid,
    pub issue: issue::Model,
    pub series_slug: String,
    pub series_name: String,
    pub position: i32,
    pub switch: NextUpListSwitch,
    pub detours: Vec<NextUpDetour>,
}

/// What [`after_list_end`] found.
pub(crate) enum AfterListEnd {
    /// Reading goes on in another list.
    Next(Box<LinkedNext>),
    /// The list was an event and its parent has nothing left to read
    /// (nor do the parts after it): the reading order ends here. The
    /// switch names the parent to go back to, with no issue to open.
    BackTo(NextUpListSwitch),
    /// The list is not linked to anything the reader would go on to.
    Nothing,
}

/// How many `continues` hops Up Next follows past parts with nothing to
/// read. A bound on top of the seen-set; real chains are tens of parts.
const MAX_CONTINUES_HOPS: usize = 64;

type CblPick = (issue::Model, String, String, i32);

/// The first part after `list_id` (following `continues`, past parts
/// that are finished, empty or not in the library) with something to
/// read, and that pick.
async fn next_part_with_unread(
    app: &AppState,
    user_id: Uuid,
    acl: &VisibleLibraries,
    list_id: Uuid,
) -> Result<Option<(cbl_list::Model, CblPick)>, axum::response::Response> {
    let db_err = |e: sea_orm::DbErr| {
        tracing::error!(error = %e, "next_up: next part lookup failed");
        super::cbl_links::internal()
    };
    let mut seen: HashSet<Uuid> = HashSet::from([list_id]);
    let mut at = list_id;
    for _ in 0..MAX_CONTINUES_HOPS {
        let Some(link) = reading_list_link::Entity::find()
            .filter(reading_list_link::Column::ParentListId.eq(at))
            .filter(reading_list_link::Column::Kind.eq(LinkKind::Continues))
            .one(&app.db)
            .await
            .map_err(db_err)?
        else {
            return Ok(None);
        };
        // `find_also_related` would follow the parent relation; the child
        // is the next part.
        let Some(part) = cbl_list::Entity::find_by_id(link.child_list_id)
            .one(&app.db)
            .await
            .map_err(db_err)?
        else {
            return Ok(None);
        };
        if !visible(&part, user_id) || !seen.insert(part.id) {
            return Ok(None);
        }
        if let Some(pick) =
            super::next_up::pick_next_in_cbl(app, user_id, part.id, acl, None).await?
        {
            return Ok(Some((part, pick)));
        }
        at = part.id;
    }
    Ok(None)
}

/// `from_list` is the parent the reader came from (`?via=`, resolved to
/// a list id). The bool is `true` when `from_list` was given but isn't a
/// parent of `list_id` the caller can see (a stale param).
///
/// Order: the list's own next part; else, for an event, back into a
/// parent after the event's place; else that parent's next part; else
/// [`AfterListEnd::BackTo`] the parent. A parent is eligible when it is
/// the one the reader came from, or one they have read something in
/// (finishing an event opened on its own doesn't jump into a master the
/// reader never opened), and only while the event's anchor is placed.
pub(crate) async fn after_list_end(
    app: &AppState,
    user_id: Uuid,
    acl: &VisibleLibraries,
    list_id: Uuid,
    from_list: Option<Uuid>,
) -> Result<(AfterListEnd, bool), axum::response::Response> {
    let db_err = |e: sea_orm::DbErr| {
        tracing::error!(error = %e, "next_up: linked lists lookup failed");
        super::cbl_links::internal()
    };
    let continues = |part: cbl_list::Model, pick: CblPick, view: Option<String>| LinkedNext {
        list_id: part.id,
        issue: pick.0,
        series_slug: pick.1,
        series_name: pick.2,
        position: pick.3,
        switch: NextUpListSwitch {
            reason: NextUpSwitchReason::Continues,
            list_id: Some(part.id.to_string()),
            collection: false,
            name: part.parsed_name,
            view_id: view,
        },
        detours: Vec::new(),
    };

    // The next part.
    if let Some((part, pick)) = next_part_with_unread(app, user_id, acl, list_id).await? {
        let view = caller_views(app, user_id, &[part.id])
            .await
            .map_err(db_err)?;
        let view_id = view.get(&part.id).map(Uuid::to_string);
        return Ok((
            AfterListEnd::Next(Box::new(continues(part, pick, view_id))),
            false,
        ));
    }

    // Back to a parent this list is an event in.
    let from = from_list.map(|f| f.to_string());
    let all = parents_with_status(app, user_id, list_id)
        .await
        .map_err(db_err)?;
    let stale = from
        .as_ref()
        .is_some_and(|f| !all.iter().any(|(p, _)| &p.list_id == f));
    let mut parents: Vec<ReadingListParentView> = all
        .into_iter()
        .filter(|(p, status)| {
            p.kind == LinkKind::Event
                && p.anchor_position.is_some()
                && *status == AnchorStatus::Placed
                && (from.as_ref() == Some(&p.list_id) || p.last_read_at.is_some())
        })
        .map(|(p, _)| p)
        .collect();
    if let Some(f) = &from
        && let Some(i) = parents.iter().position(|p| &p.list_id == f)
    {
        let p = parents.remove(i);
        parents.insert(0, p);
    }
    let home = parents.first().cloned();
    for p in parents {
        let (Ok(parent_id), Some(anchor)) = (Uuid::parse_str(&p.list_id), p.anchor_position) else {
            continue;
        };
        if let Some((issue, series_slug, series_name, position)) =
            super::next_up::pick_next_in_cbl(app, user_id, parent_id, acl, Some(anchor)).await?
        {
            // Other events due between this one's place and that issue.
            let detours = detours_between(
                app,
                user_id,
                acl,
                parent_id,
                anchor,
                // `position` is the pick's 1-based badge.
                Some(position - 1),
                Some(list_id),
            )
            .await
            .map_err(db_err)?;
            return Ok((
                AfterListEnd::Next(Box::new(LinkedNext {
                    list_id: parent_id,
                    issue,
                    series_slug,
                    series_name,
                    position,
                    switch: NextUpListSwitch {
                        reason: NextUpSwitchReason::Return,
                        list_id: Some(p.list_id),
                        collection: false,
                        name: p.name,
                        view_id: p.view_id,
                    },
                    detours,
                })),
                stale,
            ));
        }
        // Nothing left in the parent after the event: its next part.
        if let Some((part, pick)) = next_part_with_unread(app, user_id, acl, parent_id).await? {
            let view = caller_views(app, user_id, &[part.id])
                .await
                .map_err(db_err)?;
            let view_id = view.get(&part.id).map(Uuid::to_string);
            let mut next = continues(part, pick, view_id);
            // Events still due at the end of the parent.
            next.detours =
                detours_between(app, user_id, acl, parent_id, anchor, None, Some(list_id))
                    .await
                    .map_err(db_err)?;
            return Ok((AfterListEnd::Next(Box::new(next)), stale));
        }
    }
    Ok((
        match home {
            Some(p) => AfterListEnd::BackTo(NextUpListSwitch {
                reason: NextUpSwitchReason::Return,
                list_id: Some(p.list_id),
                collection: false,
                name: p.name,
                view_id: p.view_id,
            }),
            None => AfterListEnd::Nothing,
        },
        stale,
    ))
}

/// A linked list as an OPDS feed links to it.
#[derive(Debug, Clone)]
pub(crate) struct FeedListLink {
    pub list_id: Uuid,
    pub name: String,
    /// For an event: the 0-based position and label of the issue it is
    /// read after.
    pub after: Option<(i32, String)>,
}

/// What a reading list's OPDS feed links to (PR 6): its events in
/// reading order, its next part, and the lists it is part of — only
/// lists the caller can see. A fixed number of statements.
#[derive(Debug, Default)]
pub(crate) struct FeedLinks {
    pub events: Vec<FeedListLink>,
    pub continues: Option<FeedListLink>,
    pub parents: Vec<FeedListLink>,
}

pub(crate) async fn feed_links(
    app: &AppState,
    user_id: Uuid,
    list_id: Uuid,
) -> Result<FeedLinks, sea_orm::DbErr> {
    let mut links = reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ParentListId.eq(list_id))
        .all(&app.db)
        .await?;
    let children: HashMap<Uuid, cbl_list::Model> = if links.is_empty() {
        HashMap::new()
    } else {
        cbl_list::Entity::find()
            .filter(
                cbl_list::Column::Id
                    .is_in(links.iter().map(|l| l.child_list_id).collect::<Vec<_>>()),
            )
            .all(&app.db)
            .await?
            .into_iter()
            .filter(|c| visible(c, user_id))
            .map(|c| (c.id, c))
            .collect()
    };
    links.sort_by_key(event_order);
    let mut out = FeedLinks::default();
    for l in links {
        let Some(child) = children.get(&l.child_list_id) else {
            continue;
        };
        let link = FeedListLink {
            list_id: child.id,
            name: child.parsed_name.clone(),
            after: match (l.kind, l.anchor_status, l.anchor_position_hint) {
                (LinkKind::Event, AnchorStatus::Placed, Some(p)) => Some((
                    p,
                    match (&l.anchor_series, &l.anchor_number) {
                        (Some(s), Some(n)) => format!("{s} #{n}"),
                        _ => format!("#{}", p + 1),
                    },
                )),
                _ => None,
            },
        };
        match l.kind {
            LinkKind::Event => out.events.push(link),
            LinkKind::Continues => out.continues = Some(link),
        }
    }
    out.parents = parents_of(app, user_id, list_id)
        .await?
        .into_iter()
        .filter_map(|p| {
            Some(FeedListLink {
                list_id: Uuid::parse_str(&p.list_id).ok()?,
                name: p.name,
                after: p.anchor_position.zip(p.anchor_label),
            })
        })
        .collect();
    Ok(out)
}

impl FeedListLink {
    /// "Event after #379 FF #8: Age of Ultron" / "Continues in: Part 2" /
    /// "Part of: Marvel Master Part #07".
    pub(crate) fn title(&self, role: &str) -> String {
        match &self.after {
            Some((p, label)) if role == "event" => {
                format!("Event after #{} {label}: {}", p + 1, self.name)
            }
            _ => match role {
                "continues" => format!("Continues in: {}", self.name),
                "parent" => format!("Part of: {}", self.name),
                _ => self.name.clone(),
            },
        }
    }
}

/// `GET /me/cbl-lists/{id}/export?inline_events=true`: the list's CBL
/// with each placed event's books written in after its anchor issue — a
/// "with events" file for readers that don't know about links. Events
/// the caller can't see, and links whose anchor is lost, are left out
/// (never guessed into place). Returns `None` when the stored XML does
/// not parse.
pub(crate) async fn inline_events_xml(
    app: &AppState,
    user_id: Uuid,
    list: &cbl_list::Model,
) -> Result<Option<String>, sea_orm::DbErr> {
    let Ok(mut master) = parsers::cbl::parse(list.raw_xml.as_bytes()) else {
        return Ok(None);
    };
    let mut links: Vec<reading_list_link::Model> = reading_list_link::Entity::find()
        .filter(reading_list_link::Column::ParentListId.eq(list.id))
        .filter(reading_list_link::Column::Kind.eq(LinkKind::Event))
        .filter(reading_list_link::Column::AnchorStatus.eq(AnchorStatus::Placed))
        .all(&app.db)
        .await?;
    links.retain(|l| l.anchor_position_hint.is_some());
    if links.is_empty() {
        return Ok(parsers::cbl::to_xml(&master).ok());
    }
    let children: HashMap<Uuid, cbl_list::Model> = cbl_list::Entity::find()
        .filter(
            cbl_list::Column::Id.is_in(links.iter().map(|l| l.child_list_id).collect::<Vec<_>>()),
        )
        .all(&app.db)
        .await?
        .into_iter()
        .filter(|c| visible(c, user_id))
        .map(|c| (c.id, c))
        .collect();
    links.sort_by_key(event_order);
    let mut after: HashMap<usize, Vec<parsers::cbl::ParsedCblBook>> = HashMap::new();
    for l in &links {
        let Some(child) = children.get(&l.child_list_id) else {
            continue;
        };
        let Ok(parsed) = parsers::cbl::parse(child.raw_xml.as_bytes()) else {
            continue;
        };
        let at = usize::try_from(l.anchor_position_hint.unwrap_or_default()).unwrap_or(0);
        after.entry(at).or_default().extend(parsed.books);
    }
    let books = std::mem::take(&mut master.books);
    for (i, b) in books.into_iter().enumerate() {
        master.books.push(b);
        if let Some(extra) = after.remove(&i) {
            master.books.extend(extra);
        }
    }
    Ok(parsers::cbl::to_xml(&master).ok())
}
