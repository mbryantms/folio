//! Security remediation: DB-backed, single-use password-reset tokens.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum PasswordResetUses {
    Table,
    TokenId,
    UserId,
    TokenHash,
    ExpiresAt,
    ConsumedAt,
    CreatedAt,
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
                    .table(PasswordResetUses::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PasswordResetUses::TokenId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(PasswordResetUses::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(PasswordResetUses::TokenHash)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PasswordResetUses::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PasswordResetUses::ConsumedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(PasswordResetUses::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(PasswordResetUses::Table, PasswordResetUses::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("password_reset_uses_user_id_idx")
                    .table(PasswordResetUses::Table)
                    .col(PasswordResetUses::UserId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("password_reset_uses_token_hash_uniq")
                    .table(PasswordResetUses::Table)
                    .col(PasswordResetUses::TokenHash)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS password_reset_uses_active_idx \
                 ON password_reset_uses (user_id, expires_at) WHERE consumed_at IS NULL",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS password_reset_uses_active_idx")
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("password_reset_uses_token_hash_uniq")
                    .table(PasswordResetUses::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("password_reset_uses_user_id_idx")
                    .table(PasswordResetUses::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(PasswordResetUses::Table).to_owned())
            .await
    }
}
