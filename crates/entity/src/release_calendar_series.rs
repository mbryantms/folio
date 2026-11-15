//! A Metron series with an issue in the release-calendar window
//! (migration `m20271118`). The list endpoint gives the name, volume and
//! start year; `publisher`, `series_type` and `cv_series_id` come from
//! one series-detail request made by the enrich sweep.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "release_calendar_series")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub metron_series_id: i64,
    pub name: String,
    #[sea_orm(nullable)]
    pub volume: Option<i32>,
    #[sea_orm(nullable)]
    pub year_began: Option<i32>,
    #[sea_orm(nullable)]
    pub publisher: Option<String>,
    #[sea_orm(nullable)]
    pub series_type: Option<String>,
    /// ComicVine volume id, the key Folio's `external_ids` and Mylar's
    /// watchlist share. Null until the detail fetch, and for a series
    /// Metron has not linked to ComicVine yet.
    #[sea_orm(nullable)]
    pub cv_series_id: Option<i64>,
    /// When the series detail was last fetched; null = never.
    #[sea_orm(nullable)]
    pub detail_fetched_at: Option<DateTimeWithTimeZone>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::release_calendar_issue::Entity")]
    Issues,
}

impl Related<super::release_calendar_issue::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Issues.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
