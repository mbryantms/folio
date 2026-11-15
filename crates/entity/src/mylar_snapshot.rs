//! The latest Mylar snapshot's single header row (`id = 1`, migration
//! `m20271119`). `received_at` is Folio's clock; staleness is judged by
//! it, never by the sender's `generated_at`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// The only row's primary key.
pub const SINGLETON_ID: i16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "mylar_snapshot")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i16,
    pub received_at: DateTimeWithTimeZone,
    pub generated_at: DateTimeWithTimeZone,
    pub schema_version: i32,
    /// SHA-256 of the snapshot without its generation time. An equal hash
    /// is a heartbeat: only the two timestamps move.
    pub content_hash: String,
    #[sea_orm(nullable)]
    pub mylar_db_version: Option<i32>,
    pub annuals_included: bool,
    pub pull_count: i32,
    pub watch_count: i32,
    pub wanted_count: i32,
    /// `{counts: {status: n}, items: [...]}` — the download queue.
    pub queue: Json,
    /// Mylar's scheduler rows.
    pub jobs: Json,
    /// Recent snatch / post-process events.
    pub history: Json,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
