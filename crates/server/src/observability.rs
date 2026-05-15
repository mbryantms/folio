//! Tracing + OpenTelemetry + Prometheus metrics setup (§12.4).
//!
//! - Structured JSON to stdout, level via `COMIC_LOG_LEVEL`.
//! - In-process ring buffer (M6d) — the same events also push into a bounded
//!   FIFO that backs `GET /admin/logs`. Capped at [`LOG_RING_CAPACITY`]; the
//!   oldest entry is evicted on every push past the cap.
//! - OpenTelemetry default-on (stdout exporter); OTLP when `COMIC_OTLP_ENDPOINT` set.
//! - Trace IDs threaded through requests via `tower-http`'s request-id layer (added in `app`).
//! - Prometheus recorder installed process-wide; `/metrics` reads from the
//!   returned [`PrometheusHandle`].

use crate::config::Config;
use chrono::{DateTime, Utc};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry, fmt, reload};

/// Maximum number of log events retained in the in-process ring buffer.
/// Sized for triage — if you need full history, ship logs to Loki.
pub const LOG_RING_CAPACITY: usize = 5_000;

/// One captured tracing event. `id` is monotonic; clients pass the most
/// recent one back as `?since=N` to pull only newer rows.
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub id: u64,
    pub timestamp: DateTime<Utc>,
    /// `'error' | 'warn' | 'info' | 'debug' | 'trace'`
    pub level: String,
    /// Module path (e.g. `server::api::reading_sessions`).
    pub target: String,
    pub message: String,
    /// Captured field key/value pairs, stringified.
    pub fields: BTreeMap<String, String>,
}

/// Shared ring buffer. Cheap to clone (Arc internally); locking is a single
/// `std::sync::Mutex` since the critical sections are O(1).
#[derive(Clone)]
pub struct LogRingBuffer {
    inner: Arc<RingInner>,
}

struct RingInner {
    next_id: AtomicU64,
    capacity: usize,
    buf: Mutex<VecDeque<LogEntry>>,
}

impl Default for LogRingBuffer {
    fn default() -> Self {
        Self::with_capacity(LOG_RING_CAPACITY)
    }
}

impl LogRingBuffer {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(RingInner {
                next_id: AtomicU64::new(1),
                capacity,
                buf: Mutex::new(VecDeque::with_capacity(capacity)),
            }),
        }
    }

    pub fn push(&self, mut entry: LogEntry) {
        entry.id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let mut buf = match self.inner.buf.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if buf.len() == self.inner.capacity {
            buf.pop_front();
        }
        buf.push_back(entry);
    }

    /// Snapshot the current contents matching the filter, in chronological
    /// order (oldest first). The returned `Vec` is a clone of the live
    /// queue — safe to release the lock immediately.
    pub fn snapshot(&self, filter: SnapshotFilter) -> Vec<LogEntry> {
        let buf = match self.inner.buf.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let mut out = Vec::with_capacity(buf.len().min(filter.limit));
        for entry in buf.iter() {
            if entry.id <= filter.since {
                continue;
            }
            if !filter.level.matches(&entry.level) {
                continue;
            }
            if let Some(needle) = filter.q {
                let needle = needle.to_lowercase();
                let hay = format!(
                    "{} {} {}",
                    entry.message.to_lowercase(),
                    entry.target.to_lowercase(),
                    entry
                        .fields
                        .values()
                        .map(|v| v.to_lowercase())
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                if !hay.contains(&needle) {
                    continue;
                }
            }
            out.push(entry.clone());
            if out.len() >= filter.limit {
                break;
            }
        }
        out
    }

    /// Capacity bound (not the live len). Exposed for the dashboard so the
    /// admin can see how big the buffer is.
    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }
}

/// Snapshot filter passed to [`LogRingBuffer::snapshot`].
pub struct SnapshotFilter<'a> {
    /// Return only entries with `id > since`.
    pub since: u64,
    /// Lower bound on level severity.
    pub level: LevelFilter,
    /// Free-text substring (case-insensitive) over message + target + fields.
    pub q: Option<&'a str>,
    /// Hard cap on returned entries. The default endpoint passes 500.
    pub limit: usize,
}

impl Default for SnapshotFilter<'_> {
    fn default() -> Self {
        Self {
            since: 0,
            level: LevelFilter::Trace,
            q: None,
            limit: 500,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum LevelFilter {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LevelFilter {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "error" | "err" => Self::Error,
            "warn" | "warning" => Self::Warn,
            "info" => Self::Info,
            "debug" => Self::Debug,
            "trace" => Self::Trace,
            _ => return None,
        })
    }

    fn rank(self) -> u8 {
        match self {
            Self::Error => 0,
            Self::Warn => 1,
            Self::Info => 2,
            Self::Debug => 3,
            Self::Trace => 4,
        }
    }

    fn rank_of(level: &str) -> u8 {
        match level {
            "error" => 0,
            "warn" => 1,
            "info" => 2,
            "debug" => 3,
            _ => 4,
        }
    }

    pub fn matches(self, level: &str) -> bool {
        Self::rank_of(level) <= self.rank()
    }
}

/// `tracing-subscriber` layer that copies each event into the ring buffer.
struct RingLayer {
    buffer: LogRingBuffer,
}

impl<S> Layer<S> for RingLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let level = level_str(metadata.level());
        let target = metadata.target().to_owned();

        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        // The conventional `message` field — usually the format string. If
        // missing, fall back to the metadata name (often the action verb).
        let message = visitor
            .fields
            .remove("message")
            .unwrap_or_else(|| metadata.name().to_owned());

        let entry = LogEntry {
            id: 0, // overwritten in push()
            timestamp: Utc::now(),
            level,
            target,
            message,
            fields: visitor.fields,
        };
        self.buffer.push(entry);
    }
}

#[derive(Default)]
struct FieldVisitor {
    fields: BTreeMap<String, String>,
}

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_owned(), value.to_owned());
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_owned(), value.to_string());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_owned(), value.to_string());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_owned(), value.to_string());
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields
            .insert(field.name().to_owned(), value.to_string());
    }
    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.fields
            .insert(field.name().to_owned(), value.to_string());
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let mut s = String::new();
        let _ = write!(s, "{value:?}");
        self.fields.insert(field.name().to_owned(), s);
    }
}

fn level_str(level: &Level) -> String {
    match *level {
        Level::ERROR => "error".into(),
        Level::WARN => "warn".into(),
        Level::INFO => "info".into(),
        Level::DEBUG => "debug".into(),
        Level::TRACE => "trace".into(),
    }
}

/// Handle to swap the active `EnvFilter` directive without restarting the
/// process. Replaced at runtime by `PATCH /admin/settings` when
/// `observability.log_level` changes (M4 of the runtime-config-admin plan).
pub type LogReloadHandle = reload::Handle<EnvFilter, Registry>;

/// Bootstrap result — both handles are needed by `app::serve`.
pub struct ObservabilityHandles {
    pub prometheus: PrometheusHandle,
    pub log_buffer: LogRingBuffer,
    pub log_reload: LogReloadHandle,
}

pub fn init(cfg: &Config) -> anyhow::Result<ObservabilityHandles> {
    let env_filter = EnvFilter::try_new(&cfg.log_level).unwrap_or_else(|_| EnvFilter::new("info"));
    let (filter_layer, log_reload) = reload::Layer::new(env_filter);

    let fmt_layer = fmt::layer()
        .json()
        .with_current_span(true)
        .with_span_list(false)
        .with_target(true);

    let log_buffer = LogRingBuffer::default();
    let ring_layer = RingLayer {
        buffer: log_buffer.clone(),
    };

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt_layer)
        .with(ring_layer)
        .init();

    if cfg.otlp_endpoint.is_some() {
        // The original §13 plan was to ship a tracing-opentelemetry
        // exporter behind this env var. After implementing Phases 2-3
        // it was reconsidered and dropped for v1 (2026-05-15): the
        // opentelemetry crate stack has a notoriously volatile compat
        // matrix, no real demand has materialized (the Prometheus
        // `/metrics` endpoint already covers the operator-monitoring
        // use case), and the runtime-config admin slice for OTLP
        // would still need design work. Logged here so a self-hoster
        // who set the env var sees a clear "not wired" hint.
        //
        // Re-evaluate if: a hosted Folio deployment ships (needs
        // remote-traces shipping), OR a user reports a real need.
        // See docs/dev/incompleteness-audit.md §D-9.
        tracing::info!(
            "COMIC_OTLP_ENDPOINT is set but OTLP wiring is intentionally not shipped in v1 \
             (considered, not chosen — see incompleteness-audit.md §D-9)"
        );
    }

    let prometheus = PrometheusBuilder::new()
        .install_recorder()
        .map_err(|e| anyhow::anyhow!("install prometheus recorder: {e}"))?;
    Ok(ObservabilityHandles {
        prometheus,
        log_buffer,
        log_reload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_evicts_oldest_past_capacity() {
        let ring = LogRingBuffer::with_capacity(3);
        for n in 0..5 {
            ring.push(LogEntry {
                id: 0,
                timestamp: Utc::now(),
                level: "info".into(),
                target: "t".into(),
                message: format!("msg{n}"),
                fields: BTreeMap::new(),
            });
        }
        let snap = ring.snapshot(SnapshotFilter::default());
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].message, "msg2");
        assert_eq!(snap[2].message, "msg4");
    }

    #[test]
    fn level_filter_drops_lower_severity() {
        let ring = LogRingBuffer::default();
        for level in ["error", "warn", "info", "debug", "trace"] {
            ring.push(LogEntry {
                id: 0,
                timestamp: Utc::now(),
                level: level.into(),
                target: "t".into(),
                message: format!("{level} msg"),
                fields: BTreeMap::new(),
            });
        }
        let snap = ring.snapshot(SnapshotFilter {
            level: LevelFilter::Warn,
            ..Default::default()
        });
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].level, "error");
        assert_eq!(snap[1].level, "warn");
    }

    #[test]
    fn since_filter_returns_only_newer() {
        let ring = LogRingBuffer::default();
        for n in 0..5 {
            ring.push(LogEntry {
                id: 0,
                timestamp: Utc::now(),
                level: "info".into(),
                target: "t".into(),
                message: format!("msg{n}"),
                fields: BTreeMap::new(),
            });
        }
        let snap_all = ring.snapshot(SnapshotFilter::default());
        assert_eq!(snap_all.len(), 5);
        let cutoff = snap_all[2].id;
        let snap = ring.snapshot(SnapshotFilter {
            since: cutoff,
            ..Default::default()
        });
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].message, "msg3");
        assert_eq!(snap[1].message, "msg4");
    }

    #[test]
    fn q_filter_substring_matches_case_insensitive() {
        let ring = LogRingBuffer::default();
        for msg in ["scan started", "SCAN COMPLETED", "thumb generated"] {
            ring.push(LogEntry {
                id: 0,
                timestamp: Utc::now(),
                level: "info".into(),
                target: "t".into(),
                message: msg.into(),
                fields: BTreeMap::new(),
            });
        }
        let snap = ring.snapshot(SnapshotFilter {
            q: Some("scan"),
            ..Default::default()
        });
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn level_parse_round_trips() {
        assert!(matches!(
            LevelFilter::parse("error").unwrap(),
            LevelFilter::Error
        ));
        assert!(matches!(
            LevelFilter::parse("WARN").unwrap(),
            LevelFilter::Warn
        ));
        assert!(LevelFilter::parse("garbage").is_none());
    }
}
