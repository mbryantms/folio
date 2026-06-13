//! Wire-level error envelope (§15.3).
//!
//! The on-the-wire shape is `{"error": {"code": "...", "message": "...", "details": ...}}`.
//! Error codes are stable identifiers drawn from [`ApiErrorCode`]; new codes are
//! added here, never invented at the call site.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiError {
    pub error: ApiErrorBody,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
    /// Optional structured detail. For **validation** errors (422) this
    /// is the lint-enforced field-error list — a JSON array of
    /// [`FieldError`] (`[{"field": "...", "message": "..."}]`) so a
    /// client form can bind each message to its input. Other error
    /// kinds may use it for endpoint-specific structured context. The
    /// human-readable `message` always stays a complete summary on its
    /// own, so a client that ignores `details` loses nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// One field-scoped validation failure. The wire shape of a single
/// entry in `ApiErrorBody.details` for 422 responses.
///
/// `field` is the dotted path garde emits (e.g. `"port"`, `"smtp.host"`,
/// `"items[2].name"`); top-level/whole-body errors use an empty string.
/// `message` is the human-readable rule violation.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

impl ApiError {
    /// Construct an envelope from a stable code + free-form message.
    pub fn new(code: ApiErrorCode, message: impl Into<String>) -> Self {
        Self {
            error: ApiErrorBody {
                code: code.as_str().to_owned(),
                message: message.into(),
                details: None,
            },
        }
    }

    /// As [`Self::new`] but attaches free-form structured details.
    pub fn with_details(code: ApiErrorCode, message: impl Into<String>, details: Value) -> Self {
        Self {
            error: ApiErrorBody {
                code: code.as_str().to_owned(),
                message: message.into(),
                details: Some(details),
            },
        }
    }

    /// As [`Self::new`] but attaches the field-error list used by 422
    /// validation responses. Serialises `fields` into `details` as
    /// `[{"field", "message"}]`. Empty `fields` leaves `details` unset
    /// so the wire shape stays identical to a plain error.
    pub fn with_field_errors(
        code: ApiErrorCode,
        message: impl Into<String>,
        fields: Vec<FieldError>,
    ) -> Self {
        let details = if fields.is_empty() {
            None
        } else {
            serde_json::to_value(&fields).ok()
        };
        Self {
            error: ApiErrorBody {
                code: code.as_str().to_owned(),
                message: message.into(),
                details,
            },
        }
    }
}

/// Stable error codes surfaced to clients. Categories follow `<resource>.<reason>`.
///
/// **Adding a code:** add a variant here, map it in [`Self::as_str`], use it
/// at the call site via [`crate::api::respond`]. Never pass a raw string code
/// — the enum exists to keep client-facing codes exhaustive and grep-able.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApiErrorCode {
    // --- Authentication / authorization -------------------------------------
    AuthRequired,
    AuthInvalid,
    AuthEmailUnverified,
    AuthCsrf,
    AuthDisabled,
    AuthLockedOut,
    AuthOidcError,
    PermissionDenied,
    LibraryAccessDenied,
    SelfDemote,
    SelfDisable,

    // --- Generic ------------------------------------------------------------
    NotFound,
    Conflict,
    RateLimited,
    Internal,
    Database,
    ServiceUnavailable,

    // --- Validation ---------------------------------------------------------
    Validation,
    ValidationRating,
    ValidationState,
    BadCursor,
    BadFilter,
    PatchEmpty,
    InvalidUrl,

    // --- Resource-specific (more precise than the generic above) -----------
    UserNotFound,
    UserInactive,
    PageNotFound,
    ConflictSlug,

    // --- Operations ---------------------------------------------------------
    ArchiveUnreadable,
    ParseFailed,
    FetchFailed,
    RefreshFailed,

    // --- Settings -----------------------------------------------------------
    SettingsInvalidCombination,

    // --- Email --------------------------------------------------------------
    EmailSendFailed,

    // --- HTTP semantics -----------------------------------------------------
    RangeNotSatisfiable,
    TooLarge,
    UnsupportedMediaType,

    // --- Pipeline-specific --------------------------------------------------
    ThumbBusy,
    PseMissingParams,
}

impl ApiErrorCode {
    /// Stable wire representation. Frontends key on this string; do not
    /// change a variant's mapping without coordinating a client release.
    pub fn as_str(self) -> &'static str {
        match self {
            // Auth
            Self::AuthRequired => "auth.required",
            Self::AuthInvalid => "auth.invalid",
            Self::AuthEmailUnverified => "auth.email_unverified",
            Self::AuthCsrf => "auth.csrf",
            Self::AuthDisabled => "auth.disabled",
            Self::AuthLockedOut => "auth.locked_out",
            Self::AuthOidcError => "auth.oidc_error",
            Self::PermissionDenied => "auth.permission_denied",
            Self::LibraryAccessDenied => "library_access_denied",
            Self::SelfDemote => "self_demote",
            Self::SelfDisable => "self_disable",

            // Generic
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::RateLimited => "rate_limited",
            Self::Internal => "internal",
            Self::Database => "db",
            Self::ServiceUnavailable => "service_unavailable",

            // Validation
            Self::Validation => "validation",
            Self::ValidationRating => "validation.rating",
            Self::ValidationState => "validation.state",
            Self::BadCursor => "bad_cursor",
            Self::BadFilter => "filter_invalid",
            Self::PatchEmpty => "patch_empty",
            Self::InvalidUrl => "invalid_url",

            // Resource-specific
            Self::UserNotFound => "user_not_found",
            Self::UserInactive => "user_inactive",
            Self::PageNotFound => "page_not_found",
            Self::ConflictSlug => "conflict.slug",

            // Operations
            Self::ArchiveUnreadable => "archive_unreadable",
            Self::ParseFailed => "parse_failed",
            Self::FetchFailed => "fetch_failed",
            Self::RefreshFailed => "refresh_failed",

            // Settings
            Self::SettingsInvalidCombination => "settings.invalid_combination",

            // Email
            Self::EmailSendFailed => "email.send_failed",

            // HTTP semantics
            Self::RangeNotSatisfiable => "range_not_satisfiable",
            Self::TooLarge => "too_large",
            Self::UnsupportedMediaType => "unsupported_media_type",

            // Pipeline-specific
            Self::ThumbBusy => "thumb.busy",
            Self::PseMissingParams => "pse_missing_params",
        }
    }
}

impl fmt::Display for ApiErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<ApiErrorCode> for &'static str {
    fn from(code: ApiErrorCode) -> Self {
        code.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_through_display() {
        for code in [
            ApiErrorCode::AuthRequired,
            ApiErrorCode::NotFound,
            ApiErrorCode::Validation,
            ApiErrorCode::ConflictSlug,
            ApiErrorCode::SettingsInvalidCombination,
        ] {
            assert_eq!(code.to_string(), code.as_str());
        }
    }

    #[test]
    fn envelope_serializes_with_optional_details_omitted() {
        let err = ApiError::new(ApiErrorCode::NotFound, "missing");
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(
            json,
            r#"{"error":{"code":"not_found","message":"missing"}}"#
        );
    }

    #[test]
    fn envelope_includes_details_when_provided() {
        let err = ApiError::with_details(
            ApiErrorCode::Validation,
            "bad input",
            serde_json::json!({"field": "name"}),
        );
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains(r#""details":{"field":"name"}"#));
    }

    #[test]
    fn field_errors_serialize_as_a_list_of_field_message_pairs() {
        let err = ApiError::with_field_errors(
            ApiErrorCode::Validation,
            "port: must be 1-65535; host: required",
            vec![
                FieldError {
                    field: "port".to_owned(),
                    message: "must be 1-65535".to_owned(),
                },
                FieldError {
                    field: "host".to_owned(),
                    message: "required".to_owned(),
                },
            ],
        );
        let json: Value = serde_json::to_value(&err).unwrap();
        let details = json["error"]["details"].as_array().unwrap();
        assert_eq!(details.len(), 2);
        assert_eq!(details[0]["field"], "port");
        assert_eq!(details[0]["message"], "must be 1-65535");
        assert_eq!(details[1]["field"], "host");
    }

    #[test]
    fn empty_field_errors_leave_details_unset() {
        // Wire shape must stay byte-identical to a plain error when there
        // are no field-scoped failures — a client that branches on the
        // presence of `details` shouldn't see an empty array.
        let err = ApiError::with_field_errors(ApiErrorCode::Validation, "nope", vec![]);
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, r#"{"error":{"code":"validation","message":"nope"}}"#);
    }

    #[test]
    fn codes_are_dotted_or_snake_case_only() {
        // Convention guard: no spaces, no camelCase, no slashes.
        for code in [
            ApiErrorCode::AuthCsrf,
            ApiErrorCode::ValidationRating,
            ApiErrorCode::SettingsInvalidCombination,
        ] {
            let s = code.as_str();
            assert!(
                s.chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_'),
                "code {s:?} violates lowercase/./_ convention"
            );
        }
    }
}
