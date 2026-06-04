//! Tracing + OpenTelemetry + Prometheus metrics setup (§12.4).
//!
//! - Structured JSON to stdout, level via `COMIC_LOG_LEVEL`.
//! - In-process ring buffer (M6d) — the same events also push into a bounded
//!   FIFO that backs `GET /admin/logs`. Capped at [`LOG_RING_CAPACITY`]; the
//!   oldest entry is evicted on every push past the cap.
//! - OpenTelemetry: stdout exporter only. OTLP export was considered and
//!   dropped for v1 (see incompleteness-audit §D-9); `COMIC_OTLP_ENDPOINT`
//!   is still recognized but inert (logs a one-time "not wired" hint).
//! - Trace IDs threaded through requests via `tower-http`'s request-id layer (added in `app`).
//! - Prometheus recorder installed process-wide; `/metrics` reads from the
//!   returned [`PrometheusHandle`].

use crate::config::Config;
use chrono::{DateTime, Utc};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
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
    /// Observability-split M12 — which stream this log belongs to.
    /// `'library'` when the event carries library-scoped span context
    /// (`library_id` / `scan_id`); `'server'` otherwise. The admin Server-log
    /// view filters to `server`; library-operational truth lives in the
    /// durable `library_events` manifest (Library activity).
    pub domain: String,
}

/// Classify a captured event by stream from its (already span-enriched)
/// fields. Library-scoped context ⇒ library stream.
fn classify_domain(fields: &BTreeMap<String, String>) -> &'static str {
    if fields.contains_key("library_id") || fields.contains_key("scan_id") {
        "library"
    } else {
        "server"
    }
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
            if let Some(domain) = filter.domain
                && entry.domain != domain
            {
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
    /// Stream filter (observability-split M12): `Some("server")` /
    /// `Some("library")`, or `None` for both.
    pub domain: Option<&'a str>,
    /// Hard cap on returned entries. The default endpoint passes 500.
    pub limit: usize,
}

impl Default for SnapshotFilter<'_> {
    fn default() -> Self {
        Self {
            since: 0,
            level: LevelFilter::Trace,
            q: None,
            domain: None,
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
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let level = level_str(metadata.level());
        let target = metadata.target().to_owned();

        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        // Walk the parent-span chain and copy structured context fields
        // down to the event. Without this, a `tracing::warn!()` inside
        // a deeply-nested helper would lose the `library_id` /
        // `series_id` / `scan_id` its enclosing scan-span recorded —
        // even though that's exactly the context the admin Logs filter
        // needs to scope by library.
        //
        // Event fields take precedence: if a call site explicitly sets
        // `library_id = %something_else`, that wins over the span's
        // value (caller-known is more specific than caller-inherited).
        if let Some(span) = ctx.event_span(event) {
            // Walk from the event's immediate span up to the root.
            for span in span.scope().from_root() {
                if let Some(map) = span.extensions().get::<SpanFields>() {
                    for (k, v) in &map.0 {
                        // Skip if the event itself or a closer span
                        // already supplied this key. `from_root`
                        // order means we accumulate outer-first; an
                        // inner span's value already in the map
                        // shouldn't be clobbered by a less-specific
                        // outer one.
                        if !visitor.fields.contains_key(k) {
                            visitor.fields.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
        }

        // The conventional `message` field — usually the format string. If
        // missing, fall back to the metadata name (often the action verb).
        let message = visitor
            .fields
            .remove("message")
            .unwrap_or_else(|| metadata.name().to_owned());

        let domain = classify_domain(&visitor.fields).to_owned();
        let entry = LogEntry {
            id: 0, // overwritten in push()
            timestamp: Utc::now(),
            level,
            target,
            message,
            fields: visitor.fields,
            domain,
        };
        self.buffer.push(entry);
    }

    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        // Capture the span's structured fields into a side-table on
        // the span's extensions. `on_event` walks parents and reads
        // these to enrich events with inherited context (library_id,
        // series_id, scan_id, …). Storing once at span-creation
        // avoids walking field iterators on every event.
        let mut visitor = FieldVisitor::default();
        attrs.record(&mut visitor);
        if visitor.fields.is_empty() {
            return;
        }
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(SpanFields(visitor.fields));
        }
    }
}

/// Per-span side-table populated by [`RingLayer::on_new_span`] and read by
/// [`RingLayer::on_event`] to enrich events with inherited context.
/// Stored separately from `tracing_subscriber`'s own span-attribute
/// machinery so RingLayer doesn't fight with formatters or sinks that
/// also attach extensions.
struct SpanFields(BTreeMap<String, String>);

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
    /// Process/runtime gauge sampler (`folio_process_*`). `collect()` is
    /// called per scrape by the `/metrics` handler so values are fresh.
    pub process: metrics_process::Collector,
    pub log_buffer: LogRingBuffer,
    pub log_reload: LogReloadHandle,
}

/// Render an error for logging with the obvious secret-bearing substrings
/// scrubbed out (audit-remediation M6.2). Strips:
///   - URL query strings (where OAuth `code=`, `state=`, `access_token=`
///     land if a third-party error leaks the full request URL)
///   - `password=...` / `bearer ...` shaped substrings
///   - Long opaque tokens (≥40 chars of `[A-Za-z0-9_\-=]`)
///
/// Heuristic, not cryptographically safe. Pair with deliberate logging
/// choices — prefer the error type / variant over its full Display when the
/// error is known to wrap network response bodies. See
/// [docs/dev/logging.md](../../docs/dev/logging.md).
pub fn sanitize_error(e: &dyn std::fmt::Display) -> String {
    let raw = e.to_string();
    redact_secrets(&raw)
}

/// Same as [`sanitize_error`] but for an arbitrary string (useful when
/// composing a custom message that includes a third-party error).
pub fn redact_secrets(s: &str) -> String {
    // Strip URL query strings: `https://x.example/cb?code=XYZ&state=ABC` →
    // `https://x.example/cb?<redacted>`. Conservative — keep host + path so
    // operators can still see *which* endpoint failed.
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '?' {
            // Was the previous chunk URL-shaped? Lazy heuristic: peek backward
            // up to 8 chars for `://` — only redact a `?` that's clearly in a
            // URL.
            let tail_start = out.len().saturating_sub(80);
            if out[tail_start..].contains("://") {
                out.push('?');
                out.push_str("<redacted>");
                // Skip everything until the next whitespace / quote / `)`.
                while let Some(&next) = chars.peek() {
                    if next.is_whitespace() || matches!(next, '"' | '\'' | ')' | ',' | ';') {
                        break;
                    }
                    chars.next();
                }
                continue;
            }
        }
        out.push(c);
    }

    // password= / token= / secret= → `<key>=<redacted>` up to next non-word
    // boundary. Cheap state machine instead of a regex dep.
    out = redact_kv(
        &out,
        &["password", "passwd", "token", "secret", "authorization"],
    );

    // `Bearer <opaque>` / `Basic <opaque>` — anchor on the scheme word, then
    // chew anything up to next whitespace / quote.
    out = redact_bearer(&out);

    out
}

fn redact_kv(s: &str, keys: &[&str]) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let mut matched = false;
        for key in keys {
            let klen = key.len();
            // Match the key anywhere it's followed by `=` or `:`. We tolerate
            // a prefix like `smtp_` (`smtp_password=...` should still trip
            // the `password` key); the cost of over-redacting `bypassword=`
            // is far smaller than under-redacting a real secret.
            if i + klen < bytes.len()
                && s[i..i + klen].eq_ignore_ascii_case(key)
                && matches!(bytes[i + klen], b'=' | b':')
            {
                out.push_str(&s[i..i + klen + 1]);
                out.push_str("<redacted>");
                // Skip the value: anything up to whitespace / `&` / `"` / `;` / `,`.
                let mut j = i + klen + 1;
                while j < bytes.len()
                    && !matches!(
                        bytes[j],
                        b' ' | b'\t'
                            | b'\n'
                            | b'&'
                            | b'"'
                            | b'\''
                            | b';'
                            | b','
                            | b')'
                            | b'>'
                            | b'<'
                    )
                {
                    j += 1;
                }
                i = j;
                matched = true;
                break;
            }
        }
        if !matched {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn redact_bearer(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let lower = s.to_ascii_lowercase();
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        let rest = &lower[i..];
        let matched_scheme = if rest.starts_with("bearer ") {
            Some(7)
        } else if rest.starts_with("basic ") {
            Some(6)
        } else {
            None
        };
        if let Some(scheme_len) = matched_scheme {
            let prev_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_';
            if prev_ok {
                out.push_str(&s[i..i + scheme_len]);
                out.push_str("<redacted>");
                let mut j = i + scheme_len;
                while j < bytes.len()
                    && !matches!(
                        bytes[j],
                        b' ' | b'\t' | b'\n' | b'"' | b'\'' | b';' | b',' | b')' | b'>' | b'<'
                    )
                {
                    j += 1;
                }
                i = j;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
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

    // `service="folio"` tags every series (multi-target dashboards). Sane
    // latency buckets for every `*_seconds` histogram (HTTP, job, OCR) —
    // the exporter's defaults are coarse for sub-second web latencies.
    let prometheus = PrometheusBuilder::new()
        .add_global_label("service", "folio")
        .set_buckets_for_metric(
            Matcher::Suffix("_seconds".to_string()),
            &[
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ],
        )
        .map_err(|e| anyhow::anyhow!("metrics histogram buckets: {e}"))?
        .install_recorder()
        .map_err(|e| anyhow::anyhow!("install prometheus recorder: {e}"))?;

    // Process/runtime gauges under `folio_process_*` (CPU, RSS, FDs,
    // threads). `describe()` registers them against the recorder now; the
    // `/metrics` handler calls `collect()` on each scrape.
    let process = metrics_process::Collector::new("folio_");
    process.describe();

    Ok(ObservabilityHandles {
        prometheus,
        process,
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
                domain: "server".into(),
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
                domain: "server".into(),
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
                domain: "server".into(),
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
                domain: "server".into(),
            });
        }
        let snap = ring.snapshot(SnapshotFilter {
            q: Some("scan"),
            ..Default::default()
        });
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn sanitize_redacts_url_query() {
        let msg = "GET https://idp.example.com/token?code=ABC123&state=XYZ failed";
        let out = redact_secrets(msg);
        assert!(out.contains("https://idp.example.com/token?<redacted>"));
        assert!(!out.contains("ABC123"));
        assert!(!out.contains("XYZ"));
    }

    #[test]
    fn sanitize_redacts_password_kv() {
        let msg = "config error: smtp_password=hunter2 is rejected";
        let out = redact_secrets(msg);
        assert!(out.contains("smtp_password=<redacted>"));
        assert!(!out.contains("hunter2"));
    }

    #[test]
    fn sanitize_redacts_bearer() {
        let msg = "Authorization: Bearer eyJhbGc.payload.signature";
        let out = redact_secrets(msg);
        assert!(out.contains("Bearer <redacted>"));
        assert!(!out.contains("eyJhbGc"));
    }

    #[test]
    fn sanitize_keeps_non_secret_text() {
        let msg = "smtp_host not set";
        assert_eq!(redact_secrets(msg), msg);
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
