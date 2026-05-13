//! OPDS readiness M7 — scoped app passwords.
//!
//! Adds `app_passwords.scope`, a per-token capability tag. Two values:
//!
//! - `read` — browse + page stream + download (the default; what every
//!   pre-M7 token implicitly was)
//! - `read+progress` — also lets the token write reading progress via
//!   `PUT /opds/v1/issues/{id}/progress` and the KOReader sync shim
//!
//! Default is `'read'` so existing tokens are correctly back-filled to
//! their pre-M7 capability without any application-level work.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum AppPasswords {
    Table,
    Scope,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AppPasswords::Table)
                    .add_column(
                        ColumnDef::new(AppPasswords::Scope)
                            .text()
                            .not_null()
                            .default("read"),
                    )
                    .to_owned(),
            )
            .await?;

        // Constrain to the known values. Adding via raw SQL because
        // sea-query's CHECK builder lands them as table-level
        // constraints with names sea-query auto-generates; the explicit
        // SQL gives us a stable name to drop in `down()`.
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE app_passwords \
                 ADD CONSTRAINT app_passwords_scope_chk \
                 CHECK (scope IN ('read', 'read+progress'))",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE app_passwords DROP CONSTRAINT IF EXISTS app_passwords_scope_chk",
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(AppPasswords::Table)
                    .drop_column(AppPasswords::Scope)
                    .to_owned(),
            )
            .await
    }
}
