//! `series.json` parser (Mylar3 schema, §4.3).
//!
//! Top-level shape:
//!
//! ```json
//! {
//!   "metadata": {
//!     "type": "comicSeries",
//!     "name": "Saga",
//!     "description_text": "...",
//!     "description_formatted": "<p>...</p>",
//!     "publisher": "Image Comics",
//!     "imprint": "Image",
//!     "comic_image": "https://...",
//!     "year_began": 2012,
//!     "year_end": null,
//!     "total_issues": 54,
//!     "publication_run": "March 2012 - Present",
//!     "status": "Continuing",
//!     "booktype": "Print",
//!     "age_rating": "Mature 17+",
//!     "comicid": 12345,
//!     "volume": 1
//!   }
//! }
//! ```
//!
//! Per §4.3 we cap input at 256 KiB before parsing. Unknown fields are tolerated
//! and surfaced via the `extra` map for forward-compat.

use crate::ParseError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const MAX_INPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeriesJson {
    #[serde(default)]
    pub metadata: SeriesMetadata,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SeriesMetadata {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub name: Option<String>,
    pub description_text: Option<String>,
    pub description_formatted: Option<String>,
    pub publisher: Option<String>,
    pub imprint: Option<String>,
    pub comic_image: Option<String>,
    pub year_began: Option<i32>,
    pub year_end: Option<i32>,
    pub total_issues: Option<i32>,
    pub publication_run: Option<String>,
    /// Free-form: typically `"Continuing"`, `"Ended"`, `"Cancelled"`, `"Hiatus"`.
    pub status: Option<String>,
    pub booktype: Option<String>,
    pub age_rating: Option<String>,
    /// Mylar3's name for the ComicVine series id.
    pub comicid: Option<i64>,
    pub volume: Option<i32>,

    /// Anything we don't model explicitly is kept here so future fields don't drop on the floor.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

pub fn parse(bytes: &[u8]) -> Result<SeriesJson, ParseError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(ParseError::TooLarge {
            actual: bytes.len(),
            limit: MAX_INPUT_BYTES,
        });
    }
    serde_json::from_slice(bytes).map_err(|e| ParseError::Malformed(e.to_string()))
}

/// Normalize the series.json `status` to one of the values the spec uses for
/// completion semantics (§5.5.1). Unknown → `continuing` (the conservative default).
pub fn normalize_status(status: Option<&str>) -> &'static str {
    match status.unwrap_or("").to_ascii_lowercase().as_str() {
        "ended" | "complete" | "completed" | "finished" => "ended",
        "cancelled" | "canceled" => "cancelled",
        "hiatus" | "on hiatus" => "hiatus",
        _ => "continuing",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "metadata": {
            "type": "comicSeries",
            "name": "Saga",
            "description_text": "An interplanetary love story.",
            "publisher": "Image Comics",
            "imprint": "Image",
            "comic_image": "https://example.com/saga.jpg",
            "year_began": 2012,
            "year_end": null,
            "total_issues": 54,
            "publication_run": "March 2012 - Present",
            "status": "Continuing",
            "booktype": "Print",
            "age_rating": "Mature 17+",
            "comicid": 12345,
            "volume": 1,
            "future_field": "should not break parse"
        }
    }"#;

    #[test]
    fn parses_full_sample() {
        let s = parse(SAMPLE.as_bytes()).unwrap();
        let m = &s.metadata;
        assert_eq!(m.name.as_deref(), Some("Saga"));
        assert_eq!(m.publisher.as_deref(), Some("Image Comics"));
        assert_eq!(m.year_began, Some(2012));
        assert_eq!(m.year_end, None);
        assert_eq!(m.total_issues, Some(54));
        assert_eq!(m.comicid, Some(12345));
        assert_eq!(m.status.as_deref(), Some("Continuing"));
        assert!(m.extra.contains_key("future_field"));
    }

    #[test]
    fn missing_metadata_is_ok() {
        let s = parse(b"{}").unwrap();
        assert!(s.metadata.name.is_none());
    }

    #[test]
    fn malformed_yields_error() {
        let err = parse(b"not json").unwrap_err();
        assert!(matches!(err, ParseError::Malformed(_)));
    }

    #[test]
    fn oversize_rejected() {
        let huge = vec![b'x'; MAX_INPUT_BYTES + 1];
        let err = parse(&huge).unwrap_err();
        assert!(matches!(err, ParseError::TooLarge { .. }));
    }

    #[test]
    fn status_normalization() {
        assert_eq!(normalize_status(Some("Continuing")), "continuing");
        assert_eq!(normalize_status(Some("Ended")), "ended");
        assert_eq!(normalize_status(Some("Completed")), "ended");
        assert_eq!(normalize_status(Some("Cancelled")), "cancelled");
        assert_eq!(normalize_status(Some("Canceled")), "cancelled");
        assert_eq!(normalize_status(Some("On Hiatus")), "hiatus");
        assert_eq!(normalize_status(None), "continuing");
        assert_eq!(normalize_status(Some("???")), "continuing");
    }
}
