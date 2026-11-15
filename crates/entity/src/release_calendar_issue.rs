//! A Metron issue with a store date in the release-calendar window
//! (migration `m20271118`).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "release_calendar_issue")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub metron_issue_id: i64,
    pub metron_series_id: i64,
    /// Issue number as Metron writes it (`"12"`, `"1.MU"`, `"Annual 1"`).
    pub number: String,
    pub store_date: Date,
    #[sea_orm(nullable)]
    pub cover_date: Option<Date>,
    /// Cover on Metron's CDN (`static.metron.cloud`, allowed by the CSP).
    #[sea_orm(nullable)]
    pub image_url: Option<String>,
    #[sea_orm(nullable)]
    pub cover_hash: Option<String>,
    /// The cover's average and most vibrant colours, twelve hex digits
    /// (`metadata::cover_tone`, migration `m20271122`). Read from the
    /// image by the calendar sweep.
    #[sea_orm(nullable)]
    pub cover_tone: Option<String>,
    /// The `image_url` `cover_tone` was read from; the colours count
    /// only while it still equals `image_url`.
    #[sea_orm(nullable)]
    pub cover_tone_url: Option<String>,
    /// Last attempt to read the cover's colours.
    #[sea_orm(nullable)]
    pub cover_tone_checked_at: Option<DateTimeWithTimeZone>,
    /// Metron's own `modified` stamp for the issue.
    #[sea_orm(nullable)]
    pub metron_modified: Option<DateTimeWithTimeZone>,
    pub first_seen_at: DateTimeWithTimeZone,
    /// Stamped by every list pass that returns the issue. A complete
    /// pass deletes window rows it did not stamp.
    pub last_seen_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::release_calendar_series::Entity",
        from = "Column::MetronSeriesId",
        to = "super::release_calendar_series::Column::MetronSeriesId",
        on_delete = "Cascade"
    )]
    Series,
}

impl Related<super::release_calendar_series::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Series.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
