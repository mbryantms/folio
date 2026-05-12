//! Markers + Collections M3 follow-up — idempotent rename of the M9
//! "Want to Read" filter template to "Unstarted".
//!
//! M1 (m20261215_000001_collections) added the rename UPDATE inline,
//! but DBs where M1 had already been applied at schema-only time
//! ended up stuck with both rows visible:
//!
//!   - the M9 filter template (`kind='filter_series'`, `user_id IS NULL`,
//!     name="Want to Read") — left over from the partial M1 apply,
//!   - the M3 auto-seeded user collection (`kind='collection'`,
//!     `system_key='want_to_read'`, name="Want to Read") — new.
//!
//! Both surfaced under /settings/views with the same name. This
//! migration renames the M9 template idempotently on those DBs and is
//! a no-op everywhere else.

use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "UPDATE saved_views SET \
                    name = 'Unstarted', \
                    description = 'Series in your library you haven''t started yet.' \
                 WHERE id = '00000000-0000-0000-0000-000000000004' AND name = 'Want to Read'",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "UPDATE saved_views SET \
                    name = 'Want to Read', \
                    description = 'Series sitting in your library that you haven''t started yet.' \
                 WHERE id = '00000000-0000-0000-0000-000000000004' AND name = 'Unstarted'",
            )
            .await?;
        Ok(())
    }
}
