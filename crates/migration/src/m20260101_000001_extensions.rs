use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Postgres extensions enabled day one (§6.2).
        let conn = manager.get_connection();
        let backend = manager.get_database_backend();
        for sql in [
            "CREATE EXTENSION IF NOT EXISTS pg_trgm;",
            "CREATE EXTENSION IF NOT EXISTS unaccent;",
            "CREATE EXTENSION IF NOT EXISTS fuzzystrmatch;",
            // pgcrypto is needed by gen_random_uuid() if we ever use it; cheap and ubiquitous.
            "CREATE EXTENSION IF NOT EXISTS pgcrypto;",
        ] {
            conn.execute(sea_orm::Statement::from_string(backend, sql.to_string()))
                .await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Extensions are intentionally not dropped on `down` — they may be in use by other DBs.
        Ok(())
    }
}
