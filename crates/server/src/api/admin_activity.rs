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

use std::collections::{HashMap, HashSet};

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use base64::Engine;
use chrono::{DateTime, FixedOffset};
use entity::{issue, library, series, user};
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult, QueryFilter, Statement, Value,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::error;
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

#[derive(FromQueryResult)]
struct Row {
    kind: String,
    source_id: String,
    ts: DateTime<FixedOffset>,
    summary_a: String,
    summary_b: String,
    payload: serde_json::Value,
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

    let page: Vec<Row> = rows.into_iter().take(limit as usize).collect();

    // Resolve UUID references in every payload to human-readable labels so
    // the admin UI doesn't render `library d4f2e8c0…` / `series 9a1b…` /
    // `Issue 12af…`. Mirrors the pattern in `audit.rs::resolve_labels`:
    // collect IDs, bulk-fetch each entity table, decorate the payload.
    let labels = match resolve_activity_labels(&app, &page).await {
        Ok(l) => l,
        Err(e) => {
            // Fail soft — labels are presentation only; the IDs are still
            // there and the page still renders.
            tracing::warn!(error = %e, "admin_activity: label resolution failed; serving IDs only");
            ActivityLabels::default()
        }
    };

    let entries: Vec<ActivityEntryView> = page
        .into_iter()
        .map(|r| ActivityEntryView {
            summary: format_summary(&r.kind, &r.summary_a, &r.summary_b, &r.payload),
            payload: decorate_payload(&r.kind, r.payload, &labels),
            kind: r.kind,
            source_id: r.source_id,
            timestamp: r.ts.to_rfc3339(),
        })
        .collect();

    Json(ActivityListView {
        entries,
        next_cursor,
    })
    .into_response()
}

#[derive(Default)]
struct ActivityLabels {
    users: HashMap<Uuid, String>,
    libraries: HashMap<Uuid, String>,
    series: HashMap<Uuid, String>,
    /// Issue PKs are BLAKE3 hex strings, not UUIDs. Value is a display
    /// label of the form `"{series_name} #{number}"` (or just the
    /// series + title fallback when there's no number).
    issues: HashMap<String, String>,
}

async fn resolve_activity_labels(
    app: &AppState,
    rows: &[Row],
) -> Result<ActivityLabels, sea_orm::DbErr> {
    let mut user_ids: HashSet<Uuid> = HashSet::new();
    let mut library_ids: HashSet<Uuid> = HashSet::new();
    let mut series_ids: HashSet<Uuid> = HashSet::new();
    let mut issue_ids: HashSet<String> = HashSet::new();

    for r in rows {
        collect_uuid(&r.payload, "actor_id", &mut user_ids);
        collect_uuid(&r.payload, "library_id", &mut library_ids);
        collect_uuid(&r.payload, "series_id", &mut series_ids);
        collect_str(&r.payload, "issue_id", &mut issue_ids);

        // Audit `target_id` is a stringified UUID; the entity type lives
        // in `target_type`. Resolve into the right bucket so the UI can
        // render `user Alice` / `series Action Comics` / `library Main`.
        if r.kind == "audit"
            && let (Some(tt), Some(tid)) = (
                r.payload.get("target_type").and_then(|v| v.as_str()),
                r.payload.get("target_id").and_then(|v| v.as_str()),
            )
        {
            match tt {
                "user" => {
                    if let Ok(id) = Uuid::parse_str(tid) {
                        user_ids.insert(id);
                    }
                }
                "library" => {
                    if let Ok(id) = Uuid::parse_str(tid) {
                        library_ids.insert(id);
                    }
                }
                "series" => {
                    if let Ok(id) = Uuid::parse_str(tid) {
                        series_ids.insert(id);
                    }
                }
                "issue" => {
                    if !tid.is_empty() {
                        issue_ids.insert(tid.to_owned());
                    }
                }
                _ => {}
            }
        }
    }

    let mut out = ActivityLabels::default();
    if !user_ids.is_empty() {
        for u in user::Entity::find()
            .filter(user::Column::Id.is_in(user_ids.iter().copied()))
            .all(&app.db)
            .await?
        {
            let label = match u.email.as_deref() {
                Some(email) if !email.is_empty() => format!("{} <{}>", u.display_name, email),
                _ => u.display_name.clone(),
            };
            out.users.insert(u.id, label);
        }
    }
    if !library_ids.is_empty() {
        for l in library::Entity::find()
            .filter(library::Column::Id.is_in(library_ids.iter().copied()))
            .all(&app.db)
            .await?
        {
            out.libraries.insert(l.id, l.name);
        }
    }
    if !series_ids.is_empty() {
        for s in series::Entity::find()
            .filter(series::Column::Id.is_in(series_ids.iter().copied()))
            .all(&app.db)
            .await?
        {
            out.series.insert(s.id, s.name);
        }
    }
    if !issue_ids.is_empty() {
        // Issue label needs the parent series name too. Fetch both
        // tables, then build "Series Name #N" / "Series Name — Title".
        let issue_rows = issue::Entity::find()
            .filter(issue::Column::Id.is_in(issue_ids.iter().cloned()))
            .all(&app.db)
            .await?;
        let extra_series: HashSet<Uuid> = issue_rows
            .iter()
            .map(|i| i.series_id)
            .filter(|sid| !out.series.contains_key(sid))
            .collect();
        if !extra_series.is_empty() {
            for s in series::Entity::find()
                .filter(series::Column::Id.is_in(extra_series.iter().copied()))
                .all(&app.db)
                .await?
            {
                out.series.insert(s.id, s.name);
            }
        }
        for i in issue_rows {
            let series_name = out
                .series
                .get(&i.series_id)
                .cloned()
                .unwrap_or_else(|| "Unknown series".to_owned());
            let label = match (i.number_raw.as_deref(), i.title.as_deref()) {
                (Some(num), _) if !num.is_empty() => format!("{series_name} #{num}"),
                (_, Some(t)) if !t.is_empty() => format!("{series_name} — {t}"),
                _ => series_name,
            };
            out.issues.insert(i.id, label);
        }
    }
    Ok(out)
}

/// Extract a UUID-typed field from a JSON payload, when present.
fn collect_uuid(payload: &serde_json::Value, key: &str, into: &mut HashSet<Uuid>) {
    if let Some(s) = payload.get(key).and_then(|v| v.as_str())
        && let Ok(id) = Uuid::parse_str(s)
    {
        into.insert(id);
    }
}

/// Extract a string-typed field (issue ids are BLAKE3 hex, not UUIDs).
fn collect_str(payload: &serde_json::Value, key: &str, into: &mut HashSet<String>) {
    if let Some(s) = payload.get(key).and_then(|v| v.as_str())
        && !s.is_empty()
    {
        into.insert(s.to_owned());
    }
}

/// Add `*_name` / `*_label` fields next to the raw IDs in the payload so
/// the UI can render the human form without a second round-trip.
fn decorate_payload(
    kind: &str,
    mut payload: serde_json::Value,
    labels: &ActivityLabels,
) -> serde_json::Value {
    let Some(obj) = payload.as_object_mut() else {
        return payload;
    };
    // actor → user
    if let Some(uuid) = obj
        .get("actor_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        && let Some(name) = labels.users.get(&uuid)
    {
        obj.insert("actor_name".into(), serde_json::Value::String(name.clone()));
    }
    // library_id → name
    if let Some(uuid) = obj
        .get("library_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        && let Some(name) = labels.libraries.get(&uuid)
    {
        obj.insert(
            "library_name".into(),
            serde_json::Value::String(name.clone()),
        );
    }
    // series_id → name
    if let Some(uuid) = obj
        .get("series_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        && let Some(name) = labels.series.get(&uuid)
    {
        obj.insert(
            "series_name".into(),
            serde_json::Value::String(name.clone()),
        );
    }
    // issue_id → label (already "Series #N" shape)
    if let Some(s) = obj.get("issue_id").and_then(|v| v.as_str())
        && let Some(label) = labels.issues.get(s)
    {
        obj.insert(
            "issue_label".into(),
            serde_json::Value::String(label.clone()),
        );
    }
    // audit target: resolve by target_type
    if kind == "audit"
        && let (Some(tt), Some(tid)) = (
            obj.get("target_type")
                .and_then(|v| v.as_str())
                .map(String::from),
            obj.get("target_id")
                .and_then(|v| v.as_str())
                .map(String::from),
        )
    {
        let resolved = match tt.as_str() {
            "user" => Uuid::parse_str(&tid)
                .ok()
                .and_then(|id| labels.users.get(&id).cloned()),
            "library" => Uuid::parse_str(&tid)
                .ok()
                .and_then(|id| labels.libraries.get(&id).cloned()),
            "series" => Uuid::parse_str(&tid)
                .ok()
                .and_then(|id| labels.series.get(&id).cloned()),
            "issue" => labels.issues.get(&tid).cloned(),
            _ => None,
        };
        if let Some(name) = resolved {
            obj.insert("target_label".into(), serde_json::Value::String(name));
        }
    }
    payload
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
