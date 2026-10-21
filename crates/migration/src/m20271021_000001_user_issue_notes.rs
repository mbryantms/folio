//! Per-user, issue-level closing note.
//!
//! Distinct from `markers` (which are page-anchored: bookmark / note /
//! favorite / highlight on a `page_index`). A closing note is what the
//! reader writes about the *issue* once they finish it — "what stuck with
//! you" — and has no page: the last page of an issue is often an ad or a
//! letters column, so pinning the note there would misfile it. Written
//! from the end-of-issue card's Wrap up pane and shown on the issue page
//! next to the user's rating.
//!
//! One row per `(user, issue)`. Cascades with both parents so an account
//! delete or an issue hard-purge takes the note with it, same as markers.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum UserIssueNotes {
    Table,
    UserId,
    IssueId,
    Body,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}

#[derive(Iden)]
enum Issues {
    Table,
    Id,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UserIssueNotes::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(UserIssueNotes::UserId).uuid().not_null())
                    .col(ColumnDef::new(UserIssueNotes::IssueId).text().not_null())
                    .col(ColumnDef::new(UserIssueNotes::Body).text().not_null())
                    .col(
                        ColumnDef::new(UserIssueNotes::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserIssueNotes::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(UserIssueNotes::UserId)
                            .col(UserIssueNotes::IssueId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_issue_notes_user")
                            .from(UserIssueNotes::Table, UserIssueNotes::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_issue_notes_issue")
                            .from(UserIssueNotes::Table, UserIssueNotes::IssueId)
                            .to(Issues::Table, Issues::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        // "Every note this user wrote, newest first" — the Bookmarks-page
        // and export read paths.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("user_issue_notes_by_user_updated_idx")
                    .table(UserIssueNotes::Table)
                    .col(UserIssueNotes::UserId)
                    .col(UserIssueNotes::UpdatedAt)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(UserIssueNotes::Table).to_owned())
            .await
    }
}
