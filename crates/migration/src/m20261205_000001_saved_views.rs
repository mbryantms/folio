//! Saved smart views — M3: polymorphic `saved_views` + `user_view_pins`.
//!
//! Two view kinds today (one in v1):
//!
//!   - `filter_series` — chained conditions over series, compiled to SQL by
//!     `server::views::compile`. Inline DSL stored in `conditions` JSONB.
//!   - `cbl` — pointer to a `cbl_lists` row owned by M4. Schema columns are
//!     present here so the polymorphic discriminator is settled in a single
//!     migration; the FK to `cbl_lists` lands when M4 creates that table.
//!
//! `user_id NULL` means a system view (admin-curated). System views can't
//! be edited by regular users; the lazy per-user pin seed surfaces them in
//! every user's home rail via `user_view_pins` rows.
//!
//! Seeded with two filter views: "Recently Added" (sort by `created_at`
//! desc) and "Recently Updated" (sort by `updated_at` desc). These are the
//! two rails the home page renders today, so a fresh user sees parity with
//! the legacy hardcoded rails.

use sea_orm::{ConnectionTrait, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum SavedViews {
    Table,
    Id,
    UserId,
    Kind,
    Name,
    Description,
    CustomYearStart,
    CustomYearEnd,
    CustomTags,
    MatchMode,
    Conditions,
    SortField,
    SortOrder,
    ResultLimit,
    CblListId,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum UserViewPins {
    Table,
    UserId,
    ViewId,
    Position,
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
                    .table(SavedViews::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(SavedViews::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(SavedViews::UserId).uuid().null())
                    .col(ColumnDef::new(SavedViews::Kind).text().not_null())
                    .col(ColumnDef::new(SavedViews::Name).text().not_null())
                    .col(ColumnDef::new(SavedViews::Description).text().null())
                    .col(ColumnDef::new(SavedViews::CustomYearStart).integer().null())
                    .col(ColumnDef::new(SavedViews::CustomYearEnd).integer().null())
                    .col(
                        ColumnDef::new(SavedViews::CustomTags)
                            .array(ColumnType::Text)
                            .not_null()
                            .default(Expr::cust("ARRAY[]::text[]")),
                    )
                    .col(ColumnDef::new(SavedViews::MatchMode).text().null())
                    .col(ColumnDef::new(SavedViews::Conditions).json_binary().null())
                    .col(ColumnDef::new(SavedViews::SortField).text().null())
                    .col(ColumnDef::new(SavedViews::SortOrder).text().null())
                    .col(ColumnDef::new(SavedViews::ResultLimit).integer().null())
                    .col(ColumnDef::new(SavedViews::CblListId).uuid().null())
                    .col(
                        ColumnDef::new(SavedViews::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(SavedViews::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_saved_views_user")
                            .from(SavedViews::Table, SavedViews::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Filter-vs-CBL discriminator. `kind = 'filter_series'` requires the
        // filter columns to be populated and `cbl_list_id` to be NULL;
        // `kind = 'cbl'` is the inverse. Caught at the schema layer so a bad
        // INSERT can never produce an ambiguous row.
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE saved_views ADD CONSTRAINT saved_views_kind_chk CHECK (\
                    (kind = 'filter_series' AND match_mode IS NOT NULL AND conditions IS NOT NULL \
                        AND sort_field IS NOT NULL AND sort_order IS NOT NULL AND result_limit IS NOT NULL \
                        AND cbl_list_id IS NULL) \
                    OR (kind = 'cbl' AND match_mode IS NULL AND conditions IS NULL \
                        AND sort_field IS NULL AND sort_order IS NULL AND result_limit IS NULL \
                        AND cbl_list_id IS NOT NULL)\
                 )",
            )
            .await?;

        // Per-user pin lookup (drives the home page). System views land in
        // every user's pin set via the lazy seed at first GET /me/saved-views.
        manager
            .create_index(
                Index::create()
                    .name("saved_views_user_kind_idx")
                    .table(SavedViews::Table)
                    .col(SavedViews::UserId)
                    .col(SavedViews::Kind)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(UserViewPins::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(UserViewPins::UserId).uuid().not_null())
                    .col(ColumnDef::new(UserViewPins::ViewId).uuid().not_null())
                    .col(ColumnDef::new(UserViewPins::Position).integer().not_null())
                    .primary_key(
                        Index::create()
                            .col(UserViewPins::UserId)
                            .col(UserViewPins::ViewId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_view_pins_user")
                            .from(UserViewPins::Table, UserViewPins::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_view_pins_view")
                            .from(UserViewPins::Table, UserViewPins::ViewId)
                            .to(SavedViews::Table, SavedViews::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("user_view_pins_position_idx")
                    .table(UserViewPins::Table)
                    .col(UserViewPins::UserId)
                    .col(UserViewPins::Position)
                    .to_owned(),
            )
            .await?;

        // Seed the two system filter views. Fixed IDs let tests look them up
        // without race conditions across parallel processes; the IDs are
        // stable across deploys so existing user_view_pins rows survive.
        let backend = manager.get_database_backend();
        for (id, name, sort_field) in &[
            (
                "00000000-0000-0000-0000-000000000001",
                "Recently Added",
                "created_at",
            ),
            (
                "00000000-0000-0000-0000-000000000002",
                "Recently Updated",
                "updated_at",
            ),
        ] {
            manager
                .get_connection()
                .execute(Statement::from_sql_and_values(
                    backend,
                    r"INSERT INTO saved_views
                        (id, user_id, kind, name, description, custom_tags,
                         match_mode, conditions, sort_field, sort_order, result_limit)
                      VALUES
                        ($1::uuid, NULL, 'filter_series', $2, NULL, ARRAY[]::text[],
                         'all', '[]'::jsonb, $3, 'desc', 12)
                      ON CONFLICT (id) DO NOTHING",
                    [(*id).into(), (*name).into(), (*sort_field).into()],
                ))
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(UserViewPins::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(SavedViews::Table).to_owned())
            .await?;
        Ok(())
    }
}
