//! Refresh-token grace window (SESS-GRACE).
//!
//! A rotation commits before its response leaves the server. Within
//! `REFRESH_GRACE` of a rotation, the token it replaced is accepted ONCE:
//! the grace path issues a new pair, clears the remembered previous token
//! and retires the token whose response was lost, so one token line stays
//! valid. Outside the window, or for any older token, the refusal is the
//! same as before. Each refusal test has an acceptance control.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, TransactionTrait};
use server::auth::cookies::sha256_hex;
use server::auth::local::REFRESH_GRACE;
use tower::ServiceExt;

// ───── helpers ─────

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.expect("collect body");
    serde_json::from_slice(&bytes).expect("json body")
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

/// A registered user: their id, CSRF value and first refresh token.
struct Signed {
    user_id: uuid::Uuid,
    csrf: String,
    refresh: String,
}

async fn register(app: &TestApp, email: &str) -> Signed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"correctly-horse-battery"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_success(), "register: {}", resp.status());
    let csrf = extract_cookie(&resp, "__Host-comic_csrf").unwrap();
    let refresh = extract_cookie(&resp, "__Host-comic_refresh").unwrap();
    let body = body_json(resp.into_body()).await;
    let user_id = body["user"]["id"].as_str().unwrap().parse().unwrap();
    Signed {
        user_id,
        csrf,
        refresh,
    }
}

async fn refresh_with(
    app: &TestApp,
    refresh: &str,
    csrf: &str,
    send_csrf_header: bool,
    user_agent: Option<&str>,
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/auth/refresh")
        .header(
            header::COOKIE,
            format!("__Host-comic_refresh={refresh}; __Host-comic_csrf={csrf}"),
        );
    if send_csrf_header {
        req = req.header("x-csrf-token", csrf);
    }
    if let Some(ua) = user_agent {
        req = req.header(header::USER_AGENT, ua);
    }
    app.router
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn refresh_call(app: &TestApp, refresh: &str, csrf: &str) -> axum::http::Response<Body> {
    refresh_with(app, refresh, csrf, true, None).await
}

/// Refresh and return the new refresh token, asserting success.
async fn rotate(app: &TestApp, refresh: &str, csrf: &str) -> String {
    let resp = refresh_call(app, refresh, csrf).await;
    assert_eq!(resp.status(), StatusCode::OK, "refresh should be accepted");
    extract_cookie(&resp, "__Host-comic_refresh").expect("new refresh cookie")
}

async fn assert_refused(resp: axum::http::Response<Body>, what: &str) {
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{what}");
    assert!(
        extract_cookie(&resp, "__Host-comic_session").is_none(),
        "{what}: no access cookie on a refusal"
    );
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.invalid", "{what}");
}

async fn session_for(
    db: &sea_orm::DatabaseConnection,
    user_id: uuid::Uuid,
) -> entity::auth_session::Model {
    entity::auth_session::Entity::find()
        .filter(entity::auth_session::Column::UserId.eq(user_id))
        .one(db)
        .await
        .unwrap()
        .expect("session row")
}

/// Move the last rotation `secs` seconds into the past.
async fn rotated_ago(db: &sea_orm::DatabaseConnection, user_id: uuid::Uuid, secs: i64) {
    db.execute_unprepared(&format!(
        "UPDATE auth_sessions SET refresh_rotated_at = now() - interval '{secs} seconds' \
         WHERE user_id = '{user_id}'"
    ))
    .await
    .unwrap();
}

async fn set_state(db: &sea_orm::DatabaseConnection, user_id: uuid::Uuid, state: &str) {
    db.execute_unprepared(&format!(
        "UPDATE users SET state = '{state}' WHERE id = '{user_id}'"
    ))
    .await
    .unwrap();
}

fn grace_secs() -> i64 {
    REFRESH_GRACE.as_secs() as i64
}

/// Wait until `n` connections are blocked on a lock running the refresh
/// rotation. Panics after 20 s.
async fn wait_for_lock_waiters(db: &sea_orm::DatabaseConnection, n: i64) {
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

// ───── sequences ─────

/// (1) The response carrying T1 is lost; the client retries with T0 inside
/// the window and gets a new pair. T1, which it never received, is retired.
#[tokio::test]
async fn lost_response_retry_with_the_previous_token_is_accepted_once() {
    let app = TestApp::spawn().await;
    let s = register(&app, "lost@example.com").await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();

    let t1 = rotate(&app, &s.refresh, &s.csrf).await; // response "lost"
    let row = session_for(&db, s.user_id).await;
    assert!(row.previous_refresh_token_hash.is_some());
    assert!(row.refresh_rotated_at.is_some());

    let retry = refresh_call(&app, &s.refresh, &s.csrf).await;
    assert_eq!(retry.status(), StatusCode::OK, "grace retry accepted");
    assert!(extract_cookie(&retry, "__Host-comic_session").is_some());
    let t2 = extract_cookie(&retry, "__Host-comic_refresh").unwrap();
    assert_ne!(t2, t1);
    let row = session_for(&db, s.user_id).await;
    assert!(
        row.previous_refresh_token_hash.is_none(),
        "the grace path consumes the previous token"
    );

    // The token whose response was lost is dead; one line remains.
    assert_refused(refresh_call(&app, &t1, &s.csrf).await, "T1 retired").await;
    let t3 = rotate(&app, &t2, &s.csrf).await;
    assert_ne!(t3, t2);
}

/// (2) The same retry after the window is refused, and the refusal does not
/// harm the live token. Control: just inside the window it is accepted.
#[tokio::test]
async fn previous_token_after_the_window_is_refused() {
    let app = TestApp::spawn().await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();

    let s = register(&app, "late@example.com").await;
    let t1 = rotate(&app, &s.refresh, &s.csrf).await;
    rotated_ago(&db, s.user_id, grace_secs() + 5).await;
    assert_refused(
        refresh_call(&app, &s.refresh, &s.csrf).await,
        "previous token past the window",
    )
    .await;
    let row = session_for(&db, s.user_id).await;
    assert!(row.revoked_at.is_none(), "a refusal revokes nothing");
    rotate(&app, &t1, &s.csrf).await;

    // Control.
    let c = register(&app, "late-control@example.com").await;
    rotate(&app, &c.refresh, &c.csrf).await;
    rotated_ago(&db, c.user_id, grace_secs() - 5).await;
    rotate(&app, &c.refresh, &c.csrf).await;
}

/// A token older than the previous one is refused even inside the window.
/// Control: the previous token itself is accepted.
#[tokio::test]
async fn a_token_older_than_the_previous_one_is_refused() {
    let app = TestApp::spawn().await;
    let s = register(&app, "older@example.com").await;
    let t0 = s.refresh.clone();
    let t1 = rotate(&app, &t0, &s.csrf).await;
    let t2 = rotate(&app, &t1, &s.csrf).await;

    assert_refused(
        refresh_call(&app, &t0, &s.csrf).await,
        "T0 two rotations back",
    )
    .await;
    // Control: T1 is the previous token, inside the window.
    let t3 = rotate(&app, &t1, &s.csrf).await;
    // ... which retired T2.
    assert_refused(refresh_call(&app, &t2, &s.csrf).await, "T2 retired").await;
    rotate(&app, &t3, &s.csrf).await;
}

/// (5) The grace path taken twice with the same previous token: the second
/// attempt is refused and does not disturb the token the first one issued.
#[tokio::test]
async fn the_grace_is_single_use_per_rotation() {
    let app = TestApp::spawn().await;
    let s = register(&app, "twice@example.com").await;
    rotate(&app, &s.refresh, &s.csrf).await;

    let t2 = rotate(&app, &s.refresh, &s.csrf).await; // first grace use
    assert_refused(
        refresh_call(&app, &s.refresh, &s.csrf).await,
        "second grace use of the same token",
    )
    .await;
    rotate(&app, &t2, &s.csrf).await;
}

/// (3) Several refreshes carrying the same token queued on the row lock:
/// the first rotates normally, the second takes the grace path, the third
/// finds the grace consumed. Of the two tokens issued, exactly one works.
#[tokio::test]
async fn queued_refreshes_with_one_token_leave_one_valid_token() {
    let app = TestApp::spawn().await;
    let s = register(&app, "queued@example.com").await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();

    let holder = db.begin().await.unwrap();
    holder
        .execute_unprepared(&format!(
            "SELECT id FROM auth_sessions WHERE user_id = '{}' FOR UPDATE",
            s.user_id
        ))
        .await
        .unwrap();
    let (a, b, c, ()) = tokio::join!(
        refresh_call(&app, &s.refresh, &s.csrf),
        refresh_call(&app, &s.refresh, &s.csrf),
        refresh_call(&app, &s.refresh, &s.csrf),
        async {
            wait_for_lock_waiters(&db, 3).await;
            holder.commit().await.unwrap();
        }
    );
    let mut statuses = [a.status(), b.status(), c.status()];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::OK, StatusCode::OK, StatusCode::UNAUTHORIZED],
        "one normal rotation, one grace use, one refusal"
    );
    let issued: Vec<String> = [&a, &b, &c]
        .iter()
        .filter_map(|r| extract_cookie(r, "__Host-comic_refresh"))
        .collect();
    assert_eq!(issued.len(), 2);

    // From the row, without presenting any token: exactly one issued token
    // is current, and the grace use left no previous token.
    let row = session_for(&db, s.user_id).await;
    let current: Vec<&String> = issued
        .iter()
        .filter(|t| sha256_hex(t) == row.refresh_token_hash)
        .collect();
    assert_eq!(current.len(), 1, "exactly one issued token is current");
    assert!(row.previous_refresh_token_hash.is_none());

    assert_refused(refresh_call(&app, &s.refresh, &s.csrf).await, "T0 spent").await;
    rotate(&app, current[0], &s.csrf).await;
}

/// The current token and the previous token of one session, queued on the
/// locked row: whichever runs first wins (a normal rotation or the grace),
/// the other gets 401, and one token line remains.
#[tokio::test]
async fn current_and_previous_token_queued_together_have_one_winner() {
    let app = TestApp::spawn().await;
    let s = register(&app, "cur-prev@example.com").await;
    let t1 = rotate(&app, &s.refresh, &s.csrf).await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();

    let holder = db.begin().await.unwrap();
    holder
        .execute_unprepared(&format!(
            "SELECT id FROM auth_sessions WHERE user_id = '{}' FOR UPDATE",
            s.user_id
        ))
        .await
        .unwrap();
    let (with_current, with_previous, ()) = tokio::join!(
        refresh_call(&app, &t1, &s.csrf),
        refresh_call(&app, &s.refresh, &s.csrf),
        async {
            wait_for_lock_waiters(&db, 2).await;
            holder.commit().await.unwrap();
        }
    );
    let mut statuses = [with_current.status(), with_previous.status()];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::UNAUTHORIZED]);
    let current_won = with_current.status() == StatusCode::OK;
    let (winner, loser) = if current_won {
        (with_current, with_previous)
    } else {
        (with_previous, with_current)
    };
    let issued = extract_cookie(&winner, "__Host-comic_refresh").unwrap();
    assert_refused(loser, "the second of the two").await;

    let row = session_for(&db, s.user_id).await;
    assert_eq!(row.refresh_token_hash, sha256_hex(&issued));
    if current_won {
        // A normal rotation: T1 is now the previous token, T0 is gone.
        assert_eq!(row.previous_refresh_token_hash, Some(sha256_hex(&t1)));
    } else {
        // The grace: nothing previous remains, and T1 is retired.
        assert!(row.previous_refresh_token_hash.is_none());
    }
    assert_refused(refresh_call(&app, &s.refresh, &s.csrf).await, "T0").await;
    rotate(&app, &issued, &s.csrf).await;
}

/// A password change keeps the caller's session but drops its previous
/// token: inside the window that token no longer gets the grace. Control:
/// the same token is accepted inside the window without the change (see
/// `lost_response_retry_with_the_previous_token_is_accepted_once`), and the
/// kept session's current token still refreshes.
#[tokio::test]
async fn a_password_change_drops_the_kept_sessions_previous_token() {
    let app = TestApp::spawn().await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let s = register(&app, "pw-grace@example.com").await;
    let resp = refresh_call(&app, &s.refresh, &s.csrf).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let t1 = extract_cookie(&resp, "__Host-comic_refresh").unwrap();
    let access = extract_cookie(&resp, "__Host-comic_session").unwrap();
    let csrf = extract_cookie(&resp, "__Host-comic_csrf").unwrap();
    assert!(
        session_for(&db, s.user_id)
            .await
            .previous_refresh_token_hash
            .is_some()
    );

    let changed = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/api/me/account")
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={access}; __Host-comic_csrf={csrf}; __Host-comic_refresh={t1}"
                    ),
                )
                .header("x-csrf-token", &csrf)
                .body(Body::from(
                    r#"{"current_password":"correctly-horse-battery","new_password":"another-horse-battery-2"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::OK);

    let row = session_for(&db, s.user_id).await;
    assert!(row.revoked_at.is_none(), "the caller's session is kept");
    assert!(row.previous_refresh_token_hash.is_none());
    assert_refused(
        refresh_call(&app, &s.refresh, &csrf).await,
        "previous token after a password change, inside the window",
    )
    .await;
    rotate(&app, &t1, &csrf).await;
}

/// (4) + (6) Someone else presents the previous token inside the window.
/// Before the real client's retry: they get a pair, and the real client's
/// retry with the same token is refused. If the real client had received
/// T1, its next refresh with T1 is refused too (6). Outside the window the
/// previous token gets nothing (control: `previous_token_after_the_window_is_refused`).
#[tokio::test]
async fn a_previous_token_used_by_someone_else_inside_the_window() {
    let app = TestApp::spawn().await;
    let s = register(&app, "stolen@example.com").await;
    let t1 = rotate(&app, &s.refresh, &s.csrf).await;

    // The other party, inside the window.
    let other = rotate(&app, &s.refresh, &s.csrf).await;

    // (4) The real client retries with the same previous token: refused.
    assert_refused(
        refresh_call(&app, &s.refresh, &s.csrf).await,
        "real client's retry after the grace was used",
    )
    .await;
    // (6) The real client holding T1: refused.
    assert_refused(refresh_call(&app, &t1, &s.csrf).await, "T1 retired").await;
    // Only the other party's line remains, until it is revoked.
    rotate(&app, &other, &s.csrf).await;
}

/// (4) The previous token presented by someone else after the real client
/// already retried inside the window: refused.
#[tokio::test]
async fn a_previous_token_presented_after_the_real_retry_is_refused() {
    let app = TestApp::spawn().await;
    let s = register(&app, "after-retry@example.com").await;
    rotate(&app, &s.refresh, &s.csrf).await;
    let real = rotate(&app, &s.refresh, &s.csrf).await; // real client's retry
    assert_refused(
        refresh_call(&app, &s.refresh, &s.csrf).await,
        "previous token after the real retry",
    )
    .await;
    rotate(&app, &real, &s.csrf).await;
}

// ───── shared checks on the grace path ─────

/// A disabled or pending account cannot take the grace path; the rotation
/// rolls back, so the grace is still there once the account is active
/// again (control).
#[tokio::test]
async fn grace_path_refuses_an_account_that_is_not_active() {
    let app = TestApp::spawn().await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();

    for (i, state) in ["disabled", "pending_verification"].into_iter().enumerate() {
        let s = register(&app, &format!("inactive{i}@example.com")).await;
        rotate(&app, &s.refresh, &s.csrf).await;
        let before = session_for(&db, s.user_id).await;

        set_state(&db, s.user_id, state).await;
        assert_refused(
            refresh_call(&app, &s.refresh, &s.csrf).await,
            &format!("grace path for a {state} account"),
        )
        .await;
        let after = session_for(&db, s.user_id).await;
        assert_eq!(after.refresh_token_hash, before.refresh_token_hash);
        assert_eq!(
            after.previous_refresh_token_hash,
            before.previous_refresh_token_hash
        );

        set_state(&db, s.user_id, "active").await;
        rotate(&app, &s.refresh, &s.csrf).await;
    }
}

/// The absolute session age applies to the grace path: an over-age session
/// is refused (and revoked). Control: inside the age the grace is accepted.
#[tokio::test]
async fn grace_path_respects_the_absolute_session_age() {
    let app = TestApp::spawn().await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let age = |user_id: uuid::Uuid, days: i64| {
        let db = db.clone();
        async move {
            db.execute_unprepared(&format!(
                "UPDATE auth_sessions SET created_at = now() - interval '{days} days' \
                 WHERE user_id = '{user_id}'"
            ))
            .await
            .unwrap();
        }
    };

    let s = register(&app, "aged@example.com").await;
    rotate(&app, &s.refresh, &s.csrf).await;
    age(s.user_id, 91).await;
    assert_refused(
        refresh_call(&app, &s.refresh, &s.csrf).await,
        "grace path on an over-age session",
    )
    .await;
    assert!(session_for(&db, s.user_id).await.revoked_at.is_some());

    let c = register(&app, "aged-control@example.com").await;
    rotate(&app, &c.refresh, &c.csrf).await;
    age(c.user_id, 89).await;
    rotate(&app, &c.refresh, &c.csrf).await;
    let row = session_for(&db, c.user_id).await;
    assert!(row.expires_at <= row.created_at + chrono::Duration::days(90));
}

/// A revoked session offers no grace; an unrevoked one does (control).
#[tokio::test]
async fn grace_path_refuses_a_revoked_session() {
    let app = TestApp::spawn().await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let s = register(&app, "revoked@example.com").await;
    rotate(&app, &s.refresh, &s.csrf).await;
    db.execute_unprepared(&format!(
        "UPDATE auth_sessions SET revoked_at = now() WHERE user_id = '{}'",
        s.user_id
    ))
    .await
    .unwrap();
    assert_refused(
        refresh_call(&app, &s.refresh, &s.csrf).await,
        "grace on a revoked session",
    )
    .await;

    // Control: the same steps without the revocation.
    let c = register(&app, "revoked-control@example.com").await;
    rotate(&app, &c.refresh, &c.csrf).await;
    rotate(&app, &c.refresh, &c.csrf).await;
}

/// CSRF is checked before the grace path, and a CSRF failure does not use
/// up the grace. The grace path records the user agent and slides the
/// expiry like a normal refresh.
#[tokio::test]
async fn grace_path_is_csrf_checked_and_records_the_request() {
    let app = TestApp::spawn().await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let s = register(&app, "csrf-grace@example.com").await;
    rotate(&app, &s.refresh, &s.csrf).await;

    let no_header = refresh_with(&app, &s.refresh, &s.csrf, false, None).await;
    assert_eq!(no_header.status(), StatusCode::FORBIDDEN);

    db.execute_unprepared(&format!(
        "UPDATE auth_sessions SET expires_at = now() + interval '1 hour', \
                last_used_at = now() - interval '1 hour' WHERE user_id = '{}'",
        s.user_id
    ))
    .await
    .unwrap();
    let before = session_for(&db, s.user_id).await;
    let ok = refresh_with(&app, &s.refresh, &s.csrf, true, Some("grace-agent/1.0")).await;
    assert_eq!(ok.status(), StatusCode::OK, "control: with the header");
    let after = session_for(&db, s.user_id).await;
    assert_eq!(after.user_agent.as_deref(), Some("grace-agent/1.0"));
    assert!(after.last_used_at > before.last_used_at);
    assert!(
        after.expires_at > before.expires_at + chrono::Duration::days(1),
        "sliding expiry moved forward"
    );
}

/// Signing out with the previous token inside the window ends the session
/// (the grace would otherwise keep it reachable). Control: outside the
/// window the previous token matches nothing and the session lives on.
#[tokio::test]
async fn logout_with_the_previous_token_inside_the_window_revokes_the_session() {
    let app = TestApp::spawn().await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let logout = |rt: String, csrf: String| {
        let router = app.router.clone();
        async move {
            router
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/auth/logout")
                        .header(
                            header::COOKIE,
                            format!("__Host-comic_csrf={csrf}; __Host-comic_refresh={rt}"),
                        )
                        .header("x-csrf-token", csrf)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
        }
    };

    let s = register(&app, "logout-grace@example.com").await;
    let t1 = rotate(&app, &s.refresh, &s.csrf).await;
    let out = logout(s.refresh.clone(), s.csrf.clone()).await;
    assert_eq!(out.status(), StatusCode::NO_CONTENT);
    assert!(session_for(&db, s.user_id).await.revoked_at.is_some());
    assert_refused(
        refresh_call(&app, &s.refresh, &s.csrf).await,
        "T0 after logout",
    )
    .await;
    assert_refused(refresh_call(&app, &t1, &s.csrf).await, "T1 after logout").await;

    let c = register(&app, "logout-late@example.com").await;
    let t1 = rotate(&app, &c.refresh, &c.csrf).await;
    rotated_ago(&db, c.user_id, grace_secs() + 5).await;
    let out = logout(c.refresh.clone(), c.csrf.clone()).await;
    assert_eq!(out.status(), StatusCode::NO_CONTENT);
    assert!(session_for(&db, c.user_id).await.revoked_at.is_none());
    rotate(&app, &t1, &c.csrf).await;
}

/// `/me/sessions` marks the session as this device when the cookie holds
/// its previous token inside the window (the browser whose response was
/// lost), and not after the window.
#[tokio::test]
async fn sessions_list_marks_this_device_by_the_previous_token_in_the_window() {
    let app = TestApp::spawn().await;
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let s = register(&app, "this-device@example.com").await;
    let resp = refresh_call(&app, &s.refresh, &s.csrf).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let access = extract_cookie(&resp, "__Host-comic_session").unwrap();

    let listed_current = |rt: String| {
        let router = app.router.clone();
        let access = access.clone();
        async move {
            let resp = router
                .oneshot(
                    Request::builder()
                        .uri("/api/me/sessions")
                        .header(
                            header::COOKIE,
                            format!("__Host-comic_session={access}; __Host-comic_refresh={rt}"),
                        )
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let body = body_json(resp.into_body()).await;
            body["sessions"][0]["current"].as_bool().unwrap()
        }
    };

    assert!(listed_current(s.refresh.clone()).await, "inside the window");
    rotated_ago(&db, s.user_id, grace_secs() + 5).await;
    assert!(!listed_current(s.refresh.clone()).await, "after the window");
}
