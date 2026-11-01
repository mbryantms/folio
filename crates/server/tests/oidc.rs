//! OIDC integration tests (M6, audit M-4 / M-5 / M-7 / M-12 / M-13).
//!
//! Spins up a wiremock-backed mock OP for each test and drives the full
//! `/auth/oidc/start` → `/auth/oidc/callback` round-trip. ID tokens are
//! signed with an RSA-2048 keypair generated once per test process and
//! served via the mock OP's JWKS endpoint.
//!
//! Covers:
//!   - happy path (callback upserts the user + sets session cookies)
//!   - state-cookie mismatch (400 before discovery)
//!   - missing state cookie (400 before discovery)
//!   - email_verified=false → 403 auth.email_unverified
//!   - email_verified missing + trust flag off → 403 (no email_present
//!     in claims so the body is `email_unverified=false → !email_present`
//!     short-circuit; covered by a dedicated test)
//!   - email collision with an existing local user → 409 auth.email_in_use
//!   - public /auth/config returns oidc_enabled: true when configured
//!   - RP-initiated logout 302s to the issuer's end_session_endpoint
//!
//! NOTE: full integration tests (testcontainers Postgres + wiremock OP)
//! are slow; expect ~30s warmup per test. Run a single test via
//! `cargo test -p server --test oidc <name>`.

mod common;

use std::sync::OnceLock;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64URL};
use common::TestApp;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::{
    RsaPrivateKey, RsaPublicKey, pkcs1::EncodeRsaPrivateKey, pkcs8::LineEnding,
    traits::PublicKeyParts,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower::ServiceExt;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, method, path},
};

// ───────────────────── RSA key fixture ─────────────────────
//
// Generated once per test binary, shared across tests via OnceLock. Pem
// is consumed by jsonwebtoken to sign; modulus + exponent are exposed as
// a JWK from the mock OP. `kid = "test-key-1"` lets openidconnect's
// id-token verifier find the right key.

struct TestKeyMaterial {
    pem: String,
    n_b64url: String,
    e_b64url: String,
}

fn key_material() -> &'static TestKeyMaterial {
    static MAT: OnceLock<TestKeyMaterial> = OnceLock::new();
    MAT.get_or_init(|| {
        // `rsa` 0.9 wants a rand_core 0.6 RNG; workspace rand moved to the
        // 0.10 core, so use the OsRng that rsa itself re-exports.
        let mut rng = rsa::rand_core::OsRng;
        let priv_key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
        let pub_key = RsaPublicKey::from(&priv_key);
        let pem = priv_key
            .to_pkcs1_pem(LineEnding::LF)
            .expect("encode pkcs1 pem")
            .to_string();
        let n_b64url = B64URL.encode(pub_key.n().to_bytes_be());
        let e_b64url = B64URL.encode(pub_key.e().to_bytes_be());
        TestKeyMaterial {
            pem,
            n_b64url,
            e_b64url,
        }
    })
}

const KID: &str = "test-key-1";

// ───────────────────── ID-token claim shape ─────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct IdTokenClaims {
    iss: String,
    sub: String,
    aud: String,
    exp: i64,
    iat: i64,
    nonce: String,
    email: Option<String>,
    email_verified: Option<bool>,
    preferred_username: Option<String>,
    name: Option<String>,
}

impl IdTokenClaims {
    fn standard(issuer: &str, audience: &str, subject: &str, nonce: &str, email: &str) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            iss: issuer.to_string(),
            sub: subject.to_string(),
            aud: audience.to_string(),
            exp: now + 3600,
            iat: now,
            nonce: nonce.to_string(),
            email: Some(email.to_string()),
            email_verified: Some(true),
            preferred_username: Some("Test User".into()),
            name: Some("Test User".into()),
        }
    }
}

fn sign_id_token(claims: &IdTokenClaims) -> String {
    let mat = key_material();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(KID.into());
    let key = EncodingKey::from_rsa_pem(mat.pem.as_bytes()).expect("encoding key");
    encode(&header, claims, &key).expect("encode jwt")
}

// ───────────────────── Mock OP ─────────────────────

struct MockOp {
    server: MockServer,
}

impl MockOp {
    async fn start() -> Self {
        let server = MockServer::start().await;
        let issuer = server.uri();

        // Discovery doc — Folio handlers consume `authorization_endpoint`,
        // `token_endpoint`, `jwks_uri`, and (separately) read
        // `end_session_endpoint` via the side-fetch in `oidc.rs`.
        let discovery = json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
            "jwks_uri": format!("{issuer}/jwks"),
            "end_session_endpoint": format!("{issuer}/end_session"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["RS256"],
            "scopes_supported": ["openid", "email", "profile"],
            "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
            "claims_supported": ["sub", "email", "email_verified", "name"],
        });
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(discovery))
            .mount(&server)
            .await;

        // JWKS — single RSA key matching our test signer.
        let mat = key_material();
        let jwks = json!({
            "keys": [{
                "kty": "RSA",
                "use": "sig",
                "kid": KID,
                "alg": "RS256",
                "n": mat.n_b64url,
                "e": mat.e_b64url,
            }]
        });
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(jwks))
            .mount(&server)
            .await;

        Self { server }
    }

    fn issuer(&self) -> String {
        self.server.uri()
    }

    /// Register a stub `POST /token` that returns the provided id_token +
    /// a throwaway access_token. Folio's callback ignores access tokens
    /// for the id_token flow but openidconnect requires the field.
    async fn register_token_response(&self, id_token: &str) {
        let body = json!({
            "access_token": "test-access-token",
            "token_type": "Bearer",
            "expires_in": 3600,
            "id_token": id_token,
        });
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    async fn register_token_response_for_code(&self, code: &str, id_token: &str) {
        let body = json!({
            "access_token": "test-access-token",
            "token_type": "Bearer",
            "expires_in": 3600,
            "id_token": id_token,
        });
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains(format!("code={code}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }
}

// ───────────────────── Common helpers ─────────────────────

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

fn extract_set_cookie(resp: &Response<Body>, name: &str) -> Option<String> {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|s| {
            let prefix = format!("{name}=");
            s.split(';')
                .next()
                .and_then(|kv| kv.strip_prefix(&prefix))
                .map(|v| v.to_owned())
        })
}

/// Issue a `/auth/oidc/start` and pull the state cookie + state param out.
/// `redirect_after` is forwarded; pass `None` for the default-/ flow.
async fn start_oidc_flow(app: &TestApp, redirect_after: Option<&str>) -> (String, String) {
    let uri = match redirect_after {
        Some(r) => format!("/auth/oidc/start?redirect_after={}", urlencoding::encode(r)),
        None => "/auth/oidc/start".to_string(),
    };
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER, "/start should 302");
    // Pull the state param out of the Location header to feed back to /callback.
    let location = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .expect("Location header")
        .to_string();
    let url = url::Url::parse(&location).expect("parse auth url");
    let state_param = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .expect("state query param");
    let state_cookie = extract_set_cookie(&resp, &attempt_cookie_name(&state_param))
        .expect("per-attempt state cookie present after /start");
    assert!(
        extract_set_cookie(&resp, LEGACY_SINGLE_STATE_COOKIE).is_none(),
        "/start must not set the shared pre-2026-11 cookie any more"
    );
    (state_cookie, state_param)
}

/// Pre-2026-11 single state cookie; still accepted by /callback as a fallback.
const LEGACY_SINGLE_STATE_COOKIE: &str = "__Host-comic_oidc";

/// Mirrors `oidc::attempt_cookie_name`: one state cookie per `/start`, keyed by
/// the first 16 chars of its state.
fn attempt_cookie_name(state: &str) -> String {
    format!("__Host-comic_oidc_{}", &state[..16])
}

/// Cookie name the browser would send `state_cookie` under for `state_param`.
/// Deliberately malformed states (shorter than an attempt id) fall back to the
/// legacy name so mismatch tests still exercise the state comparison.
fn cookie_name_for(state_param: &str) -> String {
    let well_formed = state_param.len() >= 16
        && state_param[..16]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if well_formed {
        attempt_cookie_name(state_param)
    } else {
        LEGACY_SINGLE_STATE_COOKIE.to_string()
    }
}

fn parse_state_cookie_nonce(state_cookie: &str) -> String {
    // urldecode + json-parse the signed payload shape set by oidc::start.
    let decoded = urlencoding::decode(state_cookie).expect("decode state cookie");
    let signed: serde_json::Value = serde_json::from_str(&decoded).expect("signed state json");
    let payload = signed["payload"].as_str().expect("payload");
    let v: serde_json::Value = serde_json::from_str(payload).expect("state payload json");
    v["nonce"].as_str().expect("nonce").to_string()
}

async fn callback_with(
    app: &TestApp,
    state_cookie: &str,
    state_param: &str,
    code: &str,
) -> Response<Body> {
    let uri = format!(
        "/auth/oidc/callback?code={}&state={}",
        urlencoding::encode(code),
        urlencoding::encode(state_param)
    );
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(
                    header::COOKIE,
                    format!("{}={state_cookie}", cookie_name_for(state_param)),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

/// /callback with an explicit Cookie header (several cookies, custom names).
async fn callback_with_cookie_header(
    app: &TestApp,
    cookie_header: &str,
    state_param: &str,
    code: &str,
) -> Response<Body> {
    let uri = format!(
        "/auth/oidc/callback?code={}&state={}",
        urlencoding::encode(code),
        urlencoding::encode(state_param)
    );
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::COOKIE, cookie_header)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

// ───────────────────── Tests ─────────────────────

#[tokio::test]
async fn public_auth_config_advertises_oidc_when_configured() {
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc(op.issuer(), false).await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/auth/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["auth_mode"], "both");
    assert_eq!(body["oidc_enabled"], true);
    assert_eq!(body["registration_open"], true);
    assert!(
        body["client_id"].is_null(),
        "public surface must not leak client_id"
    );
    // Security-audit M-2 (WP-6.3 status check): the anonymous surface must
    // not reveal the IdP at all — not as `issuer`, nor embedded anywhere
    // else in the body.
    let issuer = op.issuer();
    let raw = body.to_string();
    assert!(
        !raw.contains(issuer.trim_end_matches('/')),
        "public /auth/config leaked the OIDC issuer: {raw}"
    );
}

#[tokio::test]
async fn callback_rejects_state_mismatch() {
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc(op.issuer(), false).await;
    let (state_cookie, _state_param) = start_oidc_flow(&app, None).await;

    let resp = callback_with(&app, &state_cookie, "tampered-state", "any-code").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.invalid");
}

#[tokio::test]
async fn callback_rejects_tampered_state_cookie() {
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc(op.issuer(), false).await;
    let (state_cookie, state_param) = start_oidc_flow(&app, None).await;
    let tampered_cookie = format!("{state_cookie}x");

    let resp = callback_with(&app, &tampered_cookie, &state_param, "any-code").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.invalid");
}

#[tokio::test]
async fn callback_rejects_missing_state_cookie() {
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc(op.issuer(), false).await;

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/auth/oidc/callback?code=x&state=y")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn callback_rejects_legacy_state_cookie_name() {
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc(op.issuer(), false).await;
    let (state_cookie, state_param) = start_oidc_flow(&app, None).await;
    let uri = format!(
        "/auth/oidc/callback?code=x&state={}",
        urlencoding::encode(&state_param)
    );

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(
                    header::COOKIE,
                    format!("__Secure-comic_oidc={state_cookie}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn callback_happy_path_upserts_user_and_sets_cookies() {
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc(op.issuer(), false).await;
    let (state_cookie, state_param) = start_oidc_flow(&app, None).await;
    let nonce = parse_state_cookie_nonce(&state_cookie);

    let claims = IdTokenClaims::standard(
        &op.issuer(),
        "folio-test-client",
        "subject-1",
        &nonce,
        "happy@example.com",
    );
    let id_token = sign_id_token(&claims);
    op.register_token_response(&id_token).await;

    let resp = callback_with(&app, &state_cookie, &state_param, "auth-code-xyz").await;
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "callback should 302 to redirect_after on success"
    );
    assert!(extract_set_cookie(&resp, "__Host-comic_session").is_some());
    assert!(extract_set_cookie(&resp, "__Host-comic_refresh").is_some());

    // User row must exist with the right external_id.
    use sea_orm::EntityTrait;
    let state = app.state();
    let users = entity::user::Entity::find().all(&state.db).await.unwrap();
    let oidc_user = users
        .iter()
        .find(|u| u.external_id.starts_with("oidc:"))
        .expect("oidc user upserted");
    assert_eq!(oidc_user.email.as_deref(), Some("happy@example.com"));
    assert!(oidc_user.email_verified);
    assert_eq!(
        oidc_user.role,
        entity::user::UserRole::Admin,
        "first OIDC-created user should still receive first-admin bootstrap"
    );
}

/// Regression (2026-11-01, Safari): one click produced several `/start`
/// requests. Each used to overwrite the single state cookie, so the callback —
/// carrying the FIRST attempt's state — failed with "oidc state mismatch".
/// With per-attempt cookies the browser holds both and the first one matches.
#[tokio::test]
async fn callback_accepts_first_of_two_concurrent_starts() {
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc(op.issuer(), false).await;
    let (cookie_a, state_a) = start_oidc_flow(&app, None).await;
    let (cookie_b, state_b) = start_oidc_flow(&app, None).await;
    assert_ne!(state_a, state_b, "each /start mints its own state");
    assert_ne!(attempt_cookie_name(&state_a), attempt_cookie_name(&state_b));

    let nonce = parse_state_cookie_nonce(&cookie_a);
    let claims = IdTokenClaims::standard(
        &op.issuer(),
        "folio-test-client",
        "subject-dup",
        &nonce,
        "dup@example.com",
    );
    op.register_token_response(&sign_id_token(&claims)).await;

    // The browser sends every cookie it holds; the callback carries attempt A.
    let header = format!(
        "{}={cookie_a}; {}={cookie_b}",
        attempt_cookie_name(&state_a),
        attempt_cookie_name(&state_b)
    );
    let resp = callback_with_cookie_header(&app, &header, &state_a, "auth-code-dup").await;
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "first attempt signs in"
    );
    assert!(extract_set_cookie(&resp, "__Host-comic_session").is_some());
    // The used attempt cookie is cleared; the other expires on its own TTL.
    assert_eq!(
        extract_set_cookie(&resp, &attempt_cookie_name(&state_a)).as_deref(),
        Some(""),
        "the matched attempt cookie is cleared on success"
    );
}

/// A flow started on the previous build (single shared cookie) still finishes.
#[tokio::test]
async fn callback_accepts_legacy_single_state_cookie() {
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc(op.issuer(), false).await;
    let (state_cookie, state_param) = start_oidc_flow(&app, None).await;
    let nonce = parse_state_cookie_nonce(&state_cookie);
    let claims = IdTokenClaims::standard(
        &op.issuer(),
        "folio-test-client",
        "subject-legacy",
        &nonce,
        "legacy@example.com",
    );
    op.register_token_response(&sign_id_token(&claims)).await;

    let header = format!("{LEGACY_SINGLE_STATE_COOKIE}={state_cookie}");
    let resp = callback_with_cookie_header(&app, &header, &state_param, "auth-code-legacy").await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert!(extract_set_cookie(&resp, "__Host-comic_session").is_some());
}

/// A state that cannot name an attempt cookie (wrong alphabet) is never used
/// to build a cookie name; with no legacy cookie either, the flow is rejected.
#[tokio::test]
async fn callback_rejects_state_outside_cookie_name_alphabet() {
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc(op.issuer(), false).await;
    let (state_cookie, state_param) = start_oidc_flow(&app, None).await;
    let evil_state = format!("a;b=c\r\nX-Evil:{}", &state_param[..4]);
    let header = format!("{}={state_cookie}", attempt_cookie_name(&state_param));
    let resp = callback_with_cookie_header(&app, &header, &evil_state, "any-code").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.invalid");
}

#[tokio::test]
async fn concurrent_first_oidc_signups_create_only_one_admin() {
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc(op.issuer(), false).await;
    let mut flows = Vec::new();

    for idx in 0..4 {
        let (state_cookie, state_param) = start_oidc_flow(&app, None).await;
        let nonce = parse_state_cookie_nonce(&state_cookie);
        let code = format!("auth-code-{idx}");
        let claims = IdTokenClaims::standard(
            &op.issuer(),
            "folio-test-client",
            &format!("subject-{idx}"),
            &nonce,
            &format!("oidc-race-{idx}@example.com"),
        );
        let id_token = sign_id_token(&claims);
        op.register_token_response_for_code(&code, &id_token).await;
        flows.push((state_cookie, state_param, code));
    }

    let responses =
        futures::future::join_all(flows.iter().map(|(state_cookie, state_param, code)| {
            callback_with(&app, state_cookie, state_param, code)
        }))
        .await;

    for resp in responses {
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    }

    use sea_orm::EntityTrait;
    let users = entity::user::Entity::find()
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(users.len(), 4);
    assert_eq!(
        users
            .iter()
            .filter(|u| u.role == entity::user::UserRole::Admin)
            .count(),
        1,
        "exactly one concurrent OIDC signup should receive first-admin bootstrap"
    );
    assert_eq!(
        users
            .iter()
            .filter(|u| u.role == entity::user::UserRole::User)
            .count(),
        3
    );
}

#[tokio::test]
async fn callback_rejects_unverified_email() {
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc(op.issuer(), false).await;
    let (state_cookie, state_param) = start_oidc_flow(&app, None).await;
    let nonce = parse_state_cookie_nonce(&state_cookie);

    let mut claims = IdTokenClaims::standard(
        &op.issuer(),
        "folio-test-client",
        "subject-unverified",
        &nonce,
        "unverified@example.com",
    );
    claims.email_verified = Some(false);
    let id_token = sign_id_token(&claims);
    op.register_token_response(&id_token).await;

    let resp = callback_with(&app, &state_cookie, &state_param, "code").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.email_unverified");
}

#[tokio::test]
async fn callback_returns_409_on_email_collision_with_local_user() {
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc(op.issuer(), false).await;

    // Seed a local user with the same email.
    let reg = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"clash@example.com","password":"correctly-horse-battery"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reg.status(), StatusCode::CREATED);

    let (state_cookie, state_param) = start_oidc_flow(&app, None).await;
    let nonce = parse_state_cookie_nonce(&state_cookie);

    let claims = IdTokenClaims::standard(
        &op.issuer(),
        "folio-test-client",
        "subject-clash",
        &nonce,
        "clash@example.com",
    );
    let id_token = sign_id_token(&claims);
    op.register_token_response(&id_token).await;

    let resp = callback_with(&app, &state_cookie, &state_param, "code").await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.email_in_use");
}

#[tokio::test]
async fn callback_auto_links_local_user_when_opt_in_enabled() {
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc_link_local(op.issuer()).await;

    // Seed a local (password) user with the same email.
    let reg = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"link-me@example.com","password":"correctly-horse-battery"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reg.status(), StatusCode::CREATED);

    use sea_orm::EntityTrait;
    let state = app.state();
    let before = entity::user::Entity::find().all(&state.db).await.unwrap();
    assert_eq!(before.len(), 1, "exactly the seeded local user exists");
    let local = &before[0];
    assert!(local.external_id.starts_with("local:"));
    let local_id = local.id;

    // Drive an OIDC login with a verified email matching the local account.
    let (state_cookie, state_param) = start_oidc_flow(&app, None).await;
    let nonce = parse_state_cookie_nonce(&state_cookie);
    let claims = IdTokenClaims::standard(
        &op.issuer(),
        "folio-test-client",
        "subject-link",
        &nonce,
        "link-me@example.com",
    );
    let id_token = sign_id_token(&claims);
    op.register_token_response(&id_token).await;

    let resp = callback_with(&app, &state_cookie, &state_param, "code").await;
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "auto-link should sign the user in, not 409"
    );
    assert!(extract_set_cookie(&resp, "__Host-comic_session").is_some());

    // The SAME row was relinked — no second user was created, the OIDC
    // identity replaced the `local:` external_id, and the row id is stable.
    let after = entity::user::Entity::find().all(&state.db).await.unwrap();
    assert_eq!(
        after.len(),
        1,
        "no new user row — the local one was relinked"
    );
    let linked = &after[0];
    assert_eq!(linked.id, local_id, "row id preserved across the link");
    assert!(
        linked.external_id.starts_with("oidc:"),
        "external_id now points at the OIDC identity: {}",
        linked.external_id
    );
    assert_eq!(linked.email.as_deref(), Some("link-me@example.com"));
    // First registered user is admin; the role survives the link.
    assert_eq!(linked.role, entity::user::UserRole::Admin);
}

/// Force a user row's state straight in the DB (the OIDC tests have no
/// second admin to do it through the API).
async fn force_state(app: &TestApp, id: uuid::Uuid, state: entity::user::UserState) {
    use sea_orm::{ActiveModelTrait, EntityTrait, IntoActiveModel, Set};
    let row = entity::user::Entity::find_by_id(id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap();
    let mut am = row.into_active_model();
    am.state = Set(state);
    am.update(&app.state().db).await.unwrap();
}

async fn user_row(app: &TestApp, id: uuid::Uuid) -> entity::user::Model {
    use sea_orm::EntityTrait;
    entity::user::Entity::find_by_id(id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
}

/// Register a local account and move it to `pending_verification`, the
/// state an unverified registrant's row sits in. Returns its id.
async fn pending_local_user(app: &TestApp, email: &str) -> uuid::Uuid {
    use sea_orm::EntityTrait;
    let reg = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"registrant-chosen-pw"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reg.status(), StatusCode::CREATED);
    let id = entity::user::Entity::find()
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .find(|u| u.email.as_deref() == Some(email))
        .unwrap()
        .id;
    force_state(app, id, entity::user::UserState::PendingVerification).await;
    id
}

async fn local_login_status(app: &TestApp, email: &str, password: &str) -> StatusCode {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "email": email, "password": password }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn me_status(app: &TestApp, session: &str) -> StatusCode {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/auth/me")
                .header(header::AUTHORIZATION, format!("Bearer {session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// Run one OIDC sign-in for `subject` / `email`; `verified` is the ID
/// token's `email_verified` claim (`None` omits it).
async fn oidc_sign_in(
    app: &TestApp,
    op: &MockOp,
    subject: &str,
    email: &str,
    verified: Option<bool>,
    code: &str,
) -> Response<Body> {
    let (state_cookie, state_param) = start_oidc_flow(app, None).await;
    let nonce = parse_state_cookie_nonce(&state_cookie);
    let mut claims =
        IdTokenClaims::standard(&op.issuer(), "folio-test-client", subject, &nonce, email);
    claims.email_verified = verified;
    op.register_token_response_for_code(code, &sign_id_token(&claims))
        .await;
    callback_with(app, &state_cookie, &state_param, code).await
}

/// E15: auto-linking a `pending_verification` local account on the IdP's own
/// verified-email claim activates it — the proof the verify-email link asks
/// for — and drops the password whoever registered it chose, so the
/// registrant can't sign in to the verified owner's account.
#[tokio::test]
async fn auto_link_activates_a_pending_local_user_and_drops_its_password() {
    use sea_orm::EntityTrait;
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc_link_local(op.issuer()).await;
    let email = "pending-link@example.com";
    let id = pending_local_user(&app, email).await;
    let before = user_row(&app, id).await;
    assert!(before.password_hash.is_some());

    let resp = oidc_sign_in(&app, &op, "subject-pending-link", email, Some(true), "c1").await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let session = extract_set_cookie(&resp, "__Host-comic_session").expect("session cookie");

    let linked = user_row(&app, id).await;
    assert_eq!(linked.state, entity::user::UserState::Active);
    assert!(linked.email_verified);
    assert!(
        linked.password_hash.is_none(),
        "registrant's password dropped"
    );
    assert_eq!(linked.token_version, before.token_version + 1);
    assert_eq!(me_status(&app, &session).await, StatusCode::OK);
    assert_eq!(
        local_login_status(&app, email, "registrant-chosen-pw").await,
        StatusCode::UNAUTHORIZED,
        "the registrant's password no longer signs in"
    );

    let audit = entity::audit_log::Entity::find()
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .find(|a| a.action == "auth.oidc.auto_link")
        .expect("auto-link audit row");
    assert_eq!(audit.payload["activated"], true, "{:?}", audit.payload);
}

/// E15: without the IdP's own claim (the trust-unverified opt-in), nothing
/// proves the address, so a pending local account is neither linked nor
/// activated, and no session is minted.
#[tokio::test]
async fn auto_link_refuses_a_pending_local_user_without_a_verified_claim() {
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc_link_local_trusting_unverified(op.issuer()).await;
    let email = "pending-trust@example.com";
    let id = pending_local_user(&app, email).await;
    let before = user_row(&app, id).await;

    let resp = oidc_sign_in(&app, &op, "subject-pending-trust", email, None, "c1").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(extract_set_cookie(&resp, "__Host-comic_session").is_none());
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.email_unverified", "{body}");

    let after = user_row(&app, id).await;
    assert_eq!(after.state, entity::user::UserState::PendingVerification);
    assert_eq!(after.external_id, before.external_id, "not linked");
    assert_eq!(after.password_hash, before.password_hash);
}

/// E15: an OIDC account in `pending_verification` is refused by the callback
/// while nothing proves its address, and activated by the IdP's own
/// verified-email claim.
#[tokio::test]
async fn pending_oidc_user_is_refused_until_the_idp_verifies_the_email() {
    use sea_orm::EntityTrait;
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc(op.issuer(), true).await;
    let email = "pending-oidc@example.com";

    let resp = oidc_sign_in(&app, &op, "subject-pending", email, Some(true), "c1").await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let id = entity::user::Entity::find()
        .all(&app.state().db)
        .await
        .unwrap()[0]
        .id;
    force_state(&app, id, entity::user::UserState::PendingVerification).await;

    // Claim absent (trusted by policy only): refused, no session.
    let resp = oidc_sign_in(&app, &op, "subject-pending", email, None, "c2").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(extract_set_cookie(&resp, "__Host-comic_session").is_none());
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.email_unverified", "{body}");
    assert_eq!(
        user_row(&app, id).await.state,
        entity::user::UserState::PendingVerification
    );

    // The IdP's own claim proves the address: activated and signed in.
    let resp = oidc_sign_in(&app, &op, "subject-pending", email, Some(true), "c3").await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let session = extract_set_cookie(&resp, "__Host-comic_session").expect("session cookie");
    assert_eq!(
        user_row(&app, id).await.state,
        entity::user::UserState::Active
    );
    assert_eq!(me_status(&app, &session).await, StatusCode::OK);
}

/// E15: a disabled OIDC account stays disabled whatever the IdP says.
#[tokio::test]
async fn disabled_oidc_user_is_refused() {
    use sea_orm::EntityTrait;
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc(op.issuer(), false).await;
    let email = "disabled-oidc@example.com";
    let resp = oidc_sign_in(&app, &op, "subject-disabled", email, Some(true), "c1").await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let id = entity::user::Entity::find()
        .all(&app.state().db)
        .await
        .unwrap()[0]
        .id;
    force_state(&app, id, entity::user::UserState::Disabled).await;

    let resp = oidc_sign_in(&app, &op, "subject-disabled", email, Some(true), "c2").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.disabled", "{body}");
    assert_eq!(
        user_row(&app, id).await.state,
        entity::user::UserState::Disabled
    );
}

#[tokio::test]
async fn logout_redirects_to_end_session_endpoint_for_oidc_session() {
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc(op.issuer(), false).await;
    let (state_cookie, state_param) = start_oidc_flow(&app, None).await;
    let nonce = parse_state_cookie_nonce(&state_cookie);

    let claims = IdTokenClaims::standard(
        &op.issuer(),
        "folio-test-client",
        "rp-logout-subject",
        &nonce,
        "rplogout@example.com",
    );
    let id_token = sign_id_token(&claims);
    op.register_token_response(&id_token).await;

    let callback_resp = callback_with(&app, &state_cookie, &state_param, "code").await;
    assert_eq!(callback_resp.status(), StatusCode::SEE_OTHER);
    let session =
        extract_set_cookie(&callback_resp, "__Host-comic_session").expect("session cookie");
    let csrf = extract_set_cookie(&callback_resp, "__Host-comic_csrf").expect("csrf cookie");
    let refresh =
        extract_set_cookie(&callback_resp, "__Host-comic_refresh").expect("refresh cookie");

    let logout_resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/logout")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={session}; __Host-comic_csrf={csrf}; __Host-comic_refresh={refresh}"
                    ),
                )
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        logout_resp.status(),
        StatusCode::SEE_OTHER,
        "OIDC logout should 302 to end_session_endpoint"
    );
    let location = logout_resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .expect("Location header")
        .to_string();
    assert!(
        location.starts_with(&format!("{}/end_session", op.issuer())),
        "expected end_session prefix, got {location}"
    );
    assert!(
        location.contains("id_token_hint="),
        "RP-logout URL must include id_token_hint"
    );
    assert!(
        location.contains("post_logout_redirect_uri="),
        "RP-logout URL must include post_logout_redirect_uri"
    );
}

// ───────────────────── Discovery-cache eviction ─────────────────────
//
// Audit gap: `clear_discovery_cache()` is called by `PATCH /admin/settings`
// when any `oidc.*` row changes, but no test asserted the cache actually
// gets evicted. Without this, a future refactor could break the
// invalidation path silently — operators would keep hitting the old
// issuer indefinitely.

/// Register a local user via the form-action endpoint and return its
/// `(session, csrf)` cookies. First registered user becomes admin per
/// Folio's first-user-bootstrap convention.
async fn register_local_admin(app: &TestApp, email: &str) -> (String, String) {
    let body = format!(r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#);
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "register");
    let session =
        extract_set_cookie(&resp, "__Host-comic_session").expect("session cookie on register");
    let csrf = extract_set_cookie(&resp, "__Host-comic_csrf").expect("csrf cookie on register");
    (session, csrf)
}

#[tokio::test]
async fn patching_oidc_issuer_evicts_discovery_cache() {
    let op_a = MockOp::start().await;
    let op_b = MockOp::start().await;

    // Spawn pointing at A.
    let app = TestApp::spawn_with_oidc(op_a.issuer(), false).await;

    // 1) Populate the discovery cache via /auth/oidc/start. The redirect
    //    Location header should reference op_a's `authorize` endpoint.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/auth/oidc/start")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location_before = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .expect("Location after first /start")
        .to_string();
    assert!(
        location_before.starts_with(&format!("{}/authorize", op_a.issuer())),
        "first /start must point at op_a; got {location_before}"
    );

    // 2) Register a local admin, then PATCH /admin/settings to swap the
    //    issuer over to op_b. Because the new issuer is a different
    //    wiremock host, dry-run validation needs new client_id /
    //    client_secret (in practice they're the same string — the test
    //    harness uses the same values for both ops).
    let (session, csrf) = register_local_admin(&app, "admin@example.com").await;
    let patch_body = json!({
        "auth.oidc.issuer": op_b.issuer(),
        "auth.oidc.client_id": "folio-test-client",
        "auth.oidc.client_secret": "folio-test-secret",
    });
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/api/admin/settings")
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={session}; __Host-comic_csrf={csrf}"),
                )
                .header("X-CSRF-Token", &csrf)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(patch_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = body_json(resp.into_body()).await;
    assert_eq!(status, StatusCode::OK, "PATCH /admin/settings: {body}");

    // 3) Next /auth/oidc/start should re-fetch discovery — this time
    //    against op_b — and the redirect Location should point at op_b's
    //    `/authorize` endpoint. If the cache had NOT been evicted, the
    //    server would keep using op_a's cached authorize URL.
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/auth/oidc/start")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location_after = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .expect("Location after PATCH")
        .to_string();
    assert!(
        location_after.starts_with(&format!("{}/authorize", op_b.issuer())),
        "post-PATCH /start must point at op_b (cache should be evicted); got {location_after}",
    );
    assert!(
        !location_after.starts_with(&op_a.issuer()),
        "post-PATCH /start MUST NOT point at op_a (stale cache); got {location_after}",
    );
}

/// CQ-TEST-8 (audit 2026-07): the nonce check is the replay defense — an
/// id_token minted for a DIFFERENT login attempt (wrong nonce) must be
/// rejected even when state validates and the token signature is good.
/// Only the missing-nonce/happy paths were covered before; a dropped
/// nonce comparison would have passed CI.
#[tokio::test]
async fn callback_rejects_nonce_mismatch() {
    let op = MockOp::start().await;
    let app = TestApp::spawn_with_oidc(op.issuer(), false).await;
    let (state_cookie, state_param) = start_oidc_flow(&app, None).await;

    // Valid signature, valid audience/issuer — but the nonce belongs to
    // some other flow.
    let claims = IdTokenClaims::standard(
        &op.issuer(),
        "folio-test-client",
        "subject-replay",
        "nonce-from-someone-elses-flow",
        "replay@example.com",
    );
    let id_token = sign_id_token(&claims);
    op.register_token_response(&id_token).await;

    let resp = callback_with(&app, &state_cookie, &state_param, "auth-code-replay").await;
    assert_ne!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "nonce mismatch must not mint a session"
    );
    assert!(
        extract_set_cookie(&resp, "__Host-comic_session").is_none(),
        "no session cookie on nonce mismatch"
    );

    // And no user row was upserted.
    use sea_orm::EntityTrait;
    let state = app.state();
    let users = entity::user::Entity::find().all(&state.db).await.unwrap();
    assert!(
        !users.iter().any(|u| u.external_id.starts_with("oidc:")),
        "nonce mismatch must not upsert a user"
    );
}
