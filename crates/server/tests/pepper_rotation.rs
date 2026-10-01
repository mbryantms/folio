//! Security-audit L-1 / WP-6.3: argon2 pepper rotation via dual-pepper
//! verify-and-rehash.
//!
//! An operator rotates by moving `secrets/pepper` to `secrets/pepper.previous`
//! and restarting; the server mints a new current pepper. Hashes written
//! before the rotation verify against the previous pepper and are rewritten
//! under the current one on that successful verify, so once every active
//! account has signed in the operator can delete `pepper.previous`.

mod common;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use entity::{app_password, user};
use sea_orm::{ActiveModelTrait, ColumnTrait, Database, EntityTrait, QueryFilter, Set};
use server::auth::password;
use tower::ServiceExt;

const PREVIOUS: [u8; 32] = *b"previous-pepper-32-bytes-XXXXXXX";
const EMAIL: &str = "rotate@example.com";
const PASSWORD: &str = "correctly-horse-battery";

async fn register(app: &TestApp) {
    let body = format!(r#"{{"email":"{EMAIL}","password":"{PASSWORD}"}}"#);
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
    assert_eq!(resp.status(), StatusCode::CREATED);
}

async fn login(app: &TestApp, pw: &str) -> StatusCode {
    let body = format!(r#"{{"email":"{EMAIL}","password":"{pw}"}}"#);
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// Rewrite the user's stored hash as if it had been written before the
/// rotation, i.e. under the previous pepper.
async fn backdate_user_hash(db: &sea_orm::DatabaseConnection) -> user::Model {
    let row = user::Entity::find()
        .filter(user::Column::Email.eq(EMAIL))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut am: user::ActiveModel = row.into();
    am.password_hash = Set(Some(password::hash(PASSWORD, &PREVIOUS).unwrap()));
    am.update(db).await.unwrap()
}

#[tokio::test]
async fn login_under_previous_pepper_succeeds_and_rehashes_onto_current() {
    let app = TestApp::spawn_with_previous_pepper(PREVIOUS).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    register(&app).await;
    backdate_user_hash(&db).await;
    let current = app.state().secrets.pepper.to_vec();
    assert_ne!(current.as_slice(), PREVIOUS.as_slice());

    // Wrong password still fails (and doesn't rehash anything).
    assert_eq!(
        login(&app, "wrong-password-entirely").await,
        StatusCode::UNAUTHORIZED
    );

    assert_eq!(login(&app, PASSWORD).await, StatusCode::OK);

    let stored = user::Entity::find()
        .filter(user::Column::Email.eq(EMAIL))
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .password_hash
        .unwrap();
    assert!(
        password::verify(&stored, PASSWORD, &current).unwrap(),
        "hash must now verify under the current pepper"
    );
    assert!(
        !password::verify(&stored, PASSWORD, &PREVIOUS).unwrap(),
        "hash must no longer depend on the previous pepper"
    );
    // And keeps working on the next login.
    assert_eq!(login(&app, PASSWORD).await, StatusCode::OK);
}

#[tokio::test]
async fn without_previous_pepper_old_hashes_do_not_verify() {
    // Control: the fallback exists only while `pepper.previous` is present.
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    register(&app).await;
    backdate_user_hash(&db).await;
    assert_eq!(login(&app, PASSWORD).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn app_password_under_previous_pepper_verifies_and_rehashes() {
    let app = TestApp::spawn_with_previous_pepper(PREVIOUS).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    register(&app).await;
    let owner = user::Entity::find()
        .filter(user::Column::Email.eq(EMAIL))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    // Issued before the rotation → stored under the previous pepper.
    let (id, token) = server::auth::app_password::issue(&db, owner.id, "reader", "read", &PREVIOUS)
        .await
        .unwrap();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let stored = app_password::Entity::find_by_id(id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .hash;
    let current = app.state().secrets.pepper.to_vec();
    assert!(password::verify(&stored, &token, &current).unwrap());
    assert!(!password::verify(&stored, &token, &PREVIOUS).unwrap());
}
