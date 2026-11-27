//! `issue_damaged_pages` — pages Folio kept from a damaged archive.
//!
//! When a CBR is converted to CBZ and one of its pages fails its checksum,
//! the converter keeps the bytes UnRAR decoded (or, when nothing usable
//! survives, writes a placeholder page) instead of refusing the whole
//! file. One row per such page records that, so the issue page, the reader
//! and the health list can say so, and the original archive (its `.bak`)
//! is kept past backup retention.
//!
//! The row is keyed by entry name and carries the CRC-32 of the bytes the
//! CBZ holds for it: the scanner keeps a row only while an entry with
//! those bytes is still in the file, so replacing the file (or the page)
//! clears it, while a metadata rewrite (pages copied byte for byte) or a
//! rename does not.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS issue_damaged_pages (
                    issue_id        text        NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                    entry_name      text        NOT NULL,
                    source_entry    text        NOT NULL,
                    page_index      integer     NOT NULL,
                    kind            text        NOT NULL
                        CHECK (kind IN ('salvaged', 'placeholder')),
                    reason          text        NOT NULL,
                    declared_size   bigint      NOT NULL,
                    recovered_size  bigint      NOT NULL,
                    entry_crc32     bigint      NOT NULL,
                    source_name     text        NOT NULL,
                    source_backup   text        NULL,
                    detected_at     timestamptz NOT NULL DEFAULT now(),
                    PRIMARY KEY (issue_id, entry_name)
                )",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS issue_damaged_pages")
            .await?;
        Ok(())
    }
}
