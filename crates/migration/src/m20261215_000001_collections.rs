//! Markers + Collections M1 — `kind = 'collection'` discriminator on
//! `saved_views` plus the backing `collection_entries` join table.
//!
//! Collections are user-owned ordered lists of *mixed* series and issue
//! refs (one entry references exactly one of the two, never both). The
//! Want to Read list is a per-user collection with the fixed
//! `system_key = 'want_to_read'`; it's seeded lazily on first GET, not
//! at migration time, because we can't enumerate users here without
//! racing concurrent registrations.
//!
//! Schema deltas:
//!
//!   1. Relax `saved_views_kind_chk` to admit a `collection` branch
//!      with all filter columns + `cbl_list_id` NULL and `user_id` NOT
//!      NULL (collections are always user-owned).
//!   2. Replace the global `saved_views_system_key_uniq` index with two
//!      partial uniques so per-user `system_key` rows (Want to Read)
//!      coexist with global ones (`continue_reading`, `on_deck`):
//!      one unique on `system_key` where `user_id IS NULL`, one unique
//!      on `(user_id, system_key)` where `user_id IS NOT NULL`.
//!   3. Rename the M9 built-in filter template "Want to Read" →
//!      "Unstarted". The filter selects `read_progress = 0`, i.e.
//!      *library-fresh series*; the manual "Want to Read" name is freed
//!      for the per-user collection landing in M3. Idempotent UPDATE
//!      keyed on the fixed UUID so no-op on already-renamed DBs.

use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum CollectionEntries {
    Table,
    Id,
    SavedViewId,
    Position,
    EntryKind,
    SeriesId,
    IssueId,
    AddedAt,
}

#[derive(Iden)]
enum SavedViews {
    Table,
    Id,
}

#[derive(Iden)]
enum Series {
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
        let conn = manager.get_connection();

        // 1. Relax the kind check to admit `collection`. Collections
        //    leave every discriminator column NULL except `system_key`
        //    (NULL for normal collections, `'want_to_read'` for the
        //    seeded per-user list) and require `user_id` populated.
        conn.execute_unprepared(
            "ALTER TABLE saved_views DROP CONSTRAINT IF EXISTS saved_views_kind_chk",
        )
        .await?;
        conn.execute_unprepared(
            "ALTER TABLE saved_views ADD CONSTRAINT saved_views_kind_chk CHECK (\
                (kind = 'filter_series' AND match_mode IS NOT NULL AND conditions IS NOT NULL \
                    AND sort_field IS NOT NULL AND sort_order IS NOT NULL AND result_limit IS NOT NULL \
                    AND cbl_list_id IS NULL AND system_key IS NULL) \
                OR (kind = 'cbl' AND match_mode IS NULL AND conditions IS NULL \
                    AND sort_field IS NULL AND sort_order IS NULL AND result_limit IS NULL \
                    AND cbl_list_id IS NOT NULL AND system_key IS NULL) \
                OR (kind = 'system' AND match_mode IS NULL AND conditions IS NULL \
                    AND sort_field IS NULL AND sort_order IS NULL AND result_limit IS NULL \
                    AND cbl_list_id IS NULL AND system_key IS NOT NULL AND user_id IS NULL) \
                OR (kind = 'collection' AND match_mode IS NULL AND conditions IS NULL \
                    AND sort_field IS NULL AND sort_order IS NULL AND result_limit IS NULL \
                    AND cbl_list_id IS NULL AND user_id IS NOT NULL)\
             )",
        )
        .await?;

        // 2. Replace the global system_key unique with two partial
        //    uniques so global rails and per-user system-key collections
        //    don't conflict.
        conn.execute_unprepared("DROP INDEX IF EXISTS saved_views_system_key_uniq")
            .await?;
        conn.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS saved_views_global_system_key_uniq \
             ON saved_views(system_key) WHERE system_key IS NOT NULL AND user_id IS NULL",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS saved_views_user_system_key_uniq \
             ON saved_views(user_id, system_key) WHERE system_key IS NOT NULL AND user_id IS NOT NULL",
        )
        .await?;

        // 3. The collection_entries join table.
        manager
            .create_table(
                Table::create()
                    .table(CollectionEntries::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(CollectionEntries::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(CollectionEntries::SavedViewId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(CollectionEntries::Position)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(CollectionEntries::EntryKind)
                            .text()
                            .not_null(),
                    )
                    .col(ColumnDef::new(CollectionEntries::SeriesId).uuid().null())
                    .col(ColumnDef::new(CollectionEntries::IssueId).text().null())
                    .col(
                        ColumnDef::new(CollectionEntries::AddedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_collection_entries_view")
                            .from(CollectionEntries::Table, CollectionEntries::SavedViewId)
                            .to(SavedViews::Table, SavedViews::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_collection_entries_series")
                            .from(CollectionEntries::Table, CollectionEntries::SeriesId)
                            .to(Series::Table, Series::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_collection_entries_issue")
                            .from(CollectionEntries::Table, CollectionEntries::IssueId)
                            .to(Issues::Table, Issues::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // entry_kind discriminator + XOR on (series_id, issue_id).
        conn.execute_unprepared(
            "ALTER TABLE collection_entries ADD CONSTRAINT collection_entries_entry_kind_chk \
             CHECK (entry_kind IN ('series','issue'))",
        )
        .await?;
        conn.execute_unprepared(
            "ALTER TABLE collection_entries ADD CONSTRAINT collection_entries_ref_xor_chk CHECK (\
                (entry_kind = 'series' AND series_id IS NOT NULL AND issue_id IS NULL) \
             OR (entry_kind = 'issue'  AND issue_id IS NOT NULL AND series_id IS NULL)\
             )",
        )
        .await?;

        // Position uniqueness within a collection. INITIALLY DEFERRED so
        // a single tx can swap positions during reorder without juggling
        // temporary sentinel values.
        conn.execute_unprepared(
            "ALTER TABLE collection_entries ADD CONSTRAINT collection_entries_position_uniq \
             UNIQUE (saved_view_id, position) DEFERRABLE INITIALLY DEFERRED",
        )
        .await?;

        // Idempotent-add: each series/issue can appear at most once per
        // collection. Two partial uniques avoid COALESCEing across mixed
        // PK types (series_id UUID vs issue_id TEXT).
        conn.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS collection_entries_series_uniq \
             ON collection_entries(saved_view_id, series_id) WHERE series_id IS NOT NULL",
        )
        .await?;
        conn.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS collection_entries_issue_uniq \
             ON collection_entries(saved_view_id, issue_id) WHERE issue_id IS NOT NULL",
        )
        .await?;

        // Read path: list entries in position order for a given view.
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS collection_entries_view_pos_idx \
             ON collection_entries(saved_view_id, position)",
        )
        .await?;

        // 4. Free the "Want to Read" name. M9 seeded a filter template
        //    with that name; it's about library-fresh series, not a
        //    user wishlist. The manual collection landing in M3 owns
        //    the wishlist UX.
        conn.execute_unprepared(
            "UPDATE saved_views SET \
                name = 'Unstarted', \
                description = 'Series in your library you haven''t started yet.' \
             WHERE id = '00000000-0000-0000-0000-000000000004' AND name = 'Want to Read'",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();

        // Restore the M9 filter template name.
        conn.execute_unprepared(
            "UPDATE saved_views SET \
                name = 'Want to Read', \
                description = 'Series sitting in your library that you haven''t started yet.' \
             WHERE id = '00000000-0000-0000-0000-000000000004' AND name = 'Unstarted'",
        )
        .await?;

        // Tear down the join table first (CASCADE on the FK handles
        // dangling refs but we drop it outright).
        manager
            .drop_table(Table::drop().table(CollectionEntries::Table).to_owned())
            .await?;

        // Remove any user-data collection rows so the constraint flip
        // below succeeds even if Want to Read has been lazy-seeded.
        conn.execute_unprepared("DELETE FROM saved_views WHERE kind = 'collection'")
            .await?;

        conn.execute_unprepared("DROP INDEX IF EXISTS saved_views_user_system_key_uniq")
            .await?;
        conn.execute_unprepared("DROP INDEX IF EXISTS saved_views_global_system_key_uniq")
            .await?;
        conn.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS saved_views_system_key_uniq \
             ON saved_views(system_key) WHERE system_key IS NOT NULL",
        )
        .await?;

        conn.execute_unprepared(
            "ALTER TABLE saved_views DROP CONSTRAINT IF EXISTS saved_views_kind_chk",
        )
        .await?;
        conn.execute_unprepared(
            "ALTER TABLE saved_views ADD CONSTRAINT saved_views_kind_chk CHECK (\
                (kind = 'filter_series' AND match_mode IS NOT NULL AND conditions IS NOT NULL \
                    AND sort_field IS NOT NULL AND sort_order IS NOT NULL AND result_limit IS NOT NULL \
                    AND cbl_list_id IS NULL AND system_key IS NULL) \
                OR (kind = 'cbl' AND match_mode IS NULL AND conditions IS NULL \
                    AND sort_field IS NULL AND sort_order IS NULL AND result_limit IS NULL \
                    AND cbl_list_id IS NOT NULL AND system_key IS NULL) \
                OR (kind = 'system' AND match_mode IS NULL AND conditions IS NULL \
                    AND sort_field IS NULL AND sort_order IS NULL AND result_limit IS NULL \
                    AND cbl_list_id IS NULL AND system_key IS NOT NULL)\
             )",
        )
        .await?;

        Ok(())
    }
}
