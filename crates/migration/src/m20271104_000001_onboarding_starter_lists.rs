//! Onboarding tour + starter reading lists (cover-stacks-and-onboarding-tour-1.0, PR C).
//!
//! - `users.onboarding_completed_at` — `NULL` until the account has finished
//!   or skipped the first-run tour. Existing accounts are backfilled to the
//!   migration time so only accounts created from now on see it.
//! - `starter_list_overrides` — the server admin's edits to the shipped
//!   starter-list manifest (`server::starter_lists::MANIFEST`): a row keyed
//!   by a shipped `(catalog_source_id, catalog_path)` with `hidden = true`
//!   hides it; a row for any other catalog file adds it (`title`, `reason`,
//!   `publisher` describe it).
//! - `starter_list_coverage` — how much of each starter list the library
//!   holds, computed by running the CBL matcher over the catalog file
//!   without importing it. `matched_issue_ids` is stored (not a count) so
//!   each caller's coverage is filtered by their own library grants.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE users ADD COLUMN IF NOT EXISTS onboarding_completed_at TIMESTAMPTZ",
        )
        .await?;
        db.execute_unprepared(
            "UPDATE users SET onboarding_completed_at = now() \
             WHERE onboarding_completed_at IS NULL",
        )
        .await?;
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS starter_list_overrides ( \
               id UUID PRIMARY KEY, \
               catalog_source_id UUID NOT NULL REFERENCES catalog_sources(id) ON DELETE CASCADE, \
               catalog_path TEXT NOT NULL, \
               hidden BOOLEAN NOT NULL DEFAULT false, \
               title TEXT, \
               publisher TEXT, \
               reason TEXT, \
               created_by UUID REFERENCES users(id) ON DELETE SET NULL, \
               created_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
               updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
               CONSTRAINT starter_list_overrides_key UNIQUE (catalog_source_id, catalog_path) \
             )",
        )
        .await?;
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS starter_list_coverage ( \
               catalog_source_id UUID NOT NULL REFERENCES catalog_sources(id) ON DELETE CASCADE, \
               catalog_path TEXT NOT NULL, \
               total INTEGER NOT NULL DEFAULT 0, \
               matched_issue_ids TEXT[] NOT NULL DEFAULT '{}', \
               error TEXT, \
               computed_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
               PRIMARY KEY (catalog_source_id, catalog_path) \
             )",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS starter_list_coverage")
            .await?;
        db.execute_unprepared("DROP TABLE IF EXISTS starter_list_overrides")
            .await?;
        db.execute_unprepared("ALTER TABLE users DROP COLUMN IF EXISTS onboarding_completed_at")
            .await?;
        Ok(())
    }
}
