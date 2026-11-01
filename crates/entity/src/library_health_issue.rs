//! Library Scanner v1 — health-issue rows (spec §10).
//!
//! Each row is one **distinct** problem the scanner has found in a library.
//! `(library_id, fingerprint)` is unique, so on subsequent scans the same
//! issue is upserted rather than duplicated; `last_seen_at` is bumped each
//! time it's re-detected. When a scan completes without re-emitting an open
//! issue, the reconcile pass sets `resolved_at`.
//!
//! `payload` carries the typed body of [`crate::library_health_issue::*`]
//! (the Rust enum lives in `crates/server/src/library/health.rs`); this
//! entity is intentionally JSON-blob-shaped so new variants can be added
//! without migrations.

use sea_orm::Iterable;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

// Storage contract (AR-7): `library_health_issues.kind` stays `TEXT` (no
// CHECK) holding exactly these strings, which are the variant names
// (PascalCase, unlike the snake_case `kind` tag inside `payload`);
// `string_value` and the serde name are the same string (tests below). A
// row holding any other string fails to load with a `DbErr`. Each row's
// `fingerprint` hashes the same name, so a rename would orphan every
// stored row: treat the strings as a wire format.

/// What a health row reports. The scanner's typed payload enum
/// (`server::library::health::IssueKind`) maps each of its variants onto
/// one of these.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    EnumIter,
    DeriveActiveEnum,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum HealthIssueKind {
    /// An archive directly under the library root.
    #[sea_orm(string_value = "FileAtRoot")]
    FileAtRoot,
    /// A folder with no archives.
    #[sea_orm(string_value = "EmptyFolder")]
    EmptyFolder,
    /// A file the scanner could not stat or open.
    #[sea_orm(string_value = "UnreadableFile")]
    UnreadableFile,
    /// A folder the walk could not read.
    #[sea_orm(string_value = "UnreadableFolder")]
    UnreadableFolder,
    /// An archive that failed with an I/O error.
    #[sea_orm(string_value = "UnreadableArchive")]
    UnreadableArchive,
    /// An archive without ComicInfo.xml (opt-in per library).
    #[sea_orm(string_value = "MissingComicInfo")]
    MissingComicInfo,
    /// A ComicInfo.xml that failed to parse.
    #[sea_orm(string_value = "MalformedComicInfo")]
    MalformedComicInfo,
    /// An archive container that failed to parse.
    #[sea_orm(string_value = "MalformedArchive")]
    MalformedArchive,
    /// A password-protected archive.
    #[sea_orm(string_value = "EncryptedArchive")]
    EncryptedArchive,
    /// A folder name that disagrees with ComicInfo `<Series>`.
    #[sea_orm(string_value = "FolderNameMismatch")]
    FolderNameMismatch,
    /// A folder holding several ComicInfo `<Series>` values.
    #[sea_orm(string_value = "MixedSeriesInFolder")]
    MixedSeriesInFolder,
    /// Two files with the same content hash.
    #[sea_orm(string_value = "DuplicateContent")]
    DuplicateContent,
    /// A `series.json` with no archives beside it.
    #[sea_orm(string_value = "OrphanedSeriesJson")]
    OrphanedSeriesJson,
    /// An archive in a format the library does not ingest.
    #[sea_orm(string_value = "UnsupportedArchiveFormat")]
    UnsupportedArchiveFormat,
    /// An archive that opened only through a recovery path.
    #[sea_orm(string_value = "RecoveredArchive")]
    RecoveredArchive,
    /// Archive entries dropped by a soft defense.
    #[sea_orm(string_value = "SkippedArchiveEntries")]
    SkippedArchiveEntries,
    /// A page that failed to decode (deep validate).
    #[sea_orm(string_value = "UnreadablePage")]
    UnreadablePage,
    /// A structurally valid archive with no image entries.
    #[sea_orm(string_value = "NoPages")]
    NoPages,
    /// A subtree the scanner skipped because its layout is ambiguous.
    #[sea_orm(string_value = "AmbiguousFolder")]
    AmbiguousFolder,
    /// A file tagged with an external id another entity owns.
    #[sea_orm(string_value = "DuplicateExternalId")]
    DuplicateExternalId,
    /// Never stored: the health list synthesizes this row per request
    /// for a writeback library with user pins not yet in its XML
    /// (`api::health_issues`).
    #[sea_orm(string_value = "MetadataDriftFromXml")]
    MetadataDriftFromXml,
}

impl HealthIssueKind {
    /// The stored / wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FileAtRoot => "FileAtRoot",
            Self::EmptyFolder => "EmptyFolder",
            Self::UnreadableFile => "UnreadableFile",
            Self::UnreadableFolder => "UnreadableFolder",
            Self::UnreadableArchive => "UnreadableArchive",
            Self::MissingComicInfo => "MissingComicInfo",
            Self::MalformedComicInfo => "MalformedComicInfo",
            Self::MalformedArchive => "MalformedArchive",
            Self::EncryptedArchive => "EncryptedArchive",
            Self::FolderNameMismatch => "FolderNameMismatch",
            Self::MixedSeriesInFolder => "MixedSeriesInFolder",
            Self::DuplicateContent => "DuplicateContent",
            Self::OrphanedSeriesJson => "OrphanedSeriesJson",
            Self::UnsupportedArchiveFormat => "UnsupportedArchiveFormat",
            Self::RecoveredArchive => "RecoveredArchive",
            Self::SkippedArchiveEntries => "SkippedArchiveEntries",
            Self::UnreadablePage => "UnreadablePage",
            Self::NoPages => "NoPages",
            Self::AmbiguousFolder => "AmbiguousFolder",
            Self::DuplicateExternalId => "DuplicateExternalId",
            Self::MetadataDriftFromXml => "MetadataDriftFromXml",
        }
    }

    /// The variant whose stored string is `s`, if any.
    pub fn parse(s: &str) -> Option<Self> {
        Self::iter().find(|v| v.as_str() == s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "library_health_issues")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub library_id: Uuid,
    #[sea_orm(nullable)]
    pub scan_id: Option<Uuid>,
    /// Discriminant — `IssueKind::kind()` in the server crate.
    pub kind: HealthIssueKind,
    pub payload: Json,
    /// `error` | `warning` | `info`.
    pub severity: String,
    /// Stable hash of `(library_id, kind, identifying_path)` — drives the
    /// upsert behavior described above.
    pub fingerprint: String,
    pub first_seen_at: DateTimeWithTimeZone,
    pub last_seen_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub resolved_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(nullable)]
    pub dismissed_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::library::Entity",
        from = "Column::LibraryId",
        to = "super::library::Column::Id",
        on_delete = "Cascade"
    )]
    Library,
    #[sea_orm(
        belongs_to = "super::scan_run::Entity",
        from = "Column::ScanId",
        to = "super::scan_run::Column::Id",
        on_delete = "SetNull"
    )]
    ScanRun,
}

impl Related<super::library::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Library.def()
    }
}
impl Related<super::scan_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ScanRun.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enum_contract::assert_exact_strings;

    #[test]
    fn kind_round_trips_exact_db_strings() {
        assert_exact_strings(
            &[
                (HealthIssueKind::FileAtRoot, "FileAtRoot"),
                (HealthIssueKind::EmptyFolder, "EmptyFolder"),
                (HealthIssueKind::UnreadableFile, "UnreadableFile"),
                (HealthIssueKind::UnreadableFolder, "UnreadableFolder"),
                (HealthIssueKind::UnreadableArchive, "UnreadableArchive"),
                (HealthIssueKind::MissingComicInfo, "MissingComicInfo"),
                (HealthIssueKind::MalformedComicInfo, "MalformedComicInfo"),
                (HealthIssueKind::MalformedArchive, "MalformedArchive"),
                (HealthIssueKind::EncryptedArchive, "EncryptedArchive"),
                (HealthIssueKind::FolderNameMismatch, "FolderNameMismatch"),
                (HealthIssueKind::MixedSeriesInFolder, "MixedSeriesInFolder"),
                (HealthIssueKind::DuplicateContent, "DuplicateContent"),
                (HealthIssueKind::OrphanedSeriesJson, "OrphanedSeriesJson"),
                (
                    HealthIssueKind::UnsupportedArchiveFormat,
                    "UnsupportedArchiveFormat",
                ),
                (HealthIssueKind::RecoveredArchive, "RecoveredArchive"),
                (
                    HealthIssueKind::SkippedArchiveEntries,
                    "SkippedArchiveEntries",
                ),
                (HealthIssueKind::UnreadablePage, "UnreadablePage"),
                (HealthIssueKind::NoPages, "NoPages"),
                (HealthIssueKind::AmbiguousFolder, "AmbiguousFolder"),
                (HealthIssueKind::DuplicateExternalId, "DuplicateExternalId"),
                (
                    HealthIssueKind::MetadataDriftFromXml,
                    "MetadataDriftFromXml",
                ),
            ],
            HealthIssueKind::as_str,
            HealthIssueKind::parse,
            &[
                "file_at_root",
                "fileAtRoot",
                "PageCountMismatch",
                "AmbiguousVolume",
                "",
            ],
        );
    }
}
