//! Wire-level error envelope (§15.3).

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Stable error codes surfaced to clients. Categories follow `<resource>.<reason>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorCode {
    AuthRequired,
    AuthInvalid,
    AuthEmailUnverified,
    PermissionDenied,
    NotFound,
    Conflict,
    RateLimited,
    Validation,
    Internal,
}

impl ApiErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AuthRequired => "auth.required",
            Self::AuthInvalid => "auth.invalid",
            Self::AuthEmailUnverified => "auth.email_unverified",
            Self::PermissionDenied => "auth.permission_denied",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::RateLimited => "rate_limited",
            Self::Validation => "validation",
            Self::Internal => "internal",
        }
    }
}
