//! Confirmed email changes: one pending request per user.
//!
//! `PATCH /me/account` with a new address no longer writes `users.email`
//! when mail delivery is configured. It stores a row here and mails a
//! single-use link to the new address; the link's token embeds `token_id`
//! the way password-reset tokens embed theirs (`password_reset_uses`).
//! `user_id` is unique, so a new request replaces the previous one.
//!
//! `token_version` is the user's `users.token_version` when the request was
//! made; the link works only while it is unchanged, so a password change,
//! password reset, sign-out-everywhere or admin disable cancels it.
//! `window_started_at` + `request_count` bound how many requests (each
//! sends two emails) a user makes per hour; a replacing request carries
//! them over, and a cancelled request keeps its row so the count survives.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum EmailChangeRequests {
    Table,
    TokenId,
    UserId,
    NewEmail,
    TokenHash,
    ExpiresAt,
    ConsumedAt,
    CreatedAt,
    TokenVersion,
    WindowStartedAt,
    RequestCount,
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
                    .table(EmailChangeRequests::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(EmailChangeRequests::TokenId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(EmailChangeRequests::UserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EmailChangeRequests::NewEmail)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EmailChangeRequests::TokenHash)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EmailChangeRequests::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EmailChangeRequests::ConsumedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(EmailChangeRequests::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(EmailChangeRequests::TokenVersion)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EmailChangeRequests::WindowStartedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(EmailChangeRequests::RequestCount)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(EmailChangeRequests::Table, EmailChangeRequests::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("email_change_requests_user_id_uniq")
                    .table(EmailChangeRequests::Table)
                    .col(EmailChangeRequests::UserId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("email_change_requests_token_hash_uniq")
                    .table(EmailChangeRequests::Table)
                    .col(EmailChangeRequests::TokenHash)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("email_change_requests_token_hash_uniq")
                    .table(EmailChangeRequests::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("email_change_requests_user_id_uniq")
                    .table(EmailChangeRequests::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(EmailChangeRequests::Table).to_owned())
            .await
    }
}
