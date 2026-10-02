//! Candidate series relationship proposed by the WP-7.2 suggestion engine.
//! See migration `m20270502_000001_relationship_suggestion` for the schema
//! rationale (canonical direction, append-only status transitions), and
//! `m20270506_000001_relationship_suggestion_scope` (WP-7.6) for arc targets
//! and scope.
//!
//! Written only by `server::relationships::suggestions` (the engine's
//! upsert and stale marking, plus `accept` / `reject` / `reopen`). Never
//! delete rows: a rejected row is what keeps the suggestion from coming
//! back.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "series_relationship_suggestion")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// Subject: "`from` `kind` `to`" (e.g. *from* is a sequel of *to*).
    pub from_series_id: Uuid,
    /// Series target. NULL for an arc target (exactly one of
    /// `to_series_id` / `to_arc_id` is set; WP-7.6).
    #[sea_orm(nullable)]
    pub to_series_id: Option<Uuid>,
    /// Story-arc target (`tie_in_to` only; WP-7.6 arc tie-in detector).
    #[sea_orm(nullable)]
    pub to_arc_id: Option<Uuid>,
    /// Canonical kind only (DB CHECK): one direction of each directional
    /// pair (`sequel_of`, `continues`, `collects`, …) or a self-inverse kind
    /// stored with `from < to` (WP-7.5, `m20270505`).
    pub kind: String,
    /// 0.0–1.0.
    pub confidence: f32,
    /// `high | medium | low`, derived from `confidence`.
    pub bucket: String,
    /// Human-readable explanation.
    pub reason: String,
    /// Structured evidence (spec §5.7): `{ "sources": [ { "source": …, … } ] }`.
    pub evidence: Json,
    /// `pending | accepted | rejected | modified | stale` (`stale`: no
    /// longer produced by the engine; WP-7.3, `m20270503`).
    pub status: String,
    /// Kind actually created when accepted with an override (`modified`).
    #[sea_orm(nullable)]
    pub accepted_kind: Option<String>,
    /// Proposed scope (WP-7.6), read from the `from` side like
    /// `series_relationship`: issue ranges, `coverage` (collects /
    /// reprints), and `qualifier` (continuation or tie-in role).
    #[sea_orm(nullable)]
    pub from_range: Option<String>,
    #[sea_orm(nullable)]
    pub to_range: Option<String>,
    #[sea_orm(nullable)]
    pub coverage: Option<String>,
    #[sea_orm(nullable)]
    pub qualifier: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub reviewed_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(nullable)]
    pub reviewed_by: Option<Uuid>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
