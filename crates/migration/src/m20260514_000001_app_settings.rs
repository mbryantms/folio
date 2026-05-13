//! Runtime config M1: `app_setting` key/value table for admin-editable
//! server settings (SMTP, OIDC, auth mode, log level, scanner tuning…).
//!
//! Plan: `~/.claude/plans/runtime-config-admin-1.0.md`.
//!
//! Values are stored as JSONB. Secret rows (SMTP password, OIDC client
//! secret) store `{"ciphertext":"<base64>","nonce":"<base64>"}` sealed
//! with the AEAD key in `secrets/settings-encryption.key`.
//! Non-secret rows store the raw JSON value.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum AppSetting {
    Table,
    Key,
    Value,
    IsSecret,
    UpdatedAt,
    UpdatedBy,
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
                    .table(AppSetting::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AppSetting::Key)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AppSetting::Value).json_binary().not_null())
                    .col(
                        ColumnDef::new(AppSetting::IsSecret)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(AppSetting::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(AppSetting::UpdatedBy).uuid().null())
                    .foreign_key(
                        ForeignKey::create()
                            .from(AppSetting::Table, AppSetting::UpdatedBy)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AppSetting::Table).to_owned())
            .await
    }
}
