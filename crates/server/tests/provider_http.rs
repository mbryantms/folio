//! Shared provider HTTP layer (WP-2.9) — retry, `Retry-After`, body
//! cap — exercised against wiremock both directly
//! (`http::send_with_retry`) and through the Metron client so the
//! classification on top of the shared layer is covered too.
//!
//! Coverage:
//! - `Retry-After: 7` on a 429 lands in `QuotaExceeded { 7 }`
//! - `Retry-After` HTTP-date is parsed relative to now
//! - 503, 503, 200 succeeds (bounded retry)
//! - 500 ×4 exhausts to `Upstream`, exactly four requests sent
//! - 4xx is never retried
//! - a retry whose backoff passes the deadline is not attempted
//! - bodies over the cap are rejected without a retry

mod common;

use common::TestApp;
use serde_json::json;
use server::metadata::http::{self, RequestOpts, RetryPolicy};
use server::metadata::metron::MetronClient;
use server::metadata::provider::{MetadataProvider, ProviderError, SeriesQuery};
use std::time::{Duration, Instant};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

fn fast_policy() -> RetryPolicy {
    RetryPolicy {
        max_retries: 3,
        base: Duration::from_millis(1),
        cap: Duration::from_millis(5),
    }
}

fn no_redact(s: &str) -> String {
    s.to_owned()
}

fn empty_series_page() -> serde_json::Value {
    json!({ "count": 0, "next": null, "previous": null, "results": [] })
}

async fn metron_search(client: &MetronClient) -> Result<usize, ProviderError> {
    client
        .search_series(&SeriesQuery {
            name: "x".into(),
            year: None,
            publisher: None,
            limit: 1,
        })
        .await
        .map(|v| v.len())
}

// ────────────────────── send_with_retry directly ──────────────────────

#[tokio::test]
async fn transient_503_then_200_succeeds() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/thing"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .expect(2)
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/thing"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount(&mock)
        .await;

    let client = reqwest::Client::new();
    let url = format!("{}/thing", mock.uri());
    let opts = RequestOpts {
        policy: fast_policy(),
        ..Default::default()
    };
    let resp = http::send_with_retry(|| client.get(&url), &opts, &no_redact)
        .await
        .expect("third attempt succeeds");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.text(), "ok");
}

#[tokio::test]
async fn server_errors_exhaust_to_upstream_after_four_attempts() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .expect(4)
        .mount(&mock)
        .await;

    let client = reqwest::Client::new();
    let url = format!("{}/thing", mock.uri());
    let opts = RequestOpts {
        policy: fast_policy(),
        ..Default::default()
    };
    let err = http::send_with_retry(|| client.get(&url), &opts, &no_redact)
        .await
        .expect_err("500 x4 fails");
    match err {
        ProviderError::Upstream(msg) => {
            assert!(msg.contains("500"), "message names the status: {msg}");
            assert!(
                msg.contains("boom"),
                "message carries the body snippet: {msg}"
            );
        }
        other => panic!("expected Upstream, got {other:?}"),
    }
    // `.expect(4)` on the mock is verified at drop — 1 initial + 3 retries.
}

#[tokio::test]
async fn client_errors_are_not_retried() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404).set_body_string("nope"))
        .expect(1)
        .mount(&mock)
        .await;

    let client = reqwest::Client::new();
    let url = format!("{}/thing", mock.uri());
    let opts = RequestOpts {
        policy: fast_policy(),
        ..Default::default()
    };
    // 4xx comes back as Ok — the caller classifies it — and the mock's
    // `.expect(1)` proves no retry fired.
    let resp = http::send_with_retry(|| client.get(&url), &opts, &no_redact)
        .await
        .expect("4xx is a response, not a failure");
    assert_eq!(resp.status, 404);
}

#[tokio::test]
async fn retry_is_skipped_when_backoff_would_pass_deadline() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&mock)
        .await;

    let client = reqwest::Client::new();
    let url = format!("{}/thing", mock.uri());
    let opts = RequestOpts {
        policy: RetryPolicy {
            max_retries: 3,
            base: Duration::from_secs(2),
            cap: Duration::from_secs(5),
        },
        // Already-expired deadline: the first retry's 1–2 s backoff can't
        // fit, so the loop gives up after the initial attempt.
        deadline: Some(Instant::now()),
        ..Default::default()
    };
    let started = Instant::now();
    let err = http::send_with_retry(|| client.get(&url), &opts, &no_redact)
        .await
        .expect_err("deadline stops the retry");
    assert!(matches!(err, ProviderError::Upstream(_)), "{err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "no backoff sleep should have happened"
    );
}

#[tokio::test]
async fn oversized_body_is_rejected_without_retry() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(100)))
        .expect(1)
        .mount(&mock)
        .await;

    let client = reqwest::Client::new();
    let url = format!("{}/thing", mock.uri());
    let opts = RequestOpts {
        policy: fast_policy(),
        max_body_bytes: 16,
        ..Default::default()
    };
    let err = http::send_with_retry(|| client.get(&url), &opts, &no_redact)
        .await
        .expect_err("body over cap");
    assert!(matches!(err, ProviderError::InvalidResponse(_)), "{err:?}");
}

// ────────────────────── through the Metron client ──────────────────────

#[tokio::test]
async fn metron_429_honours_retry_after_seconds() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "7")
                .set_body_string("throttled"),
        )
        .expect(1)
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let client = MetronClient::with_base_url("u", "p", mock.uri(), app.state().jobs.redis.clone());
    let err = metron_search(&client).await.expect_err("429");
    assert!(
        matches!(
            err,
            ProviderError::QuotaExceeded {
                retry_after_secs: 7
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn metron_429_parses_http_date_retry_after() {
    let mock = MockServer::start().await;
    let at = (chrono::Utc::now() + chrono::Duration::seconds(90))
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string();
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", at.as_str()))
        .expect(1)
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let client = MetronClient::with_base_url("u", "p", mock.uri(), app.state().jobs.redis.clone());
    let err = metron_search(&client).await.expect_err("429");
    match err {
        ProviderError::QuotaExceeded { retry_after_secs } => {
            assert!(
                (80..=91).contains(&retry_after_secs),
                "HTTP-date parsed relative to now: {retry_after_secs}"
            );
        }
        other => panic!("expected QuotaExceeded, got {other:?}"),
    }
}

#[tokio::test]
async fn metron_retries_a_transient_5xx_then_succeeds() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/series/"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .expect(1)
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/series/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_series_page()))
        .expect(1)
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let client = MetronClient::with_base_url("u", "p", mock.uri(), app.state().jobs.redis.clone());
    let n = metron_search(&client)
        .await
        .expect("second attempt succeeds");
    assert_eq!(n, 0);
}

#[tokio::test]
async fn metron_404_is_not_retried() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404).set_body_string("missing"))
        .expect(1)
        .mount(&mock)
        .await;

    let app = TestApp::spawn_with_metron("u", "p", true).await;
    let client = MetronClient::with_base_url("u", "p", mock.uri(), app.state().jobs.redis.clone());
    let err = metron_search(&client).await.expect_err("404");
    assert!(matches!(err, ProviderError::NotFound(_)), "{err:?}");
}
