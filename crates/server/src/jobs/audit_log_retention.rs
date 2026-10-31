//! Daily export-and-trim of the `audit_log` table (audit 2026-11 N-A7 /
//! OP-5, remediation plan E1).
//!
//! `audit_log` is append-only at the role level: handlers only ever
//! `INSERT` through [`crate::audit::record`], nothing `UPDATE`s or
//! `DELETE`s a row, and `jobs/hard_purge.rs` merely nullifies references
//! into it. Without a bound the table grows for the life of the
//! deployment. This sweep is the **one sanctioned deleter**, and it is a
//! scheduled job rather than a handler so the append-only rule for the
//! API surface stays intact.
//!
//! Behaviour, driven by the live `audit_log.retention_days` setting
//! ([`crate::config::Config::audit_log_retention_days`]):
//!
//! 1. `0` (the default) — do nothing. Behaviour is unchanged until an
//!    operator opts in.
//! 2. Otherwise compute `cutoff = now - max(days, MIN_RETENTION_DAYS)`.
//!    Rows with `created_at < cutoff` are the candidates. None → done.
//! 3. **Export first.** Stream the candidates, ordered by
//!    `(created_at, id)`, as newline-delimited JSON (one
//!    [`entity::audit_log::Model`] per line) into
//!    `data_path/audit-log-exports/.audit-log-<ts>.ndjson.tmp`, `fsync` the
//!    file, rename it over the `audit-log-<ts>.ndjson` name claimed up front
//!    with `O_EXCL` (so a same-second rerun can never replace an earlier
//!    export), and `fsync` the parent directory. The export directory lives inside the app-data volume so
//!    the weekly `comic_data` backup (`docs/install/backup.md`) carries it.
//! 4. **Then delete,** in bounded batches of [`DELETE_CHUNK`], only the
//!    rows the export covered: `created_at < cutoff` **and**
//!    `(created_at, id) <= (last exported created_at, last exported id)`.
//!    Rows written after the export can never match (a fresh row's
//!    `created_at` is "now", months past the cutoff), but the tuple bound
//!    makes the invariant structural rather than argued.
//!
//! **Never delete without a successful export.** Any failure before the
//! rename — the directory cannot be created, is not writable, the write or
//! `fsync` fails — returns `Err` and the table is untouched; the scheduler
//! wrapper logs it at `error` and tries again the next day. A failure
//! *during* the delete leaves already-deleted rows safely in the export
//! file and the remainder in the table for the next run, which exports
//! them again (into a new file) before deleting — idempotent, never lossy.
//!
//! Counts land on one `tracing::info!` line (`target =
//! "folio::audit_log_retention"`) and the `folio_audit_log_trimmed_total`
//! counter; the export file's name carries the run timestamp.

use crate::state::AppState;
use chrono::{DateTime, Duration, Utc};
use entity::audit_log;
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect, Statement,
};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt as _;
use uuid::Uuid;

/// Smallest non-zero retention the setting accepts. An audit trail shorter
/// than a quarter is not much of an audit trail, and a typo (`9` for `90`)
/// must not trim it to a week.
pub const MIN_RETENTION_DAYS: u32 = 90;

/// Upper bound on the setting (100 years) — only there so an absurd value is
/// a validation error instead of an overflow when the cutoff is computed.
pub const MAX_RETENTION_DAYS: u32 = 36_500;

/// Directory under `data_path` that receives the export files.
pub const EXPORT_DIR: &str = "audit-log-exports";

/// Rows per `SELECT` page while streaming the export, and rows per
/// `DELETE` statement afterwards. Each delete is its own implicit
/// transaction so a long backlog never holds one big lock.
pub const DELETE_CHUNK: u64 = 1_000;

/// Totals for one sweep.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RetentionStats {
    /// Rows written to the export file.
    pub exported: u64,
    /// Rows deleted after the export was durable. Equals `exported` on a
    /// clean run; lower only if a delete batch failed part-way.
    pub deleted: u64,
    /// The export file, when at least one row was exported.
    pub file: Option<PathBuf>,
}

/// Where this deployment's export files land.
pub fn export_dir(data_path: &Path) -> PathBuf {
    data_path.join(EXPORT_DIR)
}

/// Cron entry point: reads the retention from the live config.
pub async fn run(state: &AppState) -> anyhow::Result<RetentionStats> {
    let cfg = state.cfg();
    run_with(
        &state.db,
        &cfg.data_path,
        cfg.audit_log_retention_days,
        Utc::now(),
    )
    .await
}

/// Run one sweep against `db` as of `now`. Split from [`run`] so tests can
/// drive the cutoff and the export directory deterministically.
pub async fn run_with(
    db: &DatabaseConnection,
    data_path: &Path,
    retention_days: u32,
    now: DateTime<Utc>,
) -> anyhow::Result<RetentionStats> {
    let mut stats = RetentionStats::default();
    if retention_days == 0 {
        return Ok(stats);
    }
    let days = retention_days.clamp(MIN_RETENTION_DAYS, MAX_RETENTION_DAYS);
    if days != retention_days {
        // `Config::validate` rejects this at boot and on PATCH; reaching it
        // means a caller bypassed the settings chain.
        tracing::warn!(
            target: "folio::audit_log_retention",
            requested = retention_days,
            effective = days,
            "audit log retention: value outside the allowed range, clamped"
        );
    }
    let cutoff = (now - Duration::days(i64::from(days))).fixed_offset();

    // Cheap existence probe so the daily no-op costs one index lookup, not
    // a count over the whole candidate set the export is about to re-read.
    let any_candidate = audit_log::Entity::find()
        .filter(audit_log::Column::CreatedAt.lt(cutoff))
        .select_only()
        .column(audit_log::Column::Id)
        .into_tuple::<Uuid>()
        .one(db)
        .await?
        .is_some();
    if !any_candidate {
        return Ok(stats);
    }

    // ── 1. Export (any failure here returns before a single delete) ──
    let dir = export_dir(data_path);
    let stamp = now.format("%Y%m%dT%H%M%SZ");
    let final_path = dir.join(format!("audit-log-{stamp}.ndjson"));
    let tmp_path = dir.join(format!(".audit-log-{stamp}.ndjson.tmp"));
    let export = export_rows(db, &dir, &tmp_path, &final_path, cutoff).await;
    let Exported {
        count: exported,
        last,
    } = match export {
        Ok(e) => e,
        Err(e) => {
            // Leave no half-written temp file behind; ignore a failure
            // here since the file may never have been created. The final
            // path is only ever removed while it is still our empty claim
            // — a populated export is never touched.
            let _ = tokio::fs::remove_file(&tmp_path).await;
            if let Ok(meta) = tokio::fs::metadata(&final_path).await
                && meta.len() == 0
            {
                let _ = tokio::fs::remove_file(&final_path).await;
            }
            return Err(e);
        }
    };
    stats.exported = exported;
    if exported == 0 {
        // The probed row vanished before the export read it — not possible
        // for an append-only table, but the only safe answer is to delete
        // nothing (`export_rows` already removed the empty claim).
        return Ok(stats);
    }
    stats.file = Some(final_path.clone());
    let (last_created_at, last_id) = last.expect("exported > 0 implies a last row");

    // ── 2. Delete only what the file holds, in bounded batches ──
    loop {
        let res = db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM audit_log WHERE id IN (\
                    SELECT id FROM audit_log \
                    WHERE created_at < $1 AND (created_at, id) <= ($2, $3) \
                    ORDER BY created_at, id \
                    LIMIT $4\
                 )",
                [
                    cutoff.into(),
                    last_created_at.into(),
                    last_id.into(),
                    i64::try_from(DELETE_CHUNK).unwrap_or(i64::MAX).into(),
                ],
            ))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "audit log retention: delete failed after exporting {exported} rows to \
                     {} ({} already deleted): {e}",
                    final_path.display(),
                    stats.deleted
                )
            })?;
        let n = res.rows_affected();
        stats.deleted += n;
        if n == 0 || stats.deleted >= exported {
            break;
        }
    }

    metrics::counter!("folio_audit_log_trimmed_total").increment(stats.deleted);
    tracing::info!(
        target: "folio::audit_log_retention",
        exported = stats.exported,
        deleted = stats.deleted,
        retention_days = days,
        cutoff = %cutoff.to_rfc3339(),
        file = %final_path.display(),
        "audit log retention: exported and trimmed rows older than the cutoff",
    );
    Ok(stats)
}

struct Exported {
    count: u64,
    /// `(created_at, id)` of the last row written — the delete's upper
    /// bound.
    last: Option<(DateTime<chrono::FixedOffset>, Uuid)>,
}

/// Stream every row with `created_at < cutoff` into `tmp_path` as NDJSON,
/// fsync it, rename it to `final_path`, fsync the directory. Returns the
/// row count and the last `(created_at, id)` written.
async fn export_rows(
    db: &DatabaseConnection,
    dir: &Path,
    tmp_path: &Path,
    final_path: &Path,
    cutoff: DateTime<chrono::FixedOffset>,
) -> anyhow::Result<Exported> {
    tokio::fs::create_dir_all(dir).await.map_err(|e| {
        anyhow::anyhow!(
            "audit log retention: export directory {} is not usable: {e}",
            dir.display()
        )
    })?;
    // Claim the final name first with O_EXCL. The stamp is second-resolution,
    // so two runs sharing a `now` (two replicas, a manual re-run) must fail
    // here rather than let the later rename replace an export whose rows
    // are already gone from the table.
    tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(final_path)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "audit log retention: export directory {} is not writable or {} already exists: {e}",
                dir.display(),
                final_path.display()
            )
        })?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(tmp_path)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "audit log retention: export directory {} is not writable: {e}",
                dir.display()
            )
        })?;

    let mut count = 0u64;
    let mut last: Option<(DateTime<chrono::FixedOffset>, Uuid)> = None;
    loop {
        let mut q = audit_log::Entity::find().filter(audit_log::Column::CreatedAt.lt(cutoff));
        if let Some((ca, id)) = last {
            // Keyset: strictly after the last row in (created_at, id) order.
            q = q.filter(
                Condition::any()
                    .add(audit_log::Column::CreatedAt.gt(ca))
                    .add(
                        Condition::all()
                            .add(audit_log::Column::CreatedAt.eq(ca))
                            .add(audit_log::Column::Id.gt(id)),
                    ),
            );
        }
        let page = q
            .order_by_asc(audit_log::Column::CreatedAt)
            .order_by_asc(audit_log::Column::Id)
            .limit(DELETE_CHUNK)
            .all(db)
            .await?;
        if page.is_empty() {
            break;
        }
        let mut buf = Vec::with_capacity(page.len() * 256);
        for row in &page {
            serde_json::to_writer(&mut buf, row)?;
            buf.push(b'\n');
        }
        file.write_all(&buf).await.map_err(|e| {
            anyhow::anyhow!(
                "audit log retention: writing {} failed: {e}",
                tmp_path.display()
            )
        })?;
        count += page.len() as u64;
        let tail = page.last().expect("non-empty page");
        last = Some((tail.created_at, tail.id));
        if (page.len() as u64) < DELETE_CHUNK {
            break;
        }
    }

    file.flush().await?;
    file.sync_all().await.map_err(|e| {
        anyhow::anyhow!(
            "audit log retention: fsync of {} failed: {e}",
            tmp_path.display()
        )
    })?;
    drop(file);
    if count == 0 {
        let _ = tokio::fs::remove_file(tmp_path).await;
        let _ = tokio::fs::remove_file(final_path).await;
        return Ok(Exported { count, last });
    }
    tokio::fs::rename(tmp_path, final_path).await.map_err(|e| {
        anyhow::anyhow!(
            "audit log retention: renaming the export into place at {} failed: {e}",
            final_path.display()
        )
    })?;
    // Make the rename durable too: fsync the directory entry.
    let dir_owned = dir.to_path_buf();
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        std::fs::File::open(&dir_owned)?.sync_all()
    })
    .await
    .map_err(|e| anyhow::anyhow!("audit log retention: directory fsync join failed: {e}"))?
    .map_err(|e| {
        anyhow::anyhow!(
            "audit log retention: fsync of {} failed: {e}",
            dir.display()
        )
    })?;
    Ok(Exported { count, last })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_dir_is_under_data_path() {
        assert_eq!(
            export_dir(Path::new("/data")),
            PathBuf::from("/data/audit-log-exports")
        );
    }

    #[test]
    fn cutoff_arithmetic_holds_at_the_ceiling() {
        const { assert!(MAX_RETENTION_DAYS > MIN_RETENTION_DAYS) };
        let now = Utc::now();
        let cutoff = now - Duration::days(i64::from(MAX_RETENTION_DAYS));
        assert!(cutoff < now);
    }
}
