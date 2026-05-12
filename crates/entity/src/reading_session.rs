use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// M6a: per-user reading session.
///
/// Captures intentional reading (heartbeat upserts during a session, finalized
/// on close or by the dangling-session sweeper). Coexists with `progress_records`,
/// which remains the source of truth for "where do I resume" — sessions are the
/// source of truth for "how much have I read."
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "reading_sessions")]
pub struct Model {
    /// UUID v7 (server-assigned). Time-ordered, so descending scans serve the
    /// timeline directly off the index.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,

    pub user_id: Uuid,

    pub issue_id: String,

    /// Denormalized so series-scoped queries don't need to join through issues.
    pub series_id: Uuid,

    /// Client-generated UUID v4 used for idempotent upsert across the heartbeat
    /// + final-flush sequence. Length-capped at 64 chars.
    pub client_session_id: String,

    pub started_at: DateTimeWithTimeZone,

    /// `None` while active. The sweeper sets this to `last_heartbeat_at` once
    /// no heartbeats have arrived for ≥ 5 min.
    #[sea_orm(nullable)]
    pub ended_at: Option<DateTimeWithTimeZone>,

    pub last_heartbeat_at: DateTimeWithTimeZone,

    /// Accumulated *active* time — excludes idle gaps detected client-side.
    pub active_ms: i64,

    /// Distinct pages dwelled on for ≥ MIN_DWELL_MS_PER_PAGE.
    pub distinct_pages_read: i32,

    /// Total page-turn events including re-visits.
    pub page_turns: i32,

    /// Min visited page index (0-based).
    pub start_page: i32,

    /// Max visited page index (0-based).
    pub end_page: i32,

    /// Equal to end_page; explicit so admin "skipped to end" detection is a
    /// single column read.
    pub furthest_page: i32,

    #[sea_orm(nullable)]
    pub device: Option<String>,

    /// `'single' | 'double' | 'webtoon'` captured per-session.
    #[sea_orm(nullable)]
    pub view_mode: Option<String>,

    /// Free-form client metadata (client version, viewport, etc.).
    pub client_meta: Json,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id",
        on_delete = "Cascade"
    )]
    User,
    #[sea_orm(
        belongs_to = "super::issue::Entity",
        from = "Column::IssueId",
        to = "super::issue::Column::Id",
        on_delete = "Cascade"
    )]
    Issue,
    #[sea_orm(
        belongs_to = "super::series::Entity",
        from = "Column::SeriesId",
        to = "super::series::Column::Id",
        on_delete = "Cascade"
    )]
    Series,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}
impl Related<super::issue::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Issue.def()
    }
}
impl Related<super::series::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Series.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
