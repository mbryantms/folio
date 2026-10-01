use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Authoritative per-(user, issue) reading-progress store. The spec's
/// original §9 plan to replace this with Automerge CRDT documents was
/// reconsidered and dropped on 2026-05-15 — the server-side reading-run
/// rule (`run` column below; `api::progress::upsert_for_run`) resolves
/// every multi-device conflict this table actually sees. See the
/// decision note at spec §9.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "progress_records")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub issue_id: String,
    pub last_page: i32,
    pub percent: f64,
    pub finished: bool,
    pub updated_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub device: Option<String>,
    /// Authoritative timestamp for when the user flipped this issue
    /// to `finished = TRUE`. Distinct from `updated_at`, which gets
    /// bumped on every per-page write. Set to `Some(now)` on the
    /// flip; cleared to `None` when the user un-finishes. Powers the
    /// reading-log event feed.
    #[sea_orm(nullable)]
    pub finished_at: Option<DateTimeWithTimeZone>,
    /// "This finish came from a catalog/sync write, not active
    /// reading." Flipped to true only by bulk-mark endpoints (when the
    /// caller passes `backfill: true`) and by sync clients writing
    /// historical progress. Per-issue reader writes always set
    /// `false`. Any "unread" write clears it back to `false`.
    ///
    /// Read-side effect: time-bound activity surfaces (reading log,
    /// heatmap, daily-pages stat, streak, Just Finished sort) filter
    /// `WHERE is_backfill = false`. Lifetime/cumulative metrics
    /// (total read count, completion %, On Deck, badges) ignore the
    /// flag — a backfilled issue is still read.
    pub is_backfill: bool,
    /// Reading-run counter. Run 0 is the first read; an explicit
    /// "start re-read" opens the next run at page 0. Within a run the
    /// server keeps the furthest page reported by implicit per-page
    /// writes, and a write tagged with an older run is ignored (a
    /// device left open on the previous read cannot regress the new
    /// one). Explicit mark-read / mark-unread writes bypass the floor.
    pub run: i32,
    /// Hex BLAKE3 of the page image at `last_page` when it was recorded
    /// (WP-6.2). Lets a rescan of a replaced archive keep the resume
    /// position on the same image; NULL falls back to the ordinal.
    #[sea_orm(nullable)]
    pub page_hash: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
