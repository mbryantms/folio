//! Saved smart views — M2: per-user reading-state surface.
//!
//! Defines a SQL view `user_series_progress(user_id, series_id,
//! finished_count, total_count, percent, last_read_at)` so filter views
//! ("Read Progress ≥ 50") can express reading-state predicates without
//! every query re-implementing the join across `progress_records` +
//! `issues` + `reading_sessions`.
//!
//! Why a view, not a materialized view: the underlying tables are written
//! per page-turn (progress) and per heartbeat (reading_sessions), so a
//! mat-view would need either trigger-based refresh or a sweeper job. A
//! plain view defers that cost to query time, which is acceptable while
//! the workload is small. Promote to a mat-view (or per-user denorm) only
//! when EXPLAIN ANALYZE shows it's hot.
//!
//! Row coverage: only `(user_id, series_id)` pairs where the user has at
//! least one `progress_records` row in the series. Users who haven't
//! started a series are represented by the absence of a row; filter
//! queries `LEFT JOIN` and `COALESCE(percent, 0)` to get a synthetic 0%.
//!
//! Per-issue state lives in `progress_records` already and is queried
//! directly via the `progress_record` entity — no separate view needed.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

const VIEW_SQL: &str = r"
CREATE OR REPLACE VIEW user_series_progress AS
SELECT
    pr.user_id,
    i.series_id,
    COUNT(*) FILTER (WHERE pr.finished)::bigint                 AS finished_count,
    st.total_count                                              AS total_count,
    CASE
        WHEN st.total_count = 0 THEN 0::bigint
        ELSE (100 * COUNT(*) FILTER (WHERE pr.finished) / st.total_count)
    END                                                         AS percent,
    rs.last_read_at                                             AS last_read_at
FROM progress_records pr
JOIN issues i
    ON i.id = pr.issue_id
   AND i.state = 'active'
   AND i.removed_at IS NULL
JOIN (
    SELECT series_id, COUNT(*)::bigint AS total_count
    FROM issues
    WHERE state = 'active' AND removed_at IS NULL
    GROUP BY series_id
) st ON st.series_id = i.series_id
LEFT JOIN LATERAL (
    SELECT MAX(last_heartbeat_at) AS last_read_at
    FROM reading_sessions
    WHERE user_id = pr.user_id
      AND series_id = i.series_id
) rs ON true
GROUP BY pr.user_id, i.series_id, st.total_count, rs.last_read_at
";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(VIEW_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP VIEW IF EXISTS user_series_progress")
            .await?;
        Ok(())
    }
}
