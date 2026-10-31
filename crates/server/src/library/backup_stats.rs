//! Stored `.bak` footprint per library (audit 2026-11, N-L4).
//!
//! The library health page shows how much disk the archive-rewrite safety
//! backups (`<archive>.bak`, `.bak.N`) take. Until this module the card's
//! endpoint walked the whole library root on every request, which on a
//! NAS at 50,000 issues is seconds per page view. The tally now lives in
//! `library_backup_stats`, one row per library:
//!
//! - [`crate::jobs::backup_prune`] stores a fresh tally for **every**
//!   library on its daily walk (it has to walk the writeback libraries to
//!   prune them; the others get a read-only walk for the count);
//! - a library with no row yet is counted on its first request and the
//!   row is stored, so every later request is a single-row read;
//! - a catalogue-changing scan marks the row [`Model::stale`] — every
//!   archive rewrite (page edit, sidecar writeback, CBR/CB7 conversion,
//!   restore) ends in a scoped rescan, so that is the one hook that sees
//!   them all. A stale row is recounted on request only once it is older
//!   than [`RECOUNT_STALE_AFTER`], which bounds the per-request walks to
//!   one per library per ten minutes while a bulk writeback is running.
//!
//! Pure-filesystem helpers ([`tally`], [`Tally`]) have no DB so the prune
//! sweep and the tests share them.

use chrono::{DateTime, Utc};
use entity::library_backup_stats::{self, Entity, Model};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use std::path::Path;
use std::time::{Duration, SystemTime};
use uuid::Uuid;

/// A stale row younger than this is served as-is; older, the request
/// recounts. Bounds the recount cost while rescans keep landing.
pub const RECOUNT_STALE_AFTER: Duration = Duration::from_secs(10 * 60);

/// What one walk found: the backup files that are (still) on disk.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    pub file_count: u64,
    pub total_bytes: u64,
    pub oldest_modified: Option<SystemTime>,
    pub newest_modified: Option<SystemTime>,
}

impl Tally {
    /// Fold one surviving backup file into the tally.
    pub fn add(&mut self, meta: &std::fs::Metadata) {
        self.file_count += 1;
        self.total_bytes += meta.len();
        if let Ok(modified) = meta.modified() {
            self.oldest_modified = Some(self.oldest_modified.map_or(modified, |o| o.min(modified)));
            self.newest_modified = Some(self.newest_modified.map_or(modified, |n| n.max(modified)));
        }
    }
}

/// Visit every `.bak` / `.bak.N` file under `root` (walk errors are
/// logged and skipped). The callback gets the entry and its metadata.
pub fn for_each_backup(root: &Path, mut f: impl FnMut(&walkdir::DirEntry, &std::fs::Metadata)) {
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| match e {
            Ok(e) => Some(e),
            Err(err) => {
                tracing::warn!(root = %root.display(), error = %err, "backup walk error");
                None
            }
        })
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if !crate::archive_rewrite::is_backup_name(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        f(&entry, &meta);
    }
}

/// Count the backup files under `root`. Pure filesystem. `None` when the
/// root itself cannot be read (an unmounted NAS, a wrong path): a zero
/// tally would then overwrite a correct stored count with a fresh
/// timestamp, so callers keep what they have instead.
pub fn tally(root: &Path) -> Option<Tally> {
    if let Err(e) = std::fs::read_dir(root) {
        tracing::warn!(root = %root.display(), error = %e, "backup tally: root unreadable, keeping the stored count");
        return None;
    }
    let mut t = Tally::default();
    for_each_backup(root, |_, meta| t.add(meta));
    Some(t)
}

fn to_fixed(t: SystemTime) -> DateTime<chrono::FixedOffset> {
    DateTime::<Utc>::from(t).fixed_offset()
}

/// Upsert the stored row for `library_id` from a fresh tally, clearing
/// `stale`.
pub async fn store(
    db: &DatabaseConnection,
    library_id: Uuid,
    t: &Tally,
    computed_at: DateTime<Utc>,
) -> Result<Model, sea_orm::DbErr> {
    let am = library_backup_stats::ActiveModel {
        library_id: Set(library_id),
        file_count: Set(i64::try_from(t.file_count).unwrap_or(i64::MAX)),
        total_bytes: Set(i64::try_from(t.total_bytes).unwrap_or(i64::MAX)),
        oldest_modified_at: Set(t.oldest_modified.map(to_fixed)),
        newest_modified_at: Set(t.newest_modified.map(to_fixed)),
        computed_at: Set(computed_at.fixed_offset()),
        stale: Set(false),
    };
    Entity::insert(am)
        .on_conflict(
            OnConflict::column(library_backup_stats::Column::LibraryId)
                .update_columns([
                    library_backup_stats::Column::FileCount,
                    library_backup_stats::Column::TotalBytes,
                    library_backup_stats::Column::OldestModifiedAt,
                    library_backup_stats::Column::NewestModifiedAt,
                    library_backup_stats::Column::ComputedAt,
                    library_backup_stats::Column::Stale,
                ])
                .to_owned(),
        )
        .exec_with_returning(db)
        .await
}

/// Walk `root` off the async runtime and store the result.
pub async fn recount(
    db: &DatabaseConnection,
    library_id: Uuid,
    root: std::path::PathBuf,
) -> anyhow::Result<Model> {
    let t = tokio::task::spawn_blocking(move || tally(&root))
        .await
        .map_err(|e| anyhow::anyhow!("backup tally walk join failed: {e}"))?;
    match t {
        Some(t) => Ok(store(db, library_id, &t, Utc::now()).await?),
        // Unreadable root: serve the stored row untouched, or zeros that are
        // deliberately not stored so the next request tries again.
        None => Ok(load(db, library_id).await?.unwrap_or(Model {
            library_id,
            file_count: 0,
            total_bytes: 0,
            oldest_modified_at: None,
            newest_modified_at: None,
            computed_at: Utc::now().fixed_offset(),
            stale: true,
        })),
    }
}

/// Load the stored row, if any.
pub async fn load(
    db: &DatabaseConnection,
    library_id: Uuid,
) -> Result<Option<Model>, sea_orm::DbErr> {
    Entity::find_by_id(library_id).one(db).await
}

/// Flag the stored tally as possibly out of date. A row that does not
/// exist yet needs no flag (the first request counts anyway). Best-effort:
/// a failure is logged, never propagated, because the callers are scan
/// finalisers.
pub async fn mark_stale(db: &DatabaseConnection, library_id: Uuid) {
    let res = Entity::update_many()
        .col_expr(
            library_backup_stats::Column::Stale,
            sea_orm::sea_query::Expr::value(true),
        )
        .filter(library_backup_stats::Column::LibraryId.eq(library_id))
        .filter(library_backup_stats::Column::Stale.eq(false))
        .exec(db)
        .await;
    if let Err(e) = res {
        tracing::warn!(%library_id, error = %e, "backup stats: mark stale failed");
    }
}

/// Does a stored row need a recount before it is served? Missing → yes;
/// stale and older than [`RECOUNT_STALE_AFTER`] → yes; otherwise no.
pub fn needs_recount(row: Option<&Model>, now: DateTime<Utc>) -> bool {
    match row {
        None => true,
        Some(r) => {
            r.stale
                && now
                    .signed_duration_since(r.computed_at.with_timezone(&Utc))
                    .to_std()
                    .is_ok_and(|age| age >= RECOUNT_STALE_AFTER)
        }
    }
}

/// Serve the stored tally for `library_id`, counting first when
/// [`needs_recount`] says so. `root` is only read on that path.
pub async fn current(
    db: &DatabaseConnection,
    library_id: Uuid,
    root: &Path,
) -> anyhow::Result<Model> {
    match load(db, library_id).await? {
        Some(row) if !needs_recount(Some(&row), Utc::now()) => Ok(row),
        _ => recount(db, library_id, root.to_path_buf()).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(stale: bool, age: Duration) -> Model {
        Model {
            library_id: Uuid::nil(),
            file_count: 1,
            total_bytes: 1,
            oldest_modified_at: None,
            newest_modified_at: None,
            computed_at: (Utc::now() - chrono::Duration::from_std(age).unwrap()).fixed_offset(),
            stale,
        }
    }

    #[test]
    fn recount_only_when_missing_or_old_and_stale() {
        let now = Utc::now();
        assert!(needs_recount(None, now));
        assert!(!needs_recount(
            Some(&row(false, Duration::from_secs(0))),
            now
        ));
        assert!(!needs_recount(
            Some(&row(false, Duration::from_secs(86_400))),
            now
        ));
        assert!(!needs_recount(
            Some(&row(true, Duration::from_secs(60))),
            now
        ));
        assert!(needs_recount(
            Some(&row(true, RECOUNT_STALE_AFTER + Duration::from_secs(1))),
            now
        ));
    }

    #[test]
    fn tally_counts_only_backup_names() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("Series (2020)");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("a.cbz"), b"archive").unwrap();
        std::fs::write(nested.join("a.cbz.bak"), b"12345").unwrap();
        std::fs::write(nested.join("a.cbz.bak.2"), b"123").unwrap();
        std::fs::write(nested.join("notes.bak.txt"), b"x").unwrap();
        let t = tally(dir.path()).expect("readable root");
        assert_eq!(t.file_count, 2);
        assert_eq!(t.total_bytes, 8);
        assert!(t.oldest_modified.is_some() && t.newest_modified.is_some());
        assert_eq!(
            tally(&dir.path().join("missing")),
            None,
            "an unreadable root is not a zero count"
        );
    }
}
