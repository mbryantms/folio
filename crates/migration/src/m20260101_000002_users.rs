use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Users {
    Table,
    Id,
    ExternalId,
    DisplayName,
    Email,
    EmailVerified,
    PasswordHash,
    TotpSecret,
    State,
    Role,
    TokenVersion,
    CreatedAt,
    UpdatedAt,
    LastLoginAt,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Users::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Users::Id).uuid().not_null().primary_key())
                    .col(
                        ColumnDef::new(Users::ExternalId)
                            .text()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(Users::DisplayName).text().not_null())
                    .col(ColumnDef::new(Users::Email).text().null().unique_key())
                    .col(
                        ColumnDef::new(Users::EmailVerified)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(Users::PasswordHash).text().null())
                    .col(ColumnDef::new(Users::TotpSecret).text().null())
                    .col(
                        ColumnDef::new(Users::State)
                            .text()
                            .not_null()
                            .default("pending_verification"),
                    )
                    .col(
                        ColumnDef::new(Users::Role)
                            .text()
                            .not_null()
                            .default("user"),
                    )
                    .col(
                        ColumnDef::new(Users::TokenVersion)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Users::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Users::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Users::LastLoginAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // Email lookup is case-insensitive in practice; functional index for it.
        manager
            .get_connection()
            .execute(sea_orm::Statement::from_string(
                manager.get_database_backend(),
                "CREATE UNIQUE INDEX IF NOT EXISTS users_email_lower_idx \
                 ON users ((lower(email))) WHERE email IS NOT NULL"
                    .to_string(),
            ))
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Users::Table).to_owned())
            .await
    }
}
