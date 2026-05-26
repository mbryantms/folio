//! Generic external-identifier storage. Replaces the fixed
//! `comicvine_id` / `metron_id` / `gtin` columns previously on
//! `series` and `issues`. Supports unlimited sources (CV, Metron,
//! GCD, Marvel, LoCG, MAL, AniList, MangaUpdates, ISBN, UPC, ASIN,
//! DOI, …) for every entity type (series, issue, person, character,
//! team, story_arc, location, concept, object, publisher, imprint,
//! universe).
//!
//! `entity_id` is TEXT so it natively accommodates both UUIDs (cast
//! via `::text`) and the BLAKE3-hex strings issue ids use. See the
//! M0 migration for the rationale.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "external_ids")]
pub struct Model {
    /// Snake-case match for the table name: `'series' | 'issue' |
    /// 'person' | 'character' | 'team' | 'story_arc' | 'location' |
    /// 'concept' | 'object' | 'publisher' | 'imprint' | 'universe'`.
    #[sea_orm(primary_key, auto_increment = false)]
    pub entity_type: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub entity_id: String,
    /// `'comicvine' | 'metron' | 'gcd' | 'marvel' | 'locg' | 'mal' |
    /// 'anilist' | 'mangaupdates' | 'isbn' | 'upc' | 'asin' | 'doi'`.
    #[sea_orm(primary_key, auto_increment = false)]
    pub source: String,
    pub external_id: String,
    /// Canonical link back to the source — satisfies the CV/Metron
    /// TOS attribution requirement when rendered in the UI.
    #[sea_orm(nullable)]
    pub external_url: Option<String>,
    /// `'user' | 'comicinfo' | 'metroninfo' | 'comicvine' | 'metron'
    /// | 'scanner_folder_tag' | 'cross_reference' | 'migration_v1'`.
    pub set_by: String,
    pub first_set_at: DateTimeWithTimeZone,
    pub last_synced_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
