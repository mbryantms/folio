//! Per-user reading-state surface (saved-views M2).
//!
//! The filter compiler in M3 uses `series_progress` to express predicates
//! like "Read Progress ≥ 50" or "Last Read in last 7 days" against the
//! `user_series_progress` SQL view (defined by migration
//! `m20261204_000001_user_series_progress_view`).

pub mod series_progress;
