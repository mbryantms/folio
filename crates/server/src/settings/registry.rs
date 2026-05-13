//! Registry of DB-backed setting keys.
//!
//! M1 shipped this empty. M2 added the SMTP block; M3 (this milestone)
//! adds the auth + OIDC identity surface so operators can flip mode, swap
//! IdPs, and toggle registration from `/admin/auth` without restarting.
//! M4+ will add log level, rate limit, workers, etc.
//!
//! Adding a key here is the *only* place that needs to change when a new
//! setting is exposed: the API surface, the overlay, and the audit payload
//! all read from this table.

/// What kind of JSON value a setting expects. Used by the API layer to
/// reject malformed PATCH bodies before they hit the encryption / DB write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    String,
    Bool,
    /// Non-negative integer in the JSON `Number` family. We accept anything
    /// that fits in `u64`; downstream validation in `Config::validate`
    /// catches values out of the meaningful range for a specific field.
    Uint,
    /// A duration string like `15m` / `30d` accepted by
    /// `Config::parse_duration`.
    Duration,
}

#[derive(Debug, Clone, Copy)]
pub struct SettingDef {
    pub key: &'static str,
    pub kind: SettingKind,
    /// True when the value should be sealed under the AEAD key before
    /// storage and redacted (`"<set>"`) in API responses.
    pub is_secret: bool,
}

/// All known DB-backed settings. See module docs for the milestone roadmap.
pub const REGISTRY: &[SettingDef] = &[
    // ───────── SMTP (M2) ─────────
    SettingDef {
        key: "smtp.host",
        kind: SettingKind::String,
        is_secret: false,
    },
    SettingDef {
        key: "smtp.port",
        kind: SettingKind::Uint,
        is_secret: false,
    },
    SettingDef {
        // `none` | `starttls` | `tls`
        key: "smtp.tls",
        kind: SettingKind::String,
        is_secret: false,
    },
    SettingDef {
        key: "smtp.username",
        kind: SettingKind::String,
        is_secret: false,
    },
    SettingDef {
        key: "smtp.password",
        kind: SettingKind::String,
        is_secret: true,
    },
    SettingDef {
        key: "smtp.from",
        kind: SettingKind::String,
        is_secret: false,
    },
    // ───────── Identity (M3) ─────────
    SettingDef {
        // `local` | `oidc` | `both`
        key: "auth.mode",
        kind: SettingKind::String,
        is_secret: false,
    },
    SettingDef {
        key: "auth.local.registration_open",
        kind: SettingKind::Bool,
        is_secret: false,
    },
    SettingDef {
        key: "auth.oidc.issuer",
        kind: SettingKind::String,
        is_secret: false,
    },
    SettingDef {
        key: "auth.oidc.client_id",
        kind: SettingKind::String,
        is_secret: false,
    },
    SettingDef {
        key: "auth.oidc.client_secret",
        kind: SettingKind::String,
        is_secret: true,
    },
    SettingDef {
        key: "auth.oidc.trust_unverified_email",
        kind: SettingKind::Bool,
        is_secret: false,
    },
    // ───────── Tokens + hardening + log level (M4) ─────────
    SettingDef {
        key: "auth.jwt.access_ttl",
        kind: SettingKind::Duration,
        is_secret: false,
    },
    SettingDef {
        key: "auth.jwt.refresh_ttl",
        kind: SettingKind::Duration,
        is_secret: false,
    },
    SettingDef {
        // Kill switch: when false, the failed-auth Redis lockout
        // short-circuits to "not locked". Per-route governor buckets
        // are installed at boot regardless (their thresholds are
        // conservative defaults that rarely need tuning).
        key: "auth.rate_limit_enabled",
        kind: SettingKind::Bool,
        is_secret: false,
    },
    SettingDef {
        // `trace` | `debug` | `info` | `warn` | `error`, or any
        // tracing-subscriber EnvFilter directive (e.g.
        // `info,server::auth=debug`).
        key: "observability.log_level",
        kind: SettingKind::String,
        is_secret: false,
    },
    // ───────── Operational tuning (M5) ─────────
    //
    // All M5 keys take effect on the **next process restart**. apalis
    // worker pools and the ZIP LRU are sized at boot; a live restart of
    // either is a non-trivial refactor that's deferred. The admin UI
    // labels these cards "applies on next restart."
    SettingDef {
        key: "cache.zip_lru_capacity",
        kind: SettingKind::Uint,
        is_secret: false,
    },
    SettingDef {
        key: "workers.scan_count",
        kind: SettingKind::Uint,
        is_secret: false,
    },
    SettingDef {
        key: "workers.post_scan_count",
        kind: SettingKind::Uint,
        is_secret: false,
    },
    SettingDef {
        key: "workers.scan_batch_size",
        kind: SettingKind::Uint,
        is_secret: false,
    },
    SettingDef {
        key: "workers.scan_hash_buffer_kb",
        kind: SettingKind::Uint,
        is_secret: false,
    },
    SettingDef {
        key: "workers.archive_work_parallel",
        kind: SettingKind::Uint,
        is_secret: false,
    },
    SettingDef {
        key: "workers.thumb_inline_parallel",
        kind: SettingKind::Uint,
        is_secret: false,
    },
];

pub fn registry() -> &'static [SettingDef] {
    REGISTRY
}

pub fn lookup(key: &str) -> Option<&'static SettingDef> {
    REGISTRY.iter().find(|d| d.key == key)
}

pub fn is_known(key: &str) -> bool {
    lookup(key).is_some()
}

pub fn is_secret(key: &str) -> bool {
    lookup(key).is_some_and(|d| d.is_secret)
}

/// True when the key belongs to a settings group that requires rebuilding
/// the [`EmailSender`] after a successful PATCH. Used by the
/// `/admin/settings` handler to decide whether to swap the live sender.
pub fn affects_email(key: &str) -> bool {
    key.starts_with("smtp.")
}

/// True when the key belongs to a settings group that requires evicting
/// the OIDC discovery cache after a successful PATCH. Triggered by any
/// change to `auth.oidc.*` (issuer / client_id / client_secret) or to
/// `auth.mode` itself.
pub fn affects_oidc(key: &str) -> bool {
    key == "auth.mode" || key.starts_with("auth.oidc.")
}

/// True when the key requires swapping the tracing reload handle (live
/// log-level change). Used by `/admin/settings` to fire `.modify(...)`
/// on the handle after a successful save.
pub fn affects_log_level(key: &str) -> bool {
    key == "observability.log_level"
}
