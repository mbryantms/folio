//! A reading list as a collection entry (linked reading lists, PR 5).
//!
//! `collection_entries.entry_kind = 'list'` points at a `cbl_lists` row.
//! A list entry is a **detour**, not inlined: the collection's reading
//! order (`collection_sequence`) skips it and offers it, at its place, as
//! an event to read before going on. Deleting the list deletes the entry.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE collection_entries
                ADD COLUMN IF NOT EXISTS cbl_list_id UUID NULL
                    REFERENCES cbl_lists(id) ON DELETE CASCADE;
             ALTER TABLE collection_entries
                DROP CONSTRAINT IF EXISTS collection_entries_entry_kind_chk,
                DROP CONSTRAINT IF EXISTS collection_entries_ref_xor_chk;
             ALTER TABLE collection_entries
                ADD CONSTRAINT collection_entries_entry_kind_chk
                    CHECK (entry_kind IN ('series', 'issue', 'list')),
                ADD CONSTRAINT collection_entries_ref_xor_chk CHECK (
                    (entry_kind = 'series' AND series_id IS NOT NULL
                        AND issue_id IS NULL AND cbl_list_id IS NULL)
                 OR (entry_kind = 'issue' AND issue_id IS NOT NULL
                        AND series_id IS NULL AND cbl_list_id IS NULL)
                 OR (entry_kind = 'list' AND cbl_list_id IS NOT NULL
                        AND series_id IS NULL AND issue_id IS NULL)
                );
             CREATE UNIQUE INDEX IF NOT EXISTS collection_entries_list_uniq
                ON collection_entries(saved_view_id, cbl_list_id)
                WHERE cbl_list_id IS NOT NULL;
             CREATE INDEX IF NOT EXISTS collection_entries_cbl_list_idx
                ON collection_entries(cbl_list_id)
                WHERE cbl_list_id IS NOT NULL;",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "DELETE FROM collection_entries WHERE entry_kind = 'list';
             DROP INDEX IF EXISTS collection_entries_cbl_list_idx;
             DROP INDEX IF EXISTS collection_entries_list_uniq;
             ALTER TABLE collection_entries
                DROP CONSTRAINT IF EXISTS collection_entries_entry_kind_chk,
                DROP CONSTRAINT IF EXISTS collection_entries_ref_xor_chk;
             ALTER TABLE collection_entries
                ADD CONSTRAINT collection_entries_entry_kind_chk
                    CHECK (entry_kind IN ('series', 'issue')),
                ADD CONSTRAINT collection_entries_ref_xor_chk CHECK (
                    (entry_kind = 'series' AND series_id IS NOT NULL AND issue_id IS NULL)
                 OR (entry_kind = 'issue' AND issue_id IS NOT NULL AND series_id IS NULL)
                );
             ALTER TABLE collection_entries DROP COLUMN IF EXISTS cbl_list_id;",
        )
        .await?;
        Ok(())
    }
}
