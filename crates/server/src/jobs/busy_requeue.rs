//! Bounded self-requeue for a job that found its per-entity lock busy.
//!
//! Three jobs serialize on a Redis mutex — the sidecar rewrite and the
//! page editor on `archive:rewrite:<issue_id>` (TTL 120 s / 180 s,
//! heartbeat-extended while the holder runs), the metadata apply on
//! `metadata:apply:{series|issue}:<id>` (TTL 90 s, no heartbeat) — and a
//! loser must not drop its work. The policy is the same everywhere: pace
//! for [`BUSY_RETRY_DELAY`], push a copy of the job with its own `attempt`
//! counter bumped, and give up loudly only once [`BUSY_MAX_ATTEMPTS`]
//! requeues (200 s) have outlasted every lock TTL.
//!
//! This counter is **not** apalis's `Attempt`: a busy lock is not a
//! failure, so the handler returns `Ok` after requeueing and the
//! `JOB_MAX_ATTEMPTS` budget stays reserved for real errors (I/O, archive,
//! DB). Only a failed push — Redis itself refusing the requeue — is
//! surfaced as `Err` so apalis retries the original task.
//!
//! The pace is an in-handler sleep, as the page editor has always done:
//! on a concurrency-1 worker it holds the queue for 5 s per requeue. The
//! busy case is rare (a leaked key after a crash, two replicas on one
//! series), so that is accepted over scheduling the copy into apalis's
//! delayed set.

use apalis::prelude::BoxDynError;
use std::future::Future;
use std::time::Duration;

/// How many times a job re-enqueues itself before giving up.
pub const BUSY_MAX_ATTEMPTS: u32 = 40;
/// Pause before each requeue so a held lock isn't hammered.
pub const BUSY_RETRY_DELAY: Duration = Duration::from_secs(5);

/// What happened to the busy job. A failed push is the `Err` of
/// [`requeue_busy`], already shaped for the handler to return.
#[derive(Debug, PartialEq, Eq)]
pub enum Requeue {
    /// A copy with `attempt` bumped is back on the queue; the handler
    /// returns `Ok`.
    Requeued { attempt: u32 },
    /// The budget is spent and nothing was pushed. The caller records the
    /// drop (library event, run failure) and returns `Ok` — there is
    /// nothing left to retry.
    Exhausted { attempts: u32 },
}

/// Apply the policy for a job whose lock was busy on `attempt` (0 for a
/// fresh enqueue). `job` / `target` only label the log lines and the
/// error; `push` receives the bumped counter and stores the copy, and is
/// only called while the budget allows.
pub async fn requeue_busy<P, Fut>(
    job: &'static str,
    target: &str,
    attempt: u32,
    push: P,
) -> Result<Requeue, BoxDynError>
where
    P: FnOnce(u32) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    requeue_busy_paced(job, target, attempt, BUSY_RETRY_DELAY, push).await
}

/// [`requeue_busy`] with the pace as a parameter (unit tests pass zero).
pub async fn requeue_busy_paced<P, Fut>(
    job: &'static str,
    target: &str,
    attempt: u32,
    pace: Duration,
    push: P,
) -> Result<Requeue, BoxDynError>
where
    P: FnOnce(u32) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    if attempt >= BUSY_MAX_ATTEMPTS {
        tracing::error!(
            job,
            target,
            attempts = attempt,
            "lock still busy after the requeue budget; dropping the job",
        );
        return Ok(Requeue::Exhausted { attempts: attempt });
    }
    tokio::time::sleep(pace).await;
    let next = attempt + 1;
    match push(next).await {
        Ok(()) => {
            tracing::info!(job, target, attempt = next, "lock busy; requeued");
            Ok(Requeue::Requeued { attempt: next })
        }
        Err(e) => {
            tracing::error!(
                job,
                target,
                error = %e,
                "busy requeue push failed; returning Err for apalis retry",
            );
            Err(format!("{job} for {target}: busy requeue push failed: {e}").into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exhausted_budget_never_pushes() {
        let out = requeue_busy_paced("t", "x", BUSY_MAX_ATTEMPTS, Duration::ZERO, |_| async {
            panic!("push must not run once the budget is spent")
        })
        .await
        .unwrap();
        assert_eq!(
            out,
            Requeue::Exhausted {
                attempts: BUSY_MAX_ATTEMPTS
            }
        );
    }

    #[tokio::test]
    async fn requeue_bumps_the_counter() {
        let out = requeue_busy_paced("t", "x", 3, Duration::ZERO, |n| async move {
            assert_eq!(n, 4);
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(out, Requeue::Requeued { attempt: 4 });
    }

    #[tokio::test]
    async fn push_failure_is_an_error_for_apalis() {
        let err = requeue_busy_paced("t", "x", 0, Duration::ZERO, |_| async {
            Err("redis down".to_owned())
        })
        .await
        .expect_err("push failure propagates");
        assert!(err.to_string().contains("redis down"), "{err}");
    }
}
