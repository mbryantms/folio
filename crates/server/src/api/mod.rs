pub mod account;
pub mod admin_activity;
pub mod admin_email;
pub mod admin_fs;
pub mod admin_logs;
pub mod admin_ocr;
pub mod admin_queue;
pub mod admin_settings;
pub mod admin_stats;
pub mod admin_thumbs;
pub mod admin_users;
pub mod app_passwords;
pub mod audit;
pub mod auth_config;
pub mod cbl_lists;
pub mod collections;
pub mod csp;
pub mod filter_options;
pub mod form_or_json;
pub mod health;
pub mod health_issues;
pub mod issue_ocr;
pub mod issues;
pub mod komga_compat;
pub mod libraries;
pub mod markers;
pub mod meta;
pub mod next_up;
pub mod opds;
pub mod opds_progression;
pub mod opds_pse;
pub mod opds_v2;
pub mod page_bytes;
pub mod pages;
pub mod people;
pub mod progress;
pub mod rails;
pub mod ratings;
pub mod reading_sessions;
pub mod reconcile;
pub mod saved_views;
pub mod scan_runs;
pub mod series;
pub mod server_info;
pub mod server_releases;
pub mod sessions;
pub mod sidebar_layout;
pub mod thumbnails;
pub mod ws_scan_events;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Canonical error-envelope helper. Every mutating handler returns
/// errors via this shape per the project convention documented in
/// CLAUDE.md: `{"error": {"code": "...", "message": "..."}}`.
///
/// Promoted from `api/libraries.rs` during code-quality-cleanup M1
/// (was duplicated verbatim across 36 sibling files).
pub(crate) fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

/// 404 with the standard envelope. Used by feed/page handlers that
/// don't want to spell out the `not_found` code inline.
pub(crate) fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", "Not found")
}
