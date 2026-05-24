//! Pagination envelopes (§15.6).
//!
//! [`CursorPage<T>`] is the canonical list envelope across the API. Bounded
//! domains (e.g., `/me/sessions`, `/me/app-passwords`) still return the same
//! envelope but with `next_cursor: None` for shape uniformity — see
//! `docs/dev/list-pagination.md`.
//!
//! [`encode_cursor`] / [`decode_cursor`] keep the opaque base64-JSON encoding
//! out of handler bodies. Cursors are server-trusted state; do not expose
//! ordering details to callers.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Cursor-paginated list response. `total` is populated only on the first
/// page of paginated lists where the count is cheap; bounded lists omit it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CursorPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

impl<T> CursorPage<T> {
    /// Build a fully-bounded response (one page, no more to walk).
    pub fn bounded(items: Vec<T>) -> Self {
        Self {
            items,
            next_cursor: None,
            total: None,
        }
    }

    /// Build a paginated response with a continuation cursor.
    pub fn paginated(items: Vec<T>, next_cursor: Option<String>, total: Option<u64>) -> Self {
        Self {
            items,
            next_cursor,
            total,
        }
    }
}

/// Offset-paginated list response. Use only for bounded domains where
/// random-access page jumps are required; otherwise prefer [`CursorPage`].
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OffsetPage<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub offset: u64,
    pub limit: u64,
}

/// Encode an opaque cursor for a `next_cursor` field.
///
/// Cursors hold whatever state the endpoint needs to resume — typically a
/// `(sort_key, last_id)` tuple. The shape is private to the endpoint; clients
/// must round-trip the value unchanged.
pub fn encode_cursor<C: Serialize>(cursor: &C) -> Result<String, CursorError> {
    let json = serde_json::to_vec(cursor).map_err(|_| CursorError::Encode)?;
    Ok(URL_SAFE_NO_PAD.encode(json))
}

/// Decode an opaque cursor previously emitted by [`encode_cursor`].
pub fn decode_cursor<C: DeserializeOwned>(raw: &str) -> Result<C, CursorError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| CursorError::Decode)?;
    serde_json::from_slice(&bytes).map_err(|_| CursorError::Decode)
}

/// Cursor (de)serialization failure modes. Map to
/// [`ApiErrorCode::BadCursor`](crate::error::ApiErrorCode::BadCursor) at the
/// API boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CursorError {
    #[error("cursor encode failed")]
    Encode,
    #[error("cursor decode failed")]
    Decode,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Demo {
        last_id: u64,
        ts: i64,
    }

    #[test]
    fn cursor_round_trips() {
        let original = Demo {
            last_id: 42,
            ts: 1_700_000_000,
        };
        let encoded = encode_cursor(&original).unwrap();
        let decoded: Demo = decode_cursor(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn cursor_is_url_safe() {
        let original = Demo {
            last_id: u64::MAX,
            ts: i64::MIN,
        };
        let encoded = encode_cursor(&original).unwrap();
        assert!(
            encoded
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "cursor {encoded:?} contains non-url-safe chars"
        );
    }

    #[test]
    fn malformed_cursor_decodes_as_error() {
        let result: Result<Demo, _> = decode_cursor("not-a-valid-cursor");
        assert_eq!(result, Err(CursorError::Decode));
    }

    #[test]
    fn bounded_helper_omits_cursor_and_total() {
        let page = CursorPage::bounded(vec![1, 2, 3]);
        let json = serde_json::to_string(&page).unwrap();
        assert!(json.contains(r#""next_cursor":null"#));
        assert!(!json.contains(r#""total""#));
    }
}
