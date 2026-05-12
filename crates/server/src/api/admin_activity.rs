//! `GET /admin/activity` — combined operational feed.
//!
//! M6d. Pulls from four sources and merges them in a single Postgres UNION
//! ALL so the cursor logic is consistent:
//!
//!   - audit  : `audit_log` (admin / security actions)
//!   - scan   : `scan_runs` (library / series / issue scans)
//!   - health : open `library_health_issues` (resolved + dismissed are skipped)
//!   - reading: per-hour aggregate of `reading_sessions` — never per-user;
//!     the dashboard owner is the admin, so we only report volume.
//!
//! Cursor: opaque base64 of the last row's `(ts, kind, source_id)` triple.
//! The server filters `(ts, kind, source_id) < cursor` lexicographically so
//! pagination is stable even across mixed-kind pages.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use base64::Engine;
use chrono::{DateTime, FixedOffset};
use sea_orm::{ConnectionTrait, FromQueryResult, Statement, Value};
use serde::{Deserialize, Serialize};

use crate::auth::RequireAdmin;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/admin/activity", get(list))
}

const DEFAULT_LIMIT: u64 = 50;
const MAX_LIMIT: u64 = 200;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ActivityQuery {
    pub limit: Option<u64>,
    pub cursor: Option<String>,
    /// Comma-separated list of kinds to include. Defaults to all.
    /// Allowed: `audit`, `scan`, `health`, `reading`.
    pub kinds: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ActivityListView {
    pub entries: Vec<ActivityEntryView>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ActivityEntryView {
    /// `'audit' | 'scan' | 'health' | 'reading'`.
    pub kind: String,
    /// Stable identifier within the kind. For aggregates, the bucket time.
    pub source_id: String,
    pub timestamp: String,
    pub summary: String,
    /// Kind-specific structured fields (action, severity, library_id, ...).
    pub payload: serde_json::Value,
}

#[utoipa::path(
    get,
    path = "/admin/activity",
    params(ActivityQuery),
    responses(
        (status = 200, body = ActivityListView),
        (status = 400, description = "validation error"),
        (status = 403, description = "admin only"),
    )
)]
pub async fn list(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<ActivityQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let kinds = parse_kinds(q.kinds.as_deref());
    if kinds.is_empty() {
        // Caller passed `kinds=` with no valid entries — return an empty
        // list rather than 400, since "no kinds selected" is a UI state.
        return Json(ActivityListView {
            entries: Vec::new(),
            next_cursor: None,
        })
        .into_response();
    }

    let cursor = match q.cursor.as_deref() {
        Some(c) => match decode_cursor(c) {
            Some(parsed) => Some(parsed),
            None => return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor"),
        },
        None => None,
    };

    let backend = app.db.get_database_backend();

    // Build the UNION ALL of the kinds the caller wants. Param order is:
    //   $1 reading hour cap (60d ago) — bounds the aggregate query.
    //   $2..$3 cursor.ts / cursor.kind — when present, repeated per
    //   sub-select. We pre-format param numbers per branch for clarity.
    let mut params: Vec<Value> = Vec::new();
    let lookback = (chrono::Utc::now() - chrono::Duration::days(60)).fixed_offset();
    params.push(lookback.into());

    // WHERE is evaluated before SELECT so we reference the underlying
    // column expressions per-kind rather than the outer `ts/kind/source_id`
    // aliases. Each branch contributes its own tuple comparison.
    let has_cursor = cursor.is_some();
    if let Some((cts, ckind, cid)) = cursor.as_ref() {
        params.push((*cts).into());
        params.push(ckind.clone().into());
        params.push(cid.clone().into());
    }

    let mut selects: Vec<String> = Vec::new();
    if kinds.contains(&"audit") {
        let cur = if has_cursor {
            " AND (created_at, 'audit', id::text) < ($2, $3, $4)"
        } else {
            ""
        };
        selects.push(format!(
            "SELECT 'audit'::text AS kind, id::text AS source_id, created_at AS ts, \
                    actor_type AS summary_a, action AS summary_b, \
                    jsonb_build_object('actor_id', actor_id::text, 'actor_type', actor_type, \
                                       'action', action, 'target_type', target_type, \
                                       'target_id', target_id, 'payload', payload) AS payload \
             FROM audit_log WHERE 1=1{cur}",
        ));
    }
    if kinds.contains(&"scan") {
        let cur = if has_cursor {
            " AND (started_at, 'scan', id::text) < ($2, $3, $4)"
        } else {
            ""
        };
        selects.push(format!(
            "SELECT 'scan'::text AS kind, id::text AS source_id, started_at AS ts, \
                    state AS summary_a, kind AS summary_b, \
                    jsonb_build_object('library_id', library_id::text, 'state', state, \
                                       'kind', kind, 'series_id', series_id::text, \
                                       'issue_id', issue_id, 'error', error) AS payload \
             FROM scan_runs WHERE 1=1{cur}",
        ));
    }
    if kinds.contains(&"health") {
        let cur = if has_cursor {
            " AND (last_seen_at, 'health', id::text) < ($2, $3, $4)"
        } else {
            ""
        };
        selects.push(format!(
            "SELECT 'health'::text AS kind, id::text AS source_id, last_seen_at AS ts, \
                    severity AS summary_a, kind AS summary_b, \
                    jsonb_build_object('library_id', library_id::text, 'severity', severity, \
                                       'kind', kind, 'fingerprint', fingerprint, \
                                       'first_seen_at', first_seen_at, 'last_seen_at', last_seen_at) AS payload \
             FROM library_health_issues \
             WHERE resolved_at IS NULL AND dismissed_at IS NULL{cur}",
        ));
    }
    if kinds.contains(&"reading") {
        // Hourly buckets — never per-user, never identifies anyone.
        // The cursor comparison goes in HAVING because it references the
        // GROUP BY expression rather than a raw column.
        let cur = if has_cursor {
            " AND (date_trunc('hour', started_at), 'reading', \
              EXTRACT(EPOCH FROM date_trunc('hour', started_at))::bigint::text) \
              < ($2, $3, $4)"
        } else {
            ""
        };
        selects.push(format!(
            "SELECT 'reading'::text AS kind, \
                    EXTRACT(EPOCH FROM date_trunc('hour', started_at))::bigint::text AS source_id, \
                    date_trunc('hour', started_at) AS ts, \
                    'reading'::text AS summary_a, 'volume'::text AS summary_b, \
                    jsonb_build_object('sessions', COUNT(*), \
                                       'active_ms', COALESCE(SUM(active_ms), 0), \
                                       'pages', COALESCE(SUM(distinct_pages_read), 0), \
                                       'distinct_users', COUNT(DISTINCT user_id)) AS payload \
             FROM reading_sessions \
             WHERE started_at >= $1 \
             GROUP BY date_trunc('hour', started_at) \
             HAVING TRUE{cur}",
        ));
    }

    let union = selects.join(" UNION ALL ");
    let sql = format!(
        "SELECT kind, source_id, ts, summary_a, summary_b, payload FROM ({union}) u \
         ORDER BY ts DESC, kind DESC, source_id DESC LIMIT {}",
        limit + 1
    );

    #[derive(FromQueryResult)]
    struct Row {
        kind: String,
        source_id: String,
        ts: DateTime<FixedOffset>,
        summary_a: String,
        summary_b: String,
        payload: serde_json::Value,
    }

    let rows = match Row::find_by_statement(Statement::from_sql_and_values(backend, &sql, params))
        .all(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "admin_activity: union query failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let next_cursor = if rows.len() as u64 > limit {
        let last = &rows[limit as usize - 1];
        Some(encode_cursor(&last.ts, &last.kind, &last.source_id))
    } else {
        None
    };

    let entries: Vec<ActivityEntryView> = rows
        .into_iter()
        .take(limit as usize)
        .map(|r| ActivityEntryView {
            summary: format_summary(&r.kind, &r.summary_a, &r.summary_b, &r.payload),
            kind: r.kind,
            source_id: r.source_id,
            timestamp: r.ts.to_rfc3339(),
            payload: r.payload,
        })
        .collect();

    Json(ActivityListView {
        entries,
        next_cursor,
    })
    .into_response()
}

fn parse_kinds(s: Option<&str>) -> std::collections::HashSet<&'static str> {
    let allowed: &[&'static str] = &["audit", "scan", "health", "reading"];
    let Some(s) = s else {
        return allowed.iter().copied().collect();
    };
    s.split(',')
        .map(str::trim)
        .filter_map(|tok| allowed.iter().copied().find(|k| *k == tok))
        .collect()
}

fn format_summary(kind: &str, a: &str, b: &str, payload: &serde_json::Value) -> String {
    match kind {
        "audit" => b.to_owned(), // action
        "scan" => match a {
            "running" => format!("{b} scan running"),
            "complete" => format!("{b} scan complete"),
            "failed" => format!("{b} scan failed"),
            "cancelled" => format!("{b} scan cancelled"),
            other => format!("{b} scan {other}"),
        },
        "health" => format!("{a}: {b}"),
        "reading" => {
            let sessions = payload
                .get("sessions")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let users = payload
                .get("distinct_users")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            format!(
                "{} reading session{} ({} reader{})",
                sessions,
                if sessions == 1 { "" } else { "s" },
                users,
                if users == 1 { "" } else { "s" },
            )
        }
        _ => String::new(),
    }
}

fn encode_cursor(ts: &DateTime<FixedOffset>, kind: &str, source_id: &str) -> String {
    let payload = format!("{}|{}|{}", ts.to_rfc3339(), kind, source_id);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload)
}

fn decode_cursor(s: &str) -> Option<(DateTime<FixedOffset>, String, String)> {
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .ok()?;
    let payload = std::str::from_utf8(&decoded).ok()?;
    let mut parts = payload.splitn(3, '|');
    let ts = parts.next()?;
    let kind = parts.next()?;
    let source_id = parts.next()?;
    Some((
        DateTime::parse_from_rfc3339(ts).ok()?,
        kind.to_owned(),
        source_id.to_owned(),
    ))
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
