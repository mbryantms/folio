//! "New in series you're reading" (`GET /me/new-in-series`, system view
//! `new_in_series`).
//!
//! Covers:
//!   - the definition: an issue added within the window to a series the
//!     caller has started appears; one in an unstarted series, one the
//!     caller finished, one added before the window, a removed one, one in
//!     a hidden library, one over the caller's age cap and one in a series
//!     the caller hid from the rails do not;
//!   - order (newest addition first, id tie-break) and cursor paging past
//!     one page with no repeat or gap, `total` on the first page only;
//!   - the seeded system view is opt-in (unpinned) for existing users, and
//!     the migration's down/up round trip removes and restores it without
//!     touching the user's pins.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{DateTime, Duration, FixedOffset, Utc};
use common::TestApp;
use common::seed::{IssueSeed, seed_library, seed_progress_at, seed_series};
use migration::MigratorTrait;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use std::collections::HashSet;
use std::path::Path;
use tower::ServiceExt;
use uuid::Uuid;

const MIGRATION: &str = "m20271110_000001_new_in_series_rail";

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

async fn demote_to_user(db: &DatabaseConnection, user_id: Uuid) {
    exec(
        db,
        "UPDATE users SET role = 'user' WHERE id = $1",
        vec![user_id.into()],
    )
    .await;
}

async fn grant(db: &DatabaseConnection, user_id: Uuid, lib: Uuid, cap: Option<&str>) {
    exec(
        db,
        "INSERT INTO library_user_access (user_id, library_id, age_rating_max, created_at, updated_at) \
         VALUES ($1, $2, $3, now(), now())",
        vec![user_id.into(), lib.into(), cap.map(str::to_owned).into()],
    )
    .await;
}

/// Insert issue `n` of a series, stamped as added at `added`.
async fn issue(
    db: &DatabaseConnection,
    dir: &Path,
    lib: Uuid,
    series: Uuid,
    n: u32,
    added: DateTime<FixedOffset>,
) -> String {
    let path = dir.join(format!("{series}-{n}.cbz"));
    let payload = format!("{series}-{n}");
    let id = IssueSeed::new(lib, series, &path, payload.as_bytes(), f64::from(n))
        .insert(db)
        .await;
    exec(
        db,
        "UPDATE issues SET created_at = $1 WHERE id = $2",
        vec![added.into(), id.clone().into()],
    )
    .await;
    id
}

fn now() -> DateTime<FixedOffset> {
    Utc::now().fixed_offset()
}

fn ids_of(body: &serde_json::Value) -> Vec<String> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn lists_recent_unfinished_additions_to_started_series_only() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let user = register(&app, "nis-def@example.com").await;
    demote_to_user(&db, user.user_id).await;
    let lib = seed_library(&db, dir).await;
    grant(&db, user.user_id, lib, Some("Teen")).await;
    let long_ago = now() - Duration::days(90);
    let read_at = now() - Duration::days(60);

    // Started series: #1 finished long ago.
    let started = seed_series(&db, lib, "Started").await;
    let s1 = issue(&db, dir, lib, started, 1, long_ago).await;
    seed_progress_at(&db, user.user_id, &s1, 19, true, read_at).await;
    let new_one = issue(&db, dir, lib, started, 2, now() - Duration::hours(1)).await;
    let outside_window = issue(&db, dir, lib, started, 3, now() - Duration::days(40)).await;
    let finished_new = issue(&db, dir, lib, started, 4, now() - Duration::hours(2)).await;
    seed_progress_at(&db, user.user_id, &finished_new, 19, true, now()).await;
    let above_cap = issue(&db, dir, lib, started, 5, now() - Duration::hours(3)).await;
    exec(
        &db,
        "UPDATE issues SET age_rating = 'Mature 17+' WHERE id = $1",
        vec![above_cap.clone().into()],
    )
    .await;
    let removed = issue(&db, dir, lib, started, 6, now() - Duration::hours(4)).await;
    exec(
        &db,
        "UPDATE issues SET removed_at = now() WHERE id = $1",
        vec![removed.clone().into()],
    )
    .await;

    // A series with an in-progress issue counts as started; its new
    // issue appears, older than the first.
    let reading = seed_series(&db, lib, "Reading").await;
    let r1 = issue(&db, dir, lib, reading, 1, long_ago).await;
    seed_progress_at(&db, user.user_id, &r1, 5, false, read_at).await;
    let reading_new = issue(&db, dir, lib, reading, 2, now() - Duration::hours(5)).await;

    // An unstarted series: its new issue does not count.
    let unstarted = seed_series(&db, lib, "Unstarted").await;
    let unstarted_new = issue(&db, dir, lib, unstarted, 1, now() - Duration::hours(1)).await;

    // A series the user hid from the rails.
    let hidden = seed_series(&db, lib, "Hidden").await;
    let h1 = issue(&db, dir, lib, hidden, 1, long_ago).await;
    seed_progress_at(&db, user.user_id, &h1, 19, true, read_at).await;
    let hidden_new = issue(&db, dir, lib, hidden, 2, now() - Duration::hours(1)).await;
    let (status, _) = http(
        &app,
        Method::POST,
        "/api/me/rail-dismissals",
        &user,
        Some(serde_json::json!({"target_kind": "series", "target_id": hidden.to_string()})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // A started series in a library the user cannot see.
    let other_lib = seed_library(&db, &dir.join("other")).await;
    let invisible = seed_series(&db, other_lib, "Invisible").await;
    let i1 = issue(&db, dir, other_lib, invisible, 1, long_ago).await;
    seed_progress_at(&db, user.user_id, &i1, 19, true, read_at).await;
    let invisible_new = issue(
        &db,
        dir,
        other_lib,
        invisible,
        2,
        now() - Duration::hours(1),
    )
    .await;

    let (status, body) = http(&app, Method::GET, "/api/me/new-in-series", &user, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        ids_of(&body),
        vec![new_one.clone(), reading_new.clone()],
        "newest addition first"
    );
    assert_eq!(body["total"], 2);
    assert!(body["next_cursor"].is_null());
    assert_eq!(body["items"][0]["series_name"], "Started");
    let got: HashSet<String> = ids_of(&body).into_iter().collect();
    for (id, why) in [
        (&s1, "added before the window and finished"),
        (&outside_window, "added before the window"),
        (&finished_new, "finished"),
        (&above_cap, "over the reader's age cap"),
        (&removed, "removed"),
        (&unstarted_new, "series not started"),
        (&hidden_new, "series hidden from the rails"),
        (&invisible_new, "library not visible"),
    ] {
        assert!(!got.contains(id), "{why}: must not appear");
    }

    // A series dismissal lists the new rail among the ones it hides from.
    let (_, dismissals) = http(&app, Method::GET, "/api/me/rail-dismissals", &user, None).await;
    let rails = dismissals["items"][0]["hidden_from"].as_array().unwrap();
    assert!(rails.iter().any(|r| r == "new_in_series"));

    // Reading in the hidden series again restores it (On deck's rule).
    let h3 = issue(&db, dir, lib, hidden, 3, long_ago).await;
    seed_progress_at(
        &db,
        user.user_id,
        &h3,
        19,
        true,
        now() + Duration::minutes(1),
    )
    .await;
    let (_, body) = http(&app, Method::GET, "/api/me/new-in-series", &user, None).await;
    assert!(
        ids_of(&body).contains(&hidden_new),
        "dismissal auto-restores"
    );
}

#[tokio::test]
async fn pages_past_one_page_without_repeats_newest_first() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let user = register(&app, "nis-page@example.com").await;
    let lib = seed_library(&db, dir).await;
    let series = seed_series(&db, lib, "Paged").await;
    let first = issue(&db, dir, lib, series, 1, now() - Duration::days(90)).await;
    seed_progress_at(
        &db,
        user.user_id,
        &first,
        19,
        true,
        now() - Duration::days(60),
    )
    .await;
    // 30 new issues; 2..=5 share one timestamp so the id tie-break shows.
    let base = now() - Duration::days(1);
    let mut expected = Vec::new();
    for n in 2..=31u32 {
        let added = base + Duration::minutes(i64::from(n.max(5)));
        expected.push((added, issue(&db, dir, lib, series, n, added).await));
    }
    expected.sort_by(|a, b| b.cmp(a));
    let expected: Vec<String> = expected.into_iter().map(|(_, id)| id).collect();

    let mut walked = Vec::new();
    let mut cursor: Option<String> = None;
    for page in 0..20 {
        let uri = match &cursor {
            Some(c) => format!("/api/me/new-in-series?limit=7&cursor={c}"),
            None => "/api/me/new-in-series?limit=7".to_owned(),
        };
        let (status, body) = http(&app, Method::GET, &uri, &user, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if page == 0 {
            assert_eq!(body["total"], 30);
        } else {
            assert!(body.get("total").is_none(), "total on the first page only");
        }
        walked.extend(ids_of(&body));
        match body["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    let unique: HashSet<&String> = walked.iter().collect();
    assert_eq!(unique.len(), walked.len(), "no item repeated");
    assert_eq!(walked, expected, "every item once, newest first");

    // The home rail's one request: the default first page.
    let (_, body) = http(&app, Method::GET, "/api/me/new-in-series", &user, None).await;
    assert_eq!(ids_of(&body), expected[..24].to_vec());
    assert!(body["next_cursor"].is_string());

    let (status, _) = http(
        &app,
        Method::GET,
        "/api/me/new-in-series?cursor=nope",
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// A whole series added at once and then started is not "new": every
/// issue arrived before the reader's first progress, even though all of
/// them are inside the window.
#[tokio::test]
async fn a_series_added_before_the_reader_started_it_yields_nothing() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let user = register(&app, "nis-bulk@example.com").await;
    let lib = seed_library(&db, dir).await;
    let series = seed_series(&db, lib, "Bulk").await;
    let added = now() - Duration::days(10);
    let mut ids = Vec::new();
    for n in 1..=10u32 {
        ids.push(issue(&db, dir, lib, series, n, added).await);
    }
    seed_progress_at(
        &db,
        user.user_id,
        &ids[0],
        19,
        true,
        now() - Duration::days(5),
    )
    .await;
    seed_progress_at(
        &db,
        user.user_id,
        &ids[1],
        4,
        false,
        now() - Duration::days(4),
    )
    .await;

    let (status, body) = http(&app, Method::GET, "/api/me/new-in-series", &user, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        ids_of(&body),
        Vec::<String>::new(),
        "nothing arrived after the start"
    );
    assert_eq!(body["total"], 0);
}

/// An issue added after the reader finished an earlier one appears; one
/// added before they started does not.
#[tokio::test]
async fn only_issues_added_after_the_reader_started_appear() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let user = register(&app, "nis-after@example.com").await;
    let lib = seed_library(&db, dir).await;
    let series = seed_series(&db, lib, "Arrivals").await;
    let first = issue(&db, dir, lib, series, 1, now() - Duration::days(20)).await;
    seed_progress_at(
        &db,
        user.user_id,
        &first,
        19,
        true,
        now() - Duration::days(15),
    )
    .await;
    let before_start = issue(&db, dir, lib, series, 2, now() - Duration::days(18)).await;
    let arrival = issue(&db, dir, lib, series, 3, now() - Duration::days(10)).await;

    let (_, body) = http(&app, Method::GET, "/api/me/new-in-series", &user, None).await;
    assert_eq!(ids_of(&body), vec![arrival]);
    assert!(!ids_of(&body).contains(&before_start));
}

/// Re-reading one finished issue and continuing another does not move the
/// series' start past a later arrival: the start is the earliest stable
/// `finished_at`. The rows mirror the progress upsert: a restart clears
/// `finished_at` and moves `updated_at`; a finished row that is read
/// again keeps its `finished_at`.
#[tokio::test]
async fn rereading_finished_issues_does_not_hide_a_later_arrival() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let user = register(&app, "nis-reread@example.com").await;
    let lib = seed_library(&db, dir).await;
    let series = seed_series(&db, lib, "Reread").await;
    let one = issue(&db, dir, lib, series, 1, now() - Duration::days(25)).await;
    let two = issue(&db, dir, lib, series, 2, now() - Duration::days(25)).await;
    seed_progress_at(
        &db,
        user.user_id,
        &one,
        19,
        true,
        now() - Duration::days(20),
    )
    .await;
    seed_progress_at(
        &db,
        user.user_id,
        &two,
        19,
        true,
        now() - Duration::days(19),
    )
    .await;
    let arrival = issue(&db, dir, lib, series, 3, now() - Duration::days(10)).await;
    // Restart #2: unfinished again, finished_at cleared, updated now.
    exec(
        &db,
        "UPDATE progress_records SET finished = false, finished_at = NULL, last_page = 3, \
         updated_at = now() WHERE user_id = $1 AND issue_id = $2",
        vec![user.user_id.into(), two.clone().into()],
    )
    .await;
    // Continue #1: still finished, finished_at kept, updated now.
    exec(
        &db,
        "UPDATE progress_records SET updated_at = now() WHERE user_id = $1 AND issue_id = $2",
        vec![user.user_id.into(), one.clone().into()],
    )
    .await;

    let (_, body) = http(&app, Method::GET, "/api/me/new-in-series", &user, None).await;
    assert_eq!(
        ids_of(&body),
        vec![arrival],
        "the arrival stays; #2, back in progress, predates the start"
    );
}

fn steps_to_undo_ours() -> u32 {
    let names: Vec<String> = migration::Migrator::migrations()
        .iter()
        .map(|m| m.name().to_owned())
        .collect();
    let pos = names
        .iter()
        .position(|n| n == MIGRATION)
        .expect("migration registered");
    u32::try_from(names.len() - pos).unwrap()
}

/// The rail is seeded opt-in, like the last built-in rails: an existing
/// user finds it unpinned; the migration's down removes it and its up
/// brings it back without touching the user's pins.
#[tokio::test]
async fn existing_users_get_the_rail_opt_in_and_the_migration_round_trips() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = register(&app, "nis-seed@example.com").await;

    let find = |body: &serde_json::Value| {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["system_key"] == "new_in_series")
            .cloned()
    };
    let (status, body) = http(&app, Method::GET, "/api/me/saved-views", &user, None).await;
    assert_eq!(status, StatusCode::OK);
    let row = find(&body).expect("new_in_series seeded");
    assert_eq!(row["kind"], "system");
    assert_eq!(row["is_system"], true);
    assert_eq!(row["pinned"], false, "opt-in, not auto-pinned");
    assert_eq!(row["id"], "00000000-0000-0000-0000-000000000017");
    let pinned_before: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["pinned"] == true)
        .map(|v| v["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(!pinned_before.is_empty());

    migration::Migrator::down(&db, Some(steps_to_undo_ours()))
        .await
        .unwrap();
    let (_, body) = http(&app, Method::GET, "/api/me/saved-views", &user, None).await;
    assert!(find(&body).is_none(), "down removes the view");

    migration::Migrator::up(&db, None).await.unwrap();
    let (_, body) = http(&app, Method::GET, "/api/me/saved-views", &user, None).await;
    let row = find(&body).expect("up restores the view");
    assert_eq!(row["pinned"], false);
    let pinned_after: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["pinned"] == true)
        .map(|v| v["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(pinned_after, pinned_before, "the user's pins are untouched");
}
