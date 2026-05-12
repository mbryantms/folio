//! Continue Reading + On Deck home rails — per-user dismissal table.
//!
//! Tracks "user hid this item from a rail". Auto-restore is implemented at
//! query time, not by a worker: a dismissal row is filtered out of the rail
//! whenever the underlying target has new activity past `dismissed_at`. The
//! row itself sticks around so re-dismissing the same target is idempotent.
//!
//! `target_kind` is a free-form text discriminator (`'issue'`, `'series'`,
//! `'cbl'`) so adding new dismissable surfaces later doesn't require a
//! schema change. `target_id` is text to admit both UUID (series, cbl) and
//! string-id (issue) primary keys without per-kind FK columns; the rail
//! query joins back to the right table via `target_kind` instead.

use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum RailDismissals {
    Table,
    UserId,
    TargetKind,
    TargetId,
    DismissedAt,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(RailDismissals::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(RailDismissals::UserId).uuid().not_null())
                    .col(ColumnDef::new(RailDismissals::TargetKind).text().not_null())
                    .col(ColumnDef::new(RailDismissals::TargetId).text().not_null())
                    .col(
                        ColumnDef::new(RailDismissals::DismissedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .primary_key(
                        Index::create()
                            .col(RailDismissals::UserId)
                            .col(RailDismissals::TargetKind)
                            .col(RailDismissals::TargetId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_rail_dismissals_user")
                            .from(RailDismissals::Table, RailDismissals::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Constrain target_kind to known values; cheaper to evolve than an
        // enum type, and matches the discriminator pattern used in
        // `audit_log.action`.
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE rail_dismissals ADD CONSTRAINT rail_dismissals_kind_chk \
                 CHECK (target_kind IN ('issue', 'series', 'cbl'))",
            )
            .await?;

        // Secondary index for "did anyone dismiss this target?" queries — not
        // strictly required but cheap and useful for admin / debug.
        manager
            .create_index(
                Index::create()
                    .name("rail_dismissals_target_idx")
                    .table(RailDismissals::Table)
                    .col(RailDismissals::TargetKind)
                    .col(RailDismissals::TargetId)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RailDismissals::Table).to_owned())
            .await?;
        Ok(())
    }
}
