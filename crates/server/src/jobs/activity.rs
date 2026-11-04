//! Per-job lifecycle hub for the admin queue lanes (Folio Flair phase 5, B3).
//!
//! [`JobMetricsLayer`](super::metrics_layer::JobMetricsLayer) wraps every
//! apalis worker, so it sees each job start and finish. It reports both here.
//! The hub keeps three things, all process-local:
//!
//! - the jobs executing right now, with when they started (the Running lane's
//!   elapsed timer; Redis only knows when a worker *fetched* a job, in whole
//!   seconds, and a prefetched job has not started at all);
//! - a ring of the last [`RECENT_PER_QUEUE`] completions per queue (the Done
//!   lane: apalis keeps no ordered record of finished jobs);
//! - what changed since the last flush. Every [`FLUSH_INTERVAL`] the flusher
//!   turns that into **one** aggregated `queue.activity` event, and only when
//!   something changed. A thumbnail storm finishing hundreds of jobs a second
//!   still costs two events a second, never one per job.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::library::events::ScanEvent;
use crate::state::AppState;

/// Completions kept per queue for the Done lane.
pub const RECENT_PER_QUEUE: usize = 50;

/// How often pending activity is flushed as one `queue.activity` event.
pub const FLUSH_INTERVAL: Duration = Duration::from_millis(500);

/// Most started ids / finished entries one queue carries in a single event.
/// The `*_count` fields stay exact; clients refetch the listing anyway.
pub const MAX_EVENT_ENTRIES: usize = 50;

/// Longest error string carried in an event or the ring.
const MAX_ERROR_CHARS: usize = 300;

/// One finished job inside a `queue.activity` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinishedActivity {
    pub task_id: String,
    pub ok: bool,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A queue's lane totals, as the `/admin/queue/jobs` listing counts them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LaneCounts {
    /// In the `active` list or due in the `scheduled` set: waiting for a worker.
    pub waiting: i64,
    /// Retry-delayed: in the `scheduled` set with a future `run_at`.
    pub scheduled: i64,
    /// Fetched by a worker: executing, or in its prefetch buffer.
    pub running: i64,
}

/// One queue's share of a `queue.activity` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueActivity {
    pub queue: String,
    /// Task ids that started executing since the last event (capped at
    /// [`MAX_EVENT_ENTRIES`]; `started_count` is exact).
    pub started: Vec<String>,
    pub started_count: u64,
    /// Jobs that finished since the last event (capped the same way).
    pub finished: Vec<FinishedActivity>,
    pub finished_count: u64,
    /// Current lane totals, when the Redis read succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counts: Option<LaneCounts>,
}

/// A job a worker is executing right now.
#[derive(Debug, Clone)]
pub struct RunningJob {
    pub started_at: DateTime<Utc>,
    pub attempt: Option<u32>,
}

/// A completed job in the per-queue ring.
#[derive(Debug, Clone, PartialEq)]
pub struct RecentJob {
    pub task_id: String,
    pub ok: bool,
    pub error: Option<String>,
    pub attempt: Option<u32>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub duration_ms: u64,
}

#[derive(Debug, Default)]
struct Pending {
    started: Vec<String>,
    started_count: u64,
    finished: Vec<FinishedActivity>,
    finished_count: u64,
}

#[derive(Debug, Default)]
struct Inner {
    running: HashMap<(&'static str, String), RunningJob>,
    recent: HashMap<&'static str, VecDeque<RecentJob>>,
    /// Ordered by queue so an event lists queues in a stable order.
    pending: BTreeMap<&'static str, Pending>,
}

/// See the module docs. Lives in [`AppState`] behind an `Arc`.
#[derive(Debug, Default)]
pub struct JobActivity {
    inner: Mutex<Inner>,
}

/// Handed back by [`JobActivity::start`]; pass it to [`JobActivity::finish`].
#[derive(Debug)]
pub struct StartedJob {
    queue: &'static str,
    task_id: String,
    started: Instant,
    started_at: DateTime<Utc>,
}

impl JobActivity {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves plain bookkeeping behind;
        // keep serving it rather than poisoning every worker.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A worker began executing `task_id` from `queue`.
    pub fn start(&self, queue: &'static str, task_id: String, attempt: Option<u32>) -> StartedJob {
        let started_at = Utc::now();
        let mut inner = self.lock();
        inner.running.insert(
            (queue, task_id.clone()),
            RunningJob {
                started_at,
                attempt,
            },
        );
        let p = inner.pending.entry(queue).or_default();
        p.started_count += 1;
        if p.started.len() < MAX_EVENT_ENTRIES {
            p.started.push(task_id.clone());
        }
        StartedJob {
            queue,
            task_id,
            started: Instant::now(),
            started_at,
        }
    }

    /// The job [`Self::start`] handed back finished. `error` is the handler's
    /// error for a failed run (`ok == false`).
    pub fn finish(&self, job: StartedJob, ok: bool, error: Option<String>, attempt: Option<u32>) {
        let duration_ms = job.started.elapsed().as_millis() as u64;
        let finished_at = Utc::now();
        let error = error.map(|e| truncate_chars(&e, MAX_ERROR_CHARS));
        let mut inner = self.lock();
        let running = inner.running.remove(&(job.queue, job.task_id.clone()));
        let attempt = attempt.or(running.and_then(|r| r.attempt));
        let ring = inner.recent.entry(job.queue).or_default();
        if ring.len() == RECENT_PER_QUEUE {
            ring.pop_back();
        }
        ring.push_front(RecentJob {
            task_id: job.task_id.clone(),
            ok,
            error: error.clone(),
            attempt,
            started_at: job.started_at,
            finished_at,
            duration_ms,
        });
        let p = inner.pending.entry(job.queue).or_default();
        p.finished_count += 1;
        if p.finished.len() < MAX_EVENT_ENTRIES {
            p.finished.push(FinishedActivity {
                task_id: job.task_id,
                ok,
                duration_ms,
                error,
            });
        }
    }

    /// The job's future was dropped before it finished (shutdown): forget it
    /// without recording a completion.
    pub fn abandon(&self, job: StartedJob) {
        self.lock().running.remove(&(job.queue, job.task_id));
    }

    /// When the hub saw `task_id` start, if it is executing right now.
    pub fn running(&self, queue: &str, task_id: &str) -> Option<RunningJob> {
        let inner = self.lock();
        inner
            .running
            .iter()
            .find(|((q, id), _)| *q == queue && id == task_id)
            .map(|(_, r)| r.clone())
    }

    /// Every job executing right now in `queue`, as `(task_id, job)`.
    pub fn running_in(&self, queue: &str) -> Vec<(String, RunningJob)> {
        let inner = self.lock();
        let mut out: Vec<(String, RunningJob)> = inner
            .running
            .iter()
            .filter(|((q, _), _)| *q == queue)
            .map(|((_, id), r)| (id.clone(), r.clone()))
            .collect();
        out.sort_by_key(|(_, r)| r.started_at);
        out
    }

    /// The completion ring for `queue`, newest first.
    pub fn recent(&self, queue: &str) -> Vec<RecentJob> {
        let inner = self.lock();
        inner
            .recent
            .iter()
            .find(|(q, _)| **q == queue)
            .map(|(_, ring)| ring.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Drain what changed since the last call, one entry per queue that saw
    /// a start or a finish. `None` when nothing happened.
    pub fn take_pending(&self) -> Option<Vec<QueueActivity>> {
        let mut inner = self.lock();
        if inner.pending.is_empty() {
            return None;
        }
        let pending = std::mem::take(&mut inner.pending);
        Some(
            pending
                .into_iter()
                .map(|(queue, p)| QueueActivity {
                    queue: queue.to_owned(),
                    started: p.started,
                    started_count: p.started_count,
                    finished: p.finished,
                    finished_count: p.finished_count,
                    counts: None,
                })
                .collect(),
        )
    }

    /// Build the event for one flush: drained activity plus, when the Redis
    /// read succeeds, each listed queue's lane totals.
    pub async fn flush_event(&self, state: &AppState) -> Option<ScanEvent> {
        let mut queues = self.take_pending()?;
        let labels: Vec<&str> = queues.iter().map(|q| q.queue.as_str()).collect();
        match state.jobs.lane_counts(&labels).await {
            Ok(counts) => {
                for (q, (_, c)) in queues.iter_mut().zip(counts) {
                    q.counts = Some(c);
                }
            }
            Err(e) => tracing::debug!(error = %e, "queue activity: lane counts failed"),
        }
        Some(ScanEvent::QueueActivity {
            at: Utc::now(),
            queues,
        })
    }
}

/// Flush the hub every [`FLUSH_INTERVAL`] until `shutdown`. Started next to
/// the apalis monitor in `app::serve`.
pub fn spawn_flusher(
    state: AppState,
    shutdown: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(FLUSH_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = tick.tick() => {}
            }
            if let Some(evt) = state.job_activity.flush_event(&state).await {
                state.events.emit(evt);
            }
        }
    })
}

fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((idx, _)) => format!("{}…", &s[..idx]),
        None => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_pending_means_no_event() {
        let hub = JobActivity::new();
        assert!(hub.take_pending().is_none());
    }

    #[test]
    fn starts_and_finishes_aggregate_per_queue_and_drain_once() {
        let hub = JobActivity::new();
        let a = hub.start("post_scan_thumbs", "A".into(), Some(1));
        let b = hub.start("post_scan_thumbs", "B".into(), Some(1));
        let c = hub.start("scan", "C".into(), None);
        hub.finish(a, true, None, Some(1));
        hub.finish(c, false, Some("boom".into()), Some(2));

        let mut queues = hub.take_pending().expect("activity pending");
        queues.sort_by(|x, y| x.queue.cmp(&y.queue));
        assert_eq!(queues.len(), 2);
        let thumbs = &queues[0];
        assert_eq!(thumbs.queue, "post_scan_thumbs");
        assert_eq!(thumbs.started, vec!["A".to_owned(), "B".to_owned()]);
        assert_eq!(thumbs.started_count, 2);
        assert_eq!(thumbs.finished.len(), 1);
        assert_eq!(thumbs.finished[0].task_id, "A");
        assert!(thumbs.finished[0].ok);
        let scan = &queues[1];
        assert_eq!(scan.finished[0].error.as_deref(), Some("boom"));
        assert!(!scan.finished[0].ok);

        // Drained: a second flush with no new activity sends nothing.
        assert!(hub.take_pending().is_none());

        // B is still running; A and C are not.
        assert!(hub.running("post_scan_thumbs", "B").is_some());
        assert!(hub.running("post_scan_thumbs", "A").is_none());
        assert_eq!(hub.running_in("post_scan_thumbs").len(), 1);
        hub.finish(b, true, None, None);
        assert!(hub.running_in("post_scan_thumbs").is_empty());
        let after = hub.take_pending().expect("B finishing is activity");
        assert_eq!(after[0].started_count, 0);
        assert_eq!(after[0].finished_count, 1);
    }

    #[test]
    fn a_storm_is_capped_per_event_but_counted_exactly() {
        let hub = JobActivity::new();
        for i in 0..(MAX_EVENT_ENTRIES + 30) {
            let j = hub.start("post_scan_thumbs", format!("t{i}"), None);
            hub.finish(j, true, None, None);
        }
        let q = &hub.take_pending().unwrap()[0];
        assert_eq!(q.started.len(), MAX_EVENT_ENTRIES);
        assert_eq!(q.finished.len(), MAX_EVENT_ENTRIES);
        assert_eq!(q.started_count, (MAX_EVENT_ENTRIES + 30) as u64);
        assert_eq!(q.finished_count, (MAX_EVENT_ENTRIES + 30) as u64);
    }

    #[test]
    fn ring_keeps_the_newest_completions_first() {
        let hub = JobActivity::new();
        for i in 0..(RECENT_PER_QUEUE + 5) {
            let j = hub.start("scan", format!("t{i}"), Some(1));
            hub.finish(j, i % 2 == 0, None, None);
        }
        let ring = hub.recent("scan");
        assert_eq!(ring.len(), RECENT_PER_QUEUE);
        assert_eq!(ring[0].task_id, format!("t{}", RECENT_PER_QUEUE + 4));
        assert_eq!(ring.last().unwrap().task_id, "t5");
        // The attempt recorded at start carries over when finish has none.
        assert_eq!(ring[0].attempt, Some(1));
        assert!(hub.recent("archive_edit").is_empty());
    }

    #[test]
    fn long_errors_are_truncated() {
        let hub = JobActivity::new();
        let j = hub.start("scan", "x".into(), None);
        hub.finish(j, false, Some("é".repeat(1000)), None);
        let err = hub.recent("scan")[0].error.clone().unwrap();
        assert_eq!(err.chars().count(), MAX_ERROR_CHARS + 1);
        assert!(err.ends_with('…'));
    }
}
