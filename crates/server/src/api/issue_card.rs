//! Slim `issues` projection for list / card surfaces (WP-3.6, audit AR-1).
//!
//! The `issues` row is wide: `comic_info_raw` + `pages` JSON, `summary`,
//! ~20 legacy CSV credit/cast columns, … — well over a kilobyte per row
//! for real archives with long `<Pages>` blocks. Every list endpoint that
//! renders [`IssueSummaryView`] cards needs only the 14 columns below, so
//! those paths select this partial model
//! (`.into_partial_model::<IssueCardRow>()`) instead of hydrating
//! `issue::Model`. The rendered card is byte-for-byte identical to
//! [`IssueSummaryView::from_model`], which itself goes through
//! `IssueCardRow::from(model).into_summary_view(..)` — one mapping, so the
//! full-model and projected paths can't drift.
//!
//! Use `issue::Model` only when the handler genuinely reads a wide column
//! (detail views, metadata editors, the scanner).
//! `docs/dev/load-testing.md` records the plans this was measured against.

use std::collections::HashMap;

use entity::{issue, progress_record};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ColumnTrait, ConnectionTrait, DerivePartialModel, EntityTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

use crate::api::series::IssueSummaryView;

/// The 16 columns an issue card (and the ACL / next-up walks that pick
/// which cards to show) reads. `month` / `day` ride along for the
/// release-date sort cursor (post-audit T2.3).
#[derive(Clone, Debug, DerivePartialModel)]
#[sea_orm(entity = "issue::Entity")]
pub(crate) struct IssueCardRow {
    pub id: String,
    pub slug: String,
    pub series_id: Uuid,
    pub library_id: Uuid,
    pub title: Option<String>,
    pub number_raw: Option<String>,
    pub sort_number: Option<f64>,
    pub year: Option<i32>,
    pub month: Option<i32>,
    pub day: Option<i32>,
    pub page_count: Option<i32>,
    pub state: entity::issue::IssueState,
    pub special_type: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    /// WP-2.7 age-rating cap checks: the issue's own rating (its series'
    /// rating is checked as well).
    pub age_rating: Option<String>,
}

impl From<issue::Model> for IssueCardRow {
    fn from(m: issue::Model) -> Self {
        Self {
            id: m.id,
            slug: m.slug,
            series_id: m.series_id,
            library_id: m.library_id,
            title: m.title,
            number_raw: m.number_raw,
            sort_number: m.sort_number,
            year: m.year,
            month: m.month,
            day: m.day,
            page_count: m.page_count,
            state: m.state,
            special_type: m.special_type,
            created_at: m.created_at,
            updated_at: m.updated_at,
            age_rating: m.age_rating,
        }
    }
}

impl IssueCardRow {
    /// Build the card view — the single `issue → IssueSummaryView`
    /// mapping (`IssueSummaryView::from_model` delegates here).
    pub(crate) fn into_summary_view(self, series_slug: &str) -> IssueSummaryView {
        let cover_url = (self.state == entity::issue::IssueState::Active)
            .then(|| format!("/issues/{}/pages/0/thumb", self.id));
        IssueSummaryView {
            id: self.id,
            slug: self.slug,
            series_id: self.series_id.to_string(),
            series_slug: series_slug.to_owned(),
            series_name: None,
            title: self.title,
            number: self.number_raw,
            sort_number: self.sort_number,
            year: self.year,
            page_count: self.page_count,
            state: self.state,
            cover_url,
            special_type: self.special_type,
            created_at: self.created_at.to_rfc3339(),
            updated_at: self.updated_at.to_rfc3339(),
            progress: None,
        }
    }
}

/// The caller's own reading progress on one issue card
/// (`IssueSummaryView.progress`): the read check and the progress bar a
/// card draws, without the client loading every progress record of the
/// user (`GET /progress`).
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct IssueCardProgress {
    pub finished: bool,
    /// Last viewed page index (0-based).
    pub page: i32,
    /// 0.0–1.0 fraction read.
    pub percent: f64,
    /// RFC 3339 time of the record's last write. Lets a client tell a
    /// card that is older than a write it made since.
    pub updated_at: String,
}

#[derive(DerivePartialModel)]
#[sea_orm(entity = "progress_record::Entity")]
struct CardProgressRow {
    issue_id: String,
    last_page: i32,
    percent: f64,
    finished: bool,
    updated_at: DateTimeWithTimeZone,
}

/// Fill `progress` on a page of issue cards with `user_id`'s own records:
/// one `progress_records` lookup on its `(user_id, issue_id)` primary key
/// for the whole page, whatever query built the cards. A card the user
/// has no record for gets `Some(None)` (serialized `null`), so the client
/// can tell "unread" from "not loaded".
///
/// Purely decorative: it never adds, drops or reorders cards. A failed
/// lookup is logged and leaves `progress` unset (the field is then absent
/// and clients fall back to `GET /progress`), rather than failing a page
/// that rendered fine before the field existed.
pub(crate) async fn attach_progress<'a, C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    cards: impl IntoIterator<Item = &'a mut IssueSummaryView>,
) {
    let mut cards: Vec<&'a mut IssueSummaryView> = cards.into_iter().collect();
    if cards.is_empty() {
        return;
    }
    let mut ids: Vec<String> = cards.iter().map(|c| c.id.clone()).collect();
    ids.sort_unstable();
    ids.dedup();
    let rows = match progress_record::Entity::find()
        .filter(progress_record::Column::UserId.eq(user_id))
        .filter(progress_record::Column::IssueId.is_in(ids))
        .into_partial_model::<CardProgressRow>()
        .all(db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "issue card progress lookup failed");
            return;
        }
    };
    let by_issue: HashMap<String, IssueCardProgress> = rows
        .into_iter()
        .map(|r| {
            (
                r.issue_id,
                IssueCardProgress {
                    finished: r.finished,
                    page: r.last_page,
                    percent: r.percent,
                    updated_at: r.updated_at.to_rfc3339(),
                },
            )
        })
        .collect();
    for card in &mut cards {
        card.progress = Some(by_issue.get(&card.id).cloned());
    }
}

impl crate::api::next_up::WalkIssue for IssueCardRow {
    fn walk_id(&self) -> &str {
        &self.id
    }
    fn walk_series_id(&self) -> Uuid {
        self.series_id
    }
    fn walk_library_id(&self) -> Uuid {
        self.library_id
    }
    fn walk_age_rating(&self) -> Option<&str> {
        self.age_rating.as_deref()
    }
}

impl crate::library::access::IssueAclFields for IssueCardRow {
    fn acl_library_id(&self) -> Uuid {
        self.library_id
    }
    fn acl_series_id(&self) -> Uuid {
        self.series_id
    }
    fn acl_age_rating(&self) -> Option<&str> {
        self.age_rating.as_deref()
    }
}
