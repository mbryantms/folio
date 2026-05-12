use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "scan_runs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub library_id: Uuid,
    /// `running` | `complete` | `failed` | `cancelled`
    pub state: String,
    pub started_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub ended_at: Option<DateTimeWithTimeZone>,
    pub stats: Json,
    #[sea_orm(nullable)]
    pub error: Option<String>,
    /// Discriminator: `'library' | 'series' | 'issue'`. Drives the History
    /// tab's filter chips; `'library'` is the legacy default for rows that
    /// existed before the migration.
    pub kind: String,
    /// The series whose folder was scanned. `None` for full-library scans.
    #[sea_orm(nullable)]
    pub series_id: Option<Uuid>,
    /// The issue that triggered the scan when `kind = 'issue'`. The scanner's
    /// unit of work is still the parent series folder; this records who
    /// clicked the button so the History view can link back.
    #[sea_orm(nullable)]
    pub issue_id: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::library::Entity",
        from = "Column::LibraryId",
        to = "super::library::Column::Id"
    )]
    Library,
}

impl Related<super::library::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Library.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
