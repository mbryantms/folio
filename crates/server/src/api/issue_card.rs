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

use entity::issue;
use sea_orm::DerivePartialModel;
use sea_orm::prelude::DateTimeWithTimeZone;
use uuid::Uuid;

use crate::api::series::IssueSummaryView;

/// The 14 columns an issue card (and the ACL / next-up walks that pick
/// which cards to show) reads.
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
    pub page_count: Option<i32>,
    pub state: String,
    pub special_type: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    /// WP-2.7 age-rating cap checks (issue rating, series fallback).
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
        let cover_url =
            (self.state == "active").then(|| format!("/issues/{}/pages/0/thumb", self.id));
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
        }
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
