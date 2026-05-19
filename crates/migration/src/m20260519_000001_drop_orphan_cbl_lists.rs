//! One-shot cleanup: remove `cbl_lists` rows that no longer have a
//! `saved_view(kind='cbl', cbl_list_id=…)` wrapper.
//!
//! Background: deleting a CBL from `/settings/views` historically hit
//! only `DELETE /me/saved-views/{id}`, which dropped the wrapper row
//! but left the underlying `cbl_lists` row behind. Both the OPDS
//! `/opds/v1/lists` feed and the On Deck rail query `cbl_lists`
//! directly, so the deleted CBL kept surfacing. The user-scoped
//! delete handler now cascades through `cbl_lists`, but operators on
//! pre-fix deployments may already have orphans. This migration sweeps
//! them out once.
//!
//! Safe because `cbl_lists` is a thin import wrapper — entries cascade
//! via FK, and there is no other surface (OPDS, On Deck, web UI) that
//! a CBL without a saved-view wrapper is reachable through.

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
                DELETE FROM cbl_lists
                WHERE NOT EXISTS (
                    SELECT 1
                    FROM saved_views
                    WHERE saved_views.kind = 'cbl'
                      AND saved_views.cbl_list_id = cbl_lists.id
                );
                "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Data cleanup — no reversible action.
        Ok(())
    }
}
