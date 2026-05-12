//! Audit-log helper. Append-only writes to `audit_log` (§5.9). Callers fire
//! and forget — failures here are logged but never bubble up to the request,
//! since the user-visible action has already succeeded by the time we record.

use crate::middleware::RequestContext;
use entity::audit_log;
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};
use uuid::Uuid;

#[derive(Debug)]
pub struct AuditEntry<'a> {
    pub actor_id: Uuid,
    pub action: &'a str,
    pub target_type: Option<&'a str>,
    pub target_id: Option<String>,
    pub payload: serde_json::Value,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

pub async fn record(db: &DatabaseConnection, entry: AuditEntry<'_>) {
    let am = audit_log::ActiveModel {
        id: Set(Uuid::now_v7()),
        actor_id: Set(entry.actor_id),
        actor_type: Set("user".into()),
        action: Set(entry.action.into()),
        target_type: Set(entry.target_type.map(str::to_owned)),
        target_id: Set(entry.target_id),
        payload: Set(entry.payload),
        ip: Set(entry.ip),
        user_agent: Set(entry.user_agent),
        created_at: Set(chrono::Utc::now().fixed_offset()),
    };
    if let Err(e) = am.insert(db).await {
        tracing::error!(error = %e, action = entry.action, "audit_log write failed");
    }
}

/// Convenience: builder-shaped audit entry that fills `ip` + `user_agent`
/// from a [`RequestContext`]. The context comes from the
/// `set_context` middleware via `Extension<RequestContext>`. Use this in
/// handlers so the `audit::record` call site never forgets to plumb the
/// forensic fields.
pub async fn record_with_ctx(
    db: &DatabaseConnection,
    ctx: &RequestContext,
    actor_id: Uuid,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<String>,
    payload: serde_json::Value,
) {
    record(
        db,
        AuditEntry {
            actor_id,
            action,
            target_type,
            target_id,
            payload,
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
}
