//! Issue-level user overrides.
//!
//! Two new columns on `issues`:
//!   - `additional_links jsonb` — array of `{label, url}` pairs the user
//!     attaches to the issue. Empty array by default. Stored separately
//!     from `web_url` (which mirrors ComicInfo's `Web` field) so a rescan
//!     can still refresh `web_url` without nuking user-curated links.
//!   - `user_edited jsonb` — array of column names that the user has
//!     explicitly overridden via `PATCH /issues/{id}`. The scanner
//!     consults this list on update and skips any field present, so user
//!     edits survive subsequent ComicInfo rescans (same "sticky" pattern
//!     as `series.match_key`).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Issues {
    Table,
    AdditionalLinks,
    UserEdited,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Issues::Table)
                    .add_column(
                        ColumnDef::new(Issues::AdditionalLinks)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'[]'::jsonb")),
                    )
                    .add_column(
                        ColumnDef::new(Issues::UserEdited)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'[]'::jsonb")),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Issues::Table)
                    .drop_column(Issues::AdditionalLinks)
                    .drop_column(Issues::UserEdited)
                    .to_owned(),
            )
            .await
    }
}
