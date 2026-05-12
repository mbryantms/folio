//! SeaORM entity definitions.
//!
//! Phase 0 entities only:
//!   - [`user`]                — local + OIDC users, role, token_version
//!   - [`auth_session`]        — refresh-token rows
//!   - [`library_user_access`] — per-user library ACL (schema in Phase 1, UI Phase 5)
//!   - [`audit_log`]           — append-only admin/security action log
//!
//! Library/series/issue entities arrive in Phase 1a. Until then this crate
//! stays small.

pub mod app_password;
pub mod audit_log;
pub mod auth_session;
pub mod catalog_source;
pub mod cbl_entry;
pub mod cbl_list;
pub mod cbl_refresh_log;
pub mod collection_entry;
pub mod issue;
pub mod issue_credit;
pub mod issue_genre;
pub mod issue_tag;
pub mod library;
pub mod library_health_issue;
pub mod library_user_access;
pub mod marker;
pub mod progress_record;
pub mod rail_dismissal;
pub mod reading_session;
pub mod saved_view;
pub mod scan_run;
pub mod series;
pub mod series_credit;
pub mod series_genre;
pub mod series_tag;
pub mod user;
pub mod user_rating;
pub mod user_series_progress;
pub mod user_view_pin;

pub mod prelude {
    pub use super::app_password::Entity as AppPassword;
    pub use super::audit_log::Entity as AuditLog;
    pub use super::auth_session::Entity as AuthSession;
    pub use super::catalog_source::Entity as CatalogSource;
    pub use super::cbl_entry::Entity as CblEntry;
    pub use super::cbl_list::Entity as CblList;
    pub use super::cbl_refresh_log::Entity as CblRefreshLog;
    pub use super::collection_entry::Entity as CollectionEntry;
    pub use super::issue::Entity as Issue;
    pub use super::issue_credit::Entity as IssueCredit;
    pub use super::issue_genre::Entity as IssueGenre;
    pub use super::issue_tag::Entity as IssueTag;
    pub use super::library::Entity as Library;
    pub use super::library_health_issue::Entity as LibraryHealthIssue;
    pub use super::library_user_access::Entity as LibraryUserAccess;
    pub use super::marker::Entity as Marker;
    pub use super::progress_record::Entity as ProgressRecord;
    pub use super::rail_dismissal::Entity as RailDismissal;
    pub use super::reading_session::Entity as ReadingSession;
    pub use super::saved_view::Entity as SavedView;
    pub use super::scan_run::Entity as ScanRun;
    pub use super::series::Entity as Series;
    pub use super::series_credit::Entity as SeriesCredit;
    pub use super::series_genre::Entity as SeriesGenre;
    pub use super::series_tag::Entity as SeriesTag;
    pub use super::user::Entity as User;
    pub use super::user_rating::Entity as UserRating;
    pub use super::user_series_progress::Entity as UserSeriesProgress;
    pub use super::user_view_pin::Entity as UserViewPin;
}
