use sea_orm::Iterable;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

// Storage contract (AR-7): `scan_runs.state` / `scan_runs.kind` stay `TEXT`
// holding exactly these strings (no Postgres enum, no migration, no CHECK).
// `string_value` is the DB form and the serde name is the JSON form; the two
// are the same string, pinned by the tests below. A row holding any other
// string fails to load with a `DbErr` instead of being coerced.

/// A scan run's lifecycle state.
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
pub enum ScanRunState {
    /// Pre-inserted at enqueue time; the worker flips it to `running`.
    #[sea_orm(string_value = "queued")]
    Queued,
    #[sea_orm(string_value = "running")]
    Running,
    #[sea_orm(string_value = "complete")]
    Complete,
    #[sea_orm(string_value = "failed")]
    Failed,
    #[sea_orm(string_value = "cancelled")]
    Cancelled,
}

impl ScanRunState {
    /// The stored / wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// The variant whose stored string is `s`, if any.
    pub fn parse(s: &str) -> Option<Self> {
        Self::iter().find(|v| v.as_str() == s)
    }

    /// `complete` | `failed` | `cancelled`: the run will not change again.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Failed | Self::Cancelled)
    }
}

/// What triggered a scan run. `library` is the legacy default for rows
/// that existed before the kind column; `deep_validate` is the opt-in
/// page-decode probe (`library::deep_validate`), which the History tab's
/// filter chips do not offer.
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
pub enum ScanRunKind {
    #[sea_orm(string_value = "library")]
    Library,
    #[sea_orm(string_value = "series")]
    Series,
    #[sea_orm(string_value = "issue")]
    Issue,
    #[sea_orm(string_value = "deep_validate")]
    DeepValidate,
}

impl ScanRunKind {
    /// The stored / wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Series => "series",
            Self::Issue => "issue",
            Self::DeepValidate => "deep_validate",
        }
    }

    /// The variant whose stored string is `s`, if any.
    pub fn parse(s: &str) -> Option<Self> {
        Self::iter().find(|v| v.as_str() == s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "scan_runs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub library_id: Uuid,
    /// `queued` | `running` | `complete` | `failed` | `cancelled` (see
    /// [`ScanRunState`]).
    pub state: ScanRunState,
    pub started_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub ended_at: Option<DateTimeWithTimeZone>,
    pub stats: Json,
    #[sea_orm(nullable)]
    pub error: Option<String>,
    /// Discriminator: `'library' | 'series' | 'issue'`. Drives the History
    /// tab's filter chips; `'library'` is the legacy default for rows that
    /// existed before the migration. See [`ScanRunKind`].
    pub kind: ScanRunKind,
    /// The series whose folder was scanned. `None` for full-library scans.
    #[sea_orm(nullable)]
    pub series_id: Option<Uuid>,
    /// The issue that triggered the scan when `kind = 'issue'`. The scanner's
    /// unit of work is still the parent series folder; this records who
    /// clicked the button so the History view can link back.
    #[sea_orm(nullable)]
    pub issue_id: Option<String>,
    /// The scan-all batch this run belongs to (observability-split M5).
    /// `None` for single-library / series / issue scans.
    #[sea_orm(nullable)]
    pub batch_id: Option<Uuid>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::library::Entity",
        from = "Column::LibraryId",
        to = "super::library::Column::Id"
    )]
    Library,
    #[sea_orm(
        belongs_to = "super::scan_batch::Entity",
        from = "Column::BatchId",
        to = "super::scan_batch::Column::Id",
        on_delete = "SetNull"
    )]
    ScanBatch,
}

impl Related<super::library::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Library.def()
    }
}

impl Related<super::scan_batch::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ScanBatch.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enum_contract::assert_exact_strings;

    #[test]
    fn state_round_trips_exact_db_strings() {
        assert_exact_strings(
            &[
                (ScanRunState::Queued, "queued"),
                (ScanRunState::Running, "running"),
                (ScanRunState::Complete, "complete"),
                (ScanRunState::Failed, "failed"),
                (ScanRunState::Cancelled, "cancelled"),
            ],
            ScanRunState::as_str,
            ScanRunState::parse,
            &["completed", "Running", "canceled", ""],
        );
        assert!(ScanRunState::Cancelled.is_terminal());
        assert!(!ScanRunState::Queued.is_terminal());
        assert!(!ScanRunState::Running.is_terminal());
    }

    #[test]
    fn kind_round_trips_exact_db_strings() {
        assert_exact_strings(
            &[
                (ScanRunKind::Library, "library"),
                (ScanRunKind::Series, "series"),
                (ScanRunKind::Issue, "issue"),
                (ScanRunKind::DeepValidate, "deep_validate"),
            ],
            ScanRunKind::as_str,
            ScanRunKind::parse,
            &["all", "Library", "deep-validate", ""],
        );
    }
}
