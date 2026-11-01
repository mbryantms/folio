//! Reader display preferences that follow the account (audit 2026-11
//! §5 items 22 and 24, remediation F6 + F7).
//!
//! Brightness, sepia and "keep zoom between pages" lived only in the
//! browser's localStorage; the four new colour filters (grayscale, invert,
//! warmth, contrast) join them. All seven are account-wide — they are
//! about the viewer's screen and eyes, not one series — so they sit on
//! `users` next to the other reader defaults rather than on
//! `user_series_reader_prefs`.
//!
//! Every column is nullable: NULL means "never set on the account", which
//! lets the web store keep a browser's existing localStorage value (and
//! adopt it on the first change) instead of overwriting it with a default.
//! Ranges match the reader's sliders; the PATCH handler enforces the same
//! bounds for a clearer 422.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(DeriveIden)]
enum Users {
    Table,
    ReaderBrightness,
    ReaderSepia,
    ReaderGrayscale,
    ReaderInvert,
    ReaderWarmth,
    ReaderContrast,
    ReaderZoomPersist,
}

const CHECKS: &[(&str, &str)] = &[
    (
        "users_reader_brightness_range",
        "reader_brightness BETWEEN 0.5 AND 1.5",
    ),
    ("users_reader_sepia_range", "reader_sepia BETWEEN 0 AND 1"),
    (
        "users_reader_grayscale_range",
        "reader_grayscale BETWEEN 0 AND 100",
    ),
    (
        "users_reader_warmth_range",
        "reader_warmth BETWEEN -50 AND 50",
    ),
    (
        "users_reader_contrast_range",
        "reader_contrast BETWEEN 50 AND 150",
    ),
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(ColumnDef::new(Users::ReaderBrightness).double().null())
                    .add_column(ColumnDef::new(Users::ReaderSepia).double().null())
                    .add_column(ColumnDef::new(Users::ReaderGrayscale).integer().null())
                    .add_column(ColumnDef::new(Users::ReaderInvert).boolean().null())
                    .add_column(ColumnDef::new(Users::ReaderWarmth).integer().null())
                    .add_column(ColumnDef::new(Users::ReaderContrast).integer().null())
                    .add_column(ColumnDef::new(Users::ReaderZoomPersist).boolean().null())
                    .to_owned(),
            )
            .await?;
        for (name, expr) in CHECKS {
            manager
                .get_connection()
                .execute_unprepared(&format!(
                    "ALTER TABLE users ADD CONSTRAINT {name} CHECK ({expr})"
                ))
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (name, _) in CHECKS {
            manager
                .get_connection()
                .execute_unprepared(&format!(
                    "ALTER TABLE users DROP CONSTRAINT IF EXISTS {name}"
                ))
                .await?;
        }
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::ReaderBrightness)
                    .drop_column(Users::ReaderSepia)
                    .drop_column(Users::ReaderGrayscale)
                    .drop_column(Users::ReaderInvert)
                    .drop_column(Users::ReaderWarmth)
                    .drop_column(Users::ReaderContrast)
                    .drop_column(Users::ReaderZoomPersist)
                    .to_owned(),
            )
            .await
    }
}
