pub mod account;
pub mod admin_activity;
pub mod admin_email;
pub mod admin_fs;
pub mod admin_logs;
pub mod admin_metadata;
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
pub mod covers;
pub mod creators;
pub mod csp;
pub mod external_ids;
pub mod extractors;
pub mod filter_options;
pub mod form_or_json;
pub mod health;
pub mod health_issues;
pub mod issue_ocr;
pub mod issues;
pub mod komga_compat;
pub mod libraries;
pub mod log_widgets;
pub mod markers;
pub mod meta;
pub mod metadata_search;
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
pub mod reading_log;
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
use shared::error::{ApiError, ApiErrorCode};

/// Build an error response from a typed [`ApiErrorCode`] + free-form message.
///
/// This is the canonical helper. Every new mutating handler should reach
/// for this; the legacy [`error`] / [`not_found`] helpers below remain only
/// as an incremental-migration aid.
pub(crate) fn respond(
    status: StatusCode,
    code: ApiErrorCode,
    message: impl Into<String>,
) -> Response {
    (status, Json(ApiError::new(code, message))).into_response()
}

/// 422 with the canonical envelope. Use for semantic validation failures
/// (rule violations, business-logic constraints). Reach for [`respond`]
/// with `StatusCode::BAD_REQUEST` when the input is malformed/unparseable.
///
/// M3 of the audit-remediation plan adopts this across handlers.
#[allow(dead_code)]
pub(crate) fn validation(message: impl Into<String>) -> Response {
    respond(
        StatusCode::UNPROCESSABLE_ENTITY,
        ApiErrorCode::Validation,
        message,
    )
}

/// 404 with the canonical envelope and a caller-provided message.
///
/// M3 of the audit-remediation plan adopts this across handlers.
#[allow(dead_code)]
pub(crate) fn not_found_msg(message: impl Into<String>) -> Response {
    respond(StatusCode::NOT_FOUND, ApiErrorCode::NotFound, message)
}

/// Legacy error helper retained during M0/M3 migration. New code uses
/// [`respond`] with an [`ApiErrorCode`] variant.
pub(crate) fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

/// 404 with the standard envelope and a generic "Not found" message.
pub(crate) fn not_found() -> Response {
    respond(StatusCode::NOT_FOUND, ApiErrorCode::NotFound, "Not found")
}
