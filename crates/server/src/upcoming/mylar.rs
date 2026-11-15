//! The Mylar window: a snapshot **pushed to** Folio by a script on the
//! operator's Mylar host (`integrations/mylar/folio-mylar-push.py`).
//!
//! **View-only invariant.** This module only receives. It has no HTTP
//! client, and Folio has no setting that holds a Mylar address or key:
//! Mylar's API key is all-or-nothing, and many installs run Mylar with
//! its login off behind a firewall. Do not add an outbound call here.
//!
//! What it owns:
//!
//! - the snapshot's wire shape ([`MylarSnapshot`], schema version
//!   [`SCHEMA_VERSION`]) and its limits, mirrored by the script;
//! - the ingest token ([`issue_token`] / [`verify_token`]): 256 random
//!   bits, stored as a SHA-256 digest, accepted by one endpoint;
//! - [`store`], which replaces the previous snapshot in one transaction
//!   (an unchanged snapshot is a heartbeat and only moves the clocks);
//! - [`freshness`], judged by Folio's own receive time.

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use constant_time_eq::constant_time_eq;
use entity::{integration_token, mylar_pull, mylar_snapshot, mylar_wanted, mylar_watch};
use rand::Rng;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, IntoActiveModel,
    QueryFilter, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use uuid::Uuid;

/// The snapshot format this server accepts.
pub const SCHEMA_VERSION: u32 = 1;
/// Every ingest token starts with this. Deliberately not `app_`: an app
/// password is a user credential and is handled by other code paths.
pub const TOKEN_PREFIX: &str = "fmi_";
const TOKEN_BYTES: usize = 32;

/// Array caps. The script applies the same ones, so a legitimate
/// snapshot never trips them.
pub const MAX_PULL: usize = 2000;
pub const MAX_WATCH: usize = 50_000;
pub const MAX_WANTED: usize = 2000;
pub const MAX_HISTORY: usize = 500;
pub const MAX_QUEUE_ITEMS: usize = 200;
pub const MAX_QUEUE_STATUSES: usize = 24;
pub const MAX_JOBS: usize = 50;
const MAX_NAME: usize = 300;
const MAX_ISSUE: usize = 40;
const MAX_STATUS: usize = 24;
const MAX_PUBLISHER: usize = 120;

/// A snapshot older than this is no longer shown at all.
pub const EXPIRE_AFTER_DAYS: i64 = 7;

/// Gauge: Unix time Folio last accepted a Mylar snapshot.
pub const LAST_SNAPSHOT_GAUGE: &str = "folio_mylar_snapshot_received_timestamp_seconds";

// ───────── wire shape ─────────

fn supported_version(v: &u32, (): &()) -> garde::Result {
    if *v == SCHEMA_VERSION {
        Ok(())
    } else {
        Err(garde::Error::new(format!(
            "snapshot schema version {v} is not supported; this server accepts version \
             {SCHEMA_VERSION}. Use the push script that ships with this Folio version"
        )))
    }
}

fn positive_id(v: &Option<i64>, (): &()) -> garde::Result {
    match v {
        Some(n) if *n <= 0 => Err(garde::Error::new("must be a positive id")),
        _ => Ok(()),
    }
}

fn watch_entries(v: &[WatchEntry], (): &()) -> garde::Result {
    for WatchEntry(id, status, _) in v {
        if *id <= 0 {
            return Err(garde::Error::new("series ids must be positive"));
        }
        if status.is_empty() || status.chars().count() > MAX_STATUS {
            return Err(garde::Error::new(format!(
                "a status must be 1 to {MAX_STATUS} characters"
            )));
        }
    }
    Ok(())
}

fn queue_counts(v: &BTreeMap<String, i64>, (): &()) -> garde::Result {
    if v.len() > MAX_QUEUE_STATUSES {
        return Err(garde::Error::new(format!(
            "at most {MAX_QUEUE_STATUSES} statuses"
        )));
    }
    if v.iter()
        .any(|(k, n)| k.chars().count() > MAX_STATUS || *n < 0)
    {
        return Err(garde::Error::new(
            "a status is too long or a count is negative",
        ));
    }
    Ok(())
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, garde::Validate, utoipa::ToSchema,
)]
#[schema(as = MylarSnapshotMeta)]
pub struct SnapshotMeta {
    #[garde(custom(supported_version))]
    pub schema_version: u32,
    /// When the script read `mylar.db`. Informational: staleness is
    /// judged by when Folio received the snapshot.
    #[garde(skip)]
    #[schema(value_type = String, format = DateTime)]
    pub generated_at: DateTime<FixedOffset>,
    #[garde(skip)]
    #[serde(default)]
    pub mylar_db_version: Option<i32>,
    /// The operator said Mylar's annual integration is on.
    #[garde(skip)]
    #[serde(default)]
    pub annuals_included: bool,
}

/// One line of Mylar's weekly pull list.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, garde::Validate, utoipa::ToSchema,
)]
#[schema(as = MylarPullRow)]
pub struct PullRow {
    #[garde(skip)]
    #[schema(value_type = String, format = Date)]
    pub ship_date: NaiveDate,
    #[garde(inner(length(chars, max = MAX_PUBLISHER)))]
    #[serde(default)]
    pub publisher: Option<String>,
    #[garde(length(chars, min = 1, max = MAX_NAME))]
    pub series: String,
    #[garde(length(chars, min = 1, max = MAX_ISSUE))]
    pub issue: String,
    #[garde(length(chars, min = 1, max = MAX_STATUS))]
    pub status: String,
    /// ComicVine volume id; null when Mylar's pull service found none.
    #[garde(custom(positive_id))]
    #[serde(default)]
    pub cv_series_id: Option<i64>,
    #[garde(custom(positive_id))]
    #[serde(default)]
    pub cv_issue_id: Option<i64>,
    #[garde(skip)]
    #[serde(default)]
    pub series_year: Option<i32>,
}

/// `[ComicVine volume id, status, still publishing]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchEntry(pub i64, pub String, pub bool);

/// An issue Mylar wants, has snatched, or failed to get.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, garde::Validate, utoipa::ToSchema,
)]
#[schema(as = MylarWantedRow)]
pub struct WantedRow {
    #[garde(custom(positive_id))]
    #[serde(default)]
    pub cv_series_id: Option<i64>,
    #[garde(custom(positive_id))]
    #[serde(default)]
    pub cv_issue_id: Option<i64>,
    #[garde(length(chars, min = 1, max = MAX_NAME))]
    pub series: String,
    #[garde(length(chars, min = 1, max = MAX_ISSUE))]
    pub issue: String,
    #[garde(skip)]
    #[serde(default)]
    #[schema(value_type = Option<String>, format = Date)]
    pub store_date: Option<NaiveDate>,
    #[garde(length(chars, min = 1, max = MAX_STATUS))]
    pub status: String,
    #[garde(skip)]
    #[serde(default)]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub added_at: Option<DateTime<FixedOffset>>,
}

/// A recent snatch or post-process event.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, garde::Validate, utoipa::ToSchema,
)]
#[schema(as = MylarHistoryRow)]
pub struct HistoryRow {
    #[garde(custom(positive_id))]
    #[serde(default)]
    pub cv_series_id: Option<i64>,
    #[garde(custom(positive_id))]
    #[serde(default)]
    pub cv_issue_id: Option<i64>,
    #[garde(length(chars, min = 1, max = MAX_NAME))]
    pub series: String,
    #[garde(inner(length(chars, max = MAX_ISSUE)))]
    #[serde(default)]
    pub issue: Option<String>,
    #[garde(length(chars, min = 1, max = MAX_STATUS))]
    pub status: String,
    #[garde(skip)]
    #[serde(default)]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub at: Option<DateTime<FixedOffset>>,
}

/// One entry of Mylar's direct-download queue. No link and no file name:
/// the script never reads them.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, garde::Validate, utoipa::ToSchema,
)]
#[schema(as = MylarQueueItem)]
pub struct QueueItem {
    #[garde(length(chars, min = 1, max = MAX_NAME))]
    pub title: String,
    #[garde(inner(length(chars, max = 12)))]
    #[serde(default)]
    pub year: Option<String>,
    #[garde(inner(length(chars, max = 24)))]
    #[serde(default)]
    pub size: Option<String>,
    #[garde(length(chars, min = 1, max = MAX_STATUS))]
    pub status: String,
    #[garde(skip)]
    #[serde(default)]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub updated_at: Option<DateTime<FixedOffset>>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, garde::Validate, utoipa::ToSchema,
)]
#[schema(as = MylarQueueBlock)]
pub struct QueueBlock {
    /// Entries per status, over the whole queue (not only `items`).
    #[garde(custom(queue_counts))]
    #[serde(default)]
    pub counts: BTreeMap<String, i64>,
    #[garde(length(max = MAX_QUEUE_ITEMS), dive)]
    #[serde(default)]
    pub items: Vec<QueueItem>,
}

/// One of Mylar's scheduled jobs.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, garde::Validate, utoipa::ToSchema,
)]
#[schema(as = MylarJobRow)]
pub struct JobRow {
    #[garde(length(chars, min = 1, max = 80))]
    pub name: String,
    #[garde(inner(length(chars, max = MAX_STATUS)))]
    #[serde(default)]
    pub status: Option<String>,
    #[garde(skip)]
    #[serde(default)]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub prev_run: Option<DateTime<FixedOffset>>,
    #[garde(skip)]
    #[serde(default)]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub next_run: Option<DateTime<FixedOffset>>,
}

/// The body of `POST /integrations/mylar/snapshot`. Unknown fields are
/// ignored, so a newer script can add to a version without breaking an
/// older server.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, garde::Validate, utoipa::ToSchema,
)]
pub struct MylarSnapshot {
    #[garde(dive)]
    pub meta: SnapshotMeta,
    #[garde(length(max = MAX_PULL), dive)]
    #[serde(default)]
    pub pull_list: Vec<PullRow>,
    /// Rows of `[ComicVine volume id, status, still publishing]`.
    #[garde(length(max = MAX_WATCH), custom(watch_entries))]
    #[serde(default)]
    #[schema(value_type = Vec<Object>)]
    pub watchlist: Vec<WatchEntry>,
    #[garde(length(max = MAX_WANTED), dive)]
    #[serde(default)]
    pub wanted: Vec<WantedRow>,
    #[garde(length(max = MAX_HISTORY), dive)]
    #[serde(default)]
    pub history: Vec<HistoryRow>,
    #[garde(dive)]
    #[serde(default)]
    pub queue: QueueBlock,
    #[garde(length(max = MAX_JOBS), dive)]
    #[serde(default)]
    pub jobs: Vec<JobRow>,
}

impl MylarSnapshot {
    /// SHA-256 of the snapshot with its generation time blanked: equal
    /// for two snapshots that differ only in when they were made.
    pub fn content_hash(&self) -> String {
        let mut canonical = self.clone();
        canonical.meta.generated_at = DateTime::UNIX_EPOCH.fixed_offset();
        let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
        hex(&Sha256::digest(&bytes))
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

// ───────── ingest token ─────────

fn token_digest(plaintext: &str) -> [u8; 32] {
    Sha256::digest(plaintext.as_bytes()).into()
}

fn new_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    let body = data_encoding::BASE32_NOPAD
        .encode(&bytes)
        .to_ascii_lowercase();
    format!("{TOKEN_PREFIX}{body}")
}

/// Create the Mylar ingest token, replacing any earlier one (the old
/// token stops working at once). Returns the plaintext, which is shown
/// to the admin once and never stored.
pub async fn issue_token(db: &DatabaseConnection, actor: Uuid) -> Result<String, DbErr> {
    let plaintext = new_token();
    let row = integration_token::ActiveModel {
        name: Set(integration_token::MYLAR.to_owned()),
        digest: Set(token_digest(&plaintext).to_vec()),
        created_by: Set(Some(actor)),
        created_at: Set(Utc::now().fixed_offset()),
        last_used_at: Set(None),
        last_used_ip: Set(None),
    };
    integration_token::Entity::insert(row)
        .on_conflict(
            OnConflict::column(integration_token::Column::Name)
                .update_columns([
                    integration_token::Column::Digest,
                    integration_token::Column::CreatedBy,
                    integration_token::Column::CreatedAt,
                    integration_token::Column::LastUsedAt,
                    integration_token::Column::LastUsedIp,
                ])
                .to_owned(),
        )
        .exec(db)
        .await?;
    Ok(plaintext)
}

pub async fn token_row(db: &DatabaseConnection) -> Result<Option<integration_token::Model>, DbErr> {
    integration_token::Entity::find_by_id(integration_token::MYLAR.to_owned())
        .one(db)
        .await
}

/// True when `presented` is the current ingest token. The digests are
/// compared in constant time; a token without the prefix is refused
/// before any database read.
pub async fn verify_token(db: &DatabaseConnection, presented: &str) -> Result<bool, DbErr> {
    if !presented.starts_with(TOKEN_PREFIX) {
        return Ok(false);
    }
    let Some(row) = token_row(db).await? else {
        return Ok(false);
    };
    Ok(constant_time_eq(&row.digest, &token_digest(presented)))
}

/// Delete the ingest token. Returns whether there was one.
pub async fn revoke_token(db: &DatabaseConnection) -> Result<bool, DbErr> {
    let res = integration_token::Entity::delete_by_id(integration_token::MYLAR.to_owned())
        .exec(db)
        .await?;
    Ok(res.rows_affected > 0)
}

async fn stamp_token_use(db: &DatabaseConnection, ip: Option<String>, now: DateTime<FixedOffset>) {
    let res = integration_token::Entity::update_many()
        .col_expr(
            integration_token::Column::LastUsedAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .col_expr(
            integration_token::Column::LastUsedIp,
            sea_orm::sea_query::Expr::value(ip),
        )
        .filter(integration_token::Column::Name.eq(integration_token::MYLAR))
        .exec(db)
        .await;
    if let Err(e) = res {
        tracing::warn!(error = %e, "mylar ingest: could not stamp the token's last use");
    }
}

// ───────── storing a snapshot ─────────

/// What [`store`] did with a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stored {
    /// The content differed: the previous snapshot was replaced.
    Replaced,
    /// Same content as the stored one: only the timestamps moved.
    Heartbeat,
}

/// Keep `snapshot` as the latest, received at `received_at` from `ip`.
/// Everything happens in one transaction, so a reader never sees half of
/// an old snapshot and half of a new one.
pub async fn store(
    db: &DatabaseConnection,
    snapshot: &MylarSnapshot,
    received_at: DateTime<FixedOffset>,
    ip: Option<String>,
) -> Result<Stored, DbErr> {
    let hash = snapshot.content_hash();
    let txn = db.begin().await?;
    let existing = mylar_snapshot::Entity::find_by_id(mylar_snapshot::SINGLETON_ID)
        .one(&txn)
        .await?;
    if let Some(row) = existing.as_ref().filter(|r| r.content_hash == hash) {
        let mut active = row.clone().into_active_model();
        active.received_at = Set(received_at);
        active.generated_at = Set(snapshot.meta.generated_at);
        active.update(&txn).await?;
        txn.commit().await?;
        stamp_token_use(db, ip, received_at).await;
        return Ok(Stored::Heartbeat);
    }

    mylar_watch::Entity::delete_many().exec(&txn).await?;
    mylar_pull::Entity::delete_many().exec(&txn).await?;
    mylar_wanted::Entity::delete_many().exec(&txn).await?;

    // The id is the primary key: keep the first of any repeat.
    let mut seen: HashSet<i64> = HashSet::with_capacity(snapshot.watchlist.len());
    let watch: Vec<mylar_watch::ActiveModel> = snapshot
        .watchlist
        .iter()
        .filter(|w| seen.insert(w.0))
        .map(
            |WatchEntry(id, status, continuing)| mylar_watch::ActiveModel {
                cv_series_id: Set(*id),
                status: Set(status.clone()),
                continuing: Set(*continuing),
            },
        )
        .collect();
    let watch_count = watch.len();
    // Three bind parameters a row; stay far below Postgres's 65,535.
    for chunk in watch.chunks(5000) {
        mylar_watch::Entity::insert_many(chunk.to_vec())
            .exec(&txn)
            .await?;
    }
    if !snapshot.pull_list.is_empty() {
        let rows = snapshot.pull_list.iter().map(|p| mylar_pull::ActiveModel {
            ship_date: Set(p.ship_date),
            publisher: Set(p.publisher.clone()),
            series_name: Set(p.series.clone()),
            issue_number: Set(p.issue.clone()),
            status: Set(p.status.clone()),
            cv_series_id: Set(p.cv_series_id),
            cv_issue_id: Set(p.cv_issue_id),
            series_year: Set(p.series_year),
            ..Default::default()
        });
        mylar_pull::Entity::insert_many(rows).exec(&txn).await?;
    }
    if !snapshot.wanted.is_empty() {
        let rows = snapshot.wanted.iter().map(|w| mylar_wanted::ActiveModel {
            cv_series_id: Set(w.cv_series_id),
            cv_issue_id: Set(w.cv_issue_id),
            series_name: Set(w.series.clone()),
            issue_number: Set(w.issue.clone()),
            store_date: Set(w.store_date),
            status: Set(w.status.clone()),
            added_at: Set(w.added_at),
            ..Default::default()
        });
        mylar_wanted::Entity::insert_many(rows).exec(&txn).await?;
    }

    let to_json = |v: serde_json::Result<serde_json::Value>| v.unwrap_or(serde_json::Value::Null);
    let header = mylar_snapshot::ActiveModel {
        id: Set(mylar_snapshot::SINGLETON_ID),
        received_at: Set(received_at),
        generated_at: Set(snapshot.meta.generated_at),
        schema_version: Set(i32::try_from(snapshot.meta.schema_version).unwrap_or(i32::MAX)),
        content_hash: Set(hash),
        mylar_db_version: Set(snapshot.meta.mylar_db_version),
        annuals_included: Set(snapshot.meta.annuals_included),
        pull_count: Set(i32::try_from(snapshot.pull_list.len()).unwrap_or(i32::MAX)),
        watch_count: Set(i32::try_from(watch_count).unwrap_or(i32::MAX)),
        wanted_count: Set(i32::try_from(snapshot.wanted.len()).unwrap_or(i32::MAX)),
        queue: Set(to_json(serde_json::to_value(&snapshot.queue))),
        jobs: Set(to_json(serde_json::to_value(&snapshot.jobs))),
        history: Set(to_json(serde_json::to_value(&snapshot.history))),
    };
    if existing.is_some() {
        header.update(&txn).await?;
    } else {
        header.insert(&txn).await?;
    }
    txn.commit().await?;
    stamp_token_use(db, ip, received_at).await;
    Ok(Stored::Replaced)
}

/// The stored wanted list: soonest in-store date first, undated last,
/// then by title. Bounded by [`MAX_WANTED`].
pub async fn wanted_rows(db: &DatabaseConnection) -> Result<Vec<WantedRow>, DbErr> {
    let mut rows: Vec<WantedRow> = mylar_wanted::Entity::find()
        .all(db)
        .await?
        .into_iter()
        .map(|m| WantedRow {
            cv_series_id: m.cv_series_id,
            cv_issue_id: m.cv_issue_id,
            series: m.series_name,
            issue: m.issue_number,
            store_date: m.store_date,
            status: m.status,
            added_at: m.added_at,
        })
        .collect();
    rows.sort_by(|a, b| {
        (a.store_date.is_none(), a.store_date, &a.series, &a.issue).cmp(&(
            b.store_date.is_none(),
            b.store_date,
            &b.series,
            &b.issue,
        ))
    });
    Ok(rows)
}

pub async fn snapshot_row(db: &DatabaseConnection) -> Result<Option<mylar_snapshot::Model>, DbErr> {
    mylar_snapshot::Entity::find_by_id(mylar_snapshot::SINGLETON_ID)
        .one(db)
        .await
}

/// Delete the stored snapshot (an admin chose to when revoking the
/// token). The token row is not touched.
pub async fn purge(db: &DatabaseConnection) -> Result<(), DbErr> {
    let txn = db.begin().await?;
    mylar_watch::Entity::delete_many().exec(&txn).await?;
    mylar_pull::Entity::delete_many().exec(&txn).await?;
    mylar_wanted::Entity::delete_many().exec(&txn).await?;
    mylar_snapshot::Entity::delete_many().exec(&txn).await?;
    txn.commit().await
}

// ───────── freshness ─────────

/// How far a snapshot can be trusted, by when Folio received it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = MylarFreshness)]
pub enum Freshness {
    /// Within `integrations.mylar.stale_after_minutes`.
    Fresh,
    /// Older than that: shown with its time, and admins are warned.
    Stale,
    /// Older than [`EXPIRE_AFTER_DAYS`]: no longer shown to readers.
    Expired,
}

pub fn freshness(
    received_at: DateTime<FixedOffset>,
    now: DateTime<FixedOffset>,
    stale_after_minutes: u32,
) -> Freshness {
    let age = now - received_at;
    if age > Duration::days(EXPIRE_AFTER_DAYS) {
        Freshness::Expired
    } else if age > Duration::minutes(i64::from(stale_after_minutes)) {
        Freshness::Stale
    } else {
        Freshness::Fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use garde::Validate;

    fn at(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    fn sample() -> MylarSnapshot {
        serde_json::from_value(serde_json::json!({
            "meta": {"schema_version": 1, "generated_at": "2026-11-15T03:36:49+00:00",
                     "mylar_db_version": 1, "annuals_included": false},
            "pull_list": [{"ship_date": "2026-11-11", "publisher": "DC Comics",
                           "series": "Action Comics", "issue": "1104", "status": "Downloaded",
                           "cv_series_id": 91078, "cv_issue_id": 1198752, "series_year": 2016}],
            "watchlist": [[91078, "Active", true], [4050, "Paused", false]],
            "wanted": [],
            "history": [],
            "queue": {"counts": {"Queued": 1}, "items": []},
            "jobs": [],
            "a_field_from_a_newer_script": 1
        }))
        .unwrap()
    }

    #[test]
    fn a_script_snapshot_parses_and_validates() {
        let s = sample();
        assert!(s.validate().is_ok());
        assert_eq!(s.watchlist[0], WatchEntry(91078, "Active".into(), true));
    }

    #[test]
    fn the_hash_ignores_only_the_generation_time() {
        let a = sample();
        let mut b = a.clone();
        b.meta.generated_at = at("2030-01-01T00:00:00Z");
        assert_eq!(a.content_hash(), b.content_hash());
        b.pull_list[0].status = "Wanted".into();
        assert_ne!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn an_unsupported_version_names_the_supported_one() {
        let mut s = sample();
        s.meta.schema_version = 2;
        let report = s.validate().unwrap_err().to_string();
        assert!(report.contains("version 2 is not supported"), "{report}");
        assert!(report.contains("accepts version 1"), "{report}");
    }

    #[test]
    fn caps_and_ids_are_enforced() {
        let mut s = sample();
        s.watchlist = vec![WatchEntry(0, "Active".into(), true)];
        assert!(s.validate().is_err());

        let mut s = sample();
        s.pull_list[0].cv_series_id = Some(-4);
        assert!(s.validate().is_err());

        let mut s = sample();
        s.pull_list[0].series = "x".repeat(MAX_NAME + 1);
        assert!(s.validate().is_err());

        let mut s = sample();
        s.jobs = (0..=MAX_JOBS)
            .map(|i| JobRow {
                name: format!("job {i}"),
                status: None,
                prev_run: None,
                next_run: None,
            })
            .collect();
        assert!(s.validate().is_err());
    }

    #[test]
    fn tokens_are_prefixed_and_distinct() {
        let (a, b) = (new_token(), new_token());
        assert!(a.starts_with(TOKEN_PREFIX));
        assert_eq!(a.len(), TOKEN_PREFIX.len() + 52);
        assert_ne!(a, b);
        assert_ne!(token_digest(&a), token_digest(&b));
    }

    #[test]
    fn freshness_follows_the_receive_time() {
        let received = at("2026-11-14T12:00:00Z");
        assert_eq!(
            freshness(received, at("2026-11-14T14:59:00Z"), 180),
            Freshness::Fresh
        );
        assert_eq!(
            freshness(received, at("2026-11-14T15:01:00Z"), 180),
            Freshness::Stale
        );
        assert_eq!(
            freshness(received, at("2026-11-21T12:01:00Z"), 180),
            Freshness::Expired
        );
    }
}
