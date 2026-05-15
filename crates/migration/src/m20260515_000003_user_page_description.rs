//! Multi-page rails follow-up — optional `description` on `user_page`.
//!
//! Pages already mirror saved views in most respects (name, slug,
//! pin set); a free-form description string fills the last remaining
//! gap so users can annotate what a page is for (e.g. "All horror —
//! Hickman's run + Locke & Key + IDW imports") right under the title.
//! Stored nullable so the field is optional; the UI hides the row
//! entirely when empty.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum UserPage {
    Table,
    Description,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(UserPage::Table)
                    .add_column(ColumnDef::new(UserPage::Description).text().null())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(UserPage::Table)
                    .drop_column(UserPage::Description)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
