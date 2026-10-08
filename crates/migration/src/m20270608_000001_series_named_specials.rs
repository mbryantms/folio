//! Superseded: originally the one-off repair clearing `issues.special_type`
//! where the series *name* carries its own marker ("X Annual"). `series.name`
//! comes from the first-scanned file's `<Series>`, which an annual under
//! `Batman (2016)/Annuals/` legitimately sets to "Batman Annual", so keying
//! on it could strip tags the scanner keeps. The repair now runs keyed on
//! the series folder in `m20270609_000001_series_named_specials_by_folder`.
//!
//! This id shipped in v0.48.1 and is already recorded as applied on
//! upgraded instances, so it stays registered and does nothing; instances
//! upgrading from earlier releases skip the name-keyed query entirely.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
