//! Upcoming releases, the reader-facing API (upcoming-releases 1.0,
//! parts D and E): `GET /api/upcoming`, `/api/upcoming/summary` and
//! `/api/series/{slug}/next-release`.
//!
//! Covers:
//! - off answers empty, with `enabled: false`;
//! - weeks, the opaque week cursor, and `total` on the first page only;
//! - scope and every server-side filter;
//! - reconciliation end to end: a paired pull line, a pull-list-only
//!   release, tracked, wanted, downloaded, owned;
//! - **access**: an admin and a full-grant reader see Mylar data; a
//!   reader with a partial grant sees none of it and no link into a
//!   library they cannot open; a reader with an age cap is held to their
//!   own series;
//! - Mylar off, or an expired snapshot, is ignored for everyone;
//! - the next-release endpoint, and that a hidden series is a 404.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{Datelike, Duration, NaiveDate, Utc};
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{
    external_id, library_user_access, mylar_snapshot, release_calendar_issue,
    release_calendar_series,
};
use sea_orm::{ActiveModelTrait, EntityTrait, IntoActiveModel, Set};
use serde_json::{Value, json};
use server::upcoming::mylar::{self, MylarSnapshot};
use server::upcoming::week_start;
use tower::ServiceExt;
use uuid::Uuid;

// ────────────────────── harness ──────────────────────

fn set_flags(app: &TestApp, upcoming: bool, mylar_on: bool) {
    let mut cfg = (*app.state().cfg()).clone();
    cfg.upcoming_enabled = upcoming;
    cfg.integrations_mylar_enabled = mylar_on;
    app.state().replace_cfg(cfg);
}

struct Authed {
    session: String,
    user_id: Uuid,
}

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
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
    let session = resp
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("__Host-comic_session="))
        .map(|c| c.split(';').next().unwrap().to_owned())
        .expect("session cookie");
    let body = body_json(resp.into_body()).await;
    Authed {
        session,
        user_id: Uuid::parse_str(body["user"]["id"].as_str().unwrap()).unwrap(),
    }
}

async fn get(app: &TestApp, uri: &str, user: Option<&Authed>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(Method::GET).uri(uri);
    if let Some(u) = user {
        req = req.header(header::COOKIE, &u.session);
    }
    let resp = app
        .router
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

fn today() -> NaiveDate {
    Utc::now().date_naive()
}

/// A day in the week `n` weeks after this one (its Wednesday).
fn wednesday(n: i64) -> NaiveDate {
    week_start(today()) + Duration::weeks(n) + Duration::days(2)
}

fn iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

struct Release<'a> {
    issue: i64,
    series: i64,
    name: &'a str,
    publisher: &'a str,
    cv: Option<i64>,
    number: &'a str,
    date: NaiveDate,
}

async fn seed_release(app: &TestApp, r: Release<'_>) {
    let db = &app.state().db;
    let now = Utc::now().fixed_offset();
    if release_calendar_series::Entity::find_by_id(r.series)
        .one(db)
        .await
        .unwrap()
        .is_none()
    {
        release_calendar_series::ActiveModel {
            metron_series_id: Set(r.series),
            name: Set(r.name.to_owned()),
            volume: Set(Some(1)),
            year_began: Set(Some(2024)),
            publisher: Set(Some(r.publisher.to_owned())),
            series_type: Set(Some("Single Issue".to_owned())),
            cv_series_id: Set(r.cv),
            detail_fetched_at: Set(Some(now)),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap();
    }
    release_calendar_issue::ActiveModel {
        metron_issue_id: Set(r.issue),
        metron_series_id: Set(r.series),
        number: Set(r.number.to_owned()),
        store_date: Set(r.date),
        cover_date: Set(None),
        image_url: Set(Some(format!(
            "https://static.metron.cloud/media/issue/{}.jpg",
            r.issue
        ))),
        cover_hash: Set(None),
        metron_modified: Set(None),
        first_seen_at: Set(now),
        last_seen_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn link(app: &TestApp, entity_type: &str, entity_id: &str, source: &str, external: i64) {
    let now = Utc::now().fixed_offset();
    external_id::ActiveModel {
        entity_type: Set(entity_type.into()),
        entity_id: Set(entity_id.to_owned()),
        source: Set(source.into()),
        external_id: Set(external.to_string()),
        external_url: Set(None),
        set_by: Set("user".into()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
}

async fn grant(app: &TestApp, user: &Authed, lib: Uuid, cap: Option<&str>) {
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        library_id: Set(lib),
        user_id: Set(user.user_id),
        age_rating_max: Set(cap.map(str::to_owned)),
        hide_unrated: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
}

async fn push_snapshot(app: &TestApp, body: Value) {
    let snap: MylarSnapshot = serde_json::from_value(body).unwrap();
    mylar::store(&app.state().db, &snap, Utc::now().fixed_offset(), None)
        .await
        .unwrap();
}

/// The world most tests share.
///
/// Calendar (Metron), this week unless noted:
/// - Action Comics #1104 — cv 91078, in library A, owned (by Metron id)
/// - Saga #70            — cv 4050,  in library B
/// - Absolute Batman #1  — cv 7001,  tracked by Mylar only (a #1)
/// - Indie Thing #3      — no cv,    nothing known
/// - Action Comics #1105 — next week
/// - Saga #71            — in three weeks
///
/// Mylar: watches 91078, 4050 (paused) and 7001; pull list has Action
/// #1104 Downloaded, Absolute Batman #1 Wanted, and "Small Press" #2
/// (Metron does not list it); wanted table has Action #1105 Snatched.
struct World {
    app: TestApp,
    admin: Authed,
    lib_a: Uuid,
    lib_b: Uuid,
    action_slug: String,
    saga_slug: String,
    _dir: tempfile::TempDir,
}

async fn world() -> World {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    set_flags(&app, true, true);
    let db = app.state().db.clone();
    let dir = tempfile::tempdir().unwrap();
    for sub in ["a", "b"] {
        std::fs::create_dir_all(dir.path().join(sub)).unwrap();
    }
    let lib_a = LibrarySeed::new(&dir.path().join("a")).insert(&db).await;
    let lib_b = LibrarySeed::new(&dir.path().join("b")).insert(&db).await;
    let action = SeriesSeed::new(lib_a, "Action Comics").insert(&db).await;
    let saga = SeriesSeed::new(lib_b, "Saga").insert(&db).await;
    link(&app, "series", &action.to_string(), "comicvine", 91078).await;
    link(&app, "series", &saga.to_string(), "comicvine", 4050).await;
    let owned = IssueSeed::new(
        lib_a,
        action,
        &dir.path().join("a/action-1104.cbz"),
        b"action-1104",
        1104.0,
    )
    .insert(&db)
    .await;
    link(&app, "issue", &owned, "metron", 1001).await;

    for r in [
        Release {
            issue: 1001,
            series: 50,
            name: "Action Comics",
            publisher: "DC Comics",
            cv: Some(91078),
            number: "1104",
            date: wednesday(0),
        },
        Release {
            issue: 1002,
            series: 51,
            name: "Saga",
            publisher: "Image Comics",
            cv: Some(4050),
            number: "70",
            date: wednesday(0),
        },
        Release {
            issue: 1003,
            series: 52,
            name: "Absolute Batman",
            publisher: "DC Comics",
            cv: Some(7001),
            number: "1",
            date: wednesday(0),
        },
        Release {
            issue: 1004,
            series: 53,
            name: "Indie Thing",
            publisher: "Tiny Press",
            cv: None,
            number: "3",
            date: wednesday(0),
        },
        Release {
            issue: 1005,
            series: 50,
            name: "Action Comics",
            publisher: "DC Comics",
            cv: Some(91078),
            number: "1105",
            date: wednesday(1),
        },
        Release {
            issue: 1006,
            series: 51,
            name: "Saga",
            publisher: "Image Comics",
            cv: Some(4050),
            number: "71",
            date: wednesday(3),
        },
    ] {
        seed_release(&app, r).await;
    }

    push_snapshot(
        &app,
        json!({
            "meta": {"schema_version": 1, "generated_at": "2026-11-15T03:36:49+00:00"},
            "pull_list": [
                {"ship_date": iso(wednesday(0) - Duration::days(1)), "publisher": "DC Comics",
                 "series": "Action Comics", "issue": "1104", "status": "Downloaded",
                 "cv_series_id": 91078, "cv_issue_id": 9001, "series_year": 2016},
                {"ship_date": iso(wednesday(0)), "publisher": "DC Comics",
                 "series": "Absolute Batman", "issue": "1", "status": "Wanted",
                 "cv_series_id": 7001, "cv_issue_id": 9003, "series_year": 2024},
                {"ship_date": iso(wednesday(0)), "publisher": "Small Press",
                 "series": "Small Press", "issue": "2", "status": "Skipped",
                 "cv_series_id": null, "cv_issue_id": null, "series_year": 2026}
            ],
            "watchlist": [[91078, "Active", true], [4050, "Paused", false], [7001, "Active", true]],
            "wanted": [
                {"cv_series_id": 91078, "cv_issue_id": 9005, "series": "Action Comics",
                 "issue": "1105", "store_date": iso(wednesday(1)), "status": "Snatched"}
            ]
        }),
    )
    .await;

    let slug = |id: Uuid| {
        let db = db.clone();
        async move {
            entity::series::Entity::find_by_id(id)
                .one(&db)
                .await
                .unwrap()
                .unwrap()
                .slug
        }
    };
    World {
        action_slug: slug(action).await,
        saga_slug: slug(saga).await,
        app,
        admin,
        lib_a,
        lib_b,
        _dir: dir,
    }
}

/// Every entry of every week of a list response, flattened.
fn entries(body: &Value) -> Vec<&Value> {
    body["items"]
        .as_array()
        .expect("items")
        .iter()
        .flat_map(|w| w["entries"].as_array().unwrap())
        .collect()
}

fn find<'a>(body: &'a Value, series: &str, number: &str) -> &'a Value {
    entries(body)
        .into_iter()
        .find(|e| e["series_name"] == series && e["number"] == number)
        .unwrap_or_else(|| panic!("{series} #{number} not in {body}"))
}

fn names(body: &Value) -> Vec<String> {
    entries(body)
        .into_iter()
        .map(|e| {
            format!(
                "{} #{}",
                e["series_name"].as_str().unwrap(),
                e["number"].as_str().unwrap()
            )
        })
        .collect()
}

const ALL: &str = "/api/upcoming?scope=all&limit=12";
const MINE: &str = "/api/upcoming?limit=12";

// ────────────────────── off ──────────────────────

#[tokio::test]
async fn off_answers_empty_and_requires_a_session() {
    let w = world().await;
    let (status, _) = get(&w.app, MINE, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    set_flags(&w.app, false, true);
    let (status, body) = get(&w.app, ALL, Some(&w.admin)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"], json!([]));
    assert_eq!(body["total"], 0);
    let (_, body) = get(&w.app, "/api/upcoming/summary", Some(&w.admin)).await;
    assert_eq!(body["enabled"], false);
    assert_eq!(body["this_week"], 0);
    assert!(body["mylar"].is_null());
    let (status, body) = get(
        &w.app,
        &format!("/api/series/{}/next-release", w.action_slug),
        Some(&w.admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["entries"], json!([]));
}

// ────────────────────── shape and paging ──────────────────────

#[tokio::test]
async fn weeks_page_by_an_opaque_cursor_with_total_on_the_first_page() {
    let w = world().await;
    let (status, first) = get(&w.app, "/api/upcoming?scope=all&limit=1", Some(&w.admin)).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["total"], 7, "six calendar rows + one pull-list-only");
    let weeks = first["items"].as_array().unwrap();
    assert_eq!(weeks.len(), 1);
    assert_eq!(weeks[0]["week_start"], iso(week_start(today())));
    assert_eq!(
        weeks[0]["week_end"],
        iso(week_start(today()) + Duration::days(6))
    );
    assert_eq!(weeks[0]["entries"].as_array().unwrap().len(), 5);
    let cursor = first["next_cursor"].as_str().expect("more weeks");
    assert!(
        !cursor.contains('-') || NaiveDate::parse_from_str(cursor, "%Y-%m-%d").is_err(),
        "the cursor is not a bare date"
    );

    let (_, second) = get(
        &w.app,
        &format!("/api/upcoming?scope=all&limit=1&cursor={cursor}"),
        Some(&w.admin),
    )
    .await;
    assert!(second.get("total").is_none() || second["total"].is_null());
    assert_eq!(names(&second), ["Action Comics #1105"]);
    // Empty weeks are skipped: the third page is three weeks out.
    let cursor = second["next_cursor"].as_str().unwrap();
    let (_, third) = get(
        &w.app,
        &format!("/api/upcoming?scope=all&limit=1&cursor={cursor}"),
        Some(&w.admin),
    )
    .await;
    assert_eq!(names(&third), ["Saga #71"]);
    assert!(third["next_cursor"].is_null());

    let (status, body) = get(&w.app, "/api/upcoming?cursor=nonsense", Some(&w.admin)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn an_entry_carries_what_a_card_shows() {
    let w = world().await;
    let (_, body) = get(&w.app, ALL, Some(&w.admin)).await;
    let action = find(&body, "Action Comics", "1104");
    assert_eq!(action["publisher"], "DC Comics");
    assert_eq!(action["release_date"], iso(wednesday(0)));
    assert_eq!(
        action["cover_url"],
        "https://static.metron.cloud/media/issue/1001.jpg"
    );
    assert!(
        action["metron_url"]
            .as_str()
            .unwrap()
            .contains("metron.cloud")
    );
    assert_eq!(action["library_series"]["slug"], w.action_slug);
    assert_eq!(action["first_issue"], false);
    assert_eq!(action["key"], "m1001");

    // A cover from any other host is not passed on.
    let mut off_host = release_calendar_issue::Entity::find_by_id(1002_i64)
        .one(&w.app.state().db)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    off_host.image_url = Set(Some("https://elsewhere.example/x.jpg".to_owned()));
    off_host.update(&w.app.state().db).await.unwrap();
    let (_, body) = get(&w.app, ALL, Some(&w.admin)).await;
    assert!(find(&body, "Saga", "70")["cover_url"].is_null());
}

// ────────────────────── reconciliation, as an admin ──────────────────────

#[tokio::test]
async fn the_two_sources_are_reconciled() {
    let w = world().await;
    let (_, body) = get(&w.app, ALL, Some(&w.admin)).await;

    // Paired with its pull line (a day apart), but Folio already has it.
    let action = find(&body, "Action Comics", "1104");
    assert_eq!(action["source"], "both");
    assert_eq!(action["mylar_status"], "downloaded");
    assert_eq!(action["owned"], true);
    assert_eq!(action["stage"], "in_library");
    assert_eq!(action["tracked"], true);

    // Mylar wants it; the series is not in Folio.
    let batman = find(&body, "Absolute Batman", "1");
    assert_eq!(batman["source"], "both");
    assert_eq!(batman["stage"], "wanted");
    assert_eq!(batman["tracked"], true);
    assert_eq!(batman["first_issue"], true);
    assert!(batman["library_series"].is_null());

    // In the library; Mylar has it paused, so it is not "tracked".
    let saga = find(&body, "Saga", "70");
    assert_eq!(saga["source"], "metron");
    assert_eq!(saga["tracked"], false);
    assert_eq!(saga["stage"], "announced");
    assert_eq!(saga["library_series"]["slug"], w.saga_slug);

    // Nobody knows anything about it.
    let indie = find(&body, "Indie Thing", "3");
    assert_eq!(indie["stage"], "announced");
    assert!(indie["library_series"].is_null());

    // Only on Mylar's pull list.
    let small = find(&body, "Small Press", "2");
    assert_eq!(small["source"], "pull_list");
    assert!(small["cover_url"].is_null());
    assert!(small["metron_url"].is_null());
    assert_eq!(small["key"].as_str().unwrap().chars().next(), Some('p'));

    // Next week: no pull line yet, but Mylar's wanted table knows it.
    let next = find(&body, "Action Comics", "1105");
    assert_eq!(next["source"], "metron");
    assert_eq!(next["mylar_status"], "snatched");
    assert_eq!(next["stage"], "snatched");
    assert_eq!(next["owned"], false);
}

#[tokio::test]
async fn scope_and_filters_run_on_the_server() {
    let w = world().await;
    let (_, mine) = get(&w.app, MINE, Some(&w.admin)).await;
    assert_eq!(
        names(&mine),
        [
            "Absolute Batman #1",
            "Action Comics #1104",
            "Saga #70",
            "Action Comics #1105",
            "Saga #71"
        ],
        "in a library or tracked; ordered by date then name"
    );
    assert_eq!(mine["total"], 5);

    let q = |params: &str| format!("/api/upcoming?scope=all&limit=12&{params}");
    let (_, tracked) = get(&w.app, &q("tracked=true"), Some(&w.admin)).await;
    assert_eq!(
        names(&tracked),
        [
            "Absolute Batman #1",
            "Action Comics #1104",
            "Action Comics #1105"
        ]
    );
    let (_, lib) = get(&w.app, &q("in_library=true"), Some(&w.admin)).await;
    assert_eq!(lib["total"], 4);
    let (_, firsts) = get(&w.app, &q("first_issues=true"), Some(&w.admin)).await;
    assert_eq!(names(&firsts), ["Absolute Batman #1"]);
    let (_, image) = get(&w.app, &q("publisher=image%20comics"), Some(&w.admin)).await;
    assert_eq!(names(&image), ["Saga #70", "Saga #71"]);
    let (_, both) = get(
        &w.app,
        &q("publisher=DC%20Comics&tracked=true&first_issues=true"),
        Some(&w.admin),
    )
    .await;
    assert_eq!(names(&both), ["Absolute Batman #1"]);
    let (status, _) = get(&w.app, &q("scope=everything"), Some(&w.admin)).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an unknown scope is rejected"
    );
}

#[tokio::test]
async fn owned_by_number_needs_the_cover_month() {
    let w = world().await;
    let db = &w.app.state().db;
    // A local Saga #71 with no cover date: number alone is not enough.
    let saga = entity::series::Entity::find()
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.name == "Saga")
        .unwrap();
    let id = IssueSeed::new(
        w.lib_b,
        saga.id,
        &w._dir.path().join("b/saga-71.cbz"),
        b"saga-71",
        71.0,
    )
    .insert(db)
    .await;
    let (_, body) = get(&w.app, ALL, Some(&w.admin)).await;
    assert_eq!(find(&body, "Saga", "71")["owned"], false);

    // With a cover month near the release, it is the same issue.
    let date = wednesday(3);
    let mut local = entity::issue::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    local.year = Set(Some(date.year()));
    local.month = Set(Some(date.month() as i32));
    local.update(db).await.unwrap();
    let (_, body) = get(&w.app, ALL, Some(&w.admin)).await;
    let saga71 = find(&body, "Saga", "71");
    assert_eq!(saga71["owned"], true);
    assert_eq!(saga71["stage"], "in_library");
}

// ────────────────────── access ──────────────────────

#[tokio::test]
async fn a_full_grant_reader_sees_mylar_data() {
    let w = world().await;
    let reader = register(&w.app, "reader@example.com").await;
    grant(&w.app, &reader, w.lib_a, None).await;
    grant(&w.app, &reader, w.lib_b, None).await;

    let (_, body) = get(&w.app, MINE, Some(&reader)).await;
    assert_eq!(find(&body, "Absolute Batman", "1")["stage"], "wanted");
    assert_eq!(find(&body, "Action Comics", "1104")["tracked"], true);
    let (_, summary) = get(&w.app, "/api/upcoming/summary", Some(&reader)).await;
    assert_eq!(summary["mylar"]["freshness"], "fresh");
    assert_eq!(summary["all_releases_available"], true);
}

#[tokio::test]
async fn a_partial_grant_reader_sees_no_mylar_data_and_no_hidden_library() {
    let w = world().await;
    let reader = register(&w.app, "reader@example.com").await;
    grant(&w.app, &reader, w.lib_a, None).await;

    // "My series" is only what is in the library they can open.
    let (_, mine) = get(&w.app, MINE, Some(&reader)).await;
    assert_eq!(names(&mine), ["Action Comics #1104", "Action Comics #1105"]);
    for e in entries(&mine) {
        assert_eq!(e["tracked"], false, "{e}");
        assert!(e["mylar_status"].is_null(), "{e}");
    }
    assert_eq!(find(&mine, "Action Comics", "1104")["stage"], "in_library");
    assert_eq!(
        find(&mine, "Action Comics", "1105")["stage"],
        "announced",
        "Mylar's 'snatched' is not shown"
    );

    // All releases: the calendar is public, but nothing says which
    // titles are held in the library they cannot open, or tracked.
    let (_, all) = get(&w.app, ALL, Some(&reader)).await;
    assert_eq!(all["total"], 6, "no pull-list-only row");
    assert!(
        entries(&all).iter().all(|e| e["source"] == "metron"),
        "no pull-list data at all"
    );
    let saga = find(&all, "Saga", "70");
    assert!(saga["library_series"].is_null(), "library B is hidden");
    assert_eq!(saga["stage"], "announced");
    assert_eq!(find(&all, "Absolute Batman", "1")["stage"], "announced");

    let (_, tracked) = get(
        &w.app,
        "/api/upcoming?scope=all&tracked=true",
        Some(&reader),
    )
    .await;
    assert_eq!(tracked["total"], 0);

    let (_, summary) = get(&w.app, "/api/upcoming/summary", Some(&reader)).await;
    assert!(summary["mylar"].is_null());
    assert_eq!(summary["this_week"], 1);
    assert_eq!(summary["next_week"], 1);
    assert_eq!(summary["all_releases_available"], true);

    // The series in the hidden library reads as missing.
    let (hidden, hidden_body) = get(
        &w.app,
        &format!("/api/series/{}/next-release", w.saga_slug),
        Some(&reader),
    )
    .await;
    let (unknown, unknown_body) = get(
        &w.app,
        "/api/series/no-such-series/next-release",
        Some(&reader),
    )
    .await;
    assert_eq!(hidden, StatusCode::NOT_FOUND);
    assert_eq!(unknown, StatusCode::NOT_FOUND);
    assert_eq!(hidden_body["error"]["code"], unknown_body["error"]["code"]);
}

#[tokio::test]
async fn a_capped_reader_is_held_to_their_own_series() {
    let w = world().await;
    let reader = register(&w.app, "teen@example.com").await;
    grant(&w.app, &reader, w.lib_a, Some("Teen")).await;
    grant(&w.app, &reader, w.lib_b, Some("Teen")).await;

    // Asking for everything still returns only their series, and no
    // Mylar data, although they are granted every library.
    let (_, all) = get(&w.app, ALL, Some(&reader)).await;
    assert_eq!(
        names(&all),
        [
            "Action Comics #1104",
            "Saga #70",
            "Action Comics #1105",
            "Saga #71"
        ]
    );
    assert!(
        entries(&all)
            .iter()
            .all(|e| e["tracked"] == false && e["mylar_status"].is_null())
    );

    let (_, summary) = get(&w.app, "/api/upcoming/summary", Some(&reader)).await;
    assert_eq!(summary["all_releases_available"], false);
    assert_eq!(summary["publishers"], json!([]));
    assert!(summary["mylar"].is_null());
}

#[tokio::test]
async fn mylar_off_or_an_expired_snapshot_is_ignored_for_everyone() {
    let w = world().await;

    set_flags(&w.app, true, false);
    let (_, body) = get(&w.app, ALL, Some(&w.admin)).await;
    assert_eq!(body["total"], 6);
    assert!(
        entries(&body)
            .iter()
            .all(|e| e["tracked"] == false && e["mylar_status"].is_null())
    );
    assert_eq!(find(&body, "Absolute Batman", "1")["stage"], "announced");
    let (_, summary) = get(&w.app, "/api/upcoming/summary", Some(&w.admin)).await;
    assert!(summary["mylar"].is_null());

    set_flags(&w.app, true, true);
    let db = &w.app.state().db;
    let mut head = mylar_snapshot::Entity::find_by_id(mylar_snapshot::SINGLETON_ID)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    // Stale: still shown, and reported as stale.
    head.received_at = Set((Utc::now() - Duration::hours(5)).fixed_offset());
    let head = head.update(db).await.unwrap();
    let (_, summary) = get(&w.app, "/api/upcoming/summary", Some(&w.admin)).await;
    assert_eq!(summary["mylar"]["freshness"], "stale");
    let (_, body) = get(&w.app, ALL, Some(&w.admin)).await;
    assert_eq!(find(&body, "Absolute Batman", "1")["stage"], "wanted");

    // Expired: gone.
    let mut head = head.into_active_model();
    head.received_at = Set((Utc::now() - Duration::days(8)).fixed_offset());
    head.update(db).await.unwrap();
    let (_, body) = get(&w.app, ALL, Some(&w.admin)).await;
    assert_eq!(body["total"], 6);
    assert_eq!(find(&body, "Absolute Batman", "1")["stage"], "announced");
    let (_, summary) = get(&w.app, "/api/upcoming/summary", Some(&w.admin)).await;
    assert!(summary["mylar"].is_null());
}

// ────────────────────── summary and next release ──────────────────────

#[tokio::test]
async fn summary_counts_my_series_for_two_weeks() {
    let w = world().await;
    let (status, body) = get(&w.app, "/api/upcoming/summary", Some(&w.admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], true);
    assert_eq!(body["this_week"], 3, "Action, Saga, Absolute Batman");
    assert_eq!(body["next_week"], 1);
    assert_eq!(
        body["publishers"],
        json!(["DC Comics", "Image Comics", "Tiny Press"])
    );
    assert!(body["calendar_synced_at"].is_null(), "no list pass has run");
    assert_eq!(body["mylar"]["freshness"], "fresh");

    // A caller's date a day off the server's is honoured; a wild one is not.
    let tomorrow = iso(today() + Duration::days(1));
    let (_, near) = get(
        &w.app,
        &format!("/api/upcoming/summary?today={tomorrow}"),
        Some(&w.admin),
    )
    .await;
    assert_eq!(near["enabled"], true);
    let (_, far) = get(
        &w.app,
        "/api/upcoming/summary?today=2031-01-01",
        Some(&w.admin),
    )
    .await;
    assert_eq!(far["this_week"], 3, "falls back to the server's date");
}

#[tokio::test]
async fn next_release_lists_a_series_own_upcoming_issues() {
    let w = world().await;
    // Make sure "today or later" includes this week's Wednesday.
    let monday = iso(week_start(today()));
    let (status, body) = get(
        &w.app,
        &format!("/api/series/{}/next-release?today={monday}", w.action_slug),
        Some(&w.admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let list = body["entries"].as_array().unwrap();
    assert!(!list.is_empty() && list.len() <= 2, "{body}");
    assert!(list.iter().all(|e| e["series_name"] == "Action Comics"));
    let last = list.last().unwrap();
    assert_eq!(last["number"], "1105");
    assert_eq!(last["stage"], "snatched");

    let (_, body) = get(
        &w.app,
        &format!("/api/series/{}/next-release", w.saga_slug),
        Some(&w.admin),
    )
    .await;
    assert!(
        body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["series_name"] == "Saga"),
        "{body}"
    );
}

// ────────────────────── the home rail ──────────────────────

const RAIL: &str = "/api/me/upcoming-this-week";

fn rail_names(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|e| {
            format!(
                "{} #{}",
                e["series_name"].as_str().unwrap(),
                e["number"].as_str().unwrap()
            )
        })
        .collect()
}

#[tokio::test]
async fn the_rail_is_this_week_of_my_series_under_the_same_access_rules() {
    let w = world().await;
    let (status, body) = get(&w.app, RAIL, Some(&w.admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut names = rail_names(&body);
    names.sort();
    assert_eq!(
        names,
        ["Absolute Batman #1", "Action Comics #1104", "Saga #70"],
        "this week only, my series only: {body}"
    );
    let (_, summary) = get(&w.app, "/api/upcoming/summary", Some(&w.admin)).await;
    assert_eq!(
        summary["this_week"], 3,
        "the nav count is the rail's length"
    );

    // A reader with one library: that library's series, and nothing Mylar
    // said about anything.
    let reader = register(&w.app, "reader@example.com").await;
    grant(&w.app, &reader, w.lib_a, None).await;
    let (_, body) = get(&w.app, RAIL, Some(&reader)).await;
    assert_eq!(rail_names(&body), ["Action Comics #1104"]);
    for e in body["items"].as_array().unwrap() {
        assert_eq!(e["tracked"], false, "{e}");
        assert!(e["mylar_status"].is_null(), "{e}");
        assert_eq!(e["source"], "metron", "{e}");
    }

    // Off: empty for everyone. No session: refused.
    set_flags(&w.app, false, true);
    let (status, body) = get(&w.app, RAIL, Some(&w.admin)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"], json!([]));
    let (status, _) = get(&w.app, RAIL, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_rail_is_offered_only_while_the_feature_is_on() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let rail = |body: &Value| {
        body["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|v| v["system_key"] == "upcoming_this_week")
            .cloned()
    };

    let (status, off) = get(&app, "/api/me/saved-views", Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{off}");
    assert!(rail(&off).is_none(), "not offered while the feature is off");

    set_flags(&app, true, false);
    let (_, on) = get(&app, "/api/me/saved-views", Some(&admin)).await;
    let view = rail(&on).expect("offered once the feature is on");
    assert_eq!(view["name"], "Out this week");
    assert_eq!(view["kind"], "system");
    assert_eq!(view["pinned"], false, "opt-in: never pinned for the user");
}
