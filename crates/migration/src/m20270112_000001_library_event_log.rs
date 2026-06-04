//! Observability split M1 — durable library event log.
//!
//! The `library_events` table is the canonical, durable record of everything
//! the library subsystem *does*: scan lifecycle, per-entity changes (issue
//! added/updated/removed, series created/updated), thumbnail + cover
//! generation, metadata application, archive rewrites, and errors. It is the
//! "Library stream" half of the observability split — distinct from the
//! ephemeral in-memory ring buffer (server/app runtime) and from `audit_log`
//! (server-domain who-did-what compliance trail).
//!
//! Each row is one itemized fact (the "full itemized manifest" decision):
//! a scan that adds 500 issues writes 500 rows. Writes are bulk-inserted per
//! scan phase (see `crates/server/src/library/event_log.rs`, M2) and pruned
//! on a retention schedule (M4) so the table stays bounded.
//!
//! Shape mirrors the JSON-blob philosophy of `library_health_issues`:
//! `category` / `action` / `entity_type` are free-text discriminators (no
//! CHECK constraint) so new event kinds never require a migration; only the
//! small, stable `severity` set is constrained.
//!
//! `batch_id` references the `scan_batch` table that arrives in M5 — it is a
//! plain nullable column here (no FK yet); the FK constraint is added by the
//! M5 migration once the parent table exists.

use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum LibraryEvents {
    Table,
    Id,
    LibraryId,
    ScanRunId,
    BatchId,
    Category,
    EntityType,
    EntityId,
    EntityLabel,
    Action,
    Severity,
    Summary,
    Detail,
    CreatedAt,
}

#[derive(Iden)]
enum Libraries {
    Table,
    Id,
}

#[derive(Iden)]
enum ScanRuns {
    Table,
    Id,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(LibraryEvents::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(LibraryEvents::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(LibraryEvents::LibraryId).uuid().not_null())
                    .col(ColumnDef::new(LibraryEvents::ScanRunId).uuid().null())
                    .col(ColumnDef::new(LibraryEvents::BatchId).uuid().null())
                    .col(ColumnDef::new(LibraryEvents::Category).text().not_null())
                    .col(ColumnDef::new(LibraryEvents::EntityType).text().null())
                    // Entity ids are heterogeneous (issues are string-keyed,
                    // series/library are uuid) — store as text, matching
                    // `scan_runs.issue_id`.
                    .col(ColumnDef::new(LibraryEvents::EntityId).text().null())
                    .col(ColumnDef::new(LibraryEvents::EntityLabel).text().null())
                    .col(ColumnDef::new(LibraryEvents::Action).text().not_null())
                    .col(ColumnDef::new(LibraryEvents::Severity).text().not_null())
                    .col(ColumnDef::new(LibraryEvents::Summary).text().not_null())
                    .col(ColumnDef::new(LibraryEvents::Detail).json_binary().null())
                    .col(
                        ColumnDef::new(LibraryEvents::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("library_events_library_fk")
                            .from(LibraryEvents::Table, LibraryEvents::LibraryId)
                            .to(Libraries::Table, Libraries::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    // Scan runs are pruned (last-N per library); detach the
                    // event rather than cascade-deleting the manifest.
                    .foreign_key(
                        ForeignKey::create()
                            .name("library_events_scan_run_fk")
                            .from(LibraryEvents::Table, LibraryEvents::ScanRunId)
                            .to(ScanRuns::Table, ScanRuns::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        let conn = manager.get_connection();

        // Severity allow-list — the one stable discriminator. `category` /
        // `action` stay unconstrained so new event kinds don't need a
        // migration.
        conn.execute_unprepared(
            "ALTER TABLE library_events ADD CONSTRAINT library_events_severity_chk \
             CHECK (severity IN ('info','warning','error'))",
        )
        .await?;

        // Per-library reverse-chronological feed (the Library activity log).
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS library_events_library_created_idx \
             ON library_events(library_id, created_at DESC)",
        )
        .await?;
        // Per-scan itemized manifest lookup.
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS library_events_scan_run_idx \
             ON library_events(scan_run_id) WHERE scan_run_id IS NOT NULL",
        )
        .await?;
        // Per-batch aggregate (scan-all dashboard drill-down).
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS library_events_batch_idx \
             ON library_events(batch_id) WHERE batch_id IS NOT NULL",
        )
        .await?;
        // Severity-filtered library feed (e.g. "show only errors to rectify").
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS library_events_library_severity_idx \
             ON library_events(library_id, severity, created_at DESC)",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(LibraryEvents::Table).to_owned())
            .await
    }
}
