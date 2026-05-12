//! Read-only SeaORM wrapper over the `user_series_progress` SQL view.
//!
//! Defined by migration `m20261204_000001_user_series_progress_view`.
//! Coverage: `(user_id, series_id)` pairs where the user has at least one
//! `progress_records` row in that series. A user who hasn't started any
//! issue in a series is represented by the absence of a row — filter
//! queries `LEFT JOIN` this entity and `COALESCE(percent, 0)` to get
//! synthetic-zero behavior on unstarted series.
//!
//! Composite PK `(user_id, series_id)` mirrors the view's natural key;
//! Postgres views don't enforce uniqueness but the GROUP BY does.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "user_series_progress")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub series_id: Uuid,
    /// Active, non-removed issues in the series the user has marked
    /// `progress_records.finished = true`.
    pub finished_count: i64,
    /// Active, non-removed issues in the series. Independent of user.
    pub total_count: i64,
    /// `100 * finished_count / total_count` (integer, 0..100). `0` when
    /// `total_count = 0` to keep the field non-null.
    pub percent: i64,
    /// `MAX(reading_sessions.last_heartbeat_at)` for this `(user, series)`.
    /// `None` when the user has progress records but no reading sessions
    /// (e.g., a manual "mark as read" without a real reading session).
    #[sea_orm(nullable)]
    pub last_read_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id"
    )]
    User,
    #[sea_orm(
        belongs_to = "super::series::Entity",
        from = "Column::SeriesId",
        to = "super::series::Column::Id"
    )]
    Series,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}
impl Related<super::series::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Series.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
