//! Server configuration loaded from env / `config.toml` (§12.3).
//!
//! All values validated on startup; the server refuses to boot with bad/missing
//! security-sensitive values (§15.5).

use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    Oidc,
    Local,
    Both,
}

impl std::fmt::Display for AuthMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Oidc => write!(f, "oidc"),
            Self::Local => write!(f, "local"),
            Self::Both => write!(f, "both"),
        }
    }
}

impl AuthMode {
    /// Parse the same string format `Display` produces. Returns `None`
    /// for any other input — callers (overlay match arms,
    /// `validate_auth_mode_string`) decide whether to WARN + ignore or
    /// hard-fail.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "oidc" => Some(Self::Oidc),
            "local" => Some(Self::Local),
            "both" => Some(Self::Both),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub database_url: String,
    /// Required since Library Scanner v1 (Milestone 2). Apalis-backed scan
    /// dispatch and post-scan jobs are not optional.
    pub redis_url: String,

    pub library_path: PathBuf,
    pub data_path: PathBuf,
    pub public_url: String,

    #[serde(default = "default_bind")]
    pub bind_addr: SocketAddr,

    #[serde(default = "default_log_level")]
    pub log_level: String,

    #[serde(default)]
    pub trusted_proxies: String,

    #[serde(default = "default_auth_mode")]
    pub auth_mode: AuthMode,

    /// Upstream URL of the Next.js standalone server. Used by the
    /// `upstream::proxy` fallback to forward HTML/static requests that
    /// no Rust route handled. Matches the design intent in
    /// `web/next.config.ts:26,40-41` — Rust is the public origin,
    /// Next is an internal SSR upstream. See
    /// `~/.claude/plans/rust-public-origin-1.0.md` for the rollout.
    ///
    /// In dev (`just dev`), Next listens on `:3000` on localhost. In
    /// the published compose file (post-cutover), this points at the
    /// internal service hostname, e.g. `http://folio-web:3000`.
    #[serde(default = "default_web_upstream_url")]
    pub web_upstream_url: String,

    // OIDC
    #[serde(default)]
    pub oidc_issuer: Option<String>,
    #[serde(default)]
    pub oidc_client_id: Option<String>,
    #[serde(default)]
    pub oidc_client_secret: Option<String>,
    #[serde(default)]
    pub oidc_trust_unverified_email: bool,

    // Local
    #[serde(default)]
    pub local_registration_open: bool,

    #[serde(default = "default_jwt_access")]
    pub jwt_access_ttl: String,
    #[serde(default = "default_jwt_refresh")]
    pub jwt_refresh_ttl: String,

    #[serde(default = "default_true")]
    pub rate_limit_enabled: bool,

    #[serde(default)]
    pub otlp_endpoint: Option<String>,

    #[serde(default = "default_true")]
    pub auto_migrate: bool,

    #[serde(default = "default_zip_lru_capacity")]
    pub zip_lru_capacity: usize,

    /// Per-queue worker concurrency for scan jobs (spec §3.2, §11).
    #[serde(default = "default_scan_worker_count")]
    pub scan_worker_count: usize,
    /// Worker concurrency for post-scan jobs (thumbs / search / dictionary).
    #[serde(default = "default_post_scan_worker_count")]
    pub post_scan_worker_count: usize,
    /// Per-series upsert batch size (spec §9).
    #[serde(default = "default_scan_batch_size")]
    pub scan_batch_size: usize,
    /// BLAKE3 streaming buffer size in KiB (spec §11).
    #[serde(default = "default_scan_hash_buffer_kb")]
    pub scan_hash_buffer_kb: usize,
    /// Global cap for blocking archive/hash/decode work across scanner and
    /// thumbnail workers. This is intentionally separate from queue
    /// concurrency so several queued scans cannot multiply into unbounded
    /// archive I/O.
    #[serde(default = "default_archive_work_parallel")]
    pub archive_work_parallel: usize,

    /// Cap on concurrent on-demand thumbnail generation. The post-scan
    /// worker handles the bulk of the work; this only matters for
    /// freshly-added libraries where the reader hits a thumb before the
    /// worker has caught up. Default 4 keeps a page-strip burst from
    /// saturating the encoder pool while still allowing some parallelism.
    #[serde(default = "default_thumb_inline_parallel")]
    pub thumb_inline_parallel: usize,

    // Archive limits (spec §4.1.1). Defaults mirror the `archive` crate's
    // `ArchiveLimits::default()`; overridable per-deploy via the
    // `COMIC_ARCHIVE_MAX_*` env vars to tune DoS bounds for unusually
    // large libraries (or to harden against malicious uploads on a
    // public-facing deployment).
    #[serde(default = "default_archive_max_entries")]
    pub archive_max_entries: u64,
    #[serde(default = "default_archive_max_total_bytes")]
    pub archive_max_total_bytes: u64,
    #[serde(default = "default_archive_max_entry_bytes")]
    pub archive_max_entry_bytes: u64,
    #[serde(default = "default_archive_max_ratio")]
    pub archive_max_ratio: u32,
    #[serde(default = "default_archive_max_nesting")]
    pub archive_max_nesting: u8,

    // SMTP
    #[serde(default)]
    pub smtp_host: Option<String>,
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    #[serde(default)]
    pub smtp_username: Option<String>,
    #[serde(default)]
    pub smtp_password: Option<String>,
    #[serde(default = "default_smtp_tls")]
    pub smtp_tls: String,
    #[serde(default)]
    pub smtp_from: Option<String>,
}

fn default_bind() -> SocketAddr {
    "0.0.0.0:8080".parse().unwrap()
}
fn default_log_level() -> String {
    "info".into()
}
fn default_auth_mode() -> AuthMode {
    AuthMode::Both
}
fn default_web_upstream_url() -> String {
    "http://localhost:3000".into()
}
fn default_true() -> bool {
    true
}
fn default_jwt_access() -> String {
    // Long-lived access cookie. Comic reading sessions are content-consumption
    // workloads — getting kicked back to sign-in mid-issue is friction with no
    // proportional security win. The refresh cookie still rotates on every
    // refresh and is bound by `jwt_refresh_ttl`, so a leaked access cookie
    // window is at most 24h before the next forced rotation.
    "24h".into()
}
fn default_jwt_refresh() -> String {
    // Refresh-token TTL (the "stay signed in for a month" budget). The web
    // client transparently calls `POST /auth/refresh` on 401 so the user
    // never sees the access cookie expire as long as they used the app
    // within this window.
    "30d".into()
}
fn default_smtp_port() -> u16 {
    587
}
fn default_smtp_tls() -> String {
    "starttls".into()
}
fn default_zip_lru_capacity() -> usize {
    64
}
fn default_scan_worker_count() -> usize {
    // Bumped from `min(cpu, 4)` to `min(cpu, 8)` after the F-9 perf pass
    // showed cold scans are IO-bound at the kernel page-cache level on
    // modern NVMe — more concurrent workers saturate the IO pipe further
    // without thrashing CPU. Operators on memory-constrained or
    // oversubscribed deployments can still cap via COMIC_SCAN_WORKER_COUNT.
    // See docs/dev/scanner-perf.md F-9.
    std::thread::available_parallelism()
        .map(|n| n.get().min(8))
        .unwrap_or(2)
}
fn default_post_scan_worker_count() -> usize {
    // Match the shape of `default_scan_worker_count`. Half of available
    // parallelism, clamped to [2, 8]. Thumbnail jobs are CPU-bound (decode
    // + encode), so a 16-core box should run 8 concurrent issues during a
    // backfill or `THUMBNAIL_VERSION` bump catchup; a 2-core box stays at
    // 2. Operators can still override via `COMIC_POST_SCAN_WORKER_COUNT`
    // for memory-constrained or oversubscribed deployments.
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).clamp(2, 8))
        .unwrap_or(2)
}
fn default_thumb_inline_parallel() -> usize {
    // Browsers open ~6 connections per origin for the page-strip burst on
    // first-load of a freshly-added issue. A cap of 4 forced the last 2
    // requests to wait through the 2s semaphore timeout and serve 503s;
    // 8 absorbs the burst with one slot to spare.
    8
}
fn default_scan_batch_size() -> usize {
    100
}
fn default_scan_hash_buffer_kb() -> usize {
    // Bumped from 64 KB to 1024 KB (1 MiB) after the F-9 perf pass:
    // larger buffers amortize syscall + page-cache-readahead overhead. The
    // hash function used a hardcoded 1 MiB prior to F-9; this default
    // matches that, just now actually plumbed via the env var. See
    // docs/dev/scanner-perf.md F-9. Floor at 64 KB enforced inside
    // `blake3_file_with_buffer`.
    1024
}
fn default_archive_work_parallel() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().clamp(2, 8))
        .unwrap_or(2)
}
// Mirrors `archive::ArchiveLimits::default()`. Listed inline so a
// reviewer sees the contract without cross-file hopping; the
// `Config::archive_limits()` accessor downstream is the single source
// of truth that actually feeds the archive crate.
fn default_archive_max_entries() -> u64 {
    50_000
}
fn default_archive_max_total_bytes() -> u64 {
    8 * 1024 * 1024 * 1024
}
fn default_archive_max_entry_bytes() -> u64 {
    512 * 1024 * 1024
}
fn default_archive_max_ratio() -> u32 {
    200
}
fn default_archive_max_nesting() -> u8 {
    1
}

/// Where the effective value for a single setting came from. Surfaced by
/// `GET /admin/settings` so an operator can see why an admin-UI save is
/// being shadowed by an env var still set in the compose file (D2 in the
/// runtime-config-admin plan).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provenance {
    /// Hardcoded default in `Config::load` / serde defaults.
    Default,
    /// Loaded from a `COMIC_*` env var or `config.toml`.
    Env,
    /// Loaded from the `app_setting` table.
    Db,
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        // Env > config.toml > defaults.
        let cfg: Config = Figment::new()
            .merge(Toml::file("config.toml"))
            .merge(Env::prefixed("COMIC_"))
            .extract()?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Build the `archive::ArchiveLimits` the archive crate consumes,
    /// reading the env-tunable caps from this config. Subprocess-bound
    /// fields (wall_timeout / rss_bytes) stay at the `ArchiveLimits::
    /// default()` values for now — those govern CBR/CB7 extraction
    /// which is currently stubbed; revisit when those formats ship.
    pub fn archive_limits(&self) -> archive::ArchiveLimits {
        let defaults = archive::ArchiveLimits::default();
        archive::ArchiveLimits {
            max_entries: self.archive_max_entries,
            max_total_bytes: self.archive_max_total_bytes,
            max_entry_bytes: self.archive_max_entry_bytes,
            max_compression_ratio: self.archive_max_ratio,
            max_nesting_depth: self.archive_max_nesting,
            subprocess_wall_timeout: defaults.subprocess_wall_timeout,
            subprocess_rss_bytes: defaults.subprocess_rss_bytes,
        }
    }

    /// Apply DB-stored overrides on top of an env-loaded `Config`.
    ///
    /// Precedence: DB wins over env (D1 in the plan); collisions are logged
    /// at WARN so operators can see when a stale `.env` is being shadowed.
    /// M2 wires the SMTP block; M3+ adds auth/OIDC, log level, workers.
    ///
    /// Returns `Err` if a DB-stored value fails the post-overlay
    /// `validate()` — the caller falls back to the env-only Config in
    /// that case to keep the server bootable.
    pub async fn overlay_db(
        &mut self,
        db: &sea_orm::DatabaseConnection,
        secrets: &crate::secrets::Secrets,
    ) -> anyhow::Result<()> {
        let rows = crate::settings::read_all(db, secrets).await?;
        for r in rows {
            apply_overlay_row(self, &r);
        }
        self.validate()?;
        Ok(())
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        match self.auth_mode {
            AuthMode::Oidc | AuthMode::Both => {
                if self.oidc_issuer.is_none()
                    || self.oidc_client_id.is_none()
                    || self.oidc_client_secret.is_none()
                {
                    anyhow::bail!(
                        "auth_mode includes oidc but COMIC_OIDC_ISSUER / CLIENT_ID / CLIENT_SECRET are not all set"
                    );
                }
            }
            AuthMode::Local => {}
        }
        if !self.public_url.starts_with("http") {
            anyhow::bail!("COMIC_PUBLIC_URL must be an http(s) URL");
        }
        // Loud WARN when a release build is fronted by `http://localhost`
        // or `http://127.0.0.1` — the .env.example dev default. In prod
        // this is almost always a misconfiguration: CSP `connect-src`
        // gets the wrong WebSocket origin, `__Host-` cookies don't get
        // `Secure` (browser rejects them), and OIDC redirect URIs build
        // against the loopback. The original prod incident on
        // 2026-05-16 hit all three because operators carried the dev
        // value forward. Keep the WARN dev-friendly: in debug builds
        // `localhost` is the correct value, so we only warn in release.
        if !cfg!(debug_assertions) {
            let is_loopback = self.public_url.starts_with("http://localhost")
                || self.public_url.starts_with("http://127.")
                || self.public_url.starts_with("http://[::1]")
                || self.public_url.starts_with("http://0.0.0.0");
            if is_loopback {
                tracing::warn!(
                    public_url = %self.public_url,
                    "COMIC_PUBLIC_URL points at a loopback / dev address in a release build. \
                     This breaks `__Host-` cookies (no Secure), CSP `connect-src` (wrong WS origin), \
                     and OIDC redirect URIs. Set it to the actual public URL operators reach \
                     the app at, e.g. https://comics.example.com."
                );
            }
        }
        if self.oidc_trust_unverified_email {
            tracing::warn!("COMIC_OIDC_TRUST_UNVERIFIED_EMAIL=true — see §12.7 of the spec");
        }
        // Fail fast on misconfigured TTL strings rather than at first sign-in.
        Self::parse_duration(&self.jwt_access_ttl)
            .map_err(|e| anyhow::anyhow!("jwt_access_ttl: {e}"))?;
        let refresh = Self::parse_duration(&self.jwt_refresh_ttl)
            .map_err(|e| anyhow::anyhow!("jwt_refresh_ttl: {e}"))?;
        let access = Self::parse_duration(&self.jwt_access_ttl)?;
        if refresh < access {
            anyhow::bail!(
                "jwt_refresh_ttl ({}) must be ≥ jwt_access_ttl ({}); otherwise the refresh cookie expires before the access cookie and users get stuck on a hard logout",
                self.jwt_refresh_ttl,
                self.jwt_access_ttl,
            );
        }
        // Log level must be a directive `tracing_subscriber::EnvFilter` can
        // parse. Caught here so a bad value rejects the PATCH before the
        // reload handle is touched.
        tracing_subscriber::EnvFilter::try_new(&self.log_level)
            .map_err(|e| anyhow::anyhow!("log_level: {e}"))?;

        // Operational tuning ranges (M5). Bounds match the safety
        // envelope agreed in the runtime-config-admin plan; values
        // outside these reject the PATCH dry-run with 400.
        fn check_range<T: PartialOrd + std::fmt::Display>(
            name: &str,
            value: T,
            min: T,
            max: T,
        ) -> anyhow::Result<()> {
            if value < min || value > max {
                anyhow::bail!("{name} must be in [{min}, {max}], got {value}");
            }
            Ok(())
        }
        check_range("zip_lru_capacity", self.zip_lru_capacity, 1, 4096)?;
        check_range("scan_worker_count", self.scan_worker_count, 1, 64)?;
        check_range("post_scan_worker_count", self.post_scan_worker_count, 1, 64)?;
        check_range("scan_batch_size", self.scan_batch_size, 1, 10_000)?;
        check_range("scan_hash_buffer_kb", self.scan_hash_buffer_kb, 64, 65_536)?;
        check_range("archive_work_parallel", self.archive_work_parallel, 1, 64)?;
        check_range("thumb_inline_parallel", self.thumb_inline_parallel, 1, 64)?;
        Ok(())
    }

    /// Access-cookie + access-JWT TTL. Validated at startup so unwrap is safe
    /// once the server is running.
    pub fn access_ttl(&self) -> Duration {
        Self::parse_duration(&self.jwt_access_ttl).expect("jwt_access_ttl validated at startup")
    }

    /// Refresh-cookie + DB session TTL. Validated at startup.
    pub fn refresh_ttl(&self) -> Duration {
        Self::parse_duration(&self.jwt_refresh_ttl).expect("jwt_refresh_ttl validated at startup")
    }

    pub fn parse_duration(s: &str) -> anyhow::Result<Duration> {
        // Accepts `15m`, `30d`, `60s`, `1h`. Anything else errors.
        let (num, unit) = s.split_at(s.len() - 1);
        let n: u64 = num.parse()?;
        Ok(match unit {
            "s" => Duration::from_secs(n),
            "m" => Duration::from_secs(n * 60),
            "h" => Duration::from_secs(n * 60 * 60),
            "d" => Duration::from_secs(n * 60 * 60 * 24),
            _ => anyhow::bail!("invalid duration unit (use s|m|h|d): {s}"),
        })
    }
}

/// Bind a single DB-stored setting row onto the corresponding [`Config`]
/// field. Unknown keys are logged at DEBUG and ignored so an older binary
/// can roll back across a migration window without losing data. Type
/// errors are logged at WARN and the env value is kept — operators see
/// the warning in /admin/logs.
///
/// "Collision" warnings fire only when the env-set value *differs* from
/// the DB value; the bootstrap path that copies env→DB on first boot
/// would otherwise be noisy on every restart even though the values
/// agree.
pub(crate) fn apply_overlay_row(cfg: &mut Config, row: &crate::settings::Resolved) {
    use serde_json::Value;

    fn warn_if_diverges(key: &str, env: Option<&str>, db: &str) {
        if let Some(env) = env.filter(|e| !e.is_empty())
            && env != db
        {
            tracing::warn!(
                key = %key,
                "app_setting collision: env and DB disagree on this key; DB value wins"
            );
        }
    }
    fn bad_type(key: &str, expected: &str, value: &Value) {
        tracing::warn!(
            key = %key,
            expected = %expected,
            got = %value,
            "app_setting type mismatch; keeping env value"
        );
    }

    match row.key.as_str() {
        "smtp.host" => match row.value.as_str() {
            Some(s) => {
                warn_if_diverges(&row.key, cfg.smtp_host.as_deref(), s);
                cfg.smtp_host = if s.trim().is_empty() {
                    None
                } else {
                    Some(s.to_owned())
                };
            }
            None => bad_type(&row.key, "string", &row.value),
        },
        "smtp.port" => match row.value.as_u64() {
            Some(n) if (1..=65535).contains(&n) => {
                let n16 = n as u16;
                if n16 != cfg.smtp_port {
                    tracing::debug!(env = cfg.smtp_port, db = n16, "smtp.port overridden by DB");
                }
                cfg.smtp_port = n16;
            }
            Some(n) => tracing::warn!(key = "smtp.port", got = n, "out of range"),
            None => bad_type(&row.key, "uint", &row.value),
        },
        "smtp.tls" => match row.value.as_str() {
            Some(s) if matches!(s, "none" | "starttls" | "tls") => {
                warn_if_diverges(&row.key, Some(cfg.smtp_tls.as_str()), s);
                cfg.smtp_tls = s.to_owned();
            }
            Some(other) => tracing::warn!(
                key = "smtp.tls",
                got = %other,
                "expected one of none|starttls|tls; keeping env value"
            ),
            None => bad_type(&row.key, "string", &row.value),
        },
        "smtp.username" => match row.value.as_str() {
            Some(s) => {
                warn_if_diverges(&row.key, cfg.smtp_username.as_deref(), s);
                cfg.smtp_username = if s.is_empty() {
                    None
                } else {
                    Some(s.to_owned())
                };
            }
            None => bad_type(&row.key, "string", &row.value),
        },
        "smtp.password" => match row.value.as_str() {
            Some(s) => {
                // Never log password values; only flag the bare existence
                // of a divergence so the operator knows to reconcile.
                if let Some(env_pw) = cfg.smtp_password.as_deref().filter(|p| !p.is_empty())
                    && env_pw != s
                {
                    tracing::warn!(
                        key = "smtp.password",
                        "app_setting collision: env and DB disagree on this key; DB value wins"
                    );
                }
                cfg.smtp_password = if s.is_empty() {
                    None
                } else {
                    Some(s.to_owned())
                };
            }
            None => bad_type(&row.key, "string", &row.value),
        },
        "smtp.from" => match row.value.as_str() {
            Some(s) => {
                warn_if_diverges(&row.key, cfg.smtp_from.as_deref(), s);
                cfg.smtp_from = if s.trim().is_empty() {
                    None
                } else {
                    Some(s.to_owned())
                };
            }
            None => bad_type(&row.key, "string", &row.value),
        },

        // ───────── Identity (M3) ─────────
        "auth.mode" => match row.value.as_str() {
            Some(s) => match AuthMode::parse(s) {
                Some(mode) => {
                    let prev_str = cfg.auth_mode.to_string();
                    if prev_str != s {
                        tracing::debug!(env = %prev_str, db = %s, "auth.mode overridden by DB");
                    }
                    cfg.auth_mode = mode;
                }
                None => tracing::warn!(
                    key = "auth.mode",
                    got = %s,
                    "expected one of local|oidc|both; keeping env value"
                ),
            },
            None => bad_type(&row.key, "string", &row.value),
        },
        "auth.local.registration_open" => match row.value.as_bool() {
            Some(b) => {
                if cfg.local_registration_open != b {
                    tracing::debug!(
                        env = cfg.local_registration_open,
                        db = b,
                        "auth.local.registration_open overridden by DB"
                    );
                }
                cfg.local_registration_open = b;
            }
            None => bad_type(&row.key, "bool", &row.value),
        },
        "auth.oidc.issuer" => match row.value.as_str() {
            Some(s) => {
                warn_if_diverges(&row.key, cfg.oidc_issuer.as_deref(), s);
                cfg.oidc_issuer = if s.trim().is_empty() {
                    None
                } else {
                    Some(s.to_owned())
                };
            }
            None => bad_type(&row.key, "string", &row.value),
        },
        "auth.oidc.client_id" => match row.value.as_str() {
            Some(s) => {
                warn_if_diverges(&row.key, cfg.oidc_client_id.as_deref(), s);
                cfg.oidc_client_id = if s.trim().is_empty() {
                    None
                } else {
                    Some(s.to_owned())
                };
            }
            None => bad_type(&row.key, "string", &row.value),
        },
        "auth.oidc.client_secret" => match row.value.as_str() {
            Some(s) => {
                // Never log secret values; only flag the bare existence
                // of a divergence so the operator knows to reconcile.
                if let Some(env_pw) = cfg.oidc_client_secret.as_deref().filter(|p| !p.is_empty())
                    && env_pw != s
                {
                    tracing::warn!(
                        key = "auth.oidc.client_secret",
                        "app_setting collision: env and DB disagree on this key; DB value wins"
                    );
                }
                cfg.oidc_client_secret = if s.is_empty() {
                    None
                } else {
                    Some(s.to_owned())
                };
            }
            None => bad_type(&row.key, "string", &row.value),
        },
        "auth.oidc.trust_unverified_email" => match row.value.as_bool() {
            Some(b) => {
                if cfg.oidc_trust_unverified_email != b {
                    tracing::debug!(
                        env = cfg.oidc_trust_unverified_email,
                        db = b,
                        "auth.oidc.trust_unverified_email overridden by DB"
                    );
                }
                cfg.oidc_trust_unverified_email = b;
            }
            None => bad_type(&row.key, "bool", &row.value),
        },

        // ───────── Tokens + hardening + log level (M4) ─────────
        "auth.jwt.access_ttl" => match row.value.as_str() {
            Some(s) => {
                warn_if_diverges(&row.key, Some(cfg.jwt_access_ttl.as_str()), s);
                cfg.jwt_access_ttl = s.to_owned();
            }
            None => bad_type(&row.key, "duration", &row.value),
        },
        "auth.jwt.refresh_ttl" => match row.value.as_str() {
            Some(s) => {
                warn_if_diverges(&row.key, Some(cfg.jwt_refresh_ttl.as_str()), s);
                cfg.jwt_refresh_ttl = s.to_owned();
            }
            None => bad_type(&row.key, "duration", &row.value),
        },
        "auth.rate_limit_enabled" => match row.value.as_bool() {
            Some(b) => {
                if cfg.rate_limit_enabled != b {
                    tracing::debug!(
                        env = cfg.rate_limit_enabled,
                        db = b,
                        "auth.rate_limit_enabled overridden by DB"
                    );
                }
                cfg.rate_limit_enabled = b;
            }
            None => bad_type(&row.key, "bool", &row.value),
        },
        "observability.log_level" => match row.value.as_str() {
            Some(s) => {
                warn_if_diverges(&row.key, Some(cfg.log_level.as_str()), s);
                cfg.log_level = s.to_owned();
            }
            None => bad_type(&row.key, "string", &row.value),
        },

        // ───────── Operational tuning (M5) ─────────
        //
        // These values are read at boot to size the apalis worker pools
        // and the ZIP LRU. The overlay applies them so the *next* boot
        // picks up the new values; live runtime change is not wired.
        // Range validation runs in `Config::validate` so dry-run
        // rejects out-of-range values with 400 before they reach DB.
        "cache.zip_lru_capacity" => match row.value.as_u64() {
            Some(n) => cfg.zip_lru_capacity = n as usize,
            None => bad_type(&row.key, "uint", &row.value),
        },
        "workers.scan_count" => match row.value.as_u64() {
            Some(n) => cfg.scan_worker_count = n as usize,
            None => bad_type(&row.key, "uint", &row.value),
        },
        "workers.post_scan_count" => match row.value.as_u64() {
            Some(n) => cfg.post_scan_worker_count = n as usize,
            None => bad_type(&row.key, "uint", &row.value),
        },
        "workers.scan_batch_size" => match row.value.as_u64() {
            Some(n) => cfg.scan_batch_size = n as usize,
            None => bad_type(&row.key, "uint", &row.value),
        },
        "workers.scan_hash_buffer_kb" => match row.value.as_u64() {
            Some(n) => cfg.scan_hash_buffer_kb = n as usize,
            None => bad_type(&row.key, "uint", &row.value),
        },
        "workers.archive_work_parallel" => match row.value.as_u64() {
            Some(n) => cfg.archive_work_parallel = n as usize,
            None => bad_type(&row.key, "uint", &row.value),
        },
        "workers.thumb_inline_parallel" => match row.value.as_u64() {
            Some(n) => cfg.thumb_inline_parallel = n as usize,
            None => bad_type(&row.key, "uint", &row.value),
        },

        other => {
            tracing::debug!(key = %other, "app_setting row ignored (no overlay binding yet)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D-10: confirm `Config::archive_limits()` round-trips every
    /// env-tunable field into the `archive::ArchiveLimits` the
    /// archive crate consumes. Defaults the test config to absurd
    /// values per field so a swap or typo in the accessor surfaces
    /// immediately rather than getting masked by matching defaults.
    #[test]
    fn archive_limits_round_trips_env_tunable_fields() {
        let cfg = Config {
            archive_max_entries: 1234,
            archive_max_total_bytes: 5_678_900,
            archive_max_entry_bytes: 1_111_222,
            archive_max_ratio: 7,
            archive_max_nesting: 3,
            ..test_config_skeleton()
        };
        let limits = cfg.archive_limits();
        assert_eq!(limits.max_entries, 1234);
        assert_eq!(limits.max_total_bytes, 5_678_900);
        assert_eq!(limits.max_entry_bytes, 1_111_222);
        assert_eq!(limits.max_compression_ratio, 7);
        assert_eq!(limits.max_nesting_depth, 3);
        // Subprocess-bound fields are CBR/CB7-specific and keep the
        // archive crate's defaults; this is the contract.
        let defaults = archive::ArchiveLimits::default();
        assert_eq!(
            limits.subprocess_wall_timeout,
            defaults.subprocess_wall_timeout
        );
        assert_eq!(limits.subprocess_rss_bytes, defaults.subprocess_rss_bytes);
    }

    /// Minimal `Config` skeleton — only the fields needed to exist
    /// for `archive_limits()` to be callable. All non-archive fields
    /// get default-shaped values; do NOT add archive_max_* here so
    /// the round-trip test above can set them explicitly via
    /// `..test_config_skeleton()`.
    fn test_config_skeleton() -> Config {
        Config {
            database_url: String::new(),
            redis_url: String::new(),
            library_path: PathBuf::new(),
            data_path: PathBuf::new(),
            public_url: "http://localhost".into(),
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            log_level: "info".into(),
            trusted_proxies: String::new(),
            auth_mode: AuthMode::Local,
            web_upstream_url: "http://127.0.0.1:0".into(),
            oidc_issuer: None,
            oidc_client_id: None,
            oidc_client_secret: None,
            oidc_trust_unverified_email: false,
            local_registration_open: true,
            jwt_access_ttl: "24h".into(),
            jwt_refresh_ttl: "30d".into(),
            rate_limit_enabled: true,
            otlp_endpoint: None,
            auto_migrate: false,
            zip_lru_capacity: 16,
            scan_worker_count: 1,
            post_scan_worker_count: 1,
            scan_batch_size: 100,
            scan_hash_buffer_kb: 64,
            archive_work_parallel: 1,
            thumb_inline_parallel: 1,
            archive_max_entries: 0,
            archive_max_total_bytes: 0,
            archive_max_entry_bytes: 0,
            archive_max_ratio: 0,
            archive_max_nesting: 0,
            smtp_host: None,
            smtp_port: 587,
            smtp_username: None,
            smtp_password: None,
            smtp_tls: "starttls".into(),
            smtp_from: None,
        }
    }
}
