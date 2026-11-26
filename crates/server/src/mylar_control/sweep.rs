//! Carry pending `mylar_add_request` rows out against Mylar.
//!
//! Runs once a minute from the scheduler, and once right after an admin
//! queues requests. Series rows go first: Mylar only knows an issue once
//! it has imported the issue's series, so an issue row waits (with
//! backoff) until `getIssueInfo` finds it, and fails after
//! [`ISSUE_WAIT`].
//!
//! A tick is capped ([`TICK_CAP`]) so a few hundred series reach Mylar —
//! and through it ComicVine — over several minutes, not in one burst. A
//! transient fault (Mylar down, 5xx) ends the tick without counting
//! against any request.

use super::client::{MylarClient, MylarError};
use super::requests::status;
use crate::state::AppState;
use chrono::{DateTime, Duration, FixedOffset, Utc};
use entity::mylar_add_request;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect,
};

/// Rows handled per tick.
pub const TICK_CAP: u64 = 40;
/// How long an issue row waits for Mylar to import its series.
pub const ISSUE_WAIT: Duration = Duration::hours(24);
/// A series row that Mylar keeps answering oddly for gives up here.
pub const MAX_SERIES_ATTEMPTS: i32 = 5;

/// Issue statuses that need no action: Mylar already wants or has it.
const SETTLED: [&str; 4] = ["Wanted", "Downloaded", "Archived", "Snatched"];

static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tick {
    pub done: usize,
    pub failed: usize,
    pub waiting: usize,
}

enum Step {
    Done,
    Wait(String),
    Failed(String),
}

/// One sweep. Does nothing when add-to-Mylar is not configured or
/// another sweep is running in this process.
pub async fn run(state: &AppState) -> Tick {
    let Some(client) = MylarClient::from_cfg(&state.cfg()) else {
        return Tick::default();
    };
    let Ok(_guard) = RUNNING.try_lock() else {
        return Tick::default();
    };
    run_with(&state.db, &client).await
}

pub async fn run_with(db: &DatabaseConnection, client: &MylarClient) -> Tick {
    use mylar_add_request::Column as Col;
    let now = Utc::now().fixed_offset();
    let due = match mylar_add_request::Entity::find()
        .filter(Col::Status.eq(status::PENDING))
        .filter(Col::NextAttemptAt.lte(now))
        // NULL issue ids (series rows) first.
        .order_by_with_nulls(
            Col::CvIssueId,
            sea_orm::Order::Asc,
            sea_orm::sea_query::NullOrdering::First,
        )
        .order_by_asc(Col::CreatedAt)
        .limit(TICK_CAP)
        .all(db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "mylar add sweep: could not load requests");
            return Tick::default();
        }
    };

    let mut tick = Tick::default();
    // A series whose add failed this tick: its issue rows wait.
    for row in due {
        let step = match row.cv_issue_id {
            None => series_step(client, &row).await,
            Some(issue) => issue_step(client, &row, issue, now).await,
        };
        let step = match step {
            Ok(step) => step,
            Err(e) if e.is_transient() => {
                tracing::warn!(error = %e, "mylar add sweep: Mylar unavailable, will retry");
                break;
            }
            Err(e) => Step::Failed(e.to_string()),
        };
        match &step {
            Step::Done => tick.done += 1,
            Step::Wait(_) => tick.waiting += 1,
            Step::Failed(_) => tick.failed += 1,
        }
        if let Err(e) = record(db, row, step, now).await {
            tracing::warn!(error = %e, "mylar add sweep: could not record an outcome");
        }
    }
    tick
}

async fn series_step(
    client: &MylarClient,
    row: &mylar_add_request::Model,
) -> Result<Step, MylarError> {
    let outcome = async {
        if client.has_series(row.cv_series_id).await? {
            return Ok(());
        }
        client.add_series(row.cv_series_id).await
    }
    .await;
    match outcome {
        Ok(()) => Ok(Step::Done),
        Err(e) if e.is_transient() => Err(e),
        // An odd answer can be a Mylar hiccup; a few tries, then give up.
        Err(MylarError::Unexpected) if row.attempts + 1 < MAX_SERIES_ATTEMPTS => {
            Ok(Step::Wait(MylarError::Unexpected.to_string()))
        }
        Err(e) => Err(e),
    }
}

async fn issue_step(
    client: &MylarClient,
    row: &mylar_add_request::Model,
    cv_issue_id: i64,
    now: DateTime<FixedOffset>,
) -> Result<Step, MylarError> {
    match client.issue_status(cv_issue_id).await? {
        Some(s) if SETTLED.contains(&s.as_str()) => Ok(Step::Done),
        Some(_) => client.queue_issue(cv_issue_id).await.map(|()| Step::Done),
        None if now - row.created_at > ISSUE_WAIT => Ok(Step::Failed(
            "Mylar did not import this issue's series within a day".to_owned(),
        )),
        None => Ok(Step::Wait(
            "waiting for Mylar to import the series".to_owned(),
        )),
    }
}

/// Minutes until the next try: 2, 4, 8, 16, then every 30.
fn backoff(attempts: i32) -> Duration {
    Duration::minutes((1i64 << attempts.clamp(1, 5)).min(30))
}

async fn record(
    db: &DatabaseConnection,
    row: mylar_add_request::Model,
    step: Step,
    now: DateTime<FixedOffset>,
) -> Result<(), sea_orm::DbErr> {
    let attempts = row.attempts.saturating_add(1);
    let mut am: mylar_add_request::ActiveModel = row.into();
    am.attempts = Set(attempts);
    am.updated_at = Set(now);
    match step {
        Step::Done => {
            am.status = Set(status::DONE.to_owned());
            am.last_error = Set(None);
        }
        Step::Wait(why) => {
            am.last_error = Set(Some(why));
            am.next_attempt_at = Set(now + backoff(attempts));
        }
        Step::Failed(why) => {
            am.status = Set(status::FAILED.to_owned());
            am.last_error = Set(Some(why));
        }
    }
    am.update(db).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_then_holds_at_thirty_minutes() {
        let mins: Vec<i64> = (1..=7).map(|a| backoff(a).num_minutes()).collect();
        assert_eq!(mins, [2, 4, 8, 16, 30, 30, 30]);
    }
}
