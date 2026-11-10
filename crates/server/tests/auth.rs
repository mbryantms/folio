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

    // Rotate once.
    let rotated = refresh_call(&app, &original_refresh, &csrf).await;
    assert_eq!(rotated.status(), StatusCode::OK);

    // Try to use the original (now-rotated) token again.
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

/// Refresh rotation is one conditional UPDATE: of two concurrent refreshes
/// presenting the same token, exactly one rotates it. Repeated a few times
/// so the two requests really interleave on the database at least once.
#[tokio::test]
async fn concurrent_refreshes_with_one_token_have_one_winner() {
    let app = TestApp::spawn().await;
    let reg = register(&app, "race@example.com", "correctly-horse-battery").await;
    let csrf = extract_cookie(&reg, "__Host-comic_csrf").unwrap();
    let mut current = extract_cookie(&reg, "__Host-comic_refresh").unwrap();

    for round in 0..5 {
        let (a, b) = tokio::join!(
            refresh_call(&app, &current, &csrf),
            refresh_call(&app, &current, &csrf)
        );
        let mut statuses = [a.status(), b.status()];
        statuses.sort();
        assert_eq!(
            statuses,
            [StatusCode::OK, StatusCode::UNAUTHORIZED],
            "round {round}: exactly one refresh may win"
        );
        let (winner, loser) = if a.status() == StatusCode::OK {
            (a, b)
        } else {
            (b, a)
        };
        let loser_body = body_json(loser.into_body()).await;
        assert_eq!(loser_body["error"]["code"], "auth.invalid");
        let rotated = extract_cookie(&winner, "__Host-comic_refresh").unwrap();
        let access = extract_cookie(&winner, "__Host-comic_session").unwrap();
        assert_ne!(rotated, current);

        // The presented token is dead for good.
        let replay = refresh_call(&app, &current, &csrf).await;
        assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);

        // The winner's access token works.
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
        current = rotated;
    }

    // ... and so does the winner's refresh token.
    let next = refresh_call(&app, &current, &csrf).await;
    assert_eq!(next.status(), StatusCode::OK);
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
