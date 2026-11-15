//! The release-calendar list pass's single state row (`id = 1`,
//! migration `m20271118`): where a stopped pass resumes, when one last
//! finished, and the last failure with its backoff.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// The only row's primary key.
pub const SINGLETON_ID: i16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "release_calendar_state")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i16,
    /// Start of the pass in flight (or of the last one). Rows stamped
    /// before it were not returned by this pass.
    #[sea_orm(nullable)]
    pub pass_started_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(nullable)]
    pub pass_window_start: Option<Date>,
    #[sea_orm(nullable)]
    pub pass_window_end: Option<Date>,
    /// Monday of the next week to list; null when no pass is in flight.
    #[sea_orm(nullable)]
    pub pass_next_week: Option<Date>,
    /// When a pass last listed every week of its window.
    #[sea_orm(nullable)]
    pub last_finished_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(nullable)]
    pub last_error: Option<String>,
    #[sea_orm(nullable)]
    pub last_error_at: Option<DateTimeWithTimeZone>,
    /// After a failure: do not start or resume a pass before this time.
    #[sea_orm(nullable)]
    pub next_attempt_at: Option<DateTimeWithTimeZone>,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
