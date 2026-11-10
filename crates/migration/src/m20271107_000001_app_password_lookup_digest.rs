//! App-password lookup digest.
//!
//! `app_passwords.lookup_digest` is the SHA-256 of the plaintext token
//! (`auth::app_password::lookup_digest`). The extractor finds a presented
//! token's row with one indexed equality lookup on it and then runs the
//! argon2 check of that single row, instead of trying the token against
//! every active row. The digest is unkeyed on purpose: the token carries
//! 256 random bits, so its SHA-256 can't be inverted or guessed, and a
//! digest that doesn't depend on the pepper survives a pepper rotation.
//!
//! Rows written before this column existed stay NULL: their plaintext is
//! not recoverable, so no migration can fill them. The extractor stores a
//! legacy row's digest the first time its token verifies. The partial
//! unique index lets any number of legacy rows coexist while keeping two
//! rows from ever sharing a digest.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE app_passwords ADD COLUMN IF NOT EXISTS lookup_digest BYTEA",
        )
        .await?;
        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS app_passwords_lookup_digest_uniq \
             ON app_passwords (lookup_digest) WHERE lookup_digest IS NOT NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS app_passwords_lookup_digest_uniq")
            .await?;
        db.execute_unprepared("ALTER TABLE app_passwords DROP COLUMN IF EXISTS lookup_digest")
            .await?;
        Ok(())
    }
}
