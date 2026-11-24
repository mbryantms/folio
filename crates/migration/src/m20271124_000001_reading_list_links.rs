//! Linked reading lists (plan `linked-reading-lists-1.0`, PR 1).
//!
//! A master reading list imported **without** events links each event
//! list at the issue it is read after (`kind = 'event'`), and links the
//! next part at its end (`kind = 'continues'`). Links join `cbl_lists`
//! rows, not saved views: several users' views can point at one shared
//! list, and a link belongs to the list's content, the same for everyone.
//!
//! The anchor is stored as the issue's identity, never a `cbl_entries`
//! id: a refresh deletes and rewrites every entry (`cbl::import`), so only
//! the ComicVine issue id and the (series, number, volume, year) key
//! survive. `anchor_status = 'lost'` marks a link whose issue a refresh
//! removed; the row is kept and drawn at `anchor_position_hint`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS reading_list_links (
                id UUID PRIMARY KEY,
                parent_list_id UUID NOT NULL REFERENCES cbl_lists(id) ON DELETE CASCADE,
                child_list_id UUID NOT NULL REFERENCES cbl_lists(id) ON DELETE CASCADE,
                kind TEXT NOT NULL,
                anchor_cv_issue_id INTEGER NULL,
                anchor_series TEXT NULL,
                anchor_number TEXT NULL,
                anchor_volume TEXT NULL,
                anchor_year TEXT NULL,
                anchor_position_hint INTEGER NULL,
                anchor_status TEXT NOT NULL DEFAULT 'placed',
                placement TEXT NOT NULL,
                sort_within_anchor INTEGER NOT NULL DEFAULT 0,
                created_by UUID NULL REFERENCES users(id) ON DELETE SET NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                CONSTRAINT reading_list_links_kind_chk
                    CHECK (kind IN ('event', 'continues')),
                CONSTRAINT reading_list_links_anchor_status_chk
                    CHECK (anchor_status IN ('placed', 'lost')),
                CONSTRAINT reading_list_links_placement_chk
                    CHECK (placement IN ('auto_twin', 'auto_chain', 'manual')),
                CONSTRAINT reading_list_links_not_self_chk
                    CHECK (parent_list_id <> child_list_id),
                CONSTRAINT reading_list_links_anchor_chk CHECK (
                    (kind = 'continues'
                        AND anchor_cv_issue_id IS NULL
                        AND anchor_series IS NULL
                        AND anchor_number IS NULL)
                    OR (kind = 'event'
                        AND (anchor_cv_issue_id IS NOT NULL
                             OR (anchor_series IS NOT NULL AND anchor_number IS NOT NULL)))
                ),
                CONSTRAINT reading_list_links_pair_uniq
                    UNIQUE (parent_list_id, child_list_id, kind)
            )",
        )
        .await?;
        // At most one "continues in" per list.
        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS reading_list_links_one_continues \
             ON reading_list_links (parent_list_id) WHERE kind = 'continues'",
        )
        .await?;
        // "Linked from N lists" and the child-side cascade.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS reading_list_links_child_idx \
             ON reading_list_links (child_list_id)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS reading_list_links")
            .await?;
        Ok(())
    }
}
