//! Local auth integration test.
//!
//! Covers register → first-user-admin bootstrap, login, /auth/me, CSRF
//! enforcement, refresh rotation, refresh-reuse rejection, logout.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use sea_orm::EntityTrait;
use tower::ServiceExt;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.expect("collect body");
    serde_json::from_slice(&bytes).expect("json body")
}

#[tokio::test]
async fn first_user_becomes_admin() {
    let app = TestApp::spawn().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"first@example.com","password":"correctly-horse-battery"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let cookies: Vec<_> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert!(cookies.iter().any(|c| c.contains("__Host-comic_session=")));
    assert!(cookies.iter().any(|c| c.contains("__Host-comic_csrf=")));
    assert!(cookies.iter().any(|c| c.contains("__Host-comic_refresh=")));

    let body = body_json(resp.into_body()).await;
    assert_eq!(body["user"]["role"], "admin");
}

#[tokio::test]
async fn second_user_is_regular_when_no_smtp() {
    let app = TestApp::spawn().await;
    // First — admin
    let _ = register(&app, "first@example.com", "correctly-horse-battery").await;
    // Second — regular user
    let resp = register(&app, "second@example.com", "another-strong-password").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["user"]["role"], "user");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_registrations_create_only_one_admin() {
    let app = TestApp::spawn().await;
    let attempts = (0..4).map(|i| {
        let app = &app;
        async move {
            register(
                app,
                &format!("first-race-{i}@example.com"),
                "correctly-horse-battery",
            )
            .await
        }
    });
    let responses = futures::future::join_all(attempts).await;
    for resp in responses {
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    let state = app.state();
    let users = entity::user::Entity::find().all(&state.db).await.unwrap();
    let admin_count = users
        .iter()
        .filter(|u| u.role == entity::user::UserRole::Admin)
        .count();
    assert_eq!(admin_count, 1, "exactly one first signup may become admin");
    assert_eq!(users.len(), 4);
}

#[tokio::test]
async fn login_wrong_password_is_401() {
    let app = TestApp::spawn().await;
    let _ = register(&app, "user@example.com", "correctly-horse-battery").await;
    let resp = login(&app, "user@example.com", "wrong-password").await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_requires_auth() {
    let app = TestApp::spawn().await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn me_with_session_cookie_works() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "user@example.com", "correctly-horse-battery").await;
    let session_cookie = extract_cookie(&reg, "__Host-comic_session").unwrap();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/me")
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={session_cookie}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["email"], "user@example.com");
    assert!(body["csrf_token"].is_string());
}

#[tokio::test]
async fn me_reuses_existing_csrf_cookie() {
    // Regression: minting a fresh CSRF token on every /auth/me call
    // created a TOCTOU race against in-flight POSTs that captured
    // the cookie value just before /auth/me's response rotated it.
    // Symptom: random 403s with `reason: "no-token"` on routine
    // mutations after a TanStack staleTime-revalidate of useMe().
    // Fix: reuse the existing cookie value; only mint when missing.
    let app = TestApp::spawn().await;
    let reg = register(&app, "user@example.com", "correctly-horse-battery").await;
    let session_cookie = extract_cookie(&reg, "__Host-comic_session").unwrap();
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/me")
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={session_cookie}; __Host-comic_csrf={csrf}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // Body's csrf_token field must match the cookie we sent in.
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["csrf_token"], csrf);
}

#[tokio::test]
async fn me_mints_csrf_when_cookie_missing() {
    // Fresh sessions (no CSRF cookie yet) still get one minted on the
    // first /auth/me call — the original "fresh on app load" intent.
    let app = TestApp::spawn().await;
    let reg = register(&app, "user@example.com", "correctly-horse-battery").await;
    let session_cookie = extract_cookie(&reg, "__Host-comic_session").unwrap();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/me")
                // Intentionally omit __Host-comic_csrf — only send session.
                .header(
                    header::COOKIE,
                    format!("__Host-comic_session={session_cookie}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let minted = extract_cookie(&resp, "__Host-comic_csrf");
    assert!(
        minted.is_some(),
        "expected a freshly-minted CSRF cookie when none was sent"
    );
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["csrf_token"], minted.unwrap());
}

#[tokio::test]
async fn csrf_enforced_on_unsafe_verbs() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "user@example.com", "correctly-horse-battery").await;
    let session_cookie = extract_cookie(&reg, "__Host-comic_session").unwrap();
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();
    let refresh = extract_cookie(&reg, "__Host-comic_refresh").unwrap();

    // Without CSRF header → 403
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/refresh")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={session_cookie}; __Host-comic_csrf={csrf}; __Host-comic_refresh={refresh}"
                    ),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // With CSRF header → 200
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/refresh")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={session_cookie}; __Host-comic_csrf={csrf}; __Host-comic_refresh={refresh}"
                    ),
                )
                .header("x-csrf-token", csrf.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn refresh_token_reuse_is_rejected() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "user@example.com", "correctly-horse-battery").await;
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();
    let original_refresh = extract_cookie(&reg, "__Host-comic_refresh").unwrap();

    let body = body_json(reg.into_body()).await;
    let user_id: uuid::Uuid = body["user"]["id"].as_str().unwrap().parse().unwrap();

    // Rotate once.
    let rotated = refresh_call(&app, &original_refresh, &csrf).await;
    assert_eq!(rotated.status(), StatusCode::OK);

    // Once the grace window after the rotation has passed (see
    // `auth_refresh_grace.rs` for inside it), the original token is dead.
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    sea_orm::ConnectionTrait::execute_unprepared(
        &db,
        &format!(
            "UPDATE auth_sessions SET refresh_rotated_at = now() - interval '{} seconds' \
             WHERE user_id = '{user_id}'",
            server::auth::local::REFRESH_GRACE.as_secs() + 1
        ),
    )
    .await
    .unwrap();
    let replay = refresh_call(&app, &original_refresh, &csrf).await;
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn logout_clears_cookies_and_revokes_session() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "user@example.com", "correctly-horse-battery").await;
    let session_cookie = extract_cookie(&reg, "__Host-comic_session").unwrap();
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();
    let refresh = extract_cookie(&reg, "__Host-comic_refresh").unwrap();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/logout")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={session_cookie}; __Host-comic_csrf={csrf}; __Host-comic_refresh={refresh}"
                    ),
                )
                .header("x-csrf-token", csrf.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Cookies should be cleared (Max-Age=0).
    let cookies: Vec<_> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert!(
        cookies
            .iter()
            .any(|c| c.contains("__Host-comic_session=") && c.contains("Max-Age=0"))
    );

    // Replaying the refresh after logout should fail.
    let replay = refresh_call(&app, &refresh, &csrf).await;
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
}

/// AR-7 (audit 2026-11 B8): `users.role` / `users.state` load as typed
/// enums. A row holding a string outside the enum (hand-edited DB, a value
/// from a future version after a downgrade) must fail loudly: the row is
/// unreadable, so the auth extractor, `/auth/me` and login answer 500. It
/// must never be coerced into a reader (or an admin), and never look like
/// "no such user".
#[tokio::test]
async fn unknown_role_in_db_fails_loudly() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "first@example.com", "correctly-horse-battery").await;
    assert_eq!(reg.status(), StatusCode::CREATED);
    let session_cookie = extract_cookie(&reg, "__Host-comic_session").unwrap();
    let body = body_json(reg.into_body()).await;
    let user_id: uuid::Uuid = body["user"]["id"].as_str().unwrap().parse().unwrap();

    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    use sea_orm::ConnectionTrait;
    db.execute_unprepared(&format!(
        "UPDATE users SET role = 'superuser' WHERE id = '{user_id}'"
    ))
    .await
    .unwrap();

    // The entity load itself is an error, not a defaulted role.
    let loaded = entity::user::Entity::find_by_id(user_id).one(&db).await;
    assert!(loaded.is_err(), "unknown role must not decode: {loaded:?}");

    let get = |uri: &str| {
        Request::builder()
            .uri(uri)
            .header(
                header::COOKIE,
                format!("__Host-comic_session={session_cookie}"),
            )
            .body(Body::empty())
            .unwrap()
    };
    let me = app.router.clone().oneshot(get("/auth/me")).await.unwrap();
    assert_eq!(me.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let admin = app
        .router
        .clone()
        .oneshot(get("/api/admin/users"))
        .await
        .unwrap();
    assert_eq!(admin.status(), StatusCode::INTERNAL_SERVER_ERROR);

    // Login reports the unreadable row instead of "invalid credentials".
    let login = login(&app, "first@example.com", "correctly-horse-battery").await;
    assert_eq!(login.status(), StatusCode::INTERNAL_SERVER_ERROR);

    // Restoring a known value makes the account usable again.
    db.execute_unprepared(&format!(
        "UPDATE users SET role = 'admin' WHERE id = '{user_id}'"
    ))
    .await
    .unwrap();
    let me = app.router.clone().oneshot(get("/auth/me")).await.unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    assert_eq!(body_json(me.into_body()).await["role"], "admin");

    // An admin opening another user's unreadable row gets a 500, not a 404
    // that would make the account look deleted. Surfaces that only need the
    // name / email (audit-log actor labels) project those columns and keep
    // working.
    let reg2 = register(&app, "second@example.com", "another-strong-password").await;
    assert_eq!(reg2.status(), StatusCode::CREATED);
    let second_id: uuid::Uuid = body_json(reg2.into_body()).await["user"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    server::audit::record(
        &app.state().db,
        server::audit::AuditEntry {
            actor_id: second_id,
            action: "test.unreadable_actor",
            target_type: None,
            target_id: None,
            payload: serde_json::json!({}),
            ip: None,
            user_agent: None,
        },
    )
    .await;
    db.execute_unprepared(&format!(
        "UPDATE users SET state = 'banned' WHERE id = '{second_id}'"
    ))
    .await
    .unwrap();
    let one = app
        .router
        .clone()
        .oneshot(get(&format!("/api/admin/users/{second_id}")))
        .await
        .unwrap();
    assert_eq!(one.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let audit = app
        .router
        .clone()
        .oneshot(get("/api/admin/audit"))
        .await
        .unwrap();
    assert_eq!(audit.status(), StatusCode::OK);
    let audit = body_json(audit.into_body()).await;
    assert!(
        audit.to_string().contains("second@example.com"),
        "actor label still resolves: {audit}"
    );
}

// ───── helpers ─────

async fn register(app: &TestApp, email: &str, password: &str) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"{password}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn login(app: &TestApp, email: &str, password: &str) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"{password}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn refresh_call(
    app: &TestApp,
    refresh_value: &str,
    csrf: &str,
) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/refresh")
                .header(
                    header::COOKIE,
                    format!("__Host-comic_refresh={refresh_value}; __Host-comic_csrf={csrf}"),
                )
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

fn extract_cookie(resp: &axum::http::Response<Body>, name: &str) -> Option<String> {
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

/// CQ-TEST-8 (audit 2026-07): the wrong-VALUE half of the CSRF
/// double-submit check. `csrf_enforced_on_unsafe_verbs` covers the
/// missing-header case; a dropped or reordered constant-time compare
/// (header vs cookie) would still have passed it. A header that is
/// present but disagrees with the cookie must 403 — including the
/// prefix/truncation shapes a naive `starts_with` would wave through.
#[tokio::test]
async fn csrf_wrong_value_rejected() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "csrfval@example.com", "correctly-horse-battery").await;
    let session_cookie = extract_cookie(&reg, "__Host-comic_session").unwrap();
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();
    let refresh = extract_cookie(&reg, "__Host-comic_refresh").unwrap();

    let attempt = |header_value: String| {
        let router = app.router.clone();
        let cookies = format!(
            "__Host-comic_session={session_cookie}; __Host-comic_csrf={csrf}; __Host-comic_refresh={refresh}"
        );
        async move {
            router
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/auth/refresh")
                        .header(header::COOKIE, cookies)
                        .header("x-csrf-token", header_value)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
        }
    };

    // Entirely different token.
    let resp = attempt("attacker-supplied-token-value-000".into()).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "different value");

    // Truncated prefix of the real token.
    let resp = attempt(csrf[..csrf.len() / 2].to_string()).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "truncated value");

    // Real token with a suffix appended.
    let resp = attempt(format!("{csrf}xx")).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "extended value");

    // Sanity: the untouched token still passes (guards the guard).
    let resp = attempt(csrf.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK, "correct value still passes");
}

/// Refresh rotation is one conditional UPDATE. Two concurrent refreshes
/// presenting the same token both get a pair: one rotates normally, the
/// other takes the single grace use of the previous token, which retires
/// the first one's token. Exactly one of the two issued tokens works
/// afterwards and the presented one is spent. Repeated a few times so the
/// two requests really interleave on the database at least once.
#[tokio::test]
async fn concurrent_refreshes_with_one_token_leave_one_valid_token() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "race@example.com", "correctly-horse-battery").await;
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();
    let mut current = extract_cookie(&reg, "__Host-comic_refresh").unwrap();
    let body = body_json(reg.into_body()).await;
    let user_id: uuid::Uuid = body["user"]["id"].as_str().unwrap().parse().unwrap();
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();

    for round in 0..5 {
        let (a, b) = tokio::join!(
            refresh_call(&app, &current, &csrf),
            refresh_call(&app, &current, &csrf)
        );
        assert_eq!(
            [a.status(), b.status()],
            [StatusCode::OK, StatusCode::OK],
            "round {round}: one rotation and one grace use"
        );
        let issued = [
            extract_cookie(&a, "__Host-comic_refresh").unwrap(),
            extract_cookie(&b, "__Host-comic_refresh").unwrap(),
        ];
        // Read from the row, without presenting any token: exactly one
        // issued token is current and no previous token is left to use.
        let survivor = surviving_token(&session_for(&db, user_id).await, &issued);
        assert_eq!(
            refresh_call(&app, &current, &csrf).await.status(),
            StatusCode::UNAUTHORIZED,
            "round {round}: the presented token is spent"
        );
        current = survivor;
    }

    // The survivor refreshes, and its access token works.
    let next = refresh_call(&app, &current, &csrf).await;
    assert_eq!(next.status(), StatusCode::OK);
    let access = extract_cookie(&next, "__Host-comic_session").unwrap();
    let me = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/me")
                .header(header::COOKIE, format!("__Host-comic_session={access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(me.status(), StatusCode::OK);
}

/// After a grace use, the row's current hash is the SHA-256 of exactly one
/// of `issued` and no previous hash remains. Returns that token.
fn surviving_token(row: &entity::auth_session::Model, issued: &[String]) -> String {
    let matching: Vec<&String> = issued
        .iter()
        .filter(|t| server::auth::cookies::sha256_hex(t) == row.refresh_token_hash)
        .collect();
    assert_eq!(matching.len(), 1, "exactly one issued token is current");
    assert!(
        row.previous_refresh_token_hash.is_none(),
        "the grace use left no previous token"
    );
    matching[0].clone()
}

async fn session_for(
    db: &sea_orm::DatabaseConnection,
    user_id: uuid::Uuid,
) -> entity::auth_session::Model {
    use sea_orm::{ColumnTrait, QueryFilter};
    entity::auth_session::Entity::find()
        .filter(entity::auth_session::Column::UserId.eq(user_id))
        .one(db)
        .await
        .unwrap()
        .expect("session row")
}

async fn age_session(db: &sea_orm::DatabaseConnection, user_id: uuid::Uuid, days: i64) {
    use sea_orm::ConnectionTrait;
    db.execute_unprepared(&format!(
        "UPDATE auth_sessions SET created_at = now() - interval '{days} days' \
         WHERE user_id = '{user_id}'"
    ))
    .await
    .unwrap();
}

/// A session older than the absolute maximum age (90 d by default) is
/// refused and revoked even though its sliding expiry is still ahead.
#[tokio::test]
async fn refresh_past_max_session_age_is_refused_and_revokes_the_session() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "old@example.com", "correctly-horse-battery").await;
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();
    let refresh = extract_cookie(&reg, "__Host-comic_refresh").unwrap();
    let body = body_json(reg.into_body()).await;
    let user_id: uuid::Uuid = body["user"]["id"].as_str().unwrap().parse().unwrap();

    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    age_session(&db, user_id, 91).await;
    let before = session_for(&db, user_id).await;
    assert!(
        before.expires_at > chrono::Utc::now().fixed_offset(),
        "the sliding expiry is still in the future"
    );

    let resp = refresh_call(&app, &refresh, &csrf).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.invalid");

    let after = session_for(&db, user_id).await;
    assert!(
        after.revoked_at.is_some(),
        "the over-age session is revoked"
    );
    assert_eq!(after.refresh_token_hash, before.refresh_token_hash);
}

/// Inside the maximum age a session still refreshes, and its new sliding
/// expiry is clamped to `created_at + max age`.
#[tokio::test]
async fn refresh_inside_max_session_age_rotates_and_clamps_expiry() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "young@example.com", "correctly-horse-battery").await;
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();
    let refresh = extract_cookie(&reg, "__Host-comic_refresh").unwrap();
    let body = body_json(reg.into_body()).await;
    let user_id: uuid::Uuid = body["user"]["id"].as_str().unwrap().parse().unwrap();

    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    age_session(&db, user_id, 89).await;

    let resp = refresh_call(&app, &refresh, &csrf).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let after = session_for(&db, user_id).await;
    assert!(after.revoked_at.is_none());
    let limit = after.created_at + chrono::Duration::days(90);
    assert!(
        after.expires_at <= limit,
        "expiry {} runs past the absolute age {limit}",
        after.expires_at
    );
    // Roughly one day left, far less than the 30 d sliding window.
    assert!(after.expires_at < chrono::Utc::now().fixed_offset() + chrono::Duration::days(2));
}

/// The CSRF cookie lives as long as the refresh cookie, so the silent
/// refresh still has it after the short access cookie has expired.
#[tokio::test]
async fn csrf_cookie_lives_as_long_as_the_refresh_cookie() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "csrf-life@example.com", "correctly-horse-battery").await;
    let max_age = |name: &str| -> String {
        reg.headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|c| c.starts_with(&format!("{name}=")))
            .and_then(|c| {
                c.split(';')
                    .map(str::trim)
                    .find_map(|p| p.strip_prefix("Max-Age=").map(str::to_owned))
            })
            .unwrap()
    };
    assert_eq!(
        max_age("__Host-comic_csrf"),
        max_age("__Host-comic_refresh")
    );
    assert_ne!(
        max_age("__Host-comic_csrf"),
        max_age("__Host-comic_session")
    );
}

/// The whole `Set-Cookie` line for `name`.
fn cookie_line(resp: &axum::http::Response<Body>, name: &str) -> Option<String> {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with(&format!("{name}=")))
        .map(str::to_owned)
}

fn cookie_attr(line: &str, key: &str) -> Option<String> {
    line.split(';')
        .map(str::trim)
        .find_map(|p| p.strip_prefix(&format!("{key}=")).map(str::to_owned))
}

fn max_age_of(resp: &axum::http::Response<Body>, name: &str) -> i64 {
    cookie_attr(&cookie_line(resp, name).expect(name), "Max-Age")
        .expect("Max-Age")
        .parse()
        .unwrap()
}

/// `__Host-comic_access_exp` is readable by scripts, carries the access
/// token's expiry, and lives exactly as long as the session cookie.
fn assert_access_exp_cookie(resp: &axum::http::Response<Body>, access_ttl: i64) {
    let line = cookie_line(resp, "__Host-comic_access_exp").expect("access-exp cookie set");
    assert!(!line.contains("HttpOnly"), "readable by scripts: {line}");
    assert!(line.contains("Secure"), "{line}");
    assert!(line.contains("SameSite=Lax"), "{line}");
    assert!(line.contains("Path=/"), "{line}");
    assert_eq!(
        max_age_of(resp, "__Host-comic_access_exp"),
        max_age_of(resp, "__Host-comic_session")
    );
    assert_eq!(max_age_of(resp, "__Host-comic_access_exp"), access_ttl);
    // `<exp>.<lifetime seconds>`.
    let value = extract_cookie(resp, "__Host-comic_access_exp").unwrap();
    let (exp, lifetime) = value.split_once('.').expect("exp.lifetime");
    let exp: i64 = exp.parse().unwrap();
    assert_eq!(lifetime.parse::<i64>().unwrap(), access_ttl);
    let expected = chrono::Utc::now().timestamp() + access_ttl;
    assert!(
        (exp - expected).abs() <= 5,
        "exp {exp}, expected about {expected}"
    );
}

#[tokio::test]
async fn access_expiry_cookie_is_set_on_register_login_and_refresh_and_cleared_on_logout() {
    let app = TestApp::spawn().await;
    let ttl = app.state().cfg().access_ttl().as_secs() as i64;
    let reg = register(&app, "exp@example.com", "correctly-horse-battery").await;
    assert_access_exp_cookie(&reg, ttl);

    let log = login(&app, "exp@example.com", "correctly-horse-battery").await;
    assert_eq!(log.status(), StatusCode::OK);
    assert_access_exp_cookie(&log, ttl);
    let session = extract_cookie(&log, "__Host-comic_session").unwrap();
    let csrf = extract_cookie(&log, "__Host-comic_csrf").unwrap();
    let refresh = extract_cookie(&log, "__Host-comic_refresh").unwrap();

    let rotated = refresh_call(&app, &refresh, &csrf).await;
    assert_eq!(rotated.status(), StatusCode::OK);
    assert_access_exp_cookie(&rotated, ttl);
    let refresh = extract_cookie(&rotated, "__Host-comic_refresh").unwrap();
    let csrf = extract_cookie(&rotated, "__Host-comic_csrf").unwrap();
    let exp = extract_cookie(&rotated, "__Host-comic_access_exp").unwrap();

    let out = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/logout")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={session}; __Host-comic_csrf={csrf}; __Host-comic_refresh={refresh}; __Host-comic_access_exp={exp}"
                    ),
                )
                .header("x-csrf-token", csrf.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(out.status(), StatusCode::NO_CONTENT);
    let cleared = cookie_line(&out, "__Host-comic_access_exp").expect("cleared on logout");
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
}

/// Near the end of the absolute session age, a rotation clamps the refresh
/// and CSRF cookies to what is left of it instead of the 30 d refresh TTL.
#[tokio::test]
async fn refresh_and_csrf_cookies_are_clamped_to_the_remaining_session_age() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "clamp@example.com", "correctly-horse-battery").await;
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();
    let refresh = extract_cookie(&reg, "__Host-comic_refresh").unwrap();
    let body = body_json(reg.into_body()).await;
    let user_id: uuid::Uuid = body["user"]["id"].as_str().unwrap().parse().unwrap();

    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    age_session(&db, user_id, 89).await;

    let resp = refresh_call(&app, &refresh, &csrf).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let day = 86_400;
    let refresh_age = max_age_of(&resp, "__Host-comic_refresh");
    assert!(
        (day - 120..=day).contains(&refresh_age),
        "refresh Max-Age {refresh_age} is about one day"
    );
    let csrf_age = max_age_of(&resp, "__Host-comic_csrf");
    assert!(
        (day - 120..=day).contains(&csrf_age),
        "csrf Max-Age {csrf_age}"
    );
    // The access cookie keeps its own, shorter lifetime.
    assert_eq!(
        max_age_of(&resp, "__Host-comic_session"),
        app.state().cfg().access_ttl().as_secs() as i64
    );
}

/// Hold the session row so both refreshes are queued on its lock, then
/// release it: the rotation is decided by the conditional UPDATE, so exactly
/// one rotates normally even when the two are guaranteed to overlap, and
/// the other takes the grace path (a third is refused: see
/// `auth_refresh_grace.rs`).
#[tokio::test]
async fn refreshes_queued_on_a_locked_row_leave_one_valid_token() {
    use sea_orm::{ConnectionTrait, TransactionTrait};
    let app = TestApp::spawn().await;
    let reg = register(&app, "locked@example.com", "correctly-horse-battery").await;
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();
    let refresh = extract_cookie(&reg, "__Host-comic_refresh").unwrap();
    let body = body_json(reg.into_body()).await;
    let user_id: uuid::Uuid = body["user"]["id"].as_str().unwrap().parse().unwrap();

    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let holder = db.begin().await.unwrap();
    holder
        .execute_unprepared(&format!(
            "SELECT id FROM auth_sessions WHERE user_id = '{user_id}' FOR UPDATE"
        ))
        .await
        .unwrap();

    let (a, b, ()) = tokio::join!(
        refresh_call(&app, &refresh, &csrf),
        refresh_call(&app, &refresh, &csrf),
        async {
            // Release only once both requests are queued on the row lock.
            wait_for_lock_waiters(&db, 2).await;
            holder.commit().await.unwrap();
        }
    );
    assert_eq!([a.status(), b.status()], [StatusCode::OK, StatusCode::OK]);
    let issued = [
        extract_cookie(&a, "__Host-comic_refresh").unwrap(),
        extract_cookie(&b, "__Host-comic_refresh").unwrap(),
    ];
    let survivor = surviving_token(&session_for(&db, user_id).await, &issued);
    assert_eq!(
        refresh_call(&app, &survivor, &csrf).await.status(),
        StatusCode::OK,
        "the survivor refreshes"
    );
}

/// The first of two queued refreshes finds the account disabled and rolls
/// its rotation back. The second then sees the original token again, and
/// is refused for the same reason — not with a 500 and not with a session.
/// The session survives untouched for when the account is re-enabled.
#[tokio::test]
async fn the_loser_of_a_race_whose_winner_rolled_back_is_refused_too() {
    use sea_orm::{ConnectionTrait, TransactionTrait};
    let app = TestApp::spawn().await;
    let reg = register(&app, "rollback@example.com", "correctly-horse-battery").await;
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();
    let refresh = extract_cookie(&reg, "__Host-comic_refresh").unwrap();
    let body = body_json(reg.into_body()).await;
    let user_id: uuid::Uuid = body["user"]["id"].as_str().unwrap().parse().unwrap();

    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    db.execute_unprepared(&format!(
        "UPDATE users SET state = 'disabled' WHERE id = '{user_id}'"
    ))
    .await
    .unwrap();
    let before = session_for(&db, user_id).await;

    let holder = db.begin().await.unwrap();
    holder
        .execute_unprepared(&format!(
            "SELECT id FROM auth_sessions WHERE user_id = '{user_id}' FOR UPDATE"
        ))
        .await
        .unwrap();
    let (a, b, ()) = tokio::join!(
        refresh_call(&app, &refresh, &csrf),
        refresh_call(&app, &refresh, &csrf),
        async {
            wait_for_lock_waiters(&db, 2).await;
            holder.commit().await.unwrap();
        }
    );
    for resp in [a, b] {
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(extract_cookie(&resp, "__Host-comic_session").is_none());
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["error"]["code"], "auth.invalid");
    }
    let after = session_for(&db, user_id).await;
    assert_eq!(after.refresh_token_hash, before.refresh_token_hash);
    assert!(after.revoked_at.is_none());

    db.execute_unprepared(&format!(
        "UPDATE users SET state = 'active' WHERE id = '{user_id}'"
    ))
    .await
    .unwrap();
    let resp = refresh_call(&app, &refresh, &csrf).await;
    assert_eq!(resp.status(), StatusCode::OK, "the session survived");
}

/// Wait until `n` sessions in this test's database are blocked on a lock
/// while running the refresh rotation, so a test proves its requests really
/// queued on the held row. Panics after 20 s.
async fn wait_for_lock_waiters(db: &sea_orm::DatabaseConnection, n: i64) {
    use sea_orm::ConnectionTrait;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let row = db
            .query_one_raw(sea_orm::Statement::from_string(
                sea_orm::DbBackend::Postgres,
                "SELECT count(*)::bigint AS waiting FROM pg_stat_activity \
                  WHERE datname = current_database() \
                    AND wait_event_type = 'Lock' \
                    AND query LIKE 'UPDATE auth_sessions%'",
            ))
            .await
            .unwrap()
            .expect("count row");
        let waiting: i64 = row.try_get("", "waiting").unwrap();
        if waiting >= n {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "only {waiting} of {n} refreshes queued on the row lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// At the very end of the absolute age a refresh that is still accepted
/// must not hand out a zero (immediately expired) refresh or CSRF cookie;
/// the CSRF cookie lasts at least as long as the access cookie.
#[tokio::test]
async fn a_refresh_at_the_age_limit_sets_no_zero_max_age() {
    use sea_orm::ConnectionTrait;
    let app = TestApp::spawn().await;
    let reg = register(&app, "edge@example.com", "correctly-horse-battery").await;
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();
    let refresh = extract_cookie(&reg, "__Host-comic_refresh").unwrap();
    let body = body_json(reg.into_body()).await;
    let user_id: uuid::Uuid = body["user"]["id"].as_str().unwrap().parse().unwrap();

    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    db.execute_unprepared(&format!(
        "UPDATE auth_sessions \
            SET created_at = now() - interval '90 days' + interval '20 seconds' \
          WHERE user_id = '{user_id}'"
    ))
    .await
    .unwrap();

    let resp = refresh_call(&app, &refresh, &csrf).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let refresh_age = max_age_of(&resp, "__Host-comic_refresh");
    assert!(
        (1..=20).contains(&refresh_age),
        "refresh Max-Age {refresh_age}"
    );
    let access = app.state().cfg().access_ttl().as_secs() as i64;
    assert_eq!(max_age_of(&resp, "__Host-comic_csrf"), access);
    assert_eq!(max_age_of(&resp, "__Host-comic_session"), access);
}

/// Durations above the cap that reached the config without the PATCH
/// validation (a row saved before the cap existed) are clamped: refresh and
/// `/me/sessions` keep working instead of panicking or overflowing.
#[tokio::test]
async fn over_cap_durations_that_bypassed_validation_do_not_break_refresh() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "overcap@example.com", "correctly-horse-battery").await;
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();
    let refresh = extract_cookie(&reg, "__Host-comic_refresh").unwrap();

    let mut cfg = (*app.state().cfg()).clone();
    cfg.session_max_age = "10000000000d".into();
    cfg.jwt_refresh_ttl = "18446744073709551615s".into();
    app.state().replace_cfg(cfg);

    let resp = refresh_call(&app, &refresh, &csrf).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let session = extract_cookie(&resp, "__Host-comic_session").unwrap();
    let csrf = extract_cookie(&resp, "__Host-comic_csrf").unwrap();
    let refresh = extract_cookie(&resp, "__Host-comic_refresh").unwrap();

    let list = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/me/sessions")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={session}; __Host-comic_csrf={csrf}; __Host-comic_refresh={refresh}"
                    ),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let body = body_json(list.into_body()).await;
    assert_eq!(body["sessions"].as_array().unwrap().len(), 1);
}
