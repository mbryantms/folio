use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "catalog_sources")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub display_name: String,
    pub github_owner: String,
    pub github_repo: String,
    pub github_branch: String,
    pub enabled: bool,
    #[sea_orm(nullable)]
    pub last_indexed_at: Option<DateTimeWithTimeZone>,
    /// HTTP `ETag` from the most recent tree fetch. Sent back as
    /// `If-None-Match` to short-circuit when the upstream tree hasn't
    /// changed.
    #[sea_orm(nullable)]
    pub index_etag: Option<String>,
    /// Cached parse of the GitHub tree, filtered to `*.cbl`. Shape:
    /// `{ entries: [{ path, name, publisher, sha, size }, ...] }`.
    #[sea_orm(nullable)]
    pub index_json: Option<Json>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
