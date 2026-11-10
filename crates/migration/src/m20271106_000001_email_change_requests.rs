//! Confirmed email changes: one pending request per user.
//!
//! `PATCH /me/account` with a new address no longer writes `users.email`
//! when mail delivery is configured. It stores a row here and mails a
//! single-use link to the new address; the link's token embeds `token_id`
//! the way password-reset tokens embed theirs (`password_reset_uses`).
//! `user_id` is unique, so a new request replaces the previous one.

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
