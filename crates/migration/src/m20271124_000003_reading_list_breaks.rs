//! Chapter breaks in a reading list (linked reading lists, PR 3a).
//!
//! A break says "a new chapter starts at this issue". Like an event link it
//! names its issue by identity (ComicVine id, then series / number / volume
//! / year), never by `cbl_entries` id, because a refresh rewrites every
//! entry; `crate::cbl::links::reresolve_parent` re-resolves breaks in the
//! same pass as links. A break is not a link: no child list, no quota, no
//! per-reader skip.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS reading_list_breaks (
                id UUID PRIMARY KEY,
                list_id UUID NOT NULL REFERENCES cbl_lists(id) ON DELETE CASCADE,
                label TEXT NULL,
                anchor_cv_issue_id INTEGER NULL,
                anchor_series TEXT NULL,
                anchor_number TEXT NULL,
                anchor_volume TEXT NULL,
                anchor_year TEXT NULL,
                anchor_position_hint INTEGER NULL,
                anchor_status TEXT NOT NULL DEFAULT 'placed',
                created_by UUID NULL REFERENCES users(id) ON DELETE SET NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                CONSTRAINT reading_list_breaks_anchor_status_chk
                    CHECK (anchor_status IN ('placed', 'lost')),
                CONSTRAINT reading_list_breaks_anchor_chk CHECK (
                    anchor_cv_issue_id IS NOT NULL
                    OR (anchor_series IS NOT NULL AND anchor_number IS NOT NULL)
                ),
                CONSTRAINT reading_list_breaks_label_chk
                    CHECK (label IS NULL OR char_length(label) BETWEEN 1 AND 120)
            )",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS reading_list_breaks_list_idx \
             ON reading_list_breaks (list_id)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS reading_list_breaks")
            .await?;
        Ok(())
    }
}
