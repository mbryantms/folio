//! `GET /admin/audit` — paginated read of the append-only `audit_log` table
//! (§5.9). Filters: `actor_id`, `action`, `target_type`, `since`. Results are
//! ordered newest-first; the cursor encodes the boundary `(created_at, id)`.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use entity::{audit_log, library, user};
use sea_orm::{ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::auth::RequireAdmin;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/admin/audit", get(list))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AuditEntryView {
    pub id: String,
    pub actor_id: String,
    pub actor_type: String,
    /// Human-readable name for the actor (e.g. user display name + email).
    /// `None` when the actor cannot be resolved — typically because the user
    /// row was deleted after the entry was written.
    pub actor_label: Option<String>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    /// Human-readable name for the target. Resolved server-side for
    /// `target_type` values of `user` and `library`. `None` for unknown types
    /// or unresolvable rows.
    pub target_label: Option<String>,
    pub payload: serde_json::Value,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AuditListView {
    pub items: Vec<AuditEntryView>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct AuditQuery {
    pub limit: Option<u64>,
    pub cursor: Option<String>,
    pub actor_id: Option<String>,
    pub action: Option<String>,
    pub target_type: Option<String>,
    /// RFC 3339 timestamp; only entries strictly newer than this are returned.
    pub since: Option<String>,
}

#[utoipa::path(
    get,
    path = "/admin/audit",
    params(AuditQuery),
    responses(
        (status = 200, body = AuditListView),
        (status = 403, description = "admin only"),
    )
)]
pub async fn list(
    State(app): State<AppState>,
    _admin: RequireAdmin,
    Query(q): Query<AuditQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);

    let mut query = audit_log::Entity::find()
        .order_by_desc(audit_log::Column::CreatedAt)
        .order_by_desc(audit_log::Column::Id);

    if let Some(actor) = q.actor_id.as_deref() {
        let Ok(uuid) = Uuid::parse_str(actor) else {
            return error(StatusCode::BAD_REQUEST, "validation", "invalid actor_id");
        };
        query = query.filter(audit_log::Column::ActorId.eq(uuid));
    }
    if let Some(action) = q.action.as_deref()
        && !action.is_empty()
    {
        // Allow `prefix.*` to match any action starting with `prefix.`.
        if let Some(prefix) = action.strip_suffix(".*") {
            query = query.filter(audit_log::Column::Action.starts_with(prefix));
        } else {
            query = query.filter(audit_log::Column::Action.eq(action));
        }
    }
    if let Some(tt) = q.target_type.as_deref()
        && !tt.is_empty()
    {
        query = query.filter(audit_log::Column::TargetType.eq(tt));
    }
    if let Some(since) = q.since.as_deref() {
        let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(since) else {
            return error(StatusCode::BAD_REQUEST, "validation", "invalid since");
        };
        query = query.filter(audit_log::Column::CreatedAt.gt(parsed.fixed_offset()));
    }
    if let Some(cursor) = q.cursor.as_deref() {
        let Ok((c_at, c_id)) = parse_cursor(cursor) else {
            return error(StatusCode::BAD_REQUEST, "validation", "invalid cursor");
        };
        // Newer-than-or-equal-to created_at is filtered to "older than", giving
        // the next page when ordered DESC. Tie-break on id to stay stable when
        // multiple rows share a microsecond.
        query = query.filter(
            Condition::any()
                .add(audit_log::Column::CreatedAt.lt(c_at))
                .add(
                    Condition::all()
                        .add(audit_log::Column::CreatedAt.eq(c_at))
                        .add(audit_log::Column::Id.lt(c_id)),
                ),
        );
    }

    let rows = match query.limit(limit + 1).all(&app.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "audit list failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let next_cursor = if rows.len() as u64 > limit {
        rows.get(limit as usize - 1)
            .map(|r| encode_cursor(r.created_at, r.id))
    } else {
        None
    };
    let page: Vec<audit_log::Model> = rows.into_iter().take(limit as usize).collect();

    // Batch-resolve actor + target ids to human labels so the UI doesn't have
    // to chase one /admin/users/{id} request per row.
    let labels = match resolve_labels(&app, &page).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, "audit label resolution failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal");
        }
    };

    let items: Vec<AuditEntryView> = page
        .into_iter()
        .map(|m| AuditEntryView::build(m, &labels))
        .collect();
    Json(AuditListView { items, next_cursor }).into_response()
}

#[derive(Default)]
struct LabelLookup {
    users: HashMap<Uuid, String>,
    libraries: HashMap<Uuid, String>,
}

async fn resolve_labels(
    app: &AppState,
    rows: &[audit_log::Model],
) -> Result<LabelLookup, sea_orm::DbErr> {
    let mut user_ids: HashSet<Uuid> = HashSet::new();
    let mut library_ids: HashSet<Uuid> = HashSet::new();

    for r in rows {
        // Every actor today is a user; resolve regardless of `actor_type` so
        // future actor kinds with user-shaped ids still get a label.
        user_ids.insert(r.actor_id);
        if let (Some(tt), Some(tid)) = (r.target_type.as_deref(), r.target_id.as_deref())
            && let Ok(uuid) = Uuid::parse_str(tid)
        {
            match tt {
                "user" => {
                    user_ids.insert(uuid);
                }
                "library" => {
                    library_ids.insert(uuid);
                }
                _ => {}
            }
        }
    }

    let mut out = LabelLookup::default();
    if !user_ids.is_empty() {
        let rows = user::Entity::find()
            .filter(user::Column::Id.is_in(user_ids))
            .all(&app.db)
            .await?;
        for u in rows {
            let label = match u.email.as_deref() {
                Some(email) if !email.is_empty() => format!("{} <{}>", u.display_name, email),
                _ => u.display_name.clone(),
            };
            out.users.insert(u.id, label);
        }
    }
    if !library_ids.is_empty() {
        let rows = library::Entity::find()
            .filter(library::Column::Id.is_in(library_ids))
            .all(&app.db)
            .await?;
        for l in rows {
            out.libraries.insert(l.id, l.name);
        }
    }
    Ok(out)
}

impl AuditEntryView {
    fn build(m: audit_log::Model, labels: &LabelLookup) -> Self {
        let actor_label = labels.users.get(&m.actor_id).cloned();
        let target_label = match m.target_type.as_deref() {
            Some("user") => m
                .target_id
                .as_deref()
                .and_then(|s| Uuid::parse_str(s).ok())
                .and_then(|id| labels.users.get(&id).cloned()),
            Some("library") => m
                .target_id
                .as_deref()
                .and_then(|s| Uuid::parse_str(s).ok())
                .and_then(|id| labels.libraries.get(&id).cloned()),
            _ => None,
        };
        Self {
            id: m.id.to_string(),
            actor_id: m.actor_id.to_string(),
            actor_type: m.actor_type,
            actor_label,
            action: m.action,
            target_type: m.target_type,
            target_id: m.target_id,
            target_label,
            payload: m.payload,
            ip: m.ip,
            user_agent: m.user_agent,
            created_at: m.created_at.to_rfc3339(),
        }
    }
}

fn encode_cursor(created_at: chrono::DateTime<chrono::FixedOffset>, id: Uuid) -> String {
    use base64::Engine;
    let s = format!("{}|{}", created_at.to_rfc3339(), id);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.as_bytes())
}

fn parse_cursor(s: &str) -> Result<(chrono::DateTime<chrono::FixedOffset>, Uuid), ()> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .map_err(|_| ())?;
    let txt = std::str::from_utf8(&bytes).map_err(|_| ())?;
    let (ts, id) = txt.split_once('|').ok_or(())?;
    let parsed_ts = chrono::DateTime::parse_from_rfc3339(ts).map_err(|_| ())?;
    let parsed_id = Uuid::parse_str(id).map_err(|_| ())?;
    Ok((parsed_ts, parsed_id))
}

fn error(status: StatusCode, code: &str, message: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
