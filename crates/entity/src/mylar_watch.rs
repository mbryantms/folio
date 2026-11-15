//! One series on Mylar's watchlist (migration `m20271119`), keyed by the
//! ComicVine volume id Folio's `external_ids` also uses.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "mylar_watch")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub cv_series_id: i64,
    /// Mylar's own word: `Active`, `Paused`, …
    pub status: String,
    /// Mylar believes the series is still publishing.
    pub continuing: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
