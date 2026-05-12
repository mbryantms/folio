//! Auth-hardening M7 / audit M-14: per-user application passwords for
//! Bearer-token clients (OPDS readers, scripts, future API consumers).
//!
//! Stored as argon2id-hashed secrets. The plaintext is shown once at
//! issue time, never persisted. Soft-deletion via `revoked_at` so audit
//! reports retain history.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum AppPasswords {
    Table,
    Id,
    UserId,
    Label,
    Hash,
    LastUsedAt,
    CreatedAt,
    RevokedAt,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AppPasswords::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AppPasswords::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AppPasswords::UserId).uuid().not_null())
                    .col(ColumnDef::new(AppPasswords::Label).text().not_null())
                    .col(ColumnDef::new(AppPasswords::Hash).text().not_null())
                    .col(
                        ColumnDef::new(AppPasswords::LastUsedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(AppPasswords::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(AppPasswords::RevokedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(AppPasswords::Table, AppPasswords::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("app_passwords_user_id_idx")
                    .table(AppPasswords::Table)
                    .col(AppPasswords::UserId)
                    .to_owned(),
            )
            .await?;

        // Partial index on active rows. Every Bearer auth attempt scans
        // active rows to find a matching argon2 hash; the index keeps the
        // hot path off the revoked-history rows.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS app_passwords_active_idx \
                 ON app_passwords (user_id) WHERE revoked_at IS NULL",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS app_passwords_active_idx")
            .await?;
        manager
            .drop_table(Table::drop().table(AppPasswords::Table).to_owned())
            .await
    }
}
