//! Local metadata databases — operator-supplied SQLite copies of a
//! provider's catalogue, read in place of (or ahead of) the provider's
//! API. Plan: `~/.claude/plans/local-metadata-databases-1.0.md`;
//! operator doc: `docs/dev/local-metadata-databases.md`.
//!
//! Two files are recognised:
//!
//! - **Local GCD** — the SQLite variant of the Grand Comics Database
//!   dump (comics.org/download, "Private Use"; CC BY-SA 4.0).
//! - **Local ComicVine** — a community-built SQLite copy of ComicVine's
//!   catalogue (`cv_volume` / `cv_issue` / … tables). Folio reads it if
//!   the operator has one; it never fetches it.
//!
//! This module (M1) owns the plumbing every later milestone builds on:
//! the per-provider [`LocalDbMode`], opening a file **read-only and
//! immutable** (a file on a mounted volume never takes a lock and is
//! never written to), and [`probe`] — the structural check behind the
//! admin card's *Test* button: SQLite header, expected tables, row
//! counts, the dump date and a staleness warning. The search / fetch
//! backends over these files land in M2 (GCD) and M3 (ComicVine).

use chrono::{NaiveDate, Utc};
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection, Statement};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use super::identifier::Source;

/// A local file is considered stale (admin card warning only — never a
/// behaviour change, decision 7) once its dump date is older than this.
pub const STALE_AFTER_DAYS: i64 = 60;

/// How long a probe result is cached in Redis. The admin card and the
/// dashboard list providers on every load; counting a 2.2 M-row table
/// each time would make the page wait on disk. Keyed by the file's
/// size + mtime so a swapped file is re-probed.
const PROBE_CACHE_TTL_SECS: u64 = 10 * 60;

/// The 16-byte magic every SQLite 3 database file starts with.
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// Which provider a local file stands in for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LocalDbKind {
    Gcd,
    ComicVine,
}

impl LocalDbKind {
    pub fn source(self) -> Source {
        match self {
            Self::Gcd => Source::Gcd,
            Self::ComicVine => Source::ComicVine,
        }
    }

    pub fn from_source(source: Source) -> Option<Self> {
        match source {
            Source::Gcd => Some(Self::Gcd),
            Source::ComicVine => Some(Self::ComicVine),
            _ => None,
        }
    }

    /// Card title (decision 3: the ComicVine copy is always "Local
    /// ComicVine", never plain ComicVine).
    pub fn label(self) -> &'static str {
        match self {
            Self::Gcd => "Local GCD",
            Self::ComicVine => "Local ComicVine",
        }
    }

    /// `app_setting` keys for the path and the mode.
    pub fn path_setting(self) -> &'static str {
        match self {
            Self::Gcd => "metadata.gcd.local_path",
            Self::ComicVine => "metadata.comicvine.local_path",
        }
    }

    pub fn mode_setting(self) -> &'static str {
        match self {
            Self::Gcd => "metadata.gcd.local_mode",
            Self::ComicVine => "metadata.comicvine.local_mode",
        }
    }

    /// Tables the backend cannot work without. A file missing any of
    /// these fails the probe.
    pub fn required_tables(self) -> &'static [&'static str] {
        match self {
            Self::Gcd => &["gcd_series", "gcd_issue", "gcd_story", "gcd_publisher"],
            Self::ComicVine => &["cv_volume", "cv_issue", "cv_publisher"],
        }
    }

    /// Tables that enrich the backend when present; their absence is a
    /// warning on the card, not a failure.
    pub fn optional_tables(self) -> &'static [&'static str] {
        match self {
            Self::Gcd => &[
                "gcd_story_credit",
                "gcd_issue_credit",
                "gcd_creator",
                "gcd_language",
                "gcd_indicia_publisher",
                "gcd_series_bond",
            ],
            Self::ComicVine => &[
                "cv_person",
                "cv_sync_metadata",
                "volume_fts",
                "comic_covers",
            ],
        }
    }

    /// `(label, table)` pairs counted for the card.
    fn counted_tables(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Gcd => &[
                ("series", "gcd_series"),
                ("issues", "gcd_issue"),
                ("stories", "gcd_story"),
                ("publishers", "gcd_publisher"),
            ],
            Self::ComicVine => &[
                ("volumes", "cv_volume"),
                ("issues", "cv_issue"),
                ("publishers", "cv_publisher"),
                ("people", "cv_person"),
                ("covers", "comic_covers"),
            ],
        }
    }
}

/// How a provider uses its local file (decision 2: a global default
/// per provider, overridable per library from M2 on).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum LocalDbMode {
    /// The API only — today's behaviour.
    #[default]
    Off,
    /// The file answers first; the API is asked only for what the file
    /// has no record of (decision 7: never by date).
    PreferLocal,
    /// The file only; a record the file lacks is `NotFound`.
    LocalOnly,
}

impl LocalDbMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::PreferLocal => "prefer_local",
            Self::LocalOnly => "local_only",
        }
    }

    pub const ALL: [LocalDbMode; 3] = [Self::Off, Self::PreferLocal, Self::LocalOnly];

    /// Does this mode read the file at all?
    pub fn uses_file(self) -> bool {
        !matches!(self, Self::Off)
    }
}

impl FromStr for LocalDbMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "off" | "" => Ok(Self::Off),
            "prefer_local" => Ok(Self::PreferLocal),
            "local_only" => Ok(Self::LocalOnly),
            other => Err(format!(
                "unknown local database mode {other:?}; expected one of off, prefer_local, local_only"
            )),
        }
    }
}

/// Why a file could not be used.
#[derive(Debug, thiserror::Error)]
pub enum LocalDbError {
    #[error("no file at {0}")]
    Missing(String),
    #[error("{0} is not a SQLite 3 database")]
    NotSqlite(String),
    #[error("could not open {path}: {reason}")]
    Open { path: String, reason: String },
    #[error("query failed: {0}")]
    Query(String),
    #[error("missing required tables: {}", .0.join(", "))]
    MissingTables(Vec<String>),
}

/// What a probe learned about a file. Serialised onto the admin card.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LocalDbProbe {
    pub kind: LocalDbKind,
    pub path: String,
    pub size_bytes: u64,
    /// Newest record date the file carries (`YYYY-MM-DD`): ComicVine's
    /// `cv_sync_metadata.last_sync_date`, GCD's newest `gcd_issue.modified`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dump_date: Option<String>,
    /// `dump_date` older than [`STALE_AFTER_DAYS`].
    pub stale: bool,
    /// Row counts per labelled table (`issues`, `series` / `volumes`, …).
    pub counts: BTreeMap<String, u64>,
    /// Optional tables the file does not have.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_optional: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub probed_at: String,
}

/// Open a local file read-only. `immutable` tells SQLite the file cannot
/// change underneath it, so no lock is taken and no journal is touched:
/// safe on a mounted volume, and a file the operator replaces by rename
/// is simply re-opened next time.
pub async fn open(path: &Path) -> Result<DatabaseConnection, LocalDbError> {
    check_header(path)?;
    let url = format!("sqlite://{}?mode=ro&immutable=true", path.to_string_lossy());
    let mut opts = ConnectOptions::new(url);
    opts.max_connections(2)
        .sqlx_logging(false)
        .connect_timeout(Duration::from_secs(10));
    Database::connect(opts)
        .await
        .map_err(|e| LocalDbError::Open {
            path: path.display().to_string(),
            reason: e.to_string(),
        })
}

fn check_header(path: &Path) -> Result<(), LocalDbError> {
    use std::io::Read;
    let shown = path.display().to_string();
    let mut f = std::fs::File::open(path).map_err(|_| LocalDbError::Missing(shown.clone()))?;
    let mut magic = [0u8; 16];
    match f.read_exact(&mut magic) {
        Ok(()) if &magic == SQLITE_MAGIC => Ok(()),
        _ => Err(LocalDbError::NotSqlite(shown)),
    }
}

async fn scalar_i64(db: &DatabaseConnection, sql: &str) -> Result<Option<i64>, LocalDbError> {
    let row = db
        .query_one_raw(Statement::from_string(db.get_database_backend(), sql))
        .await
        .map_err(|e| LocalDbError::Query(e.to_string()))?;
    Ok(row.and_then(|r| r.try_get_by_index::<Option<i64>>(0).ok().flatten()))
}

async fn scalar_text(db: &DatabaseConnection, sql: &str) -> Result<Option<String>, LocalDbError> {
    let row = db
        .query_one_raw(Statement::from_string(db.get_database_backend(), sql))
        .await
        .map_err(|e| LocalDbError::Query(e.to_string()))?;
    Ok(row.and_then(|r| r.try_get_by_index::<Option<String>>(0).ok().flatten()))
}

/// Table names in the file (`sqlite_master`, tables and views).
pub async fn table_names(db: &DatabaseConnection) -> Result<Vec<String>, LocalDbError> {
    let rows = db
        .query_all_raw(Statement::from_string(
            db.get_database_backend(),
            "SELECT name FROM sqlite_master WHERE type IN ('table', 'view') ORDER BY name",
        ))
        .await
        .map_err(|e| LocalDbError::Query(e.to_string()))?;
    Ok(rows
        .into_iter()
        .filter_map(|r| r.try_get_by_index::<String>(0).ok())
        .collect())
}

/// Structural check of a file: header, tables, counts, dump date. This
/// is what *Test* runs and what the card shows; it never reads records.
pub async fn probe(kind: LocalDbKind, path: &Path) -> Result<LocalDbProbe, LocalDbError> {
    let meta =
        std::fs::metadata(path).map_err(|_| LocalDbError::Missing(path.display().to_string()))?;
    let db = open(path).await?;
    let tables = table_names(&db).await?;
    let has = |t: &str| tables.iter().any(|n| n == t);
    let missing: Vec<String> = kind
        .required_tables()
        .iter()
        .filter(|t| !has(t))
        .map(|t| (*t).to_owned())
        .collect();
    if !missing.is_empty() {
        return Err(LocalDbError::MissingTables(missing));
    }
    let missing_optional: Vec<String> = kind
        .optional_tables()
        .iter()
        .filter(|t| !has(t))
        .map(|t| (*t).to_owned())
        .collect();
    let mut counts = BTreeMap::new();
    for (label, table) in kind.counted_tables() {
        if has(table) {
            let n = scalar_i64(&db, &format!("SELECT count(*) FROM \"{table}\"")).await?;
            counts.insert((*label).to_owned(), n.unwrap_or(0).max(0) as u64);
        }
    }
    let mut warnings = Vec::new();
    let dump_date = match kind {
        LocalDbKind::ComicVine => {
            if has("cv_sync_metadata") {
                scalar_text(&db, "SELECT max(last_sync_date) FROM cv_sync_metadata").await?
            } else {
                None
            }
        }
        LocalDbKind::Gcd => match scalar_text(&db, "SELECT max(modified) FROM gcd_issue").await {
            Ok(v) => v,
            Err(_) => {
                warnings
                    .push("gcd_issue has no `modified` column; the dump date is unknown".into());
                None
            }
        },
    }
    .and_then(|s| s.get(..10).map(str::to_owned))
    .filter(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok());
    let stale = dump_date
        .as_deref()
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .is_some_and(|d| (Utc::now().date_naive() - d).num_days() > STALE_AFTER_DAYS);
    if stale {
        warnings.push(format!(
            "the file is more than {STALE_AFTER_DAYS} days old; newer releases are missing from it"
        ));
    }
    if counts.get("issues").copied().unwrap_or(0) == 0 {
        warnings.push("the issues table is empty".into());
    }
    for t in &missing_optional {
        warnings.push(format!("optional table {t} is missing"));
    }
    Ok(LocalDbProbe {
        kind,
        path: path.display().to_string(),
        size_bytes: meta.len(),
        dump_date,
        stale,
        counts,
        missing_optional,
        warnings,
        probed_at: Utc::now().to_rfc3339(),
    })
}

/// File identity for the probe cache: size + mtime, so a replaced file
/// re-probes.
fn file_stamp(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(format!("{}:{}", meta.len(), mtime))
}

fn cache_key(kind: LocalDbKind) -> String {
    format!("metadata:local_db:probe:{}", kind.source().as_str())
}

#[derive(Serialize, Deserialize)]
struct CachedProbe {
    stamp: String,
    path: String,
    result: Result<LocalDbProbe, String>,
}

/// [`probe`] through a short Redis cache (the card and dashboard list
/// providers on every load). `force` skips the cache — *Test* uses it.
pub async fn probe_cached(
    redis: &ConnectionManager,
    kind: LocalDbKind,
    path: &Path,
    force: bool,
) -> Result<LocalDbProbe, LocalDbError> {
    let key = cache_key(kind);
    let stamp = file_stamp(path).unwrap_or_default();
    let mut conn = redis.clone();
    if !force
        && let Ok(Some(raw)) = conn.get::<_, Option<String>>(&key).await
        && let Ok(cached) = serde_json::from_str::<CachedProbe>(&raw)
        && cached.stamp == stamp
        && cached.path == path.display().to_string()
    {
        return cached.result.map_err(LocalDbError::Query);
    }
    let result = probe(kind, path).await;
    let cached = CachedProbe {
        stamp,
        path: path.display().to_string(),
        result: result
            .as_ref()
            .map(Clone::clone)
            .map_err(ToString::to_string),
    };
    if let Ok(raw) = serde_json::to_string(&cached) {
        let _: Result<(), _> = conn.set_ex(&key, raw, PROBE_CACHE_TTL_SECS).await;
    }
    result
}

/// Drop the cached probe (after *Test*, or when the path setting changes).
pub async fn invalidate_probe(redis: &ConnectionManager, kind: LocalDbKind) {
    let mut conn = redis.clone();
    let _: Result<(), _> = conn.del(cache_key(kind)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_round_trips_and_rejects_garbage() {
        for m in LocalDbMode::ALL {
            assert_eq!(m.as_str().parse::<LocalDbMode>().unwrap(), m);
        }
        assert_eq!("".parse::<LocalDbMode>().unwrap(), LocalDbMode::Off);
        assert_eq!(
            " prefer_local ".parse::<LocalDbMode>().unwrap(),
            LocalDbMode::PreferLocal
        );
        assert!("local".parse::<LocalDbMode>().is_err());
        assert!(!LocalDbMode::Off.uses_file());
        assert!(LocalDbMode::LocalOnly.uses_file());
    }

    #[test]
    fn header_check_rejects_non_sqlite_and_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("dump.sql");
        std::fs::write(&text, b"CREATE TABLE gcd_issue (id INTEGER);").unwrap();
        assert!(matches!(
            check_header(&text),
            Err(LocalDbError::NotSqlite(_))
        ));
        assert!(matches!(
            check_header(&dir.path().join("nope.db")),
            Err(LocalDbError::Missing(_))
        ));
        let short = dir.path().join("short.db");
        std::fs::write(&short, b"SQLite").unwrap();
        assert!(matches!(
            check_header(&short),
            Err(LocalDbError::NotSqlite(_))
        ));
    }
}
