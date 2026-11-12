use sea_orm::Iterable;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

// Storage contract (AR-7): `cbl_lists.last_refresh_error_kind` stays
// `TEXT`, pinned by the `cbl_lists_refresh_error_kind_check` CHECK to
// exactly these strings (`m20271117_000001_cbl_refresh_outcome`);
// `string_value` and the serde name are the same string (tests below).

/// Why a reading list's last refresh failed. Each failure path of a refresh
/// maps onto exactly one kind (`server::cbl::refresh::RefreshError::failure`).
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
#[serde(rename_all = "snake_case")]
pub enum CblRefreshErrorKind {
    /// The source could not be reached: name not found, connection failed,
    /// or no answer in time.
    #[sea_orm(string_value = "unreachable")]
    Unreachable,
    /// The source answered with an error status (or a rate limit / bot
    /// challenge).
    #[sea_orm(string_value = "http_status")]
    HttpStatus,
    /// The list is no longer at its source (HTTP 404 / 410, or the path is
    /// gone from its catalog).
    #[sea_orm(string_value = "missing")]
    Missing,
    /// The outbound address guard refused the source address or a redirect
    /// target. Reads the same whatever the reason.
    #[sea_orm(string_value = "address_refused")]
    AddressRefused,
    /// The file is larger than a list download may be.
    #[sea_orm(string_value = "too_large")]
    TooLarge,
    /// The new content has more entries than a list may have.
    #[sea_orm(string_value = "too_many_entries")]
    TooManyEntries,
    /// The source did not return a valid reading list.
    #[sea_orm(string_value = "invalid_list")]
    InvalidList,
    /// Anything else (a redirect problem, a missing source setting, a
    /// server-side error).
    #[sea_orm(string_value = "other")]
    Other,
}

impl CblRefreshErrorKind {
    /// The stored / wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::HttpStatus => "http_status",
            Self::Missing => "missing",
            Self::AddressRefused => "address_refused",
            Self::TooLarge => "too_large",
            Self::TooManyEntries => "too_many_entries",
            Self::InvalidList => "invalid_list",
            Self::Other => "other",
        }
    }

    /// The variant whose stored string is `s`, if any.
    pub fn parse(s: &str) -> Option<Self> {
        Self::iter().find(|v| v.as_str() == s)
    }
}

/// One imported CBL reading list. Three source kinds: `'upload'` (raw
/// bytes posted by a user), `'url'` (HTTPS endpoint serving a `.cbl`),
/// `'catalog'` (resolved from a `catalog_sources` row + `catalog_path`).
/// CHECK constraint on the table enforces shape per kind.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "cbl_lists")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// `None` for admin-curated system CBLs.
    #[sea_orm(nullable)]
    pub owner_user_id: Option<Uuid>,
    pub source_kind: String,
    /// Populated for `source_kind` of `'url'` and `'catalog'` (the raw
    /// blob URL when catalog-sourced).
    #[sea_orm(nullable)]
    pub source_url: Option<String>,
    /// FK to `catalog_sources.id` when `source_kind = 'catalog'`.
    #[sea_orm(nullable)]
    pub catalog_source_id: Option<Uuid>,
    /// Path within the catalog repo (e.g. `Image/Invincible Universe.cbl`).
    #[sea_orm(nullable)]
    pub catalog_path: Option<String>,
    /// Git blob SHA from the last successful catalog fetch. Used for
    /// change detection — same SHA → upstream unchanged, skip refetch.
    #[sea_orm(nullable)]
    pub github_blob_sha: Option<String>,
    /// HTTP ETag fallback for direct-URL imports.
    #[sea_orm(nullable)]
    pub source_etag: Option<String>,
    #[sea_orm(nullable)]
    pub source_last_modified: Option<String>,
    /// SHA-256 digest of the `.cbl` bytes — final-fallback change signal
    /// when neither ETag nor blob SHA are available.
    pub raw_sha256: Vec<u8>,
    /// Original `.cbl` XML preserved verbatim for re-parse after a
    /// matcher / schema change without needing to refetch.
    pub raw_xml: String,
    /// `<Name>` from the file.
    pub parsed_name: String,
    /// `true` when the parsed file shipped non-empty `<Matchers>` rules
    /// — informational badge in the UI; v1 does not evaluate matchers.
    pub parsed_matchers_present: bool,
    /// `<NumIssues>` from the file. Informational; trust the entry count
    /// for actual lookups.
    #[sea_orm(nullable)]
    pub num_issues_declared: Option<i32>,
    /// User/admin-authored description (markdown supported in the UI).
    #[sea_orm(nullable)]
    pub description: Option<String>,
    pub imported_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub last_refreshed_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(nullable)]
    pub last_match_run_at: Option<DateTimeWithTimeZone>,
    /// Cron expression for scheduled refreshes; `None` = manual-only.
    #[sea_orm(nullable)]
    pub refresh_schedule: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    /// When true, the `/opds/v1/lists/{id}` + v2 mirror render in
    /// strict CBL position order regardless of the caller's
    /// progress. Default false — feeds auto-reorder the up-next
    /// issue to position 0. Curators of strict-order lists ("DC
    /// Year-One Reading Order") flip this on. M2 of
    /// `opds-sync-cleanup-1.0`.
    pub preserve_canonical_order: bool,
    /// When the last refresh attempt (scheduled or manual) finished,
    /// successful or not. `None` until the first one.
    #[sea_orm(nullable)]
    pub last_refresh_attempt_at: Option<DateTimeWithTimeZone>,
    /// Why the last attempt failed; `None` when it succeeded.
    #[sea_orm(nullable)]
    pub last_refresh_error_kind: Option<CblRefreshErrorKind>,
    /// A short note the server composes from safe values (an HTTP status
    /// number, a limit); never text from the remote source.
    #[sea_orm(nullable)]
    pub last_refresh_error_detail: Option<String>,
    /// First failed attempt of the current run of failures.
    #[sea_orm(nullable)]
    pub refresh_failing_since: Option<DateTimeWithTimeZone>,
    /// Failed attempts in a row; a success resets it to 0.
    pub refresh_failure_count: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::OwnerUserId",
        to = "super::user::Column::Id",
        on_delete = "Cascade"
    )]
    User,
    #[sea_orm(
        belongs_to = "super::catalog_source::Entity",
        from = "Column::CatalogSourceId",
        to = "super::catalog_source::Column::Id",
        on_delete = "SetNull"
    )]
    CatalogSource,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}
impl Related<super::catalog_source::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::CatalogSource.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enum_contract::assert_exact_strings;

    #[test]
    fn refresh_error_kind_round_trips_exact_db_strings() {
        assert_exact_strings(
            &[
                (CblRefreshErrorKind::Unreachable, "unreachable"),
                (CblRefreshErrorKind::HttpStatus, "http_status"),
                (CblRefreshErrorKind::Missing, "missing"),
                (CblRefreshErrorKind::AddressRefused, "address_refused"),
                (CblRefreshErrorKind::TooLarge, "too_large"),
                (CblRefreshErrorKind::TooManyEntries, "too_many_entries"),
                (CblRefreshErrorKind::InvalidList, "invalid_list"),
                (CblRefreshErrorKind::Other, "other"),
            ],
            CblRefreshErrorKind::as_str,
            CblRefreshErrorKind::parse,
            &["Unreachable", "timeout", "ssrf", ""],
        );
    }
}
