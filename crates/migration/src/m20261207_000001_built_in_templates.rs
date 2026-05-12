//! Saved-views M9: built-in template views + `auto_pin` flag.
//!
//! M3 seeded two filter views ("Recently Added" / "Recently Updated") and
//! the lazy first-touch seed pinned *every* system view to a fresh user's
//! home rail. With more templates landing here that'd crowd the home page,
//! so this migration distinguishes "auto-pinned" system views (the two
//! M3-seeded rails) from "available-to-pin" templates (the three new
//! ones) via a new `saved_views.auto_pin` column.
//!
//! New templates (`auto_pin = false`):
//!
//!   - **Just Finished** — `read_progress = 100`, sorted by `last_read`
//!     desc. Series the user just wrapped up.
//!   - **Want to Read** — `read_progress = 0`, sorted by `created_at`
//!     desc. Series sitting in the library waiting for a first session.
//!   - **Stale** — no filter; sorted by `updated_at` asc, limit 12.
//!     Surfaces the bottom of the activity barrel so curators can find
//!     series the scanner hasn't touched in a while. (The DSL has no
//!     "older than N days" op today; sort+limit is the closest stable
//!     interpretation without a new op.)

use sea_orm::{ConnectionTrait, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

#[derive(Iden)]
enum SavedViews {
    Table,
    AutoPin,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(SavedViews::Table)
                    .add_column(
                        ColumnDef::new(SavedViews::AutoPin)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;

        let backend = manager.get_database_backend();
        let conn = manager.get_connection();

        // Mark the two M3 originals as auto-pinned so the seed continues
        // to pin them for fresh users.
        for id in &[
            "00000000-0000-0000-0000-000000000001",
            "00000000-0000-0000-0000-000000000002",
        ] {
            conn.execute(Statement::from_sql_and_values(
                backend,
                "UPDATE saved_views SET auto_pin = TRUE WHERE id = $1::uuid",
                [(*id).into()],
            ))
            .await?;
        }

        // Seed the three new templates. `ON CONFLICT DO NOTHING` keeps
        // re-runs idempotent.
        let templates: &[(&str, &str, &str, &str, &str, &str, i32)] = &[
            (
                "00000000-0000-0000-0000-000000000003",
                "Just Finished",
                "Series you've completed — newest finishes first.",
                r#"[{"group_id":0,"field":"read_progress","op":"equals","value":100}]"#,
                "last_read",
                "desc",
                12,
            ),
            (
                "00000000-0000-0000-0000-000000000004",
                "Want to Read",
                "Series sitting in your library that you haven't started yet.",
                r#"[{"group_id":0,"field":"read_progress","op":"equals","value":0}]"#,
                "created_at",
                "desc",
                12,
            ),
            (
                "00000000-0000-0000-0000-000000000005",
                "Stale",
                "Series the scanner hasn't seen activity on in a while.",
                "[]",
                "updated_at",
                "asc",
                12,
            ),
        ];
        for (id, name, desc, conditions, sort_field, sort_order, limit) in templates {
            conn.execute(Statement::from_sql_and_values(
                backend,
                r"INSERT INTO saved_views
                    (id, user_id, kind, name, description, custom_tags,
                     match_mode, conditions, sort_field, sort_order,
                     result_limit, auto_pin)
                  VALUES
                    ($1::uuid, NULL, 'filter_series', $2, $3, ARRAY[]::text[],
                     'all', $4::jsonb, $5, $6, $7, FALSE)
                  ON CONFLICT (id) DO NOTHING",
                [
                    (*id).into(),
                    (*name).into(),
                    (*desc).into(),
                    (*conditions).into(),
                    (*sort_field).into(),
                    (*sort_order).into(),
                    (*limit).into(),
                ],
            ))
            .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let conn = manager.get_connection();
        conn.execute(Statement::from_sql_and_values(
            backend,
            "DELETE FROM saved_views WHERE id = ANY($1::uuid[])",
            [vec![
                "00000000-0000-0000-0000-000000000003".to_string(),
                "00000000-0000-0000-0000-000000000004".to_string(),
                "00000000-0000-0000-0000-000000000005".to_string(),
            ]
            .into()],
        ))
        .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SavedViews::Table)
                    .drop_column(SavedViews::AutoPin)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
