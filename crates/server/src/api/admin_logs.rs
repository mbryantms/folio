//! `GET /admin/logs` — read the in-process log ring buffer.
//!
//! M6d. Triage-grade only — production deployments should ship structured
//! JSON to Loki/Promtail. The ring is bounded ([`LOG_RING_CAPACITY`]); old
//! entries are dropped on every push past the cap.
//!
//! Filters:
//!   - `since` — monotonic id (the `id` of the last entry the client saw);
//!     pass it back to fetch only newer rows ("follow tail" mode).
//!   - `level` — minimum severity (error | warn | info | debug | trace).
//!   - `q`     — case-insensitive substring over message + target + fields.
//!   - `limit` — hard cap, default 500, max 5000.

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::error;
use crate::auth::RequireAdmin;
use crate::observability::{LOG_RING_CAPACITY, LevelFilter, LogEntry, SnapshotFilter};
use crate::state::AppState;
use server_macros::handler;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(list))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct LogsQuery {
    pub since: Option<u64>,
    pub level: Option<String>,
    pub q: Option<String>,
    pub limit: Option<usize>,
    /// Restrict to log entries whose `fields["library_id"]` matches
    /// this UUID. Populated by the scanner's instrumented spans
    /// (`scan_library`, `scan_series_folder`) — every event emitted
    /// under those spans inherits the library_id via the
    /// `RingLayer::on_event` parent-span walk. Omit / `all` for
    /// cross-library.
    pub library_id: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LogsResp {
    pub entries: Vec<LogEntryView>,
    /// Highest id in the response (or `since`, if empty). Pass back as
    /// `?since=` to tail.
    pub watermark: u64,
    /// Bound the ring buffer enforces; >= 1.
    pub capacity: usize,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LogEntryView {
    pub id: u64,
    pub timestamp: String,
    pub level: String,
    pub target: String,
    pub message: String,
    pub fields: serde_json::Value,
}

impl From<LogEntry> for LogEntryView {
    fn from(e: LogEntry) -> Self {
        let fields_obj: serde_json::Map<String, serde_json::Value> = e
            .fields
            .into_iter()
            .map(|(k, v)| (k, serde_json::Value::String(v)))
            .collect();
        Self {
            id: e.id,
            timestamp: e.timestamp.to_rfc3339(),
            level: e.level,
            target: e.target,
            message: e.message,
            fields: serde_json::Value::Object(fields_obj),
        }
    }
}

#[utoipa::path(
    operation_id = "admin_logs_list",    get,
    path = "/admin/logs",
    params(LogsQuery),
    responses(
        (status = 200, body = LogsResp),
        (status = 400, description = "validation error"),
        (status = 403, description = "admin only"),
    )
)]
#[handler]
pub async fn list(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<LogsQuery>,
) -> Response {
    let level = match q.level.as_deref() {
        Some(s) => match LevelFilter::parse(s) {
            Some(l) => l,
            None => {
                return error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation",
                    "level must be 'error', 'warn', 'info', 'debug', or 'trace'",
                );
            }
        },
        None => LevelFilter::Trace,
    };

    let limit = q.limit.unwrap_or(500).clamp(1, LOG_RING_CAPACITY);
    let since = q.since.unwrap_or(0);

    // Validate library_id format before the snapshot — bad UUIDs are
    // a caller bug, not an empty result.
    let library_filter = match q.library_id.as_deref() {
        None | Some("") | Some("all") => None,
        Some(raw) => match uuid::Uuid::parse_str(raw) {
            Ok(_) => Some(raw.to_owned()),
            Err(_) => {
                return error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "validation.library_id",
                    "library_id must be a UUID or 'all'",
                );
            }
        },
    };

    let snap = app.log_buffer.snapshot(SnapshotFilter {
        since,
        level,
        q: q.q.as_deref(),
        limit,
    });

    // Post-filter by library_id — the ring buffer doesn't index on
    // structured fields, so we apply this after the snapshot. Fine for
    // the ring's 5K-entry cap; would need an index if the buffer grew.
    let entries_iter: Box<dyn Iterator<Item = LogEntry>> = match library_filter.as_deref() {
        None => Box::new(snap.into_iter()),
        Some(lib_id) => Box::new(
            snap.into_iter()
                .filter(move |e| e.fields.get("library_id").map(|v| v.as_str()) == Some(lib_id)),
        ),
    };
    let entries: Vec<LogEntryView> = entries_iter.map(Into::into).collect();
    let watermark = entries.last().map(|e| e.id).unwrap_or(since);
    Json(LogsResp {
        entries,
        watermark,
        capacity: app.log_buffer.capacity(),
    })
    .into_response()
}
