//! `DELETE /me` — self-service account deletion (audit 2026-11 F5).
//!
//! Covers re-authentication (password for local accounts, typed
//! confirmation for identity-provider accounts, never an app password),
//! the last-admin refusal, the row-by-row deletion (every table that
//! references the user is empty afterwards; other users' rows survive),
//! the audit trail (old rows kept with the actor detached, a new
//! `account.self_delete` row carrying an email hash), credential death
//! (old session, refresh and password all fail), and the schema guard that
//! keeps the deletion list complete as tables are added.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{
    seed_collection, seed_collection_entry_issue, seed_issue, seed_library, seed_progress,
    seed_series,
};
use entity::{audit_log, marker, rail_dismissal, reading_session, user, user_rating};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait,
    QueryFilter, Set, Statement,
};
use serde_json::{Value, json};
use server::api::account_delete::{KEPT_USER_REFERENCES, UNCONSTRAINED_USER_DELETES};
use server::metadata::identifier::Source;
use server::metadata::matcher::SeriesQueryFacts;
use server::metadata::orchestrator::{self, StartRunArgs, StoredQuery};
use tower::ServiceExt;
use uuid::Uuid;

const PASSWORD: &str = "correctly-horse-battery";

struct Authed {
    session: String,
    csrf: String,
    refresh: String,
    user_id: Uuid,
}

impl Authed {
    fn cookies(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}; __Host-comic_refresh={}",
            self.session, self.csrf, self.refresh
        )
    }
}

fn set_cookies(resp: &Response<Body>) -> Vec<String> {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_owned)
        .collect()
}

fn cookie_value(cookies: &[String], name: &str) -> String {
    let prefix = format!("{name}=");
    cookies
        .iter()
        .find_map(|s| {
            s.split(';')
                .next()
                .and_then(|kv| kv.strip_prefix(&prefix))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| panic!("expected cookie {name}"))
}

async fn body_json(b: Body) -> Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

async fn auth_call(app: &TestApp, uri: &str, email: &str) -> Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"email": email, "password": PASSWORD}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

fn authed_from(resp: &Response<Body>, user_id: Uuid) -> Authed {
    let cookies = set_cookies(resp);
    Authed {
        session: cookie_value(&cookies, "__Host-comic_session"),
        csrf: cookie_value(&cookies, "__Host-comic_csrf"),
        refresh: cookie_value(&cookies, "__Host-comic_refresh"),
        user_id,
    }
}

async fn register(app: &TestApp, email: &str) -> Authed {
    let resp = auth_call(app, "/auth/local/register", email).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let mut authed = authed_from(&resp, Uuid::nil());
    let json = body_json(resp.into_body()).await;
    authed.user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    authed
}

async fn login(app: &TestApp, email: &str) -> Response<Body> {
    auth_call(app, "/auth/local/login", email).await
}

async fn delete_me(app: &TestApp, auth: &Authed, body: Value) -> (StatusCode, Vec<String>, Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/me")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookies())
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let cookies = set_cookies(&resp);
    (status, cookies, body_json(resp.into_body()).await)
}

async fn get_me_status(app: &TestApp, auth: &Authed) -> StatusCode {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/auth/me")
                .header(header::COOKIE, auth.cookies())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// `POST /auth/refresh` with only the refresh + CSRF cookies (no access
/// cookie), the way a browser whose access token expired would call it.
async fn refresh_status(app: &TestApp, auth: &Authed) -> StatusCode {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/refresh")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_refresh={}; __Host-comic_csrf={}",
                        auth.refresh, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn user_exists(db: &DatabaseConnection, id: Uuid) -> bool {
    user::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .is_some()
}

async fn count_where(db: &DatabaseConnection, table: &str, column: &str, id: Uuid) -> i64 {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("SELECT COUNT(*)::bigint FROM {table} WHERE {column} = $1"),
            [id.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get_by_index::<i64>(0).unwrap()
}

/// Every `(table, column)` with an FK to `users(id)`, plus its delete rule
/// (`c` cascade, `n` set null, …).
async fn user_fks(db: &DatabaseConnection) -> Vec<(String, String, String)> {
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT c.conrelid::regclass::text, a.attname::text, c.confdeltype::text \
             FROM pg_constraint c \
             JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = ANY (c.conkey) \
             WHERE c.contype = 'f' AND c.confrelid = 'users'::regclass \
             ORDER BY 1, 2",
        ))
        .await
        .unwrap();
    rows.iter()
        .map(|r| {
            (
                r.try_get_by_index(0).unwrap(),
                r.try_get_by_index(1).unwrap(),
                r.try_get_by_index(2).unwrap(),
            )
        })
        .collect()
}

async fn set_role(db: &DatabaseConnection, id: Uuid, role: user::UserRole) {
    let mut am: user::ActiveModel = user::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.role = Set(role);
    am.update(db).await.unwrap();
}

// ───────── re-authentication ─────────

#[tokio::test]
async fn wrong_or_missing_password_is_refused_and_nothing_is_deleted() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let _admin = register(&app, "admin@example.com").await;
    let a = register(&app, "reader@example.com").await;

    let (status, _, body) = delete_me(&app, &a, json!({"password": "not-the-password"})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "auth.invalid");

    let (status, _, body) = delete_me(&app, &a, json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["details"][0]["field"], "password");

    // A typed confirmation is no substitute for the password.
    let (status, _, _) = delete_me(&app, &a, json!({"confirm": "reader@example.com"})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    assert!(user_exists(db, a.user_id).await);
    assert_eq!(get_me_status(&app, &a).await, StatusCode::OK);
}

#[tokio::test]
async fn csrf_is_required() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let a = register(&app, "reader@example.com").await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/me")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, a.cookies())
                .body(Body::from(json!({"password": PASSWORD}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(user_exists(&app.state().db, a.user_id).await);
}

#[tokio::test]
async fn app_password_cannot_delete_the_account() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let a = register(&app, "reader@example.com").await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/me/app-passwords")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, a.cookies())
                .header("X-CSRF-Token", &a.csrf)
                .body(Body::from(json!({"label": "kobo"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let token = body_json(resp.into_body()).await["plaintext"]
        .as_str()
        .unwrap()
        .to_owned();

    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/me")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(json!({"password": PASSWORD}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error"]["code"], "auth.permission_denied");
    assert!(user_exists(&app.state().db, a.user_id).await);
}

/// Identity-provider accounts have no password Folio can check, so they
/// confirm by typing their email (case-insensitive).
#[tokio::test]
async fn oidc_account_confirms_by_typing_its_email() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let _admin = register(&app, "admin@example.com").await;
    let a = register(&app, "oidc-reader@example.com").await;
    // Turn the local account into an OIDC one, as an issuer-created row
    // looks: `oidc:` external id, no password. The session stays valid.
    let mut am: user::ActiveModel = user::Entity::find_by_id(a.user_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.external_id = Set("oidc:https://issuer.test|sub-123".into());
    am.password_hash = Set(None);
    am.update(db).await.unwrap();

    let (status, _, body) = delete_me(&app, &a, json!({"password": PASSWORD})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["details"][0]["field"], "confirm");

    let (status, _, _) = delete_me(&app, &a, json!({"confirm": "someone@else.com"})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(user_exists(db, a.user_id).await);

    let (status, _, body) =
        delete_me(&app, &a, json!({"confirm": " OIDC-Reader@Example.com "})).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert!(!user_exists(db, a.user_id).await);

    let row = audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("account.self_delete"))
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.payload["reauth"], "confirmation");
}

// ───────── last admin ─────────

#[tokio::test]
async fn last_active_admin_gets_409() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let admin = register(&app, "admin@example.com").await;
    let second = register(&app, "second@example.com").await;

    let (status, _, body) = delete_me(&app, &admin, json!({"password": PASSWORD})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "conflict");
    assert!(user_exists(db, admin.user_id).await);

    // A disabled admin can't sign in, so it doesn't count as the remaining
    // one.
    set_role(db, second.user_id, user::UserRole::Admin).await;
    let mut am: user::ActiveModel = user::Entity::find_by_id(second.user_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.state = Set(user::UserState::Disabled);
    am.update(db).await.unwrap();
    let (status, _, _) = delete_me(&app, &admin, json!({"password": PASSWORD})).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // With another active admin the first may go.
    let mut am: user::ActiveModel = user::Entity::find_by_id(second.user_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.state = Set(user::UserState::Active);
    am.update(db).await.unwrap();
    let (status, _, body) = delete_me(&app, &admin, json!({"password": PASSWORD})).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert!(!user_exists(db, admin.user_id).await);
    assert!(user_exists(db, second.user_id).await);
}

// ───────── the deletion itself ─────────

/// Give `user_id` one row in as many personal tables as practical. The
/// assertion below doesn't depend on this list being complete — it checks
/// every user-referencing column in the schema — but the seeded ones prove
/// rows really were there to delete.
async fn seed_personal_rows(
    db: &DatabaseConnection,
    tmp: &std::path::Path,
    user_id: Uuid,
    other_id: Uuid,
) -> (Uuid, String) {
    let now = Utc::now().fixed_offset();
    let lib_id = seed_library(db, tmp).await;
    let series_id = seed_series(db, lib_id, "Deletable").await;
    let issue_id = seed_issue(db, lib_id, series_id, &tmp.join("a.cbz"), b"delete-a", 1.0).await;

    seed_progress(db, user_id, &issue_id, 3, 0.2, false).await;
    // The other user's progress on the same issue must survive.
    seed_progress(db, other_id, &issue_id, 1, 0.05, false).await;

    marker::ActiveModel {
        id: Set(Uuid::now_v7()),
        user_id: Set(user_id),
        series_id: Set(series_id),
        issue_id: Set(issue_id.clone()),
        page_index: Set(1),
        kind: Set(entity::marker::MarkerKind::Note),
        is_favorite: Set(false),
        tags: Set(vec![]),
        region: Set(None),
        selection: Set(None),
        body: Set(Some("private note".into())),
        color: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        hidden_from_log: Set(false),
        page_hash: Set(None),
    }
    .insert(db)
    .await
    .unwrap();

    let collection_id = seed_collection(db, user_id, "Mine").await;
    seed_collection_entry_issue(db, collection_id, 0, &issue_id).await;

    user_rating::ActiveModel {
        user_id: Set(user_id),
        target_type: Set("issue".into()),
        target_id: Set(issue_id.clone()),
        rating: Set(4.0),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();

    rail_dismissal::ActiveModel {
        user_id: Set(user_id),
        target_kind: Set("issue".into()),
        target_id: Set(issue_id.clone()),
        dismissed_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();

    reading_session::ActiveModel {
        id: Set(Uuid::now_v7()),
        user_id: Set(user_id),
        issue_id: Set(issue_id.clone()),
        series_id: Set(series_id),
        client_session_id: Set("client-1".into()),
        started_at: Set(now),
        ended_at: Set(Some(now)),
        last_heartbeat_at: Set(now),
        active_ms: Set(60_000),
        distinct_pages_read: Set(4),
        page_turns: Set(4),
        start_page: Set(0),
        end_page: Set(3),
        furthest_page: Set(3),
        device: Set(None),
        view_mode: Set(None),
        client_meta: Set(json!({})),
        hidden_from_log: Set(false),
    }
    .insert(db)
    .await
    .unwrap();

    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO user_issue_notes (user_id, issue_id, body, created_at, updated_at) \
         VALUES ($1, $2, 'closing note', now(), now())",
        [user_id.into(), issue_id.clone().into()],
    ))
    .await
    .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO library_user_access (library_id, user_id) VALUES ($1, $2)",
        [lib_id.into(), user_id.into()],
    ))
    .await
    .unwrap();

    (series_id, issue_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deletion_removes_every_personal_row_and_keeps_the_audit_trail() {
    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let _admin = register(&app, "admin@example.com").await;
    let a = register(&app, "leaving@example.com").await;
    let b = register(&app, "staying@example.com").await;
    seed_personal_rows(&db, tmp.path(), a.user_id, b.user_id).await;

    // A second signed-in device: its session and refresh token must die too.
    let second = login(&app, "leaving@example.com").await;
    assert_eq!(second.status(), StatusCode::OK);
    let second = authed_from(&second, a.user_id);

    // An earlier audit row with A as the actor (a display-name change).
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/api/me/account")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, a.cookies())
                .header("X-CSRF-Token", &a.csrf)
                .body(Body::from(json!({"display_name": "Leaver"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let earlier_audit = count_where(&db, "audit_log", "actor_id", a.user_id).await;
    assert!(earlier_audit >= 1);

    // An attribution pointer on a shared row (`ON DELETE SET NULL`).
    let attributed = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE app_setting SET updated_by = $1 \
             WHERE key = (SELECT key FROM app_setting ORDER BY key LIMIT 1)",
            [a.user_id.into()],
        ))
        .await
        .unwrap()
        .rows_affected();

    let fks = user_fks(&db).await;
    let owned: Vec<(String, String)> = fks
        .iter()
        .filter(|(_, _, rule)| rule == "c")
        .map(|(t, c, _)| (t.clone(), c.clone()))
        .chain(
            UNCONSTRAINED_USER_DELETES
                .iter()
                .map(|(t, c)| ((*t).to_owned(), (*c).to_owned())),
        )
        .collect();

    // Rows that must exist before (seeded above, or created by register /
    // login): proves the "after" zeros mean something.
    for table in [
        "progress_records",
        "library_user_access",
        "markers",
        "saved_views",
        "user_ratings",
        "rail_dismissals",
        "reading_sessions",
        "user_issue_notes",
        "auth_sessions",
    ] {
        let (_, column) = owned
            .iter()
            .find(|(t, _)| t == table)
            .unwrap_or_else(|| panic!("{table} is not on the deletion list"));
        assert!(
            count_where(&db, table, column, a.user_id).await > 0,
            "{table} should have a seeded row for A"
        );
    }
    let b_progress = count_where(&db, "progress_records", "user_id", b.user_id).await;
    assert_eq!(b_progress, 1);

    let (status, cookies, body) = delete_me(&app, &a, json!({"password": PASSWORD})).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    for name in [
        "__Host-comic_session",
        "__Host-comic_csrf",
        "__Host-comic_refresh",
    ] {
        let c = cookies
            .iter()
            .find(|c| c.starts_with(&format!("{name}=")))
            .unwrap_or_else(|| panic!("{name} not cleared: {cookies:?}"));
        assert!(c.contains("Max-Age=0"), "{name} not expired: {c}");
    }

    assert!(!user_exists(&db, a.user_id).await);
    for (table, column) in &owned {
        assert_eq!(
            count_where(&db, table, column, a.user_id).await,
            0,
            "{table}.{column} still holds A's rows"
        );
    }
    // The other user is untouched.
    assert!(user_exists(&db, b.user_id).await);
    assert_eq!(
        count_where(&db, "progress_records", "user_id", b.user_id).await,
        b_progress
    );
    if attributed > 0 {
        assert_eq!(
            count_where(&db, "app_setting", "updated_by", a.user_id).await,
            0
        );
    }

    // Audit: the earlier rows survive with the actor detached (no user row
    // behind the id), plus one `account.self_delete` row.
    let rows = audit_log::Entity::find()
        .filter(audit_log::Column::ActorId.eq(a.user_id))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(rows.len() as i64, earlier_audit + 1);
    let deleted = rows
        .iter()
        .find(|r| r.action == "account.self_delete")
        .expect("self-delete audit row");
    assert_eq!(
        deleted.target_id.as_deref(),
        Some(a.user_id.to_string().as_str())
    );
    assert_eq!(deleted.payload["reauth"], "password");
    assert_eq!(
        deleted.payload["email_sha256"],
        server::auth::cookies::sha256_hex("leaving@example.com")
    );
    assert!(
        !deleted.payload.to_string().contains("leaving@example.com"),
        "audit payload must not carry the email: {}",
        deleted.payload
    );

    // Every credential is dead: both sessions' access cookies, both refresh
    // tokens, and the password.
    assert_eq!(get_me_status(&app, &a).await, StatusCode::UNAUTHORIZED);
    assert_eq!(get_me_status(&app, &second).await, StatusCode::UNAUTHORIZED);
    assert_eq!(refresh_status(&app, &a).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        refresh_status(&app, &second).await,
        StatusCode::UNAUTHORIZED
    );
    let (status, _, _) = delete_me(&app, &a, json!({"password": PASSWORD})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        login(&app, "leaving@example.com").await.status(),
        StatusCode::UNAUTHORIZED
    );
}

/// A job that starts a metadata run for an admin who has since deleted
/// their account must not fail on the `triggered_by` FK.
#[tokio::test]
async fn metadata_run_for_a_deleted_user_is_recorded_unattributed() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let gone = Uuid::now_v7();
    let run_id = orchestrator::start_run(
        db,
        StartRunArgs {
            scope: orchestrator::scope::SERIES,
            scope_entity_id: None,
            library_id: None,
            triggered_by: Some(gone),
            trigger_kind: orchestrator::trigger_kind::MANUAL,
            providers: &[Source::Gcd],
            query: StoredQuery::Series(SeriesQueryFacts {
                name: "Orphaned".into(),
                year: None,
                publisher: None,
                volume: None,
                format: None,
            }),
            batch_id: None,
        },
    )
    .await
    .expect("start_run falls back to an unattributed run");
    let run = entity::metadata_run::Entity::find_by_id(run_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.triggered_by, None);
}

// ───────── schema guard ─────────

/// Every FK to `users` must cascade or set null (a `RESTRICT` would make
/// `DELETE /me` fail), and every FK-less column that looks like a user
/// reference must be on one of the two lists in `api::account_delete` —
/// deleted explicitly or kept as history. A new table that stores a user id
/// without an FK fails here until its deletion policy is decided.
#[tokio::test]
async fn every_user_reference_is_accounted_for() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let fks = user_fks(db).await;
    assert!(!fks.is_empty(), "expected FKs referencing users");
    let blocking: Vec<String> = fks
        .iter()
        .filter(|(_, _, rule)| !matches!(rule.as_str(), "c" | "n"))
        .map(|(t, c, rule)| format!("{t}.{c} (confdeltype={rule})"))
        .collect();
    assert!(
        blocking.is_empty(),
        "FKs that would block DELETE /me: {blocking:#?}"
    );

    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT c.table_name::text, c.column_name::text \
             FROM information_schema.columns c \
             JOIN information_schema.tables t \
               ON t.table_schema = c.table_schema AND t.table_name = c.table_name \
             WHERE c.table_schema = 'public' AND t.table_type = 'BASE TABLE' \
               AND (c.data_type = 'uuid' OR c.udt_name = '_uuid') \
               AND (c.column_name LIKE '%user%' OR c.column_name LIKE '%owner%' \
                    OR c.column_name LIKE '%actor%' OR c.column_name LIKE '%member%' \
                    OR c.column_name LIKE '%reader%' OR c.column_name LIKE '%recipient%' \
                    OR c.column_name LIKE '%account%' OR c.column_name LIKE '%\\_by') \
             ORDER BY 1, 2",
        ))
        .await
        .unwrap();
    let unaccounted: Vec<String> = rows
        .iter()
        .map(|r| {
            (
                r.try_get_by_index::<String>(0).unwrap(),
                r.try_get_by_index::<String>(1).unwrap(),
            )
        })
        .filter(|(t, c)| !fks.iter().any(|(ft, fc, _)| ft == t && fc == c))
        .filter(|(t, c)| {
            !UNCONSTRAINED_USER_DELETES
                .iter()
                .chain(KEPT_USER_REFERENCES)
                .any(|(lt, lc)| lt == t && lc == c)
        })
        .map(|(t, c)| format!("{t}.{c}"))
        .collect();
    assert!(
        unaccounted.is_empty(),
        "user-id columns with no FK and no DELETE /me policy \
         (add to UNCONSTRAINED_USER_DELETES or KEPT_USER_REFERENCES): {unaccounted:#?}"
    );
}
