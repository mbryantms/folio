//! Remove the retired PageCountMismatch health diagnostic.
//!
//! ComicInfo PageCount is not reliable enough to use as a health signal. The
//! scanner still stores the value as metadata, but no longer emits warnings
//! when it disagrees with archive image count.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                DELETE FROM library_health_issues
                WHERE kind = 'PageCountMismatch';
                "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
