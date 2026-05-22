//! Per-user override of the home-page rail cap.
//!
//! Previously hard-coded to 12 (see `MAX_PIN_COUNT` in
//! `crates/server/src/api/saved_views.rs`). Users who maintain a lot
//! of curated filter views asked for headroom, and the
//! lazy-mounting of off-screen rails on the client side makes the
//! cost shape gentle enough to expose a per-user knob.
//!
//! Range: 1..=50. The lower bound keeps the home page legal — every
//! user has at least the auto-seeded system rails ("Continue
//! Reading", "On Deck", "Recently Added", "Recently Updated"), so
//! a cap below 4 is meaningless. The upper bound is set so a single
//! malicious or accidentally-misclicked value can't degrade
//! first-paint to multi-second territory.
//!
//! Default 12 preserves the previous behaviour for everyone.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum Users {
    Table,
    MaxRailsPerPage,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::MaxRailsPerPage)
                            .integer()
                            .not_null()
                            .default(12),
                    )
                    .to_owned(),
            )
            .await?;
        // Range constraint enforced at the DB so PATCH validation +
        // CHECK align. The handler also enforces 1..=50 server-side
        // for a clearer 400 response.
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE users \
                 ADD CONSTRAINT users_max_rails_per_page_range \
                 CHECK (max_rails_per_page BETWEEN 1 AND 50)",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE users DROP CONSTRAINT IF EXISTS users_max_rails_per_page_range",
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::MaxRailsPerPage)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
