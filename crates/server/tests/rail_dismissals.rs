//! `GET /me/rail-dismissals` — the caller's hidden rail items.
//!
//! Covers:
//!   - Only the caller's rows are listed, newest dismissal first.
//!   - Cursor paging: `next_cursor` walks every row, `total` and
//!     `unavailable` ride the first page only; a bad cursor is a 400.
//!   - Hydration per kind (issue / series / cbl): name, slugs, cover,
//!     saved-view link, the rails the kind hides from.
//!   - Visibility: a dismissal whose target is soft-removed or in a
//!     library the caller lost access to is withheld and counted in
//!     `unavailable`; it comes back when the target does.
//!   - Restore: `DELETE` removes the row from the list.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use common::TestApp;
use common::seed::{CblListSeed, IssueSeed, LibrarySeed, SeriesSeed};
use entity::{issue, library_user_access, rail_dismissal, saved_view, series, user};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Database, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, Set,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
}

async fn body_json(b: Body) -> Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

async fn register(app: &TestApp, email: &str) -> Authed {
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
    assert_eq!(resp.status(), StatusCode::CREATED);
    let cookies: Vec<String> = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_owned)
        .collect();
    let extract = |prefix: &str| -> String {
        cookies
            .iter()
            .find(|c| c.starts_with(prefix))
            .and_then(|c| c.split(';').next())
            .map(|c| c.trim_start_matches(prefix).to_owned())
            .expect(prefix)
    };
    let json = body_json(resp.into_body()).await;
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id: Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap(),
    }
}

/// Register a non-admin: the first registered user becomes admin, so the
/// harness burns that slot first.
async fn register_reader(app: &TestApp, db: &DatabaseConnection, email: &str) -> Authed {
    let a = register(app, email).await;
    let row = user::Entity::find_by_id(a.user_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    if row.role != entity::user::UserRole::User {
        let mut am: user::ActiveModel = row.into();
        am.role = Set(entity::user::UserRole::User);
        am.update(db).await.unwrap();
    }
    a
}

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: &Authed,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf);
    let req = match body {
        Some(b) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn grant(db: &DatabaseConnection, user_id: Uuid, library_id: Uuid) {
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(user_id),
        library_id: Set(library_id),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn dismiss(app: &TestApp, auth: &Authed, kind: &str, id: &str) {
    let (status, body) = http(
        app,
        Method::POST,
        "/api/me/rail-dismissals",
        auth,
        Some(json!({ "target_kind": kind, "target_id": id })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "dismiss {kind}:{id}: {body}"
    );
}

/// Pin a dismissal's timestamp so the newest-first order is deterministic.
async fn stamp(db: &DatabaseConnection, user_id: Uuid, kind: &str, id: &str, minutes_ago: i64) {
    rail_dismissal::ActiveModel {
        user_id: Set(user_id),
        target_kind: Set(kind.into()),
        target_id: Set(id.into()),
        dismissed_at: Set((Utc::now() - Duration::minutes(minutes_ago)).fixed_offset()),
    }
    .update(db)
    .await
    .unwrap();
}

struct Fixture {
    _dir: tempfile::TempDir,
    lib_id: Uuid,
    series_id: Uuid,
    issue_id: String,
}

/// One library with one series ("Saga", 2012) holding one issue titled
/// "Chapter One".
async fn fixture(db: &DatabaseConnection, tag: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let lib_id = LibrarySeed::new(dir.path()).insert(db).await;
    let mut seed = SeriesSeed::new(lib_id, "Saga");
    seed.year = Some(2012);
    let series_id = seed.insert(db).await;
    let path = dir.path().join(format!("{tag}-1.cbz"));
    let payload = format!("payload-{tag}-{}", Uuid::now_v7());
    let issue_id = IssueSeed::new(lib_id, series_id, &path, payload.as_bytes(), 1.0)
        .with_title("Chapter One")
        .insert(db)
        .await;
    Fixture {
        _dir: dir,
        lib_id,
        series_id,
        issue_id,
    }
}

async fn seed_cbl_view(db: &DatabaseConnection, owner: Uuid, list_id: Uuid) -> Uuid {
    let now = Utc::now().fixed_offset();
    let id = Uuid::now_v7();
    saved_view::ActiveModel {
        id: Set(id),
        user_id: Set(Some(owner)),
        kind: Set("cbl".into()),
        system_key: Set(None),
        name: Set("Event order".into()),
        description: Set(None),
        custom_year_start: Set(None),
        custom_year_end: Set(None),
        custom_tags: Set(vec![]),
        match_mode: Set(None),
        conditions: Set(None),
        sort_field: Set(None),
        sort_order: Set(None),
        result_limit: Set(None),
        cbl_list_id: Set(Some(list_id)),
        auto_pin: Set(false),
        preserve_canonical_order: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

fn ids(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn lists_only_the_callers_rows_newest_first_with_cursor_paging() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let _admin = register(&app, "admin@example.com").await;
    let alice = register_reader(&app, &db, "alice@example.com").await;
    let bob = register_reader(&app, &db, "bob@example.com").await;
    let f = fixture(&db, "paging").await;
    grant(&db, alice.user_id, f.lib_id).await;
    grant(&db, bob.user_id, f.lib_id).await;
    let list_id = CblListSeed::new(alice.user_id, "Event").insert(&db).await;
    let series_id = f.series_id.to_string();
    let list = list_id.to_string();

    dismiss(&app, &alice, "issue", &f.issue_id).await;
    dismiss(&app, &alice, "series", &series_id).await;
    dismiss(&app, &alice, "cbl", &list).await;
    dismiss(&app, &bob, "series", &series_id).await;
    stamp(&db, alice.user_id, "issue", &f.issue_id, 30).await;
    stamp(&db, alice.user_id, "series", &series_id, 20).await;
    stamp(&db, alice.user_id, "cbl", &list, 10).await;

    let (status, first) = http(
        &app,
        Method::GET,
        "/api/me/rail-dismissals?limit=2",
        &alice,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(
        ids(&first),
        vec![format!("cbl:{list}"), format!("series:{series_id}")]
    );
    assert_eq!(first["total"], 3);
    assert_eq!(first["unavailable"], 0);
    let cursor = first["next_cursor"]
        .as_str()
        .expect("second page")
        .to_owned();

    let (status, second) = http(
        &app,
        Method::GET,
        &format!("/api/me/rail-dismissals?limit=2&cursor={cursor}"),
        &alice,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(ids(&second), vec![format!("issue:{}", f.issue_id)]);
    assert!(second["next_cursor"].is_null());
    assert!(second.get("total").is_none(), "total is first-page only");
    assert!(second.get("unavailable").is_none());

    // Bob sees his one row, never Alice's.
    let (_, bobs) = http(&app, Method::GET, "/api/me/rail-dismissals", &bob, None).await;
    assert_eq!(ids(&bobs), vec![format!("series:{series_id}")]);
    assert_eq!(bobs["total"], 1);

    // A forged cursor is rejected, not silently restarted.
    let (status, _) = http(
        &app,
        Method::GET,
        "/api/me/rail-dismissals?cursor=not-a-cursor",
        &alice,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn hydrates_each_kind() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let _admin = register(&app, "admin@example.com").await;
    let alice = register_reader(&app, &db, "alice@example.com").await;
    let f = fixture(&db, "hydrate").await;
    grant(&db, alice.user_id, f.lib_id).await;
    let list_id = CblListSeed::new(alice.user_id, "Event").insert(&db).await;
    let view_id = seed_cbl_view(&db, alice.user_id, list_id).await;
    let series_id = f.series_id.to_string();
    let series_slug = f.series_id.to_string();

    dismiss(&app, &alice, "issue", &f.issue_id).await;
    dismiss(&app, &alice, "series", &series_id).await;
    dismiss(&app, &alice, "cbl", &list_id.to_string()).await;

    let (status, page) = http(&app, Method::GET, "/api/me/rail-dismissals", &alice, None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let by_kind = |kind: &str| -> Value {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["target_kind"] == kind)
            .cloned()
            .unwrap_or_else(|| panic!("no {kind} row in {page}"))
    };

    let issue = by_kind("issue");
    assert_eq!(issue["target_id"], f.issue_id.as_str());
    assert_eq!(issue["target"]["name"], "Saga #1");
    assert_eq!(issue["target"]["subtitle"], "Chapter One");
    assert_eq!(issue["target"]["series_slug"], series_slug.as_str());
    assert!(issue["target"]["slug"].is_string());
    assert_eq!(
        issue["target"]["cover_url"],
        format!("/issues/{}/pages/0/thumb", f.issue_id)
    );
    assert_eq!(
        issue["hidden_from"],
        json!([
            "continue_reading",
            "recent_history",
            "opds_continue_reading"
        ])
    );
    assert!(issue["dismissed_at"].is_string());

    let series = by_kind("series");
    assert_eq!(series["target"]["name"], "Saga");
    assert_eq!(series["target"]["subtitle"], "2012");
    assert_eq!(series["target"]["slug"], series_slug.as_str());
    assert_eq!(
        series["target"]["cover_url"],
        format!("/issues/{}/pages/0/thumb", f.issue_id)
    );
    assert_eq!(
        series["hidden_from"],
        json!(["on_deck", "random_picks", "similar_series"])
    );

    let cbl = by_kind("cbl");
    assert_eq!(cbl["target"]["name"], "Event");
    assert_eq!(cbl["target"]["saved_view_id"], view_id.to_string());
    assert!(cbl["target"]["cover_url"].is_null());
    assert_eq!(cbl["hidden_from"], json!(["on_deck"]));
}

#[tokio::test]
async fn invisible_targets_are_withheld_and_counted() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let _admin = register(&app, "admin@example.com").await;
    let alice = register_reader(&app, &db, "alice@example.com").await;
    let kept = fixture(&db, "kept").await;
    let removed = fixture(&db, "removed").await;
    let revoked = fixture(&db, "revoked").await;
    for f in [&kept, &removed, &revoked] {
        grant(&db, alice.user_id, f.lib_id).await;
    }
    dismiss(&app, &alice, "series", &kept.series_id.to_string()).await;
    dismiss(&app, &alice, "series", &removed.series_id.to_string()).await;
    dismiss(&app, &alice, "issue", &removed.issue_id).await;
    dismiss(&app, &alice, "issue", &revoked.issue_id).await;

    // Soft-remove one series (its issue goes with it) and revoke the
    // third library.
    let row = series::Entity::find_by_id(removed.series_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: series::ActiveModel = row.into();
    am.removed_at = Set(Some(Utc::now().fixed_offset()));
    am.update(&db).await.unwrap();
    let revoked_rows = library_user_access::Entity::delete_by_id((revoked.lib_id, alice.user_id))
        .exec(&db)
        .await
        .unwrap();
    assert_eq!(revoked_rows.rows_affected, 1);

    let (status, page) = http(&app, Method::GET, "/api/me/rail-dismissals", &alice, None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(ids(&page), vec![format!("series:{}", kept.series_id)]);
    assert_eq!(page["total"], 1);
    assert_eq!(page["unavailable"], 3);
    assert!(
        !page.to_string().contains(&revoked.issue_id),
        "a revoked target must not leak: {page}"
    );

    // Access returns → the row is listed again.
    grant(&db, alice.user_id, revoked.lib_id).await;
    let (_, page) = http(&app, Method::GET, "/api/me/rail-dismissals", &alice, None).await;
    assert_eq!(page["total"], 2);
    assert_eq!(page["unavailable"], 2);
}

#[tokio::test]
async fn restore_removes_the_row() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let _admin = register(&app, "admin@example.com").await;
    let alice = register_reader(&app, &db, "alice@example.com").await;
    let f = fixture(&db, "restore").await;
    grant(&db, alice.user_id, f.lib_id).await;
    dismiss(&app, &alice, "issue", &f.issue_id).await;
    dismiss(&app, &alice, "series", &f.series_id.to_string()).await;

    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/api/me/rail-dismissals/issue/{}", f.issue_id),
        &alice,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, page) = http(&app, Method::GET, "/api/me/rail-dismissals", &alice, None).await;
    assert_eq!(ids(&page), vec![format!("series:{}", f.series_id)]);
    assert_eq!(page["total"], 1);
    assert!(
        rail_dismissal::Entity::find_by_id((alice.user_id, "issue".to_owned(), f.issue_id.clone()))
            .one(&db)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn limit_is_clamped() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let _admin = register(&app, "admin@example.com").await;
    let alice = register_reader(&app, &db, "alice@example.com").await;
    let f = fixture(&db, "clamp").await;
    grant(&db, alice.user_id, f.lib_id).await;
    dismiss(&app, &alice, "issue", &f.issue_id).await;
    dismiss(&app, &alice, "series", &f.series_id.to_string()).await;

    let (status, page) = http(
        &app,
        Method::GET,
        "/api/me/rail-dismissals?limit=0",
        &alice,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert!(page["next_cursor"].is_string());

    let (_, page) = http(
        &app,
        Method::GET,
        "/api/me/rail-dismissals?limit=10000",
        &alice,
        None,
    )
    .await;
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    assert!(page["next_cursor"].is_null());
}

#[tokio::test]
async fn auto_restored_dismissals_are_not_listed() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let _admin = register(&app, "admin@example.com").await;
    let alice = register_reader(&app, &db, "alice@example.com").await;
    let read_again = fixture(&db, "read-again").await;
    let untouched = fixture(&db, "untouched").await;
    grant(&db, alice.user_id, read_again.lib_id).await;
    grant(&db, alice.user_id, untouched.lib_id).await;
    dismiss(&app, &alice, "issue", &read_again.issue_id).await;
    dismiss(&app, &alice, "series", &read_again.series_id.to_string()).await;
    dismiss(&app, &alice, "series", &untouched.series_id.to_string()).await;
    stamp(&db, alice.user_id, "issue", &read_again.issue_id, 30).await;
    stamp(
        &db,
        alice.user_id,
        "series",
        &read_again.series_id.to_string(),
        30,
    )
    .await;

    // Reading the issue again (progress newer than both dismissals)
    // brings it and its series back to the rails on their own.
    common::seed::seed_progress(&db, alice.user_id, &read_again.issue_id, 3, 0.15, false).await;

    let (status, page) = http(&app, Method::GET, "/api/me/rail-dismissals", &alice, None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(ids(&page), vec![format!("series:{}", untouched.series_id)]);
    assert_eq!(page["total"], 1);
    assert_eq!(page["unavailable"], 0);
}

/// E7: creating a dismissal for a target the hidden-items list would not
/// show — a soft-removed series, an issue of a removed series, a
/// soft-removed or non-active issue — answers the same 404 as a missing
/// target and stores nothing.
#[tokio::test]
async fn create_rejects_invisible_targets_like_missing_ones() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let _admin = register(&app, "admin@example.com").await;
    let alice = register_reader(&app, &db, "alice@example.com").await;
    let gone = fixture(&db, "gone").await;
    let removed_issue = fixture(&db, "removed-issue").await;
    let missing_file = fixture(&db, "missing-file").await;
    for f in [&gone, &removed_issue, &missing_file] {
        grant(&db, alice.user_id, f.lib_id).await;
    }
    let row = series::Entity::find_by_id(gone.series_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: series::ActiveModel = row.into();
    am.removed_at = Set(Some(Utc::now().fixed_offset()));
    am.update(&db).await.unwrap();
    for (id, removed, state) in [
        (&removed_issue.issue_id, true, "active"),
        (&missing_file.issue_id, false, "missing"),
    ] {
        let row = issue::Entity::find_by_id(id.clone())
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        let mut am: issue::ActiveModel = row.into();
        am.removed_at = Set(removed.then(|| Utc::now().fixed_offset()));
        am.state = Set(state.into());
        am.update(&db).await.unwrap();
    }

    let missing = Uuid::now_v7().to_string();
    for (kind, id) in [
        ("series", missing.as_str()),
        ("series", &gone.series_id.to_string()),
        ("issue", &gone.issue_id),
        ("issue", &removed_issue.issue_id),
        ("issue", &missing_file.issue_id),
    ] {
        let (status, body) = http(
            &app,
            Method::POST,
            "/api/me/rail-dismissals",
            &alice,
            Some(json!({ "target_kind": kind, "target_id": id })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{kind}:{id}: {body}");
        assert_eq!(body["error"]["code"], "not_found", "{kind}:{id}: {body}");
    }
    let stored = rail_dismissal::Entity::find()
        .filter(rail_dismissal::Column::UserId.eq(alice.user_id))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(stored, 0);

    // A live target still dismisses.
    dismiss(&app, &alice, "series", &missing_file.series_id.to_string()).await;
}
