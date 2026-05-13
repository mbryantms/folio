//! First-boot bootstrap: copy env-set settings into the `app_setting`
//! table when no DB row exists yet.
//!
//! Without this, an existing `compose.prod.yml` deployment upgrading to a
//! release that introduces `/admin/email` would show empty fields in the
//! UI while emails still worked via env. The operator would then have to
//! re-key every value into the admin form. Bootstrap closes that gap by
//! seeding the DB on first boot from whatever env vars are set.
//!
//! Idempotent: subsequent boots see the row already exists and skip the
//! INSERT. After the operator edits via UI, env values that no longer
//! match log a WARN through [`super::apply_overlay_row`].

use entity::app_setting;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};

use crate::config::{AuthMode, Config};
use crate::secrets::Secrets;

use super::crypto;

/// Seed every `smtp.*` row from the env-loaded [`Config`] when no
/// `smtp.host` row exists yet. We treat the presence of `smtp.host` as
/// the trigger: if it isn't set, there's nothing to bootstrap.
pub async fn seed_smtp_from_env(
    db: &DatabaseConnection,
    cfg: &Config,
    secrets: &Secrets,
) -> anyhow::Result<()> {
    let Some(host) = cfg
        .smtp_host
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    else {
        return Ok(());
    };

    // Short-circuit if already bootstrapped — `smtp.host` is the canonical
    // sentinel row for the SMTP slice.
    let exists = app_setting::Entity::find()
        .filter(app_setting::Column::Key.eq("smtp.host"))
        .one(db)
        .await?
        .is_some();
    if exists {
        return Ok(());
    }

    let now = chrono::Utc::now().fixed_offset();

    insert_plain(db, "smtp.host", serde_json::json!(host), now).await?;
    insert_plain(
        db,
        "smtp.port",
        serde_json::json!(cfg.smtp_port),
        now,
    )
    .await?;
    insert_plain(
        db,
        "smtp.tls",
        serde_json::json!(cfg.smtp_tls),
        now,
    )
    .await?;
    if let Some(from) = cfg.smtp_from.as_deref().filter(|s| !s.trim().is_empty()) {
        insert_plain(db, "smtp.from", serde_json::json!(from), now).await?;
    }
    if let Some(user) = cfg.smtp_username.as_deref().filter(|s| !s.is_empty()) {
        insert_plain(db, "smtp.username", serde_json::json!(user), now).await?;
    }
    if let Some(pass) = cfg.smtp_password.as_deref().filter(|s| !s.is_empty()) {
        let sealed = crypto::seal(&secrets.settings_encryption_key, pass.as_bytes())?;
        let value = serde_json::to_value(sealed)?;
        let am = app_setting::ActiveModel {
            key: Set("smtp.password".into()),
            value: Set(value),
            is_secret: Set(true),
            updated_at: Set(now),
            updated_by: Set(None),
        };
        am.insert(db).await?;
    }

    tracing::info!("seeded smtp.* rows from COMIC_SMTP_* env (one-time bootstrap)");
    Ok(())
}

async fn insert_plain(
    db: &DatabaseConnection,
    key: &str,
    value: serde_json::Value,
    now: chrono::DateTime<chrono::FixedOffset>,
) -> anyhow::Result<()> {
    let am = app_setting::ActiveModel {
        key: Set(key.into()),
        value: Set(value),
        is_secret: Set(false),
        updated_at: Set(now),
        updated_by: Set(None),
    };
    am.insert(db).await?;
    Ok(())
}

/// Seed `auth.*` rows from the env-loaded [`Config`] when no
/// `auth.mode` row exists. Mirrors [`seed_smtp_from_env`]: `auth.mode`
/// is the sentinel and we copy the *current effective values* for the
/// rest of the block (mode + registration_open + trust flag always;
/// OIDC creds only when set).
pub async fn seed_auth_from_env(
    db: &DatabaseConnection,
    cfg: &Config,
    secrets: &Secrets,
) -> anyhow::Result<()> {
    let exists = app_setting::Entity::find()
        .filter(app_setting::Column::Key.eq("auth.mode"))
        .one(db)
        .await?
        .is_some();
    if exists {
        return Ok(());
    }

    let now = chrono::Utc::now().fixed_offset();
    insert_plain(db, "auth.mode", serde_json::json!(cfg.auth_mode.to_string()), now).await?;
    insert_plain(
        db,
        "auth.local.registration_open",
        serde_json::json!(cfg.local_registration_open),
        now,
    )
    .await?;
    insert_plain(
        db,
        "auth.oidc.trust_unverified_email",
        serde_json::json!(cfg.oidc_trust_unverified_email),
        now,
    )
    .await?;

    // OIDC issuer + client_id come along only when the env actually set
    // them. We don't seed empty rows because the overlay then can't
    // tell the difference between "operator-cleared via UI" and
    // "operator never set."
    let oidc_active = matches!(cfg.auth_mode, AuthMode::Oidc | AuthMode::Both);
    if oidc_active
        && let Some(iss) = cfg
            .oidc_issuer
            .as_deref()
            .filter(|s| !s.trim().is_empty())
    {
        insert_plain(db, "auth.oidc.issuer", serde_json::json!(iss), now).await?;
    }
    if oidc_active
        && let Some(cid) = cfg
            .oidc_client_id
            .as_deref()
            .filter(|s| !s.is_empty())
    {
        insert_plain(db, "auth.oidc.client_id", serde_json::json!(cid), now).await?;
    }
    if oidc_active
        && let Some(secret) = cfg
            .oidc_client_secret
            .as_deref()
            .filter(|s| !s.is_empty())
    {
        let sealed = crypto::seal(&secrets.settings_encryption_key, secret.as_bytes())?;
        let value = serde_json::to_value(sealed)?;
        let am = app_setting::ActiveModel {
            key: Set("auth.oidc.client_secret".into()),
            value: Set(value),
            is_secret: Set(true),
            updated_at: Set(now),
            updated_by: Set(None),
        };
        am.insert(db).await?;
    }

    tracing::info!("seeded auth.* rows from COMIC_AUTH_* / COMIC_OIDC_* env (one-time bootstrap)");
    Ok(())
}

/// Seed `auth.jwt.*`, `auth.rate_limit_enabled`, and
/// `observability.log_level` rows from the env-loaded [`Config`] when
/// no `auth.jwt.access_ttl` row exists yet. Same one-time-bootstrap
/// idiom as [`seed_smtp_from_env`] and [`seed_auth_from_env`].
pub async fn seed_tokens_and_diagnostics_from_env(
    db: &DatabaseConnection,
    cfg: &Config,
) -> anyhow::Result<()> {
    let exists = app_setting::Entity::find()
        .filter(app_setting::Column::Key.eq("auth.jwt.access_ttl"))
        .one(db)
        .await?
        .is_some();
    if exists {
        return Ok(());
    }

    let now = chrono::Utc::now().fixed_offset();
    insert_plain(
        db,
        "auth.jwt.access_ttl",
        serde_json::json!(cfg.jwt_access_ttl),
        now,
    )
    .await?;
    insert_plain(
        db,
        "auth.jwt.refresh_ttl",
        serde_json::json!(cfg.jwt_refresh_ttl),
        now,
    )
    .await?;
    insert_plain(
        db,
        "auth.rate_limit_enabled",
        serde_json::json!(cfg.rate_limit_enabled),
        now,
    )
    .await?;
    insert_plain(
        db,
        "observability.log_level",
        serde_json::json!(cfg.log_level),
        now,
    )
    .await?;
    tracing::info!(
        "seeded auth.jwt.*, auth.rate_limit_enabled, observability.log_level from env \
         (one-time bootstrap)"
    );
    Ok(())
}

/// Seed `cache.*` + `workers.*` rows from the env-loaded [`Config`] when
/// no `workers.scan_count` row exists yet. Same idiom as the other
/// seeders. All keys are uints with non-zero defaults so always seeded.
pub async fn seed_operational_from_env(
    db: &DatabaseConnection,
    cfg: &Config,
) -> anyhow::Result<()> {
    let exists = app_setting::Entity::find()
        .filter(app_setting::Column::Key.eq("workers.scan_count"))
        .one(db)
        .await?
        .is_some();
    if exists {
        return Ok(());
    }
    let now = chrono::Utc::now().fixed_offset();
    insert_plain(
        db,
        "cache.zip_lru_capacity",
        serde_json::json!(cfg.zip_lru_capacity),
        now,
    )
    .await?;
    insert_plain(
        db,
        "workers.scan_count",
        serde_json::json!(cfg.scan_worker_count),
        now,
    )
    .await?;
    insert_plain(
        db,
        "workers.post_scan_count",
        serde_json::json!(cfg.post_scan_worker_count),
        now,
    )
    .await?;
    insert_plain(
        db,
        "workers.scan_batch_size",
        serde_json::json!(cfg.scan_batch_size),
        now,
    )
    .await?;
    insert_plain(
        db,
        "workers.scan_hash_buffer_kb",
        serde_json::json!(cfg.scan_hash_buffer_kb),
        now,
    )
    .await?;
    insert_plain(
        db,
        "workers.archive_work_parallel",
        serde_json::json!(cfg.archive_work_parallel),
        now,
    )
    .await?;
    insert_plain(
        db,
        "workers.thumb_inline_parallel",
        serde_json::json!(cfg.thumb_inline_parallel),
        now,
    )
    .await?;
    tracing::info!("seeded cache.* + workers.* rows from env (one-time bootstrap)");
    Ok(())
}
