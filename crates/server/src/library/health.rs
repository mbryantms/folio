//! Structured library-health diagnostics (spec §10).
//!
//! Library Scanner v1, Milestone 5.
//!
//! The scanner emits typed [`IssueKind`]s through a [`HealthCollector`]; the
//! collector buffers them and writes them to `library_health_issues` in a
//! single batch at scan end. After the batch upsert, any open issue not
//! re-emitted this run is auto-resolved (spec §10.2 "Issues persist across
//! scans until they're no longer detected — auto-resolved — or the user
//! manually dismisses them").
//!
//! Storage shape is opaque (jsonb payload) so adding a new variant doesn't
//! require a migration.

use chrono::{DateTime, FixedOffset, Utc};
use entity::library_health_issue;
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::library::events::{Broadcaster, ScanEvent};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum IssueKind {
    FileAtRoot {
        path: PathBuf,
    },
    EmptyFolder {
        path: PathBuf,
    },
    UnreadableFile {
        path: PathBuf,
        error: String,
    },
    UnreadableArchive {
        path: PathBuf,
        error: String,
    },
    MissingComicInfo {
        path: PathBuf,
    },
    MalformedComicInfo {
        path: PathBuf,
        error: String,
    },
    FolderNameMismatch {
        folder: String,
        comic_info_series: String,
    },
    MixedSeriesInFolder {
        folder: PathBuf,
        series_values: Vec<String>,
    },
    AmbiguousVolume {
        path: PathBuf,
        parsed: String,
    },
    DuplicateContent {
        path_a: PathBuf,
        path_b: PathBuf,
    },
    OrphanedSeriesJson {
        folder: PathBuf,
    },
    UnsupportedArchiveFormat {
        path: PathBuf,
        ext: String,
    },
}

impl IssueKind {
    /// Stable string identifier — also used as the `kind` discriminant in the
    /// DB row, so renames cause silent duplication. Treat as a wire format.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::FileAtRoot { .. } => "FileAtRoot",
            Self::EmptyFolder { .. } => "EmptyFolder",
            Self::UnreadableFile { .. } => "UnreadableFile",
            Self::UnreadableArchive { .. } => "UnreadableArchive",
            Self::MissingComicInfo { .. } => "MissingComicInfo",
            Self::MalformedComicInfo { .. } => "MalformedComicInfo",
            Self::FolderNameMismatch { .. } => "FolderNameMismatch",
            Self::MixedSeriesInFolder { .. } => "MixedSeriesInFolder",
            Self::AmbiguousVolume { .. } => "AmbiguousVolume",
            Self::DuplicateContent { .. } => "DuplicateContent",
            Self::OrphanedSeriesJson { .. } => "OrphanedSeriesJson",
            Self::UnsupportedArchiveFormat { .. } => "UnsupportedArchiveFormat",
        }
    }

    pub fn severity(&self) -> Severity {
        match self {
            Self::UnreadableFile { .. }
            | Self::UnreadableArchive { .. }
            | Self::MalformedComicInfo { .. } => Severity::Error,

            Self::FileAtRoot { .. }
            | Self::EmptyFolder { .. }
            | Self::FolderNameMismatch { .. }
            | Self::MixedSeriesInFolder { .. }
            | Self::AmbiguousVolume { .. }
            | Self::DuplicateContent { .. }
            | Self::OrphanedSeriesJson { .. }
            | Self::UnsupportedArchiveFormat { .. } => Severity::Warning,

            Self::MissingComicInfo { .. } => Severity::Info,
        }
    }

    /// Stable identifier for upsert. Two re-emissions of the "same" issue
    /// across scans must hash to the same value so the row is updated, not
    /// duplicated.
    pub fn fingerprint(&self) -> String {
        let key = match self {
            Self::FileAtRoot { path } => format!("FileAtRoot:{}", path.display()),
            Self::EmptyFolder { path } => format!("EmptyFolder:{}", path.display()),
            Self::UnreadableFile { path, .. } => format!("UnreadableFile:{}", path.display()),
            Self::UnreadableArchive { path, .. } => format!("UnreadableArchive:{}", path.display()),
            Self::MissingComicInfo { path } => format!("MissingComicInfo:{}", path.display()),
            Self::MalformedComicInfo { path, .. } => {
                format!("MalformedComicInfo:{}", path.display())
            }
            Self::FolderNameMismatch { folder, .. } => format!("FolderNameMismatch:{folder}"),
            Self::MixedSeriesInFolder { folder, .. } => {
                format!("MixedSeriesInFolder:{}", folder.display())
            }
            Self::AmbiguousVolume { path, .. } => format!("AmbiguousVolume:{}", path.display()),
            Self::DuplicateContent { path_a, path_b } => {
                // Order-stable: sort the two paths so emitting (a,b) and (b,a)
                // produces the same fingerprint.
                let (a, b) = if path_a <= path_b {
                    (path_a, path_b)
                } else {
                    (path_b, path_a)
                };
                format!("DuplicateContent:{}|{}", a.display(), b.display())
            }
            Self::OrphanedSeriesJson { folder } => {
                format!("OrphanedSeriesJson:{}", folder.display())
            }
            Self::UnsupportedArchiveFormat { path, ext } => {
                format!("UnsupportedArchiveFormat:{}:{ext}", path.display())
            }
        };
        // Hash the key so the column stays a fixed length and doesn't leak
        // the full path through the unique index.
        let mut h = blake3::Hasher::new();
        h.update(key.as_bytes());
        h.finalize().to_hex()[..32].to_string()
    }

    pub fn payload(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    fn event_path(&self) -> Option<String> {
        match self {
            Self::FileAtRoot { path }
            | Self::EmptyFolder { path }
            | Self::UnreadableFile { path, .. }
            | Self::UnreadableArchive { path, .. }
            | Self::MissingComicInfo { path }
            | Self::MalformedComicInfo { path, .. }
            | Self::AmbiguousVolume { path, .. }
            | Self::UnsupportedArchiveFormat { path, .. } => {
                Some(path.to_string_lossy().into_owned())
            }
            Self::FolderNameMismatch { folder, .. } => Some(folder.clone()),
            Self::MixedSeriesInFolder { folder, .. } | Self::OrphanedSeriesJson { folder } => {
                Some(folder.to_string_lossy().into_owned())
            }
            Self::DuplicateContent { path_a, path_b } => {
                Some(format!("{} | {}", path_a.display(), path_b.display()))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

/// Pull out every path-like value from an `IssueKind` payload (after JSON
/// round-trip). Variants don't share a single field name — `path`, `path_a`,
/// `path_b`, and `folder` all show up across the enum — so we look at all of
/// them. Used by the touch-on-skip logic in `HealthCollector::finalize`.
fn paths_in_payload(payload: &serde_json::Value) -> Vec<String> {
    let Some(data) = payload.get("data").and_then(|d| d.as_object()) else {
        return Vec::new();
    };
    ["path", "path_a", "path_b", "folder"]
        .iter()
        .filter_map(|k| data.get(*k).and_then(|v| v.as_str()).map(str::to_owned))
        .collect()
}

/// Per-scan buffer of emitted issues. Drained at scan completion via
/// [`finalize`].
pub struct HealthCollector {
    library_id: Uuid,
    scan_id: Uuid,
    started_at: DateTime<FixedOffset>,
    issues: Vec<IssueKind>,
    /// Files the scanner skipped this run because their `size + mtime` matched
    /// the prior scan. Without re-opening the archive we can't know whether
    /// any archive-derived warning (MissingComicInfo, MalformedComicInfo, …) is
    /// still applicable, so we conservatively keep the existing rows alive at
    /// finalize time. See `finalize` for the touch step.
    touched_files: Vec<String>,
    /// Folders the scanner skipped at the folder-level fast-path (recursive
    /// max mtime ≤ last_scanned_at). Open issues whose payload references a
    /// path under one of these folders are kept alive on the same principle.
    touched_folders: Vec<String>,
    auto_resolve: bool,
    events: Option<Broadcaster>,
}

impl HealthCollector {
    pub fn new(library_id: Uuid, scan_id: Uuid, started_at: DateTime<FixedOffset>) -> Self {
        Self {
            library_id,
            scan_id,
            started_at,
            issues: Vec::new(),
            touched_files: Vec::new(),
            touched_folders: Vec::new(),
            auto_resolve: true,
            events: None,
        }
    }

    /// Collector for narrow scans. It still upserts emitted issues, but it
    /// does not auto-resolve library-wide issues that were outside the scan
    /// scope and therefore not re-emitted.
    pub fn new_scoped(library_id: Uuid, scan_id: Uuid, started_at: DateTime<FixedOffset>) -> Self {
        Self {
            auto_resolve: false,
            ..Self::new(library_id, scan_id, started_at)
        }
    }

    pub fn with_events(mut self, events: Broadcaster) -> Self {
        self.events = Some(events);
        self
    }

    pub fn emit(&mut self, issue: IssueKind) {
        tracing::debug!(
            kind = issue.kind(),
            severity = issue.severity().as_str(),
            "scanner: health issue emitted",
        );
        if let Some(events) = &self.events {
            events.emit(ScanEvent::HealthIssue {
                library_id: self.library_id,
                scan_id: self.scan_id,
                kind: issue.kind().to_owned(),
                severity: issue.severity().as_str().to_owned(),
                path: issue.event_path(),
            });
        }
        self.issues.push(issue);
    }

    /// Mark a file as still-present-but-unchanged on disk. Open health rows
    /// whose payload references this exact path are bumped to `last_seen_at = now`
    /// at finalize time so the auto-resolve sweep doesn't close them. Without
    /// this, every fast-path skip would silently mark archive-derived warnings
    /// resolved on the next scan.
    pub fn touch_file(&mut self, path: &Path) {
        self.touched_files.push(path.to_string_lossy().into_owned());
    }

    /// Same as [`Self::touch_file`] but for a whole folder the scanner skipped
    /// because nothing inside it changed. Issues whose payload path equals the
    /// folder OR sits under it are preserved.
    pub fn touch_folder(&mut self, folder: &Path) {
        self.touched_folders
            .push(folder.to_string_lossy().into_owned());
    }

    pub fn count(&self) -> usize {
        self.issues.len()
    }

    pub fn merge(&mut self, mut other: HealthCollector) {
        self.issues.append(&mut other.issues);
        self.touched_files.append(&mut other.touched_files);
        self.touched_folders.append(&mut other.touched_folders);
        self.auto_resolve = self.auto_resolve && other.auto_resolve;
        if self.events.is_none() {
            self.events = other.events;
        }
    }

    /// Persist the buffered issues + auto-resolve any open issues for this
    /// library that weren't re-emitted this scan.
    pub async fn finalize(self, db: &DatabaseConnection) -> anyhow::Result<()> {
        let now = Utc::now().fixed_offset();

        // Per-issue upsert (keyed on (library_id, fingerprint)). On conflict:
        //   - bump last_seen_at + scan_id + payload + severity (latest wins)
        //   - re-open: clear resolved_at if the issue had been auto-resolved
        // We deliberately do NOT clear dismissed_at — admin choice is sticky.
        for chunk in self.issues.chunks(500) {
            let rows = chunk.iter().map(|issue| library_health_issue::ActiveModel {
                id: Set(Uuid::now_v7()),
                library_id: Set(self.library_id),
                scan_id: Set(Some(self.scan_id)),
                kind: Set(issue.kind().to_string()),
                payload: Set(issue.payload()),
                severity: Set(issue.severity().as_str().to_string()),
                fingerprint: Set(issue.fingerprint()),
                first_seen_at: Set(now),
                last_seen_at: Set(now),
                resolved_at: Set(None),
                dismissed_at: Set(None),
            });
            library_health_issue::Entity::insert_many(rows)
                .on_conflict(
                    OnConflict::columns([
                        library_health_issue::Column::LibraryId,
                        library_health_issue::Column::Fingerprint,
                    ])
                    .update_columns([
                        library_health_issue::Column::ScanId,
                        library_health_issue::Column::Kind,
                        library_health_issue::Column::Payload,
                        library_health_issue::Column::Severity,
                        library_health_issue::Column::LastSeenAt,
                        library_health_issue::Column::ResolvedAt,
                    ])
                    .to_owned(),
                )
                .exec(db)
                .await?;
        }

        // Touch open issues for files/folders the scanner skipped this run
        // because their on-disk fingerprint didn't change. Without this, the
        // auto-resolve sweep below would falsely close warnings whose root
        // cause is still present (the archive wasn't re-opened, so the
        // emitter never had a chance to re-fire). We pull the open set for
        // this library and inspect each row's payload in Rust — open-issue
        // cardinality is bounded so the per-scan cost is negligible.
        if !self.touched_files.is_empty() || !self.touched_folders.is_empty() {
            let open: Vec<library_health_issue::Model> = library_health_issue::Entity::find()
                .filter(library_health_issue::Column::LibraryId.eq(self.library_id))
                .filter(library_health_issue::Column::ResolvedAt.is_null())
                .filter(library_health_issue::Column::Kind.ne("PageCountMismatch"))
                .all(db)
                .await?;
            let touched_files: HashSet<&str> =
                self.touched_files.iter().map(String::as_str).collect();
            let folder_prefixes: Vec<String> = self
                .touched_folders
                .iter()
                .map(|f| {
                    if f.ends_with('/') {
                        f.clone()
                    } else {
                        format!("{f}/")
                    }
                })
                .collect();
            let to_touch: Vec<Uuid> = open
                .into_iter()
                .filter(|row| {
                    paths_in_payload(&row.payload).iter().any(|p| {
                        touched_files.contains(p.as_str())
                            || self.touched_folders.iter().any(|f| f == p)
                            || folder_prefixes.iter().any(|pre| p.starts_with(pre))
                    })
                })
                .map(|r| r.id)
                .collect();
            if !to_touch.is_empty() {
                library_health_issue::Entity::update_many()
                    .col_expr(library_health_issue::Column::LastSeenAt, Expr::value(now))
                    .filter(library_health_issue::Column::Id.is_in(to_touch))
                    .exec(db)
                    .await?;
            }
        }

        if self.auto_resolve {
            // Auto-resolve: any open issue for this library whose last_seen_at is
            // older than the scan start has implicitly disappeared. Narrow scans
            // skip this because they did not inspect the whole library.
            library_health_issue::Entity::update_many()
                .col_expr(library_health_issue::Column::ResolvedAt, Expr::value(now))
                .filter(library_health_issue::Column::LibraryId.eq(self.library_id))
                .filter(library_health_issue::Column::ResolvedAt.is_null())
                .filter(library_health_issue::Column::LastSeenAt.lt(self.started_at))
                .exec(db)
                .await?;
        }

        // Refresh the open-issues gauge per severity (Milestone 14).
        for sev in ["error", "warning", "info"] {
            let open = library_health_issue::Entity::find()
                .filter(library_health_issue::Column::LibraryId.eq(self.library_id))
                .filter(library_health_issue::Column::Severity.eq(sev))
                .filter(library_health_issue::Column::ResolvedAt.is_null())
                .filter(library_health_issue::Column::DismissedAt.is_null())
                .count(db)
                .await
                .unwrap_or(0);
            metrics::gauge!(
                "comic_scan_health_issues_open",
                "library_id" => self.library_id.to_string(),
                "severity" => sev,
            )
            .set(open as f64);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_across_emissions() {
        let a = IssueKind::FileAtRoot {
            path: PathBuf::from("/lib/orphan.cbz"),
        };
        let b = IssueKind::FileAtRoot {
            path: PathBuf::from("/lib/orphan.cbz"),
        };
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn duplicate_content_fingerprint_is_order_independent() {
        let a = IssueKind::DuplicateContent {
            path_a: PathBuf::from("/lib/A.cbz"),
            path_b: PathBuf::from("/lib/B.cbz"),
        };
        let b = IssueKind::DuplicateContent {
            path_a: PathBuf::from("/lib/B.cbz"),
            path_b: PathBuf::from("/lib/A.cbz"),
        };
        assert_eq!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn severity_classification() {
        assert_eq!(
            IssueKind::UnreadableArchive {
                path: PathBuf::from("/x"),
                error: "broken".into(),
            }
            .severity(),
            Severity::Error,
        );
        assert_eq!(
            IssueKind::FileAtRoot {
                path: PathBuf::from("/x")
            }
            .severity(),
            Severity::Warning,
        );
        assert_eq!(
            IssueKind::MissingComicInfo {
                path: PathBuf::from("/x")
            }
            .severity(),
            Severity::Info,
        );
    }
}
