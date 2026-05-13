//! Runtime-editable server settings (M1 of the runtime-config-admin plan).
//!
//! The `app_setting` table stores key/value rows that *overlay* the env-only
//! `Config` loaded at boot. Each milestone (M2 SMTP, M3 identity, M4
//! tokens/log/rate-limit, M5 workers, M6 OTLP) migrates a slice of fields
//! into the registry below and wires the corresponding form into the admin
//! UI. M1 ships only the plumbing — the registry is intentionally empty so
//! that no user-visible behavior changes.
//!
//! Secret rows (`is_secret = true`) are sealed with the AEAD key in
//! `secrets/settings-encryption.key`; see [`crypto`].

pub mod bootstrap;
pub mod crypto;
pub mod registry;

use std::collections::HashMap;

use entity::app_setting;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    TransactionTrait,
};
use uuid::Uuid;

use crate::audit::{self, AuditEntry};
use crate::middleware::RequestContext;
use crate::secrets::Secrets;

pub use registry::{SettingDef, SettingKind, is_known, is_secret, registry};

/// One row of the `app_setting` table, ready for application.
///
/// For secret rows, [`Self::value`] holds the decrypted plaintext as JSON
/// (typically a `Value::String`). The redaction step happens in the API
/// layer, not here, so the overlay can apply the real value.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub key: String,
    pub value: serde_json::Value,
    pub is_secret: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("db error: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("crypto error: {0}")]
    Crypto(#[from] crypto::CryptoError),
    #[error("malformed secret envelope for key {0}")]
    MalformedSecret(String),
    #[error("unknown setting key: {0}")]
    UnknownKey(String),
    #[error("value for {key} has wrong type: {detail}")]
    BadValue { key: String, detail: String },
}

/// Load every row from `app_setting`, decrypting secret rows.
///
/// Rows with unknown keys are silently kept so an older binary can be rolled
/// back across a migration window without losing data, but they are filtered
/// out before they reach the [`Config`] overlay.
pub async fn read_all(db: &DatabaseConnection, secrets: &Secrets) -> Result<Vec<Resolved>, Error> {
    let rows = app_setting::Entity::find().all(db).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let value = if row.is_secret {
            let sealed: crypto::SealedSecret = serde_json::from_value(row.value.clone())
                .map_err(|_| Error::MalformedSecret(row.key.clone()))?;
            let pt = crypto::open(&secrets.settings_encryption_key, &sealed)?;
            let s = String::from_utf8(pt).map_err(|_| Error::MalformedSecret(row.key.clone()))?;
            serde_json::Value::String(s)
        } else {
            row.value.clone()
        };
        out.push(Resolved {
            key: row.key,
            value,
            is_secret: row.is_secret,
        });
    }
    Ok(out)
}

/// One pending change for [`write`]. `value = None` deletes the row.
#[derive(Debug, Clone)]
pub struct Update {
    pub key: String,
    pub value: Option<serde_json::Value>,
}

/// Write a batch of settings updates in a single transaction and audit-log
/// the change. Secret values are sealed before insert. Caller is responsible
/// for type-validating values against the registry — this layer only
/// rejects unknown keys.
pub async fn write(
    db: &DatabaseConnection,
    secrets: &Secrets,
    actor: Uuid,
    ctx: &RequestContext,
    updates: Vec<Update>,
) -> Result<(), Error> {
    for u in &updates {
        if !is_known(&u.key) {
            return Err(Error::UnknownKey(u.key.clone()));
        }
    }

    let txn = db.begin().await?;
    let mut audit_changes = serde_json::Map::with_capacity(updates.len());
    let now = chrono::Utc::now().fixed_offset();

    for u in updates {
        let secret = is_secret(&u.key);
        match u.value {
            Some(value) => {
                let stored = if secret {
                    let pt = value
                        .as_str()
                        .ok_or_else(|| Error::BadValue {
                            key: u.key.clone(),
                            detail: "secret value must be a JSON string".into(),
                        })?
                        .to_owned();
                    let sealed = crypto::seal(&secrets.settings_encryption_key, pt.as_bytes())?;
                    serde_json::to_value(sealed).expect("SealedSecret -> Value")
                } else {
                    value.clone()
                };

                let existing = app_setting::Entity::find_by_id(u.key.clone())
                    .one(&txn)
                    .await?;
                let am = app_setting::ActiveModel {
                    key: Set(u.key.clone()),
                    value: Set(stored),
                    is_secret: Set(secret),
                    updated_at: Set(now),
                    updated_by: Set(Some(actor)),
                };
                if existing.is_some() {
                    am.update(&txn).await?;
                } else {
                    am.insert(&txn).await?;
                }

                // Redact secret values in audit payload — the actual content
                // belongs in the encrypted row, never in audit_log.
                let audit_value = if secret {
                    serde_json::json!("<set>")
                } else {
                    value
                };
                audit_changes.insert(u.key, audit_value);
            }
            None => {
                app_setting::Entity::delete_many()
                    .filter(app_setting::Column::Key.eq(u.key.clone()))
                    .exec(&txn)
                    .await?;
                audit_changes.insert(u.key, serde_json::Value::Null);
            }
        }
    }

    txn.commit().await?;

    audit::record(
        db,
        AuditEntry {
            actor_id: actor,
            action: "admin.settings.update",
            target_type: Some("settings"),
            target_id: None,
            payload: serde_json::Value::Object(audit_changes),
            ip: ctx.ip_string(),
            user_agent: ctx.user_agent.clone(),
        },
    )
    .await;
    Ok(())
}

/// Convenience for callers that want a `HashMap` keyed by setting name.
pub fn into_map(rows: Vec<Resolved>) -> HashMap<String, Resolved> {
    rows.into_iter().map(|r| (r.key.clone(), r)).collect()
}
