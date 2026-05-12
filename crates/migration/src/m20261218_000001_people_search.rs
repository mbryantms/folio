//! Global-search M4: trigram indexes on the credit junctions.
//!
//! `series_credits.person` and `issue_credits.person` hold raw creator
//! names. Without an index, the global-search people endpoint would
//! seq-scan both tables on every keystroke. We add a `pg_trgm` GIN
//! index on `person` for each so fuzzy substring + similarity queries
//! finish in <10ms even at hundreds of thousands of rows.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("CREATE EXTENSION IF NOT EXISTS pg_trgm")
            .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS series_credits_person_trgm \
             ON series_credits USING GIN (person gin_trgm_ops)",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS issue_credits_person_trgm \
             ON issue_credits USING GIN (person gin_trgm_ops)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS issue_credits_person_trgm")
            .await?;
        db.execute_unprepared("DROP INDEX IF EXISTS series_credits_person_trgm")
            .await?;
        Ok(())
    }
}
