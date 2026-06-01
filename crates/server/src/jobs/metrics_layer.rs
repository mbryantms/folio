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

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use apalis::prelude::{Error, Request};
use tower::{Layer, Service};

/// Apply with `.layer(JobMetricsLayer::new("scan"))` on a `WorkerBuilder`; the
/// `kind` becomes the metric label (use the worker's name).
#[derive(Clone, Copy, Debug)]
pub struct JobMetricsLayer {
    kind: &'static str,
}

impl JobMetricsLayer {
    pub fn new(kind: &'static str) -> Self {
        Self { kind }
    }
}

impl<S> Layer<S> for JobMetricsLayer {
    type Service = JobMetricsService<S>;
    fn layer(&self, service: S) -> Self::Service {
        JobMetricsService {
            service,
            kind: self.kind,
        }
    }
}

#[derive(Clone)]
pub struct JobMetricsService<S> {
    service: S,
    kind: &'static str,
}

impl<S, Req, Ctx, Res> Service<Request<Req, Ctx>> for JobMetricsService<S>
where
    S: Service<Request<Req, Ctx>, Response = Res, Error = Error>,
    S::Future: Send + 'static,
    Res: Send + 'static,
{
    type Response = Res;
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<Res, Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Req, Ctx>) -> Self::Future {
        let kind = self.kind;
        let start = Instant::now();
        let fut = self.service.call(req);
        Box::pin(async move {
            let res = fut.await;
            let status = if res.is_ok() { "success" } else { "failed" };
            metrics::counter!("folio_jobs_processed_total", "kind" => kind, "status" => status)
                .increment(1);
            metrics::histogram!("folio_job_duration_seconds", "kind" => kind)
                .record(start.elapsed().as_secs_f64());
            res
        })
    }
}
