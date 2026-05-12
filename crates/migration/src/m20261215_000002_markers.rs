//! Markers + Collections M4 — unified `markers` table.
//!
//! Single table backing the four marker sub-kinds. The `kind`
//! discriminator picks shape:
//!
//!   - `bookmark` — page pointer; `region` NULL (whole page) or
//!     optional rect for "remember this part".
//!   - `note` — markdown `body` required; `region` optional.
//!   - `favorite` — page or panel-level pointer; `region` optional.
//!   - `highlight` — `region` required (text/image-aware selection
//!     metadata in `selection`).
//!
//! `region` is `{x, y, w, h, shape}` where the rect dims are 0–100
//! percent floats normalized to the page's *natural* pixel dims (the
//! reader overlay re-anchors against `getBoundingClientRect`, so
//! resize/zoom/fit-mode never invalidates a stored region). `shape ∈
//! rect | text | image` flags the selection mode the client used.
//!
//! `selection` carries optional OCR text + cropped-pixel hash for
//! `shape='text'|'image'` highlights. M5 (Reader integration) populates
//! it client-side via tesseract.js + `crypto.subtle.digest`.

use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Markers {
    Table,
    Id,
    UserId,
    SeriesId,
    IssueId,
    PageIndex,
    Kind,
    Region,
    Selection,
    Body,
    Color,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum Users {
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
        manager
            .create_table(
                Table::create()
                    .table(Markers::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Markers::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Markers::UserId).uuid().not_null())
                    .col(ColumnDef::new(Markers::SeriesId).uuid().not_null())
                    .col(ColumnDef::new(Markers::IssueId).text().not_null())
                    .col(ColumnDef::new(Markers::PageIndex).integer().not_null())
                    .col(ColumnDef::new(Markers::Kind).text().not_null())
                    .col(ColumnDef::new(Markers::Region).json_binary().null())
                    .col(ColumnDef::new(Markers::Selection).json_binary().null())
                    .col(ColumnDef::new(Markers::Body).text().null())
                    .col(ColumnDef::new(Markers::Color).text().null())
                    .col(
                        ColumnDef::new(Markers::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Markers::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("markers_user_fk")
                            .from(Markers::Table, Markers::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("markers_series_fk")
                            .from(Markers::Table, Markers::SeriesId)
                            .to(Series::Table, Series::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("markers_issue_fk")
                            .from(Markers::Table, Markers::IssueId)
                            .to(Issues::Table, Issues::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        let conn = manager.get_connection();

        // Discriminator allow-list.
        conn.execute_unprepared(
            "ALTER TABLE markers ADD CONSTRAINT markers_kind_chk \
             CHECK (kind IN ('bookmark','note','favorite','highlight'))",
        )
        .await?;

        // Page index bound — non-negative; upper bound is per-issue and
        // enforced by handler-level validation against `issues.page_count`
        // (the schema can't enforce that cross-table check without a
        // generated column we don't want to maintain).
        conn.execute_unprepared(
            "ALTER TABLE markers ADD CONSTRAINT markers_page_index_chk \
             CHECK (page_index >= 0)",
        )
        .await?;

        // Per-kind invariants:
        //   - notes carry markdown `body` (required, ≤ 10 KB).
        //   - highlights anchor on a region rect (required).
        // Bookmarks + favorites leave both optional.
        conn.execute_unprepared(
            "ALTER TABLE markers ADD CONSTRAINT markers_body_required_for_note_chk \
             CHECK ((kind <> 'note') OR (body IS NOT NULL AND length(body) > 0))",
        )
        .await?;
        conn.execute_unprepared(
            "ALTER TABLE markers ADD CONSTRAINT markers_body_size_chk \
             CHECK (body IS NULL OR length(body) <= 10240)",
        )
        .await?;
        conn.execute_unprepared(
            "ALTER TABLE markers ADD CONSTRAINT markers_region_required_for_highlight_chk \
             CHECK ((kind <> 'highlight') OR region IS NOT NULL)",
        )
        .await?;

        // Per-page lookup for the reader overlay (fetch every marker on
        // the current page in one hit).
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS markers_user_issue_page_idx \
             ON markers(user_id, issue_id, page_index)",
        )
        .await?;
        // Global feed for the `/bookmarks` list page (filter by kind,
        // sort by recency).
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS markers_user_kind_updated_idx \
             ON markers(user_id, kind, updated_at DESC)",
        )
        .await?;
        // Per-issue marker count badges on PageStrip (which pages have
        // a marker?). Lighter than the composite above for that query
        // shape — drops the user_id prefix because page-strip fetches
        // are already scoped to the calling user by `CurrentUser`.
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS markers_issue_page_idx \
             ON markers(issue_id, page_index)",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Markers::Table).to_owned())
            .await
    }
}
