//! Issue-level smart views (roadmap WP-5.4, audit R20 / UX-8).
//!
//! Admits a fifth `saved_views.kind`, `filter_issues`: the same filter-DSL
//! shape as `filter_series` (match mode + conditions + sort + limit, no CBL
//! list, no system key) compiled against `issues` instead of `series` by
//! `server::views::compile::compile_issues`.
//!
//! `down` deletes every `filter_issues` row (their pins cascade via
//! `fk_user_view_pins_view`) and restores the four-branch constraint from
//! `m20261215_000001_collections`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

const FILTER_BRANCH: &str = "match_mode IS NOT NULL AND conditions IS NOT NULL \
    AND sort_field IS NOT NULL AND sort_order IS NOT NULL AND result_limit IS NOT NULL \
    AND cbl_list_id IS NULL AND system_key IS NULL";

const OTHER_BRANCHES: &str = "(kind = 'cbl' AND match_mode IS NULL AND conditions IS NULL \
        AND sort_field IS NULL AND sort_order IS NULL AND result_limit IS NULL \
        AND cbl_list_id IS NOT NULL AND system_key IS NULL) \
    OR (kind = 'system' AND match_mode IS NULL AND conditions IS NULL \
        AND sort_field IS NULL AND sort_order IS NULL AND result_limit IS NULL \
        AND cbl_list_id IS NULL AND system_key IS NOT NULL AND user_id IS NULL) \
    OR (kind = 'collection' AND match_mode IS NULL AND conditions IS NULL \
        AND sort_field IS NULL AND sort_order IS NULL AND result_limit IS NULL \
        AND cbl_list_id IS NULL AND user_id IS NOT NULL)";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared(
            "ALTER TABLE saved_views DROP CONSTRAINT IF EXISTS saved_views_kind_chk",
        )
        .await?;
        conn.execute_unprepared(&format!(
            "ALTER TABLE saved_views ADD CONSTRAINT saved_views_kind_chk CHECK (\
                (kind IN ('filter_series', 'filter_issues') AND {FILTER_BRANCH}) \
                OR {OTHER_BRANCHES})"
        ))
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("DELETE FROM saved_views WHERE kind = 'filter_issues'")
            .await?;
        conn.execute_unprepared(
            "ALTER TABLE saved_views DROP CONSTRAINT IF EXISTS saved_views_kind_chk",
        )
        .await?;
        conn.execute_unprepared(&format!(
            "ALTER TABLE saved_views ADD CONSTRAINT saved_views_kind_chk CHECK (\
                (kind = 'filter_series' AND {FILTER_BRANCH}) \
                OR {OTHER_BRANCHES})"
        ))
        .await?;
        Ok(())
    }
}
