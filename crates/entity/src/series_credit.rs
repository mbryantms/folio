use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "series_credits")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub series_id: Uuid,
    /// One of: `writer`, `penciller`, `inker`, `colorist`, `letterer`,
    /// `cover_artist`, `editor`, `translator`. Source of truth lives in the
    /// scanner's role enum (`scanner::metadata_rollup::CreditRole`).
    #[sea_orm(primary_key, auto_increment = false)]
    pub role: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub person: String,
    /// FK to `person.id`. Backfilled from `person` for existing rows
    /// (m20261225) and kept in sync by the scanner's series rollup.
    /// Nullable so freshly-scanned credits aren't blocked by a
    /// not-yet-populated `person` row; the rollup fills it in shortly
    /// after.
    pub person_id: Option<Uuid>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::series::Entity",
        from = "Column::SeriesId",
        to = "super::series::Column::Id",
        on_delete = "Cascade"
    )]
    Series,
    #[sea_orm(
        belongs_to = "super::person::Entity",
        from = "Column::PersonId",
        to = "super::person::Column::Id",
        on_delete = "SetNull"
    )]
    Person,
}

impl Related<super::series::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Series.def()
    }
}

impl Related<super::person::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Person.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
