//! Shared types used across the comic-reader workspace.
//!
//! Anything that crosses crate boundaries — domain IDs, error envelopes,
//! pagination shapes, and re-exported newtypes — lives here. Crates depending
//! on this should pull in [`prelude`] for the common subset.

pub mod error;
pub mod ids;
pub mod pagination;

pub mod prelude {
    pub use crate::error::{ApiError, ApiErrorCode};
    pub use crate::ids::{IssueId, LibraryId, SeriesId, UserId};
    pub use crate::pagination::{CursorPage, OffsetPage};
}
