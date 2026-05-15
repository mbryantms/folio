//! Multi-page rails M1 — `user_page` table + page-aware `user_view_pins`.
//!
//! Generalizes the home page into N user-created pages, each pinning its
//! own saved-view rails. The implicit "home is the user's only home page"
//! semantics gets replaced by an explicit per-user **system page row**
//! (`is_system = TRUE`, slug `home`, name "Home") that owns today's pins.
//! Subsequent milestones add page-CRUD endpoints, page-aware pin/unpin,
//! and the `/pages/[slug]` route; everything else in the app keeps working
//! because every existing pin row is back-filled onto the user's system
//! page during this migration.
//!
//! Schema deltas:
//!   - New `user_page` table.
//!   - One auto-created `is_system = TRUE` row per existing user.
//!   - `user_view_pins.page_id` added (NULL → backfilled → NOT NULL).
//!   - Primary key widens from `(user_id, view_id)` to
//!     `(user_id, page_id, view_id)`.

use sea_orm::ConnectionTrait;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum UserPage {
    Table,
    Id,
    UserId,
    Name,
    Slug,
    IsSystem,
    Position,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum Users {
    Table,
    Id,
}

#[derive(Iden)]
enum UserViewPins {
    Table,
    PageId,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1. user_page table.
        manager
            .create_table(
                Table::create()
                    .table(UserPage::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(UserPage::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(UserPage::UserId).uuid().not_null())
                    .col(ColumnDef::new(UserPage::Name).text().not_null())
                    .col(ColumnDef::new(UserPage::Slug).text().not_null())
                    .col(
                        ColumnDef::new(UserPage::IsSystem)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(UserPage::Position)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(UserPage::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(UserPage::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_page_user")
                            .from(UserPage::Table, UserPage::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Unique slug per user.
        manager
            .create_index(
                Index::create()
                    .name("user_page_user_slug_idx")
                    .table(UserPage::Table)
                    .col(UserPage::UserId)
                    .col(UserPage::Slug)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // One system page per user (partial unique on is_system).
        let conn = manager.get_connection();
        conn.execute_unprepared(
            "CREATE UNIQUE INDEX user_page_system_idx \
             ON user_page (user_id) WHERE is_system",
        )
        .await?;

        // Position lookup (the resolver pulls all pages for a user ordered).
        manager
            .create_index(
                Index::create()
                    .name("user_page_user_position_idx")
                    .table(UserPage::Table)
                    .col(UserPage::UserId)
                    .col(UserPage::Position)
                    .to_owned(),
            )
            .await?;

        // 2. Seed one system "Home" page per existing user. `gen_random_uuid`
        //    is available via pgcrypto (extensions migration).
        conn.execute_unprepared(
            "INSERT INTO user_page (id, user_id, name, slug, is_system, position) \
             SELECT gen_random_uuid(), id, 'Home', 'home', TRUE, 0 FROM users",
        )
        .await?;

        // 3. Add page_id to user_view_pins as nullable, backfill, then NOT NULL.
        manager
            .alter_table(
                Table::alter()
                    .table(UserViewPins::Table)
                    .add_column(ColumnDef::new(UserViewPins::PageId).uuid().null())
                    .to_owned(),
            )
            .await?;

        conn.execute_unprepared(
            "UPDATE user_view_pins p \
                SET page_id = up.id \
               FROM user_page up \
              WHERE up.user_id = p.user_id AND up.is_system = TRUE",
        )
        .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(UserViewPins::Table)
                    .modify_column(ColumnDef::new(UserViewPins::PageId).uuid().not_null())
                    .to_owned(),
            )
            .await?;

        // Add FK page_id → user_page(id) ON DELETE CASCADE.
        conn.execute_unprepared(
            "ALTER TABLE user_view_pins \
             ADD CONSTRAINT fk_user_view_pins_page \
             FOREIGN KEY (page_id) REFERENCES user_page(id) ON DELETE CASCADE",
        )
        .await?;

        // 4. Widen the primary key from (user_id, view_id) to
        //    (user_id, page_id, view_id). The original PK was created without
        //    an explicit name, so Postgres assigned the default
        //    `user_view_pins_pkey`.
        conn.execute_unprepared("ALTER TABLE user_view_pins DROP CONSTRAINT user_view_pins_pkey")
            .await?;
        conn.execute_unprepared(
            "ALTER TABLE user_view_pins \
             ADD PRIMARY KEY (user_id, page_id, view_id)",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();

        // Reverse the PK widening.
        conn.execute_unprepared("ALTER TABLE user_view_pins DROP CONSTRAINT user_view_pins_pkey")
            .await?;
        conn.execute_unprepared("ALTER TABLE user_view_pins ADD PRIMARY KEY (user_id, view_id)")
            .await?;
        conn.execute_unprepared(
            "ALTER TABLE user_view_pins DROP CONSTRAINT IF EXISTS fk_user_view_pins_page",
        )
        .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(UserViewPins::Table)
                    .drop_column(UserViewPins::PageId)
                    .to_owned(),
            )
            .await?;

        conn.execute_unprepared("DROP INDEX IF EXISTS user_page_system_idx")
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("user_page_user_position_idx")
                    .table(UserPage::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("user_page_user_slug_idx")
                    .table(UserPage::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(UserPage::Table).to_owned())
            .await?;

        Ok(())
    }
}
