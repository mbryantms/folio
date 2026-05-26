//! Per-field provenance. Generalizes the `issues.user_edited` JSON
//! array so junction-level provenance (e.g. "characters[] last set
//! by Metron") is tracked uniformly. The Apply jobs consult this on
//! every write to decide skip-vs-fill.
//!
//! `field` values come from the `MetadataField` enum's `key()` impl
//! that lands in M0b — never free-form strings at call sites.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "field_provenance")]
pub struct Model {
    /// `'series' | 'issue' | 'person' | 'character' | …` — snake-case
    /// match for the table name (same convention as
    /// [`super::external_id`]).
    #[sea_orm(primary_key, auto_increment = false)]
    pub entity_type: String,
    /// TEXT for the same reason `external_ids.entity_id` is.
    #[sea_orm(primary_key, auto_increment = false)]
    pub entity_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub field: String,
    /// `'user' | 'comicinfo' | 'metroninfo' | 'comicvine' | 'metron'
    /// | 'scanner_inference' | 'scanner_folder_tag'`. `'user'` rows
    /// are sacred — skipped on every Apply unless the caller passes
    /// `override_user_edits=true`.
    pub set_by: String,
    pub set_at: DateTimeWithTimeZone,
    /// Which provider record this came from (provider's external id
    /// for the entity). NULL for `set_by='user'` or scanner-local
    /// sources.
    #[sea_orm(nullable)]
    pub source_external_id: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
