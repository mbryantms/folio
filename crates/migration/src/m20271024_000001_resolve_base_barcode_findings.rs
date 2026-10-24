//! Retire `DuplicateExternalId` findings raised for *base-only* barcodes.
//!
//! A comic's UPC-A / EAN-13 base (12 / 13 digits) names the title; only
//! the 5-digit add-on makes the 17 / 18-digit code unique per issue.
//! Tagging tools often store the base alone, so every issue of a run (or,
//! for 1970s codes, every issue cover-dated the same month) carries the
//! same value, and the scanner reported each one as a duplicate provider
//! id — 21 of the 35 open findings on the reference instance, 18 of them
//! for one series. `writers::is_base_only_barcode` now keeps such codes
//! out of the one-owner rule entirely, but the rows already written would
//! never close on their own: touch-on-skip keeps a finding alive while its
//! file's fingerprint is unchanged, which is forever for a healthy file.
//! This resolves them once. An ISBN-13 (978/979) under `gtin` is a book id
//! and is left alone, like every full-length code.
//!
//! `down` is a no-op: the rows stay resolved (nothing is lost — a rescan
//! that still found a real collision would re-open the row).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r"UPDATE library_health_issues
                     SET resolved_at = now()
                   WHERE kind = 'DuplicateExternalId'
                     AND resolved_at IS NULL
                     AND payload->'data'->>'source' IN ('upc', 'gtin')
                     AND length(regexp_replace(payload->'data'->>'external_id', '\D', '', 'g')) < 17
                     AND NOT (
                           length(regexp_replace(payload->'data'->>'external_id', '\D', '', 'g')) = 13
                       AND regexp_replace(payload->'data'->>'external_id', '\D', '', 'g') ~ '^97[89]'
                     )",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
