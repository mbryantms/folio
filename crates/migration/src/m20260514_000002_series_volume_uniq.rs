//! Widen the series uniqueness index from `(library_id, normalized_name, year)`
//! to `(library_id, normalized_name, year, volume)`.
//!
//! The old constraint allowed only one series row per (library, name, year)
//! triple, which broke "two volumes published in overlapping years" — e.g.
//! `Wolverine & the X-Men (2011)` (Vol 1, ran 2011–2014) and `…(2014)`
//! (Vol 2, ran 2014–2015). Both folders carry filename V-tokens (`V2011`,
//! `V2014`) that the parser already extracts into `series.volume`, so
//! including volume in the unique index lets identity resolution treat
//! them as distinct without manual disambiguation.
//!
//! Postgres treats NULL as distinct in unique indexes by default. That's
//! fine: a row with volume=NULL and another with volume=2 will not
//! conflict, and the identity-resolver's NULL-safe `WHERE volume IS NULL`
//! filter handles the lookup side.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                DROP INDEX IF EXISTS series_library_normalized_uniq;
                CREATE UNIQUE INDEX series_library_normalized_uniq
                    ON series(library_id, normalized_name, year, volume);
                "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                DROP INDEX IF EXISTS series_library_normalized_uniq;
                CREATE UNIQUE INDEX series_library_normalized_uniq
                    ON series(library_id, normalized_name, year);
                "#,
            )
            .await?;
        Ok(())
    }
}
