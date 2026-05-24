//! Audit-log helper. Append-only writes to `audit_log` (§5.9). Callers fire
//! and forget — failures here are logged but never bubble up to the request,
//! since the user-visible action has already succeeded by the time we record.
//!
//! **Convention:** every admin mutation calls [`record_admin_action!`]. The
//! macro is the canonical anchor the M10 CI tool greps for; new admin
//! handlers that omit it fail the audit-log completeness check.

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

/// Canonical audit-emit macro for admin mutations.
///
/// Two forms — with or without a specific target row:
///
/// ```ignore
/// // Targeted mutation: editing a user, library, CBL list, etc.
/// record_admin_action!(
///     db = &app.db,
///     ctx = &ctx,
///     actor = admin.user_id,
///     action = "admin.user.update",
///     target = ("user", user_id.to_string()),
///     payload = serde_json::json!({"role": new_role}),
/// );
///
/// // Untargeted mutation: bulk operations, system actions.
/// record_admin_action!(
///     db = &app.db,
///     ctx = &ctx,
///     actor = admin.user_id,
///     action = "admin.libraries.scan_all",
///     payload = serde_json::json!({"count": libs.len()}),
/// );
/// ```
///
/// Why a macro? Two reasons. (1) The M10 CI tool greps for
/// `record_admin_action!` to verify every `RequireAdmin` handler emits an
/// audit row before returning success — a function call would require AST
/// analysis to detect; a macro invocation is a stable grep target. (2)
/// Changing the audit shape later (adding fields, async vs sync) is a
/// single-site edit.
#[macro_export]
macro_rules! record_admin_action {
    (
        db = $db:expr,
        ctx = $ctx:expr,
        actor = $actor:expr,
        action = $action:expr,
        target = ($target_type:expr, $target_id:expr),
        payload = $payload:expr $(,)?
    ) => {{
        $crate::audit::record(
            $db,
            $crate::audit::AuditEntry {
                actor_id: $actor,
                action: $action,
                target_type: Some($target_type),
                target_id: Some($target_id),
                payload: $payload,
                ip: $ctx.ip_string(),
                user_agent: $ctx.user_agent.clone(),
            },
        )
        .await
    }};
    (
        db = $db:expr,
        ctx = $ctx:expr,
        actor = $actor:expr,
        action = $action:expr,
        payload = $payload:expr $(,)?
    ) => {{
        $crate::audit::record(
            $db,
            $crate::audit::AuditEntry {
                actor_id: $actor,
                action: $action,
                target_type: None,
                target_id: None,
                payload: $payload,
                ip: $ctx.ip_string(),
                user_agent: $ctx.user_agent.clone(),
            },
        )
        .await
    }};
}
