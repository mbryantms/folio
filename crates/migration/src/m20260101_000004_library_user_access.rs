use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum LibraryUserAccess {
    Table,
    LibraryId,
    UserId,
    Role,
    AgeRatingMax,
    CreatedAt,
    UpdatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Note: foreign keys to libraries(id) are not added yet — libraries arrive in Phase 1a.
        // The shape is established now so the read-side ACL predicate can be added with the
        // first library queries without a schema migration.
        manager
            .create_table(
                Table::create()
                    .table(LibraryUserAccess::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(LibraryUserAccess::LibraryId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(LibraryUserAccess::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(LibraryUserAccess::Role)
                            .text()
                            .not_null()
                            .default("reader"),
                    )
                    .col(
                        ColumnDef::new(LibraryUserAccess::AgeRatingMax)
                            .text()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(LibraryUserAccess::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(LibraryUserAccess::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .primary_key(
                        Index::create()
                            .col(LibraryUserAccess::LibraryId)
                            .col(LibraryUserAccess::UserId),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("library_user_access_user_idx")
                    .table(LibraryUserAccess::Table)
                    .col(LibraryUserAccess::UserId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(LibraryUserAccess::Table).to_owned())
            .await
    }
}
