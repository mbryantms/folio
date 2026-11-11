//! `/csp-report` — CSP violation ingestion (§17.4).
//!
//! Browsers POST `application/csp-report` (legacy) or `application/reports+json` (modern).
//! We accept either by reading the raw body and parsing as JSON, then log a
//! few selected, sanitised fields per violation at `warn` and increment
//! `folio_csp_violations_total`. The route is unauthenticated and
//! CSRF-exempt, so the body is capped at [`CSP_REPORT_BODY_LIMIT`] and never
//! logged as-is: URLs lose their query string and fragment (a reset link's
//! token lives there) and every value is length-capped. Bodies in neither
//! shape only bump `folio_csp_reports_rejected_total`.

use axum::{
    body::Bytes, extract::DefaultBodyLimit, extract::State, http::StatusCode,
    response::IntoResponse,
};
use serde_json::Value;
use utoipa_axum::router::OpenApiRouter;

use crate::middleware::rate_limit;
use crate::state::AppState;
use server_macros::handler;

/// Body limit for `POST /csp-report`, well under the group's 2 MiB
/// `app::DEFAULT_BODY_LIMIT`. A real report is a few hundred bytes; a
/// Reporting API batch of [`MAX_REPORTS_PER_REQUEST`] stays far below this.
pub const CSP_REPORT_BODY_LIMIT: usize = 64 * 1024;

/// Violations logged per request; the rest of a batch is ignored.
const MAX_REPORTS_PER_REQUEST: usize = 20;
/// Cap for URL-ish values (after the query string and fragment are dropped).
const MAX_URL_CHARS: usize = 256;
/// Cap for directive and disposition values.
const MAX_TOKEN_CHARS: usize = 128;

pub fn routes() -> OpenApiRouter<AppState> {
    // CSP report endpoint isn't in the spec — the body shape is
    // browser-defined and varies by user-agent. Register via plain
    // `.route()` so we still apply the per-route rate-limit layer
    // without forcing a `#[utoipa::path]` over a polymorphic body.
    OpenApiRouter::new()
        .route(
            "/csp-report",
            axum::routing::post(csp_report).route_layer(rate_limit::CSP_REPORT.build()),
        )
        .layer(DefaultBodyLimit::max(CSP_REPORT_BODY_LIMIT))
}

/// One violation, reduced to the fields we log. Every value is sanitised.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CspViolation {
    pub document_url: Option<String>,
    pub directive: Option<String>,
    pub blocked_url: Option<String>,
    pub source_file: Option<String>,
    pub line: Option<String>,
    pub disposition: Option<String>,
}

/// Parse a report body. `None` = not JSON, or JSON in neither browser
/// shape. `Some(vec)` holds at most [`MAX_REPORTS_PER_REQUEST`] violations
/// (possibly empty, when a Reporting API batch has no `csp-violation`).
pub fn summarize_reports(body: &[u8]) -> Option<Vec<CspViolation>> {
    let value: Value = serde_json::from_slice(body).ok()?;
    match value {
        Value::Object(ref map) if map.contains_key("csp-report") => {
            let report = map.get("csp-report")?.as_object()?;
            Some(vec![legacy_violation(report)])
        }
        Value::Object(ref map) if map.contains_key("type") => {
            Some(reporting_api_violation(map).into_iter().collect())
        }
        Value::Array(entries) => {
            if !entries.iter().all(Value::is_object) {
                return None;
            }
            Some(
                entries
                    .iter()
                    .filter_map(Value::as_object)
                    .filter_map(reporting_api_violation)
                    .take(MAX_REPORTS_PER_REQUEST)
                    .collect(),
            )
        }
        _ => None,
    }
}

type JsonMap = serde_json::Map<String, Value>;

fn legacy_violation(r: &JsonMap) -> CspViolation {
    CspViolation {
        document_url: url_field(r, "document-uri"),
        directive: token_field(r, "effective-directive")
            .or_else(|| token_field(r, "violated-directive")),
        blocked_url: url_field(r, "blocked-uri"),
        source_file: url_field(r, "source-file"),
        line: scalar(r.get("line-number")).map(|s| truncate_clean(&s, MAX_TOKEN_CHARS)),
        disposition: token_field(r, "disposition"),
    }
}

fn reporting_api_violation(entry: &JsonMap) -> Option<CspViolation> {
    if entry.get("type").and_then(Value::as_str) != Some("csp-violation") {
        return None;
    }
    let empty = JsonMap::new();
    let body = entry
        .get("body")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    Some(CspViolation {
        document_url: url_field(body, "documentURL").or_else(|| url_field(entry, "url")),
        directive: token_field(body, "effectiveDirective")
            .or_else(|| token_field(body, "violatedDirective")),
        blocked_url: url_field(body, "blockedURL"),
        source_file: url_field(body, "sourceFile"),
        line: scalar(body.get("lineNumber")).map(|s| truncate_clean(&s, MAX_TOKEN_CHARS)),
        disposition: token_field(body, "disposition"),
    })
}

/// Strings and numbers only; objects, arrays, bools and null are dropped.
fn scalar(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn url_field(map: &JsonMap, key: &str) -> Option<String> {
    scalar(map.get(key)).map(|s| sanitize_url(&s))
}

fn token_field(map: &JsonMap, key: &str) -> Option<String> {
    scalar(map.get(key)).map(|s| truncate_clean(&s, MAX_TOKEN_CHARS))
}

/// Drop the query string and fragment, then cap and clean.
fn sanitize_url(s: &str) -> String {
    let end = s.find(['?', '#']).unwrap_or(s.len());
    truncate_clean(&s[..end], MAX_URL_CHARS)
}

/// First `max` chars, with characters that could forge or disguise a log
/// line replaced by `?`.
fn truncate_clean(s: &str, max: usize) -> String {
    s.chars()
        .take(max)
        .map(|c| if is_unsafe_log_char(c) { '?' } else { c })
        .collect()
}

/// Control characters, the Unicode line and paragraph separators, and the
/// bidirectional marks and overrides that can make a logged URL display as
/// something else in the admin log viewer.
fn is_unsafe_log_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}

// Note: no #[utoipa::path] — the body shape is browser-defined and varies by user-agent.
#[handler]
pub async fn csp_report(State(_state): State<AppState>, body: Bytes) -> impl IntoResponse {
    let Some(violations) = summarize_reports(&body) else {
        metrics::counter!("folio_csp_reports_rejected_total").increment(1);
        tracing::debug!(bytes = body.len(), "csp report rejected: unrecognised body");
        return StatusCode::NO_CONTENT;
    };
    for v in violations {
        metrics::counter!("folio_csp_violations_total").increment(1);
        tracing::warn!(
            document_url = v.document_url.as_deref().unwrap_or(""),
            directive = v.directive.as_deref().unwrap_or(""),
            blocked_url = v.blocked_url.as_deref().unwrap_or(""),
            source_file = v.source_file.as_deref().unwrap_or(""),
            line = v.line.as_deref().unwrap_or(""),
            disposition = v.disposition.as_deref().unwrap_or(""),
            "csp violation"
        );
    }
    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn summarize(v: &Value) -> Option<Vec<CspViolation>> {
        summarize_reports(&serde_json::to_vec(v).unwrap())
    }

    #[test]
    fn legacy_shape() {
        let got = summarize(&json!({"csp-report": {
            "document-uri": "https://comics.example.com/read",
            "violated-directive": "script-src-elem",
            "effective-directive": "script-src",
            "blocked-uri": "https://evil.example.com/x.js",
            "source-file": "https://comics.example.com/app.js",
            "line-number": 12,
            "column-number": 4,
            "disposition": "enforce",
            "original-policy": "default-src 'self'",
            "script-sample": "alert(1)",
            "referrer": "https://ref.example.com/"
        }}))
        .unwrap();
        assert_eq!(
            got,
            vec![CspViolation {
                document_url: Some("https://comics.example.com/read".into()),
                directive: Some("script-src".into()),
                blocked_url: Some("https://evil.example.com/x.js".into()),
                source_file: Some("https://comics.example.com/app.js".into()),
                line: Some("12".into()),
                disposition: Some("enforce".into()),
            }]
        );
        let dump = format!("{got:?}");
        assert!(!dump.contains("alert(1)") && !dump.contains("ref.example.com"));
        assert!(!dump.contains("default-src"));
    }

    #[test]
    fn legacy_falls_back_to_violated_directive() {
        let got = summarize(&json!({"csp-report": {"violated-directive": "img-src"}})).unwrap();
        assert_eq!(got[0].directive.as_deref(), Some("img-src"));
    }

    #[test]
    fn reporting_api_array_ignores_other_types() {
        let got = summarize(&json!([
            {"type": "csp-violation", "url": "https://comics.example.com/a",
             "body": {"documentURL": "https://comics.example.com/a",
                      "effectiveDirective": "img-src",
                      "blockedURL": "data", "sourceFile": "https://comics.example.com/b.js",
                      "lineNumber": 3, "columnNumber": 9,
                      "disposition": "report", "statusCode": 200,
                      "sample": "secret-sample", "referrer": "https://r.example.com/"}},
            {"type": "deprecation", "body": {"id": "x"}}
        ]))
        .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].directive.as_deref(), Some("img-src"));
        assert_eq!(got[0].blocked_url.as_deref(), Some("data"));
        assert_eq!(got[0].line.as_deref(), Some("3"));
        assert_eq!(got[0].disposition.as_deref(), Some("report"));
        let dump = format!("{got:?}");
        assert!(!dump.contains("secret-sample") && !dump.contains("r.example.com"));
    }

    #[test]
    fn reporting_api_single_object() {
        let got = summarize(&json!({"type": "csp-violation",
            "url": "https://comics.example.com/x",
            "body": {"effectiveDirective": "style-src"}}))
        .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].document_url.as_deref(),
            Some("https://comics.example.com/x")
        );
        assert_eq!(got[0].directive.as_deref(), Some("style-src"));
    }

    #[test]
    fn single_non_violation_object_yields_nothing() {
        let got = summarize(&json!({"type": "intervention", "body": {}})).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn non_json_and_unknown_shapes_rejected() {
        assert_eq!(summarize_reports(b"not json at all"), None);
        assert_eq!(summarize_reports(&[0xff, 0xfe, 0x00]), None);
        assert_eq!(summarize(&json!({"hello": "world"})), None);
        assert_eq!(summarize(&json!("a string")), None);
        assert_eq!(summarize(&json!([1, 2, 3])), None);
        assert_eq!(summarize(&json!({"csp-report": "not an object"})), None);
    }

    #[test]
    fn at_most_twenty_reports() {
        let entries: Vec<Value> = (0..50)
            .map(|i| json!({"type": "csp-violation", "body": {"lineNumber": i}}))
            .collect();
        let got = summarize(&Value::Array(entries)).unwrap();
        assert_eq!(got.len(), MAX_REPORTS_PER_REQUEST);
        assert_eq!(got[19].line.as_deref(), Some("19"));
    }

    #[test]
    fn query_string_and_fragment_dropped() {
        let url = "https://comics.example.com/reset-password?token=SECRET#frag";
        let got = summarize(&json!({"csp-report": {
            "document-uri": url, "blocked-uri": url, "source-file": url
        }}))
        .unwrap();
        assert_eq!(
            got[0].document_url.as_deref(),
            Some("https://comics.example.com/reset-password")
        );
        let dump = format!("{got:?}");
        assert!(!dump.contains("SECRET") && !dump.contains("frag"), "{dump}");

        let got = summarize(&json!([{"type": "csp-violation", "url": url,
            "body": {"blockedURL": url}}]))
        .unwrap();
        let dump = format!("{got:?}");
        assert!(!dump.contains("SECRET"), "{dump}");
    }

    #[test]
    fn long_values_truncated() {
        let long_url = format!("https://comics.example.com/{}", "a".repeat(5000));
        let got = summarize(&json!({"csp-report": {
            "document-uri": long_url,
            "effective-directive": "d".repeat(5000),
            "disposition": "e".repeat(5000),
        }}))
        .unwrap();
        assert_eq!(
            got[0].document_url.as_ref().unwrap().chars().count(),
            MAX_URL_CHARS
        );
        assert_eq!(
            got[0].directive.as_ref().unwrap().chars().count(),
            MAX_TOKEN_CHARS
        );
        assert_eq!(
            got[0].disposition.as_ref().unwrap().chars().count(),
            MAX_TOKEN_CHARS
        );
    }

    #[test]
    fn control_characters_replaced() {
        let got = summarize(&json!({"csp-report": {
            "document-uri": "https://comics.example.com/a\nFAKE LOG LINE\u{1b}[31m",
            "effective-directive": "script-src\r\n",
        }}))
        .unwrap();
        let doc = got[0].document_url.as_deref().unwrap();
        assert!(!doc.chars().any(char::is_control), "{doc:?}");
        assert_eq!(doc, "https://comics.example.com/a?FAKE LOG LINE?[31m");
        assert_eq!(got[0].directive.as_deref(), Some("script-src??"));
    }

    #[test]
    fn bidi_marks_and_line_separators_replaced() {
        let got = summarize(&json!({"csp-report": {
            "document-uri": "https://comics.example.com/a\u{202e}gnp.exe\u{2028}x\u{2066}y",
            "effective-directive": "script-src\u{200f}",
        }}))
        .unwrap();
        assert_eq!(
            got[0].document_url.as_deref(),
            Some("https://comics.example.com/a?gnp.exe?x?y")
        );
        assert_eq!(got[0].directive.as_deref(), Some("script-src?"));
    }

    #[test]
    fn non_scalar_values_dropped() {
        let got = summarize(&json!({"csp-report": {
            "document-uri": {"nested": "x"},
            "blocked-uri": ["a"],
            "line-number": true,
            "disposition": null,
        }}))
        .unwrap();
        assert_eq!(got, vec![CspViolation::default()]);
    }
}
