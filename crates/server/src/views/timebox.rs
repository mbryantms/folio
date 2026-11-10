//! The filter time box: every query compiled from a filter DSL runs here.
//!
//! A filter is user-written SQL in all but name, so its cost is bounded
//! at the database, not only by the HTTP timeout: when a request is
//! dropped, Postgres keeps running the statement and the pool connection
//! stays busy. [`query_all`] opens a short read-only transaction, issues
//! `SET LOCAL statement_timeout` (transaction-scoped, so it can never
//! leak onto the pooled connection), runs the statements and commits. A
//! statement that overruns is cancelled by Postgres (SQLSTATE 57014) and
//! comes back as [`FilterQueryError::TimedOut`].
//!
//! Callers: `api::saved_views` (results, issue-results, both previews,
//! the metadata batch target expansion) and the OPDS saved-view feeds
//! (`api::opds`, `api::opds_v2`). Each takes the timeout from
//! `Config::filter_statement_timeout` ([`FILTER_STATEMENT_TIMEOUT`] in
//! every real server; the test harness shortens it).

use std::time::Duration;

use sea_orm::{
    AccessMode, ConnectionTrait, DatabaseConnection, DbErr, QueryResult, RuntimeErr, Statement,
    TransactionTrait,
};

/// Statement timeout for filter-compiled queries.
pub const FILTER_STATEMENT_TIMEOUT: Duration = Duration::from_secs(5);

/// What a person sees when a filter query hits the time box.
pub const FILTER_TOO_SLOW_MESSAGE: &str =
    "This filter takes too long to run. Remove some conditions and try again.";

/// Error code of the 422 a timed-out filter query answers.
pub const FILTER_TOO_SLOW_CODE: &str = "filter_too_slow";

/// Postgres `query_canceled`: raised for `statement_timeout`.
const SQLSTATE_QUERY_CANCELED: &str = "57014";

#[derive(Debug)]
pub enum FilterQueryError {
    /// Postgres cancelled a statement at the statement timeout.
    TimedOut,
    /// Any other database error.
    Db(DbErr),
}

impl std::fmt::Display for FilterQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilterQueryError::TimedOut => f.write_str("filter query hit the statement timeout"),
            FilterQueryError::Db(e) => write!(f, "{e}"),
        }
    }
}

impl From<DbErr> for FilterQueryError {
    fn from(e: DbErr) -> Self {
        if is_query_canceled(&e) {
            FilterQueryError::TimedOut
        } else {
            FilterQueryError::Db(e)
        }
    }
}

fn is_query_canceled(e: &DbErr) -> bool {
    let (DbErr::Query(RuntimeErr::SqlxError(sqlx_err))
    | DbErr::Exec(RuntimeErr::SqlxError(sqlx_err))) = e
    else {
        return false;
    };
    match sqlx_err.as_ref() {
        sea_orm::sqlx::Error::Database(db) => db.code().as_deref() == Some(SQLSTATE_QUERY_CANCELED),
        _ => false,
    }
}

/// Run `statements` in order inside one read-only transaction under
/// `SET LOCAL statement_timeout = <timeout>`, returning each statement's
/// rows. The transaction is committed on success and rolled back on
/// every error path (dropping a `DatabaseTransaction` rolls back too), so
/// no write can happen and nothing outlives the call.
pub async fn query_all(
    db: &DatabaseConnection,
    timeout: Duration,
    statements: Vec<Statement>,
) -> Result<Vec<Vec<QueryResult>>, FilterQueryError> {
    let txn = db
        .begin_with_config(None, Some(AccessMode::ReadOnly))
        .await?;
    // `SET` takes no bind parameters; the value is our own integer.
    let ms = timeout.as_millis().max(1);
    if let Err(e) = txn
        .execute_unprepared(&format!("SET LOCAL statement_timeout = '{ms}ms'"))
        .await
    {
        let _ = txn.rollback().await;
        return Err(e.into());
    }
    let mut out = Vec::with_capacity(statements.len());
    for stmt in statements {
        match txn.query_all_raw(stmt).await {
            Ok(rows) => out.push(rows),
            Err(e) => {
                let _ = txn.rollback().await;
                return Err(e.into());
            }
        }
    }
    txn.commit().await?;
    Ok(out)
}

/// [`query_all`] for a single statement.
pub async fn query_one_statement(
    db: &DatabaseConnection,
    timeout: Duration,
    statement: Statement,
) -> Result<Vec<QueryResult>, FilterQueryError> {
    let mut out = query_all(db, timeout, vec![statement]).await?;
    Ok(out.pop().unwrap_or_default())
}
