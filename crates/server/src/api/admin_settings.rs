//! `/admin/settings` — runtime-editable server settings (M1).
//!
//! M1 ships the API surface but the registry is empty, so:
//!   - `GET /admin/settings` returns the registry shape + any existing rows
//!     (none, in a fresh install).
//!   - `PATCH /admin/settings` round-trips through the registry validator
//!     and rejects every body, since no key is yet known.
//!
//! M2 onward populates [`crate::settings::registry::REGISTRY`] so real
//! fields become editable. The shape of this endpoint stays stable across
//! milestones — the only thing that grows is the registry.

use axum::{
    Extension, Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::RequireAdmin;
use crate::middleware::RequestContext;
use crate::settings::{self, registry};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/settings", get(get_all).patch(update))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SettingsView {
    /// Catalog of every DB-overlayable setting recognized by this server
    /// build. Empty in M1; populated milestone-by-milestone.
    pub registry: Vec<RegistryEntry>,
    /// Current effective values resolved from the DB overlay. Secret
    /// rows are returned as the literal string `"<set>"` so this surface
    /// never leaks credentials. Order matches `registry`.
    pub values: Vec<ResolvedEntry>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RegistryEntry {
    pub key: String,
    /// `"string" | "bool" | "uint" | "duration"`.
    pub kind: String,
    pub is_secret: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ResolvedEntry {
    pub key: String,
    pub value: Value,
    pub is_secret: bool,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateSettingsReq {
    /// Map of setting key → new value. A `null` value deletes the row.
    /// Unknown keys reject the whole batch with a 400.
    #[serde(flatten)]
    pub updates: serde_json::Map<String, Value>,
}

#[utoipa::path(
    get,
    path = "/admin/settings",
    responses(
        (status = 200, body = SettingsView),
        (status = 403, description = "admin only"),
    )
)]
pub async fn get_all(State(app): State<AppState>, _admin: RequireAdmin) -> Response {
    let rows = match settings::read_all(&app.db, &app.secrets).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "settings::read_all failed");
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "failed to read settings",
            );
        }
    };
    let values: Vec<ResolvedEntry> = rows
        .into_iter()
        .map(|r| ResolvedEntry {
            key: r.key,
            value: if r.is_secret {
                Value::String("<set>".into())
            } else {
                r.value
            },
            is_secret: r.is_secret,
        })
        .collect();
    let reg: Vec<RegistryEntry> = registry::registry()
        .iter()
        .map(|d| RegistryEntry {
            key: d.key.to_string(),
            kind: kind_label(d.kind).to_string(),
            is_secret: d.is_secret,
        })
        .collect();
    Json(SettingsView {
        registry: reg,
        values,
    })
    .into_response()
}

#[utoipa::path(
    patch,
    path = "/admin/settings",
    request_body = UpdateSettingsReq,
    responses(
        (status = 200, body = SettingsView),
        (status = 400, description = "validation"),
        (status = 403, description = "admin only"),
    )
)]
pub async fn update(
    State(app): State<AppState>,
    RequireAdmin(actor): RequireAdmin,
    Extension(ctx): Extension<RequestContext>,
    Json(req): Json<UpdateSettingsReq>,
) -> Response {
    // 1. Validate per-key shape against the registry. A partial-batch with
    //    one bad value rejects the whole request — no half-commits.
    let mut updates = Vec::with_capacity(req.updates.len());
    let mut touched_email = false;
    let mut touched_oidc = false;
    let mut touched_log_level = false;
    for (key, value) in req.updates {
        let def = match registry::lookup(&key) {
            Some(d) => d,
            None => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "settings.unknown_key",
                    &format!("unknown setting key: {key}"),
                );
            }
        };
        if let Err(msg) = validate_value(def.kind, &value) {
            return error(
                StatusCode::BAD_REQUEST,
                "settings.invalid_value",
                &format!("{key}: {msg}"),
            );
        }
        if registry::affects_email(&key) {
            touched_email = true;
        }
        if registry::affects_oidc(&key) {
            touched_oidc = true;
        }
        if registry::affects_log_level(&key) {
            touched_log_level = true;
        }
        updates.push(settings::Update {
            key,
            value: if value.is_null() { None } else { Some(value) },
        });
    }

    // 2. Dry-run: build the Config that *would* result from this PATCH
    //    (baseline + existing DB rows + proposed updates) and run
    //    `Config::validate`. This catches cross-field invariants like
    //    "auth.mode=oidc requires oidc.issuer + client_id + client_secret"
    //    *before* we commit the write — operators can't put the server
    //    into a broken state via /admin/settings.
    let dry_run = match dry_run_validate(&app, &updates).await {
        Ok(()) => Ok(()),
        Err(e) => Err(e),
    };
    if let Err(e) = dry_run {
        return error(StatusCode::BAD_REQUEST, "settings.invalid_combination", &e);
    }

    // 3. Persist + audit.
    if let Err(e) = settings::write(&app.db, &app.secrets, actor.id, &ctx, updates).await {
        tracing::error!(error = %e, "settings::write failed");
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "failed to write settings",
        );
    }

    // 4. Rebuild Config from the env baseline + the new DB overlay so the
    //    change takes effect on the next request — and so a row deleted via
    //    `value: null` reverts to the env value. Failure here leaves the
    //    old live config in place; the DB row is still the new value, so
    //    a follow-up correction will recover.
    let mut next = (*app.cfg_baseline()).clone();
    let overlay_ok = match next.overlay_db(&app.db, &app.secrets).await {
        Ok(()) => true,
        Err(e) => {
            tracing::error!(error = %e, "Config::overlay_db failed after settings write");
            false
        }
    };
    if overlay_ok {
        app.replace_cfg(next);
        // Rebuild the email sender if any `smtp.*` row changed. We do
        // this after `replace_cfg` so the sender reads the same Config
        // snapshot the rest of the process now sees.
        if touched_email {
            match crate::email::build(&app.cfg()) {
                Ok(sender) => app.replace_email(sender).await,
                Err(e) => {
                    tracing::error!(error = %e, "email::build failed after smtp.* change");
                }
            }
        }
        // Evict the OIDC discovery cache so the next /auth/oidc/* call
        // re-fetches against the new issuer / client_id / client_secret.
        if touched_oidc {
            crate::auth::oidc::clear_discovery_cache().await;
        }
        // Swap the live tracing EnvFilter so the new directive takes
        // effect on the next event without restarting. Validation in
        // step 2 already caught invalid directives via
        // `EnvFilter::try_new`, so this should never fail; we log if
        // it does and leave the prior filter in place.
        if touched_log_level {
            let next_level = app.cfg().log_level.clone();
            if let Ok(filter) = tracing_subscriber::EnvFilter::try_new(&next_level) {
                if let Err(e) = app.log_reload.modify(|f| *f = filter) {
                    tracing::error!(error = %e, "tracing reload handle modify failed");
                } else {
                    tracing::info!(level = %next_level, "log level reloaded");
                }
            }
        }
    }

    get_all(State(app), RequireAdmin(actor)).await
}

/// Dry-run validation: build the Config that *would* result from applying
/// `updates` on top of the env baseline + existing DB rows, then run the
/// post-overlay validator. Returns a user-facing message on failure.
async fn dry_run_validate(
    app: &AppState,
    updates: &[settings::Update],
) -> Result<(), String> {
    let existing = settings::read_all(&app.db, &app.secrets)
        .await
        .map_err(|e| format!("failed to read current settings: {e}"))?;

    let mut merged: std::collections::HashMap<String, settings::Resolved> = existing
        .into_iter()
        .map(|r| (r.key.clone(), r))
        .collect();
    for u in updates {
        match &u.value {
            None => {
                merged.remove(&u.key);
            }
            Some(v) => {
                merged.insert(
                    u.key.clone(),
                    settings::Resolved {
                        key: u.key.clone(),
                        value: v.clone(),
                        is_secret: registry::is_secret(&u.key),
                    },
                );
            }
        }
    }

    let mut proposed = (*app.cfg_baseline()).clone();
    for r in merged.values() {
        crate::config::apply_overlay_row(&mut proposed, r);
    }
    proposed.validate().map_err(|e| e.to_string())
}

fn kind_label(k: registry::SettingKind) -> &'static str {
    match k {
        registry::SettingKind::String => "string",
        registry::SettingKind::Bool => "bool",
        registry::SettingKind::Uint => "uint",
        registry::SettingKind::Duration => "duration",
    }
}

fn validate_value(kind: registry::SettingKind, value: &Value) -> Result<(), &'static str> {
    if value.is_null() {
        return Ok(()); // null = delete; type check skipped
    }
    match kind {
        registry::SettingKind::String | registry::SettingKind::Duration => {
            if !value.is_string() {
                return Err("expected JSON string");
            }
        }
        registry::SettingKind::Bool => {
            if !value.is_boolean() {
                return Err("expected JSON boolean");
            }
        }
        registry::SettingKind::Uint => {
            let ok = value.as_u64().is_some();
            if !ok {
                return Err("expected non-negative JSON number");
            }
        }
    }
    Ok(())
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_labels() {
        assert_eq!(kind_label(registry::SettingKind::String), "string");
        assert_eq!(kind_label(registry::SettingKind::Bool), "bool");
        assert_eq!(kind_label(registry::SettingKind::Uint), "uint");
        assert_eq!(kind_label(registry::SettingKind::Duration), "duration");
    }

    #[test]
    fn validate_string_kind() {
        assert!(validate_value(registry::SettingKind::String, &Value::String("hi".into())).is_ok());
        assert!(validate_value(registry::SettingKind::String, &Value::Bool(true)).is_err());
        // null is treated as "delete this row" — type check skipped.
        assert!(validate_value(registry::SettingKind::String, &Value::Null).is_ok());
    }

    #[test]
    fn validate_uint_kind() {
        assert!(validate_value(registry::SettingKind::Uint, &Value::from(42)).is_ok());
        assert!(validate_value(registry::SettingKind::Uint, &Value::from(-1)).is_err());
        assert!(validate_value(registry::SettingKind::Uint, &Value::String("0".into())).is_err());
    }
}
