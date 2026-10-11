//! Provider-filled profile of a character / team / location / story arc
//! (migration `m20270612`). The entity row keeps the short fields; this
//! row carries the landing page's wiki block and the lazy-fetch state.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "entity_profile")]
pub struct Model {
    /// `character` | `team` | `location` | `story_arc`.
    #[sea_orm(primary_key, auto_increment = false)]
    pub entity_type: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub entity_id: Uuid,
    /// Long-form summary (Metron's description, else ComicVine's deck).
    #[sea_orm(nullable)]
    pub summary: Option<String>,
    /// `[{heading, level, paragraphs: [..]}]` split out of ComicVine's
    /// wiki description.
    #[sea_orm(nullable)]
    pub sections: Option<Json>,
    /// Fact sheet — see `server::metadata::entity_profile::Facts`.
    #[sea_orm(nullable)]
    pub facts: Option<Json>,
    /// `{"metron": 9680, "comicvine": 25186}` — issues each provider
    /// lists for the entity.
    #[sea_orm(nullable)]
    pub provider_counts: Option<Json>,
    /// Providers that answered, e.g. `["metron", "comicvine"]`.
    #[sea_orm(nullable)]
    pub sources: Option<Json>,
    /// When a fetch was last queued (page view or Refresh).
    #[sea_orm(nullable)]
    pub requested_at: Option<DateTimeWithTimeZone>,
    /// When a fetch last completed successfully.
    #[sea_orm(nullable)]
    pub fetched_at: Option<DateTimeWithTimeZone>,
    /// Last failure, cleared by a successful fetch.
    #[sea_orm(nullable)]
    pub error: Option<String>,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
