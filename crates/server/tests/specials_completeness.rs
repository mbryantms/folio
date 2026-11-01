//! Remediation B9: annuals and specials count toward series completion
//! when the viewer chooses — an account setting
//! (`users.count_specials_in_completeness`, default off) with a
//! per-series override (`/me/series/{id}/completion-prefs`).
//!
//! Every completeness surface must agree for the viewer: the saved-view
//! `collection_completeness` filter, the series card / detail figures
//! (`SeriesView.specials_count_toward_completion`), and the collection
//! report (`counted_owned` / `completeness_state`). Another user's choice
//! never leaks.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
}

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
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
            .map(|c| {
                c.split(';')
                    .next()
                    .unwrap()
                    .trim_start_matches(prefix)
                    .to_owned()
            })
            .expect(prefix)
    };
    let json = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id,
    }
}

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: &Authed,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
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

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}

/// One series of `main` numbered issues plus `specials` annuals, with a
/// publisher total of `total`. Returns `(series_id, slug)`.
async fn seed_series(
    db: &DatabaseConnection,
    lib_id: Uuid,
    root: &std::path::Path,
    name: &str,
    main: usize,
    specials: usize,
    total: i32,
) -> (Uuid, String) {
    let series_id = common::seed::seed_series(db, lib_id, name).await;
    for i in 0..main + specials {
        let id = common::seed::seed_issue(
            db,
            lib_id,
            series_id,
            &root.join(format!("{name}-{i}.cbz")),
            format!("{name}-{i}").as_bytes(),
            1.0 + i as f64,
        )
        .await;
        if i >= main {
            exec(
                db,
                "UPDATE issues SET special_type = 'Annual' WHERE id = $1",
                vec![id.into()],
            )
            .await;
        }
    }
    exec(
        db,
        "UPDATE series SET total_issues = $1 WHERE id = $2",
        vec![total.into(), series_id.into()],
    )
    .await;
    (series_id, series_id.to_string())
}

/// Names of the series the caller's `collection_completeness = complete`
/// filter matches, sorted.
async fn complete_names(app: &TestApp, auth: &Authed) -> Vec<String> {
    let body = serde_json::json!({
        "kind": "filter_series",
        "name": format!("complete-{}", Uuid::now_v7()),
        "filter": {
            "match_mode": "all",
            "conditions": [{
                "group_id": 0,
                "field": "collection_completeness",
                "op": "is",
                "value": "complete"
            }]
        },
        "sort_field": "name",
        "sort_order": "asc",
        "result_limit": 50,
    });
    let (status, view) = http(app, Method::POST, "/api/me/saved-views", auth, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{view}");
    let url = format!(
        "/api/me/saved-views/{}/results",
        view["id"].as_str().unwrap()
    );
    let (status, results) = http(app, Method::GET, &url, auth, None).await;
    assert_eq!(status, StatusCode::OK, "{results}");
    results["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect()
}

/// `(specials_count_toward_completion, specials_completion)` from the
/// detail endpoint and the list hydration — asserted equal.
async fn card_flag(app: &TestApp, auth: &Authed, slug: &str) -> (bool, String) {
    let (status, detail) = http(app, Method::GET, &format!("/api/series/{slug}"), auth, None).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    let (status, list) = http(app, Method::GET, "/api/series?limit=50", auth, None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let row = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["slug"] == slug)
        .expect("series listed")
        .clone();
    assert_eq!(
        row["specials_count_toward_completion"], detail["specials_count_toward_completion"],
        "list and detail disagree: {row} vs {detail}"
    );
    assert_eq!(row["specials_completion"], detail["specials_completion"]);
    (
        detail["specials_count_toward_completion"]
            .as_bool()
            .unwrap(),
        detail["specials_completion"].as_str().unwrap().to_owned(),
    )
}

/// `(counted_owned, completeness_state, total_owned)` from the report.
async fn report(app: &TestApp, auth: &Authed, slug: &str) -> (i64, String, i64) {
    let (status, r) = http(
        app,
        Method::GET,
        &format!("/api/series/{slug}/collection"),
        auth,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    (
        r["counted_owned"].as_i64().unwrap(),
        r["completeness_state"].as_str().unwrap().to_owned(),
        r["total_owned"].as_i64().unwrap(),
    )
}

async fn set_account(app: &TestApp, auth: &Authed, value: bool) {
    let (status, me) = http(
        app,
        Method::PATCH,
        "/api/me/preferences",
        auth,
        Some(serde_json::json!({ "count_specials_in_completeness": value })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["count_specials_in_completeness"], value);
}

async fn set_series(
    app: &TestApp,
    auth: &Authed,
    series_id: Uuid,
    mode: &str,
) -> (StatusCode, serde_json::Value) {
    http(
        app,
        Method::PUT,
        &format!("/api/me/series/{series_id}/completion-prefs"),
        auth,
        Some(serde_json::json!({ "count_specials": mode })),
    )
    .await
}

struct Fixture {
    app: TestApp,
    db: DatabaseConnection,
    alice: Authed,
    bob: Authed,
    /// 4 numbered + 1 annual, total 5: complete only when specials count.
    annualled: (Uuid, String),
    /// Same shape, a second series for the per-series cases.
    second: (Uuid, String),
    /// 5 numbered, total 5: complete either way.
    plain: (Uuid, String),
    _root: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let app = TestApp::spawn().await;
    let alice = register(&app, "alice-b9@example.com").await;
    let bob = register(&app, "bob-b9@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    // Both admins so the library ACL is out of the picture.
    exec(&db, "UPDATE users SET role = 'admin'", vec![]).await;
    let root = tempfile::tempdir().unwrap();
    let lib_id = common::seed::seed_library(&db, root.path()).await;
    let annualled = seed_series(&db, lib_id, root.path(), "Annualled", 4, 1, 5).await;
    let second = seed_series(&db, lib_id, root.path(), "Second", 4, 1, 5).await;
    let plain = seed_series(&db, lib_id, root.path(), "Plain", 5, 0, 5).await;
    Fixture {
        app,
        db,
        alice,
        bob,
        annualled,
        second,
        plain,
        _root: root,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_excludes_specials_everywhere() {
    let f = fixture().await;
    let (_, me) = http(&f.app, Method::GET, "/api/auth/me", &f.alice, None).await;
    assert_eq!(me["count_specials_in_completeness"], false, "{me}");

    assert_eq!(complete_names(&f.app, &f.alice).await, vec!["Plain"]);
    assert_eq!(
        card_flag(&f.app, &f.alice, &f.annualled.1).await,
        (false, "inherit".to_owned())
    );
    let (counted, state, total_owned) = report(&f.app, &f.alice, &f.annualled.1).await;
    assert_eq!((counted, state.as_str(), total_owned), (4, "incomplete", 5));
    let (counted, state, _) = report(&f.app, &f.alice, &f.plain.1).await;
    assert_eq!((counted, state.as_str()), (5, "complete"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn account_setting_counts_specials_for_that_user_only() {
    let f = fixture().await;
    set_account(&f.app, &f.alice, true).await;

    assert_eq!(
        complete_names(&f.app, &f.alice).await,
        vec!["Annualled", "Plain", "Second"]
    );
    assert_eq!(
        card_flag(&f.app, &f.alice, &f.annualled.1).await,
        (true, "inherit".to_owned())
    );
    let (counted, state, _) = report(&f.app, &f.alice, &f.annualled.1).await;
    assert_eq!((counted, state.as_str()), (5, "complete"));

    // Bob never opted in: his figures are unchanged.
    assert_eq!(complete_names(&f.app, &f.bob).await, vec!["Plain"]);
    assert!(!card_flag(&f.app, &f.bob, &f.annualled.1).await.0);
    let (counted, state, _) = report(&f.app, &f.bob, &f.annualled.1).await;
    assert_eq!((counted, state.as_str()), (4, "incomplete"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_series_exclude_beats_an_include_setting() {
    let f = fixture().await;
    set_account(&f.app, &f.alice, true).await;
    let (status, body) = set_series(&f.app, &f.alice, f.annualled.0, "exclude").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["count_specials"], "exclude");
    assert_eq!(body["effective"], false);

    assert_eq!(
        complete_names(&f.app, &f.alice).await,
        vec!["Plain", "Second"],
        "the override drops Annualled; Second still inherits the include"
    );
    assert_eq!(
        card_flag(&f.app, &f.alice, &f.annualled.1).await,
        (false, "exclude".to_owned())
    );
    assert!(card_flag(&f.app, &f.alice, &f.second.1).await.0);
    let (counted, state, _) = report(&f.app, &f.alice, &f.annualled.1).await;
    assert_eq!((counted, state.as_str()), (4, "incomplete"));

    // Bob's view of the same series is untouched by Alice's override.
    let (_, bob_prefs) = http(
        &f.app,
        Method::GET,
        &format!("/api/me/series/{}/completion-prefs", f.annualled.0),
        &f.bob,
        None,
    )
    .await;
    assert_eq!(bob_prefs["count_specials"], "inherit", "{bob_prefs}");
    assert_eq!(bob_prefs["effective"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_series_include_beats_the_default_and_inherit_clears_it() {
    let f = fixture().await;
    let (status, body) = set_series(&f.app, &f.alice, f.annualled.0, "include").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["effective"], true);

    assert_eq!(
        complete_names(&f.app, &f.alice).await,
        vec!["Annualled", "Plain"]
    );
    assert_eq!(
        card_flag(&f.app, &f.alice, &f.annualled.1).await,
        (true, "include".to_owned())
    );
    let (counted, state, _) = report(&f.app, &f.alice, &f.annualled.1).await;
    assert_eq!((counted, state.as_str()), (5, "complete"));
    // Bob: still the default.
    assert_eq!(complete_names(&f.app, &f.bob).await, vec!["Plain"]);

    // Inherit removes the row and the default applies again.
    let (status, body) = set_series(&f.app, &f.alice, f.annualled.0, "inherit").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["count_specials"], "inherit");
    assert_eq!(body["effective"], false);
    let rows =
        f.db.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*)::bigint AS n FROM user_series_completion_prefs WHERE user_id = $1",
            [f.alice.user_id.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap();
    assert_eq!(rows, 0, "inherit deletes the override row");
    assert_eq!(complete_names(&f.app, &f.alice).await, vec!["Plain"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_prefs_reject_unknown_series_and_values() {
    let f = fixture().await;
    let (status, _) = set_series(&f.app, &f.alice, Uuid::now_v7(), "include").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = set_series(&f.app, &f.alice, f.annualled.0, "sometimes").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["error"]["code"].is_string(),
        "canonical envelope: {body}"
    );
}
