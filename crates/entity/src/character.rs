//! Character — promoted from string-keyed junction rows by
//! M0 of metadata-providers-1.0. Mirrors [`super::person`]'s shape
//! with a couple of extras (`real_name`, `first_appearance_issue_id`)
//! the providers expose. Identity is shared across libraries.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "character")]
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
    pub real_name: Option<String>,
    /// Stable issue identity (BLAKE3 hex). Nullable because the
    /// provider data may name a first-appearance issue we don't have.
    #[sea_orm(nullable)]
    pub first_appearance_issue_id: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
