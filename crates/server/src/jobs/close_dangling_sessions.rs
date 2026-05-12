//! Sweeper that closes reading sessions abandoned without a final flush.
//!
//! Final-flush on the client uses `navigator.sendBeacon` which CSRF middleware
//! rejects (no header support), and is also unreliable on iOS Safari. So we
//! treat the heartbeat as the durable write path and accept that some sessions
//! are abandoned with `ended_at IS NULL`. This job runs every 2 minutes via
//! [`scheduler::start`](super::scheduler::start) and closes any session whose
//! last heartbeat is older than [`STALE_AFTER`].
//!
//! Closing means: `ended_at = last_heartbeat_at`. We do NOT advance `active_ms`
//! to the gap since the user wasn't reading during it.

use entity::reading_session;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use std::time::Duration as StdDuration;

/// Sessions are considered abandoned after this much heartbeat silence.
pub const STALE_AFTER: StdDuration = StdDuration::from_secs(5 * 60);

/// Runs the close pass. Returns the number of rows updated.
pub async fn run(db: &DatabaseConnection) -> Result<u64, DbErr> {
    let cutoff = chrono::Utc::now().fixed_offset()
        - chrono::Duration::from_std(STALE_AFTER).expect("stale_after fits in chrono");

    let res = reading_session::Entity::update_many()
        .col_expr(
            reading_session::Column::EndedAt,
            Expr::col(reading_session::Column::LastHeartbeatAt).into(),
        )
        .filter(reading_session::Column::EndedAt.is_null())
        .filter(reading_session::Column::LastHeartbeatAt.lt(cutoff))
        .exec(db)
        .await?;
    Ok(res.rows_affected)
}
