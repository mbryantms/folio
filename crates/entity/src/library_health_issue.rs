//! Library Scanner v1 — health-issue rows (spec §10).
//!
//! Each row is one **distinct** problem the scanner has found in a library.
//! `(library_id, fingerprint)` is unique, so on subsequent scans the same
//! issue is upserted rather than duplicated; `last_seen_at` is bumped each
//! time it's re-detected. When a scan completes without re-emitting an open
//! issue, the reconcile pass sets `resolved_at`.
//!
//! `payload` carries the typed body of [`crate::library_health_issue::*`]
//! (the Rust enum lives in `crates/server/src/library/health.rs`); this
//! entity is intentionally JSON-blob-shaped so new variants can be added
//! without migrations.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "library_health_issues")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub library_id: Uuid,
    #[sea_orm(nullable)]
    pub scan_id: Option<Uuid>,
    /// Discriminant — matches `LibraryHealthIssue::kind()` in the server crate.
    pub kind: String,
    pub payload: Json,
    /// `error` | `warning` | `info`.
    pub severity: String,
    /// Stable hash of `(library_id, kind, identifying_path)` — drives the
    /// upsert behavior described above.
    pub fingerprint: String,
    pub first_seen_at: DateTimeWithTimeZone,
    pub last_seen_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub resolved_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(nullable)]
    pub dismissed_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::library::Entity",
        from = "Column::LibraryId",
        to = "super::library::Column::Id",
        on_delete = "Cascade"
    )]
    Library,
    #[sea_orm(
        belongs_to = "super::scan_run::Entity",
        from = "Column::ScanId",
        to = "super::scan_run::Column::Id",
        on_delete = "SetNull"
    )]
    ScanRun,
}

impl Related<super::library::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Library.def()
    }
}
impl Related<super::scan_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ScanRun.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
