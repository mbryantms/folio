use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "metadata_cache")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub provider: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub entity: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub external_id: String,
    /// Normalized `GenericMetadata` JSON payload.
    pub payload: Json,
    pub fetched_at: DateTimeWithTimeZone,
    /// Mapping-schema version the payload was serialized under. Compared
    /// against `cache::CACHE_SCHEMA_VERSION`; a mismatch is a cache miss
    /// so payloads written before a `GenericMetadata` mapping change are
    /// re-fetched instead of serving stale defaults.
    pub schema_version: i32,
    /// `ETag` the upstream attached to the cached response, sent back as
    /// `If-None-Match` on revalidation (WP-2.9). NULL when none was sent.
    pub etag: Option<String>,
    /// `Last-Modified` the upstream attached, sent back as
    /// `If-Modified-Since` on revalidation. Metron's detail endpoints
    /// use this form.
    pub last_modified: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
