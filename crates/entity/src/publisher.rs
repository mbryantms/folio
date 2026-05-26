//! Publisher — promoted from the `series.publisher` string by M0 of
//! metadata-providers-1.0. Mirrors [`super::person`]'s shape with
//! `founded_year` exposed by both CV and Metron.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "publisher")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(unique)]
    pub slug: String,
    pub name: String,
    #[sea_orm(unique)]
    pub normalized_name: String,
    pub aliases: Json,
    #[sea_orm(nullable)]
    pub description: Option<String>,
    #[sea_orm(nullable)]
    pub image_url: Option<String>,
    #[sea_orm(nullable)]
    pub founded_year: Option<i32>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
