//! Indexes for the page-proportional entity browse indexes (audit
//! 2026-11 P-01, roadmap C7).
//!
//! `GET /characters` / `/teams` / `/locations` / `/arcs` (and the OPDS
//! navigation feeds), `GET /creators` and `GET /people` used to aggregate
//! every visible junction row before picking one page, so each request
//! seq-scanned `issue_characters` (150,000 rows at 50,000 issues) or
//! `issue_credits` (100,000). They now walk the entity table (or the
//! distinct credit names) in list order and probe visibility per
//! candidate; `docs/dev/load-testing.md` records the before/after plans.
//!
//! - `character_name_order_idx`, `team_name_order_idx`,
//!   `location_name_order_idx`, `story_arc_name_order_idx` —
//!   `(lower(name), slug)`, the browse order and keyset. The page query
//!   reads the entity rows in this order and stops after `limit + 1`
//!   visible ones instead of probing and sorting the whole table.
//!   (`publisher` keeps its sort: a library has a few hundred publishers.)
//! - `issue_credits_person_idx`, `series_credits_person_idx` — `(person)`.
//!   The creator index and people search enumerate the distinct credit
//!   names with a skip scan (`min(person) WHERE person > $prev` on each
//!   table), and read one name's credits for its aggregate. The existing
//!   `(role, person)` and trigram indexes cannot return names in order.
//! - `issue_locations_norm_name_idx`, `series_locations_norm_name_idx` —
//!   `btrim(lower(location)) WHERE location_id IS NULL`, the name-fallback
//!   arm of a location's membership (`location_id IS NULL AND
//!   btrim(lower(location)) = …`, which implies the partial predicate).
//!   Partial, so a probe for a popular name never fetches and discards the
//!   FK-linked rows that share it. The character
//!   and team twins exist since `m20270305_000001_entity_pages`; locations
//!   were added later without them, and the per-entity probe would
//!   otherwise read the whole location junction for every listed entity.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

const INDEXES: &[(&str, &str)] = &[
    (
        "character_name_order_idx",
        "ON character (lower(name), slug)",
    ),
    ("team_name_order_idx", "ON team (lower(name), slug)"),
    ("location_name_order_idx", "ON location (lower(name), slug)"),
    (
        "story_arc_name_order_idx",
        "ON story_arc (lower(name), slug)",
    ),
    ("issue_credits_person_idx", "ON issue_credits (person)"),
    ("series_credits_person_idx", "ON series_credits (person)"),
    (
        "issue_locations_norm_name_idx",
        "ON issue_locations (btrim(lower(location))) WHERE location_id IS NULL",
    ),
    (
        "series_locations_norm_name_idx",
        "ON series_locations (btrim(lower(location))) WHERE location_id IS NULL",
    ),
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for (name, def) in INDEXES {
            db.execute_unprepared(&format!("CREATE INDEX IF NOT EXISTS {name} {def}"))
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for (name, _) in INDEXES.iter().rev() {
            db.execute_unprepared(&format!("DROP INDEX IF EXISTS {name}"))
                .await?;
        }
        Ok(())
    }
}
