//! Observability split M5 — scan-all batch grouping.
//!
//! One row per "Scan all" action; the per-library [`super::scan_run`] rows it
//! enqueues carry `batch_id` pointing here. The admin Scan-all dashboard
//! aggregates live progress + a post-run roll-up across the batch's members.

use sea_orm::Iterable;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

// Storage contract (AR-7): `scan_batch.kind` / `scan_batch.state` stay `TEXT`
// holding exactly these strings (`state` is also pinned by the
// `scan_batch_state_chk` CHECK). `string_value` and the serde name are the
// same string, pinned by the tests below; an unknown stored string fails to
// load with a `DbErr`.

/// What triggered a batch. Today only the admin "Scan all" action.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    EnumIter,
    DeriveActiveEnum,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum ScanBatchKind {
    #[sea_orm(string_value = "scan_all")]
    ScanAll,
}

impl ScanBatchKind {
    /// The stored / wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ScanAll => "scan_all",
        }
    }

    /// The variant whose stored string is `s`, if any.
    pub fn parse(s: &str) -> Option<Self> {
        Self::iter().find(|v| v.as_str() == s)
    }
}

/// A batch's state, derived from its member runs as they finish: all
/// `complete` → `complete`, none → `failed`, a mix → `partial_failed`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    EnumIter,
    DeriveActiveEnum,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum ScanBatchState {
    #[sea_orm(string_value = "running")]
    Running,
    #[sea_orm(string_value = "complete")]
    Complete,
    #[sea_orm(string_value = "partial_failed")]
    PartialFailed,
    #[sea_orm(string_value = "failed")]
    Failed,
}

impl ScanBatchState {
    /// The stored / wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Complete => "complete",
            Self::PartialFailed => "partial_failed",
            Self::Failed => "failed",
        }
    }

    /// The variant whose stored string is `s`, if any.
    pub fn parse(s: &str) -> Option<Self> {
        Self::iter().find(|v| v.as_str() == s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "scan_batch")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// Trigger discriminator — today always `scan_all` (see [`ScanBatchKind`]).
    pub kind: ScanBatchKind,
    /// Who triggered it; `None` for system/cron runs. No FK (mirrors
    /// `audit_log.actor_id`).
    #[sea_orm(nullable)]
    pub actor_id: Option<Uuid>,
    /// Whether the batch was a force (content-verify) scan.
    pub force: bool,
    pub started_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub ended_at: Option<DateTimeWithTimeZone>,
    /// Number of libraries the batch enqueued.
    pub library_count: i32,
    /// `running` | `complete` | `partial_failed` | `failed`, derived as the
    /// member runs finish (see [`ScanBatchState`]).
    pub state: ScanBatchState,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::scan_run::Entity")]
    ScanRun,
}

impl Related<super::scan_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ScanRun.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enum_contract::assert_exact_strings;

    #[test]
    fn kind_round_trips_exact_db_strings() {
        assert_exact_strings(
            &[(ScanBatchKind::ScanAll, "scan_all")],
            ScanBatchKind::as_str,
            ScanBatchKind::parse,
            &["scan-all", "ScanAll", ""],
        );
    }

    #[test]
    fn state_round_trips_exact_db_strings() {
        assert_exact_strings(
            &[
                (ScanBatchState::Running, "running"),
                (ScanBatchState::Complete, "complete"),
                (ScanBatchState::PartialFailed, "partial_failed"),
                (ScanBatchState::Failed, "failed"),
            ],
            ScanBatchState::as_str,
            ScanBatchState::parse,
            &["queued", "cancelled", "partial", ""],
        );
    }
}
