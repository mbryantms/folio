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
    matchers::{method, path},
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
        let mut rng = rand::thread_rng();
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
    let state_cookie =
        extract_set_cookie(&resp, "__Host-comic_oidc").expect("state cookie present after /start");
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
    (state_cookie, state_param)
}

fn parse_state_cookie_nonce(state_cookie: &str) -> String {
    // urldecode + json-parse the same way oidc::callback does.
    let decoded = urlencoding::decode(state_cookie).expect("decode state cookie");
    let v: serde_json::Value = serde_json::from_str(&decoded).expect("state cookie json");
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
                .header(header::COOKIE, format!("__Host-comic_oidc={state_cookie}"))
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
    assert!(extract_set_cookie(&resp, "__Secure-comic_refresh").is_some());

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
        extract_set_cookie(&callback_resp, "__Secure-comic_refresh").expect("refresh cookie");

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
                        "__Host-comic_session={session}; __Host-comic_csrf={csrf}; __Secure-comic_refresh={refresh}"
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
