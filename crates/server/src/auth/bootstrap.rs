//! Helpers for first-user admin bootstrap.

use sea_orm::{ConnectionTrait, DbErr, Statement};

const FIRST_ADMIN_BOOTSTRAP_LOCK: i64 = 773_265_118_417_895_241;

/// Serialize the "are there any users yet?" check across local and OIDC
/// signup paths. Must be called inside the transaction that performs the
/// follow-up user count and insert.
pub(crate) async fn lock_first_admin_bootstrap<C>(conn: &C) -> Result<(), DbErr>
where
    C: ConnectionTrait,
{
    conn.execute(Statement::from_string(
        conn.get_database_backend(),
        format!("SELECT pg_advisory_xact_lock({FIRST_ADMIN_BOOTSTRAP_LOCK})"),
    ))
    .await?;
    Ok(())
}
