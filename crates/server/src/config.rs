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

    fn validate(&self) -> anyhow::Result<()> {
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
        if self.oidc_trust_unverified_email {
            tracing::warn!("COMIC_OIDC_TRUST_UNVERIFIED_EMAIL=true — see §12.7 of the spec");
        }
        // Fail fast on misconfigured TTL strings rather than at first sign-in.
        Self::parse_duration(&self.jwt_access_ttl)
            .map_err(|e| anyhow::anyhow!("COMIC_JWT_ACCESS_TTL: {e}"))?;
        let refresh = Self::parse_duration(&self.jwt_refresh_ttl)
            .map_err(|e| anyhow::anyhow!("COMIC_JWT_REFRESH_TTL: {e}"))?;
        let access = Self::parse_duration(&self.jwt_access_ttl)?;
        if refresh < access {
            anyhow::bail!(
                "COMIC_JWT_REFRESH_TTL ({}) must be ≥ COMIC_JWT_ACCESS_TTL ({}); otherwise the refresh cookie expires before the access cookie and users get stuck on a hard logout",
                self.jwt_refresh_ttl,
                self.jwt_access_ttl,
            );
        }
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
