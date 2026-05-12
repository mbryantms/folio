//! Background pruner for expired `auth_sessions` rows (M3, audit S-11).
//!
//! Verifies the pruner deletes rows whose `expires_at` is older than the
//! grace window, and leaves recent / unexpired rows alone.

mod common;

use chrono::{Duration, Utc};
use common::TestApp;
use entity::auth_session::{ActiveModel as SessionAM, Entity as SessionEntity};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use uuid::Uuid;

fn make_session(user_id: Uuid, expires_at: chrono::DateTime<chrono::FixedOffset>) -> SessionAM {
    let now = Utc::now().fixed_offset();
    SessionAM {
        id: Set(Uuid::now_v7()),
        user_id: Set(user_id),
        refresh_token_hash: Set(format!("hash-{}", Uuid::now_v7())),
        created_at: Set(now),
        last_used_at: Set(now),
        expires_at: Set(expires_at),
        user_agent: Set(None),
        ip: Set(None),
        revoked_at: Set(None),
        id_token_hint: Set(None),
    }
}

#[tokio::test]
async fn prune_removes_long_expired_sessions_only() {
    let app = TestApp::spawn().await;
    let state = app.state();

    // Need a real users row for the FK. Cheapest path: register one via the
    // public endpoint and pull the id out of /auth/me — but we can also
    // just INSERT directly. Using the public endpoint keeps the test in
    // step with the actual schema.
    use axum::body::Body;
    use axum::http::{Method, Request, header};
    use tower::ServiceExt;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"prune@example.com","password":"correctly-horse-battery"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::CREATED);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let user_id = Uuid::parse_str(body["user"]["id"].as_str().unwrap()).unwrap();

    // Three rows:
    //   - 30 days past expiry  → should be deleted (well past 7-day grace)
    //   - 1 day past expiry    → should be kept (still inside grace window)
    //   - 1 hour from expiry   → should be kept (still active)
    let now = Utc::now().fixed_offset();
    let long_expired = now - Duration::days(30);
    let recent_expired = now - Duration::days(1);
    let active = now + Duration::hours(1);

    // Register also inserted one session, so we already have a "current"
    // row. We're adding three more with controlled expires_at values.
    let long_id = make_session(user_id, long_expired)
        .insert(&state.db)
        .await
        .unwrap()
        .id;
    let recent_id = make_session(user_id, recent_expired)
        .insert(&state.db)
        .await
        .unwrap()
        .id;
    let active_id = make_session(user_id, active)
        .insert(&state.db)
        .await
        .unwrap()
        .id;

    let deleted = server::jobs::prune_auth_sessions::run(&state.db)
        .await
        .unwrap();
    assert_eq!(
        deleted, 1,
        "exactly one row (30d past expiry) should be deleted"
    );

    // Long-expired row gone.
    let lookup = SessionEntity::find_by_id(long_id)
        .one(&state.db)
        .await
        .unwrap();
    assert!(lookup.is_none(), "long-expired session should be deleted");

    // Recent (within grace) survives.
    let lookup = SessionEntity::find_by_id(recent_id)
        .one(&state.db)
        .await
        .unwrap();
    assert!(
        lookup.is_some(),
        "recently-expired session should survive grace window"
    );

    // Active survives.
    let lookup = SessionEntity::find_by_id(active_id)
        .one(&state.db)
        .await
        .unwrap();
    assert!(lookup.is_some(), "non-expired session should survive");
}
