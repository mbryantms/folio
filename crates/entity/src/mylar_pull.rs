//! One line of Mylar's weekly pull list (migration `m20271119`).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "mylar_pull")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub ship_date: Date,
    #[sea_orm(nullable)]
    pub publisher: Option<String>,
    pub series_name: String,
    pub issue_number: String,
    /// Mylar's own word: `Skipped`, `Wanted`, `Snatched`, `Downloaded`, …
    pub status: String,
    /// Null when Mylar's pull service could not match the line.
    #[sea_orm(nullable)]
    pub cv_series_id: Option<i64>,
    #[sea_orm(nullable)]
    pub cv_issue_id: Option<i64>,
    #[sea_orm(nullable)]
    pub series_year: Option<i32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
