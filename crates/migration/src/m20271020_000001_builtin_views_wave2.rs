//! Built-in views, wave 2 (filter-parity-and-builtin-views-1.0 M5 + M6).
//!
//! Seeds a second set of global (`user_id IS NULL`) saved views on top of
//! the fields the same plan added to the filter DSL (`finished_at`,
//! issue-side `read_progress` / `last_read` / `rating` sorts, the
//! `not_relative` op, …):
//!
//!   - **filter_issues templates** — the first issue-shaped built-ins
//!     (every earlier template was series-shaped): Recently finished,
//!     Finished this week, Rate what you read, Top rated issues, Annuals &
//!     specials, Short unread reads, Favorite issues, Annotated issues.
//!   - **filter_series templates** — Jump back in, Almost done, Complete
//!     runs to start, Ended but unfinished, Top rated series, Incomplete
//!     collections, Favorite series.
//!   - **system rails** needing their own endpoints — Random picks
//!     (`/me/random-picks`), On this day (`/me/on-this-day`), Recently
//!     read (`/me/recent-history`).
//!
//! Everything is opt-in (`auto_pin = false`) except **Recently finished**
//! issues, the issue-level twin of the series "Just Finished" template —
//! which is renamed here to "Recently finished series" so the pair reads
//! consistently (guarded on the current name, like the
//! `m20270122` renames).
//!
//! Fixed UUIDs (`…0014`–`…0016` rails, `…0020`–`…0034` templates) so
//! `user_view_pins` rows survive redeploys and tests can look rows up.
//! `ON CONFLICT (id) DO NOTHING` keeps re-runs idempotent.
use sea_orm::{ConnectionTrait, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

const JUST_FINISHED_ID: &str = "00000000-0000-0000-0000-000000000003";

/// `(id, kind, name, description, conditions JSON, match_mode, sort_field,
/// sort_order, limit, auto_pin)`.
type Template = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    i32,
    bool,
);

pub(crate) const TEMPLATES: &[Template] = &[
    // ── issue views ──
    (
        "00000000-0000-0000-0000-000000000020",
        "filter_issues",
        "Recently finished",
        "Issues you finished most recently.",
        r#"[{"group_id":0,"field":"read_status","op":"is","value":"read"}]"#,
        "all",
        "finished_at",
        "desc",
        12,
        true,
    ),
    (
        "00000000-0000-0000-0000-000000000021",
        "filter_issues",
        "Finished this week",
        "Everything you finished in the last seven days.",
        r#"[{"group_id":0,"field":"finished_at","op":"relative","value":7}]"#,
        "all",
        "finished_at",
        "desc",
        24,
        false,
    ),
    (
        "00000000-0000-0000-0000-000000000022",
        "filter_issues",
        "Rate what you read",
        "Issues you've finished but haven't rated yet.",
        r#"[{"group_id":0,"field":"read_status","op":"is","value":"read"},{"group_id":0,"field":"rating","op":"is_empty","value":null}]"#,
        "all",
        "finished_at",
        "desc",
        12,
        false,
    ),
    (
        "00000000-0000-0000-0000-000000000023",
        "filter_issues",
        "Top rated issues",
        "Issues you rated four stars or more.",
        r#"[{"group_id":0,"field":"rating","op":"gte","value":4}]"#,
        "all",
        "rating",
        "desc",
        12,
        false,
    ),
    (
        "00000000-0000-0000-0000-000000000024",
        "filter_issues",
        "Annuals & specials",
        "Annuals, specials, one-shots and collected editions, newest first.",
        r#"[{"group_id":0,"field":"special_type","op":"is_not_empty","value":null}]"#,
        "all",
        "year",
        "desc",
        12,
        false,
    ),
    (
        "00000000-0000-0000-0000-000000000025",
        "filter_issues",
        "Short unread reads",
        "Unread issues of 24 pages or fewer — something for a short break.",
        r#"[{"group_id":0,"field":"page_count","op":"lte","value":24},{"group_id":0,"field":"read_status","op":"is","value":"unread"}]"#,
        "all",
        "created_at",
        "desc",
        12,
        false,
    ),
    (
        "00000000-0000-0000-0000-000000000026",
        "filter_issues",
        "Favorite issues",
        "Issues you starred a page or a marker in.",
        r#"[{"group_id":0,"field":"has_favorites","op":"is_true","value":null}]"#,
        "all",
        "updated_at",
        "desc",
        12,
        false,
    ),
    (
        "00000000-0000-0000-0000-000000000027",
        "filter_issues",
        "Annotated issues",
        "Issues where you left notes or highlights.",
        r#"[{"group_id":0,"field":"has_notes","op":"is_true","value":null},{"group_id":0,"field":"has_highlights","op":"is_true","value":null}]"#,
        "any",
        "updated_at",
        "desc",
        12,
        false,
    ),
    // ── series views ──
    (
        "00000000-0000-0000-0000-000000000028",
        "filter_series",
        "Jump back in",
        "Series you started but haven't touched in two weeks.",
        r#"[{"group_id":0,"field":"read_status","op":"is","value":"in_progress"},{"group_id":0,"field":"last_read","op":"not_relative","value":14}]"#,
        "all",
        "last_read",
        "asc",
        12,
        false,
    ),
    (
        "00000000-0000-0000-0000-000000000029",
        "filter_series",
        "Almost done",
        "Series you're at least three-quarters of the way through.",
        r#"[{"group_id":0,"field":"read_progress","op":"between","value":[75,99]}]"#,
        "all",
        "read_progress",
        "desc",
        12,
        false,
    ),
    (
        "00000000-0000-0000-0000-000000000030",
        "filter_series",
        "Complete runs to start",
        "Unread series you own every issue of.",
        r#"[{"group_id":0,"field":"collection_completeness","op":"is","value":"complete"},{"group_id":0,"field":"read_status","op":"is","value":"unread"}]"#,
        "all",
        "created_at",
        "desc",
        12,
        false,
    ),
    (
        "00000000-0000-0000-0000-000000000031",
        "filter_series",
        "Ended, unfinished",
        "Finished runs you started but never completed.",
        r#"[{"group_id":0,"field":"status","op":"in","value":["ended","cancelled"]},{"group_id":0,"field":"read_status","op":"is","value":"in_progress"}]"#,
        "all",
        "last_read",
        "desc",
        12,
        false,
    ),
    (
        "00000000-0000-0000-0000-000000000032",
        "filter_series",
        "Top rated series",
        "Series you rated four stars or more.",
        r#"[{"group_id":0,"field":"rating","op":"gte","value":4}]"#,
        "all",
        "rating",
        "desc",
        12,
        false,
    ),
    (
        "00000000-0000-0000-0000-000000000033",
        "filter_series",
        "Incomplete collections",
        "Series with issues still missing from your library.",
        r#"[{"group_id":0,"field":"collection_completeness","op":"is","value":"incomplete"}]"#,
        "all",
        "name",
        "asc",
        12,
        false,
    ),
    (
        "00000000-0000-0000-0000-000000000034",
        "filter_series",
        "Favorite series",
        "Series you starred something in.",
        r#"[{"group_id":0,"field":"has_favorites","op":"is_true","value":null}]"#,
        "all",
        "updated_at",
        "desc",
        12,
        false,
    ),
];

/// `(id, system_key, name, description)` — all opt-in.
pub(crate) const SYSTEM_RAILS: &[(&str, &str, &str, &str)] = &[
    (
        "00000000-0000-0000-0000-000000000014",
        "random_picks",
        "Random picks",
        "A fresh shuffle of series you haven't started, every day.",
    ),
    (
        "00000000-0000-0000-0000-000000000015",
        "on_this_day",
        "On this day",
        "Issues released on today's date, across the years.",
    ),
    (
        "00000000-0000-0000-0000-000000000016",
        "recent_history",
        "Recently read",
        "Everything you've read lately, finished or not.",
    ),
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let conn = manager.get_connection();

        conn.execute_raw(Statement::from_sql_and_values(
            backend,
            "UPDATE saved_views SET name = 'Recently finished series', \
                description = 'Series you''ve completed — newest finishes first.' \
             WHERE id = $1::uuid AND name = 'Just Finished'",
            [JUST_FINISHED_ID.into()],
        ))
        .await?;

        for (
            id,
            kind,
            name,
            desc,
            conditions,
            match_mode,
            sort_field,
            sort_order,
            limit,
            auto_pin,
        ) in TEMPLATES
        {
            conn.execute_raw(Statement::from_sql_and_values(
                backend,
                r"INSERT INTO saved_views
                    (id, user_id, kind, name, description, custom_tags,
                     match_mode, conditions, sort_field, sort_order,
                     result_limit, auto_pin)
                  VALUES
                    ($1::uuid, NULL, $2, $3, $4, ARRAY[]::text[],
                     $5, $6::jsonb, $7, $8, $9, $10)
                  ON CONFLICT (id) DO NOTHING",
                [
                    (*id).into(),
                    (*kind).into(),
                    (*name).into(),
                    (*desc).into(),
                    (*match_mode).into(),
                    (*conditions).into(),
                    (*sort_field).into(),
                    (*sort_order).into(),
                    (*limit).into(),
                    (*auto_pin).into(),
                ],
            ))
            .await?;
        }

        for (id, system_key, name, desc) in SYSTEM_RAILS {
            conn.execute_raw(Statement::from_sql_and_values(
                backend,
                r"INSERT INTO saved_views
                    (id, user_id, kind, name, description, custom_tags, system_key, auto_pin)
                  VALUES
                    ($1::uuid, NULL, 'system', $2, $3, ARRAY[]::text[], $4, FALSE)
                  ON CONFLICT (id) DO NOTHING",
                [
                    (*id).into(),
                    (*name).into(),
                    (*desc).into(),
                    (*system_key).into(),
                ],
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let backend = manager.get_database_backend();
        let conn = manager.get_connection();
        let ids: Vec<String> = TEMPLATES
            .iter()
            .map(|t| t.0.to_string())
            .chain(SYSTEM_RAILS.iter().map(|r| r.0.to_string()))
            .collect();
        // Pins cascade; sidebar entries aren't FK-enforced, sweep them.
        conn.execute_raw(Statement::from_sql_and_values(
            backend,
            "DELETE FROM user_sidebar_entries WHERE kind = 'view' AND ref_id = ANY($1::text[])",
            [ids.clone().into()],
        ))
        .await?;
        conn.execute_raw(Statement::from_sql_and_values(
            backend,
            "DELETE FROM saved_views WHERE id = ANY($1::uuid[])",
            [ids.into()],
        ))
        .await?;
        conn.execute_raw(Statement::from_sql_and_values(
            backend,
            "UPDATE saved_views SET name = 'Just Finished' \
             WHERE id = $1::uuid AND name = 'Recently finished series'",
            [JUST_FINISHED_ID.into()],
        ))
        .await?;
        Ok(())
    }
}
