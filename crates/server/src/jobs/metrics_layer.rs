//! Tower layer recording per-job outcome metrics for apalis workers:
//!
//! - `folio_jobs_processed_total{kind,status}` — counter (`status` = `success`|`failed`)
//! - `folio_job_duration_seconds{kind}` — histogram
//!
//! Modelled on apalis's built-in `PrometheusLayer`, but with `folio_`-prefixed
//! names (the built-in emits an un-prefixed, ambiguous `requests_total`) and a
//! stable `kind` label set per-worker, rather than the Rust type path. Emits
//! through the global `metrics` recorder, so it renders from the same
//! `/metrics` handle as everything else.
//!
//! It also reports each job's start and finish to the
//! [`JobActivity`](super::activity::JobActivity) hub behind the admin queue
//! lanes. The apalis 1.0 `Task` derefs to its `ExecutionContext`, so the task
//! id (the Redis member id) and the shared attempt counter are read right
//! here; the `kind` doubles as the queue label.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use apalis::prelude::Task;
use tower::{Layer, Service};

use super::activity::JobActivity;

/// Apply with `.layer(JobMetricsLayer::new("scan"))` on a `WorkerBuilder`; the
/// `kind` becomes the metric label (use the worker's name).
#[derive(Clone, Debug)]
pub struct JobMetricsLayer {
    kind: &'static str,
    activity: Option<Arc<JobActivity>>,
}

impl JobMetricsLayer {
    pub fn new(kind: &'static str) -> Self {
        Self {
            kind,
            activity: None,
        }
    }

    /// Also report each job's start and finish to `activity`.
    pub fn with_activity(mut self, activity: Arc<JobActivity>) -> Self {
        self.activity = Some(activity);
        self
    }
}

impl<S> Layer<S> for JobMetricsLayer {
    type Service = JobMetricsService<S>;
    fn layer(&self, service: S) -> Self::Service {
        JobMetricsService {
            service,
            kind: self.kind,
            activity: self.activity.clone(),
        }
    }
}

#[derive(Clone)]
pub struct JobMetricsService<S> {
    service: S,
    kind: &'static str,
    activity: Option<Arc<JobActivity>>,
}

impl<S, Args, Res, Err> Service<Task<Args>> for JobMetricsService<S>
where
    S: Service<Task<Args>, Response = Res, Error = Err>,
    S::Future: Send + 'static,
    Res: Send + 'static,
    Err: std::fmt::Display + Send + 'static,
{
    type Response = Res;
    type Error = Err;
    type Future = Pin<Box<dyn Future<Output = Result<Res, Err>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    fn call(&mut self, req: Task<Args>) -> Self::Future {
        let kind = self.kind;
        let start = Instant::now();
        // The attempt counter is shared with the task, so reading it after
        // the handler sees the value the run actually used.
        let attempt = req.raw_attempt().clone();
        let mut started = match (&self.activity, req.task_id()) {
            (Some(hub), Some(id)) => Some(ActivityGuard {
                job: Some(hub.start(kind, id.to_string(), attempt_u32(attempt.current()))),
                hub: hub.clone(),
            }),
            _ => None,
        };
        let fut = self.service.call(req);
        Box::pin(async move {
            let res = fut.await;
            if let Some(guard) = started.as_mut()
                && let Some(job) = guard.job.take()
            {
                let error = res.as_ref().err().map(ToString::to_string);
                guard
                    .hub
                    .finish(job, res.is_ok(), error, attempt_u32(attempt.current()));
            }
            let status = if res.is_ok() { "success" } else { "failed" };
            metrics::counter!("folio_jobs_processed_total", "kind" => kind, "status" => status)
                .increment(1);
            metrics::histogram!("folio_job_duration_seconds", "kind" => kind)
                .record(start.elapsed().as_secs_f64());
            res
        })
    }
}

/// apalis counts attempts from 0 before the first run; report "not yet
/// counted" as `None` rather than a misleading attempt 0.
fn attempt_u32(n: usize) -> Option<u32> {
    (n > 0).then(|| u32::try_from(n).unwrap_or(u32::MAX))
}

/// Clears a job from the hub's running set if its future is dropped before
/// finishing (shutdown mid-job), so the Running lane never shows a ghost.
struct ActivityGuard {
    hub: Arc<JobActivity>,
    job: Option<super::activity::StartedJob>,
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            self.hub.abandon(job);
        }
    }
}
