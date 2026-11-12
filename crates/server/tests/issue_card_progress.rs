//! Issue cards carry the caller's own reading progress
//! (`IssueSummaryView.progress`), so a surface that only draws the read
//! check / progress bar no longer needs `GET /progress` (every record of
//! the user).
//!
//! Covers, for every endpoint that feeds the web's issue cards:
//!   - a card the caller has a record for carries it (`finished`, `page`,
//!     `percent`, `updated_at`); a card without one carries `null`;
//!   - another user's progress never appears on the caller's cards (both
//!     users have records on the same issues, different ones);
//!   - the rows, their order, the cursors and `total` are those of the
//!     known fixture: paging the series list two at a time walks the run
//!     in order with `total` on the first page only.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{DateTime, Duration, FixedOffset, Utc};
use common::TestApp;
use common::seed::{IssueSeed, seed_cbl_entry, seed_cbl_list, seed_collection, seed_progress_at};
use common::seed::{LibrarySeed, SeriesSeed};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde_json::{Value, json};
use std::collections::HashMap;
use tempfile::tempdir;
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
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
        .filter_map(|v| v.to_str().ok().map(str::to_owned))
        .collect();
    let extract = |needle: &str| -> String {
        cookies
            .iter()
            .find_map(|c| c.split(';').next()?.strip_prefix(needle).map(str::to_owned))
            .unwrap_or_default()
    };
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
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
    path: &str,
    auth: &Authed,
    body: Option<Value>,
) -> Value {
    http_expect(app, method, path, auth, body, StatusCode::OK).await
}

async fn http_expect(
    app: &TestApp,
    method: Method,
    path: &str,
    auth: &Authed,
    body: Option<Value>,
    expected: StatusCode,
) -> Value {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf);
    let body = match body {
        Some(b) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    let resp = app
        .router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(status, expected, "{path}: {json}");
    json
}

async fn get(app: &TestApp, path: &str, auth: &Authed) -> Value {
    http(app, Method::GET, path, auth, None).await
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        sql,
        values,
    ))
    .await
    .unwrap();
}

fn now() -> DateTime<FixedOffset> {
    Utc::now().fixed_offset()
}

/// What a card should say for one issue: `None` = `progress: null`,
/// `Some((finished, page))` = a record.
type Expect = HashMap<String, Option<(bool, i64)>>;

/// Every card in `cards` carries `progress` matching `expect` (cards for
/// issues outside `expect` must not appear at all).
fn assert_cards(what: &str, cards: &[&Value], expect: &Expect) {
    assert!(!cards.is_empty(), "{what}: no cards");
    for card in cards {
        let id = card["id"].as_str().unwrap();
        let want = expect
            .get(id)
            .unwrap_or_else(|| panic!("{what}: unexpected card {id}"));
        let got = card
            .get("progress")
            .unwrap_or_else(|| panic!("{what}: card {id} has no progress field: {card}"));
        match want {
            None => assert!(got.is_null(), "{what}: {id} should be null, got {got}"),
            Some((finished, page)) => {
                assert_eq!(
                    got["finished"].as_bool(),
                    Some(*finished),
                    "{what}: {id} {got}"
                );
                assert_eq!(got["page"].as_i64(), Some(*page), "{what}: {id} {got}");
                let percent = got["percent"].as_f64().expect("percent");
                assert_eq!(percent, if *finished { 1.0 } else { 0.5 }, "{what}: {id}");
                assert!(got["updated_at"].is_string(), "{what}: {id} {got}");
            }
        }
    }
}

fn items(body: &Value) -> Vec<&Value> {
    body["items"].as_array().unwrap().iter().collect()
}

fn nested<'a>(body: &'a Value, key: &str) -> Vec<&'a Value> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e.get(key).filter(|v| !v.is_null()))
        .collect()
}

struct Fixture {
    app: TestApp,
    _dir: tempfile::TempDir,
    admin: Authed,
    reader: Authed,
    /// Saga #1..#4, in run order.
    issues: Vec<String>,
    /// The seeded series' slug (its id).
    series: String,
    lib: Uuid,
}

/// Saga #1–#4. The admin finished #1 and is midway through #2 and #4;
/// the reader is midway through #1 and finished #3. #1/#2 were added ten
/// days ago, #3/#4 today (after both users started), so the new-in-series
/// rail lists #3 and #4.
async fn fixture() -> Fixture {
    let app = TestApp::spawn().await;
    let admin = register(&app, "card-admin@example.com").await;
    let reader = register(&app, "card-reader@example.com").await;
    let db = app.state().db.clone();
    let dir = tempdir().unwrap();
    let lib = LibrarySeed::new(dir.path()).insert(&db).await;
    let series = SeriesSeed::new(lib, "Saga").insert(&db).await;
    exec(
        &db,
        "INSERT INTO library_user_access (user_id, library_id, created_at, updated_at) \
         VALUES ($1, $2, now(), now())",
        vec![reader.user_id.into(), lib.into()],
    )
    .await;
    let mut issues = Vec::new();
    for n in 1..=4u32 {
        let path = dir.path().join(format!("saga-{n}.cbz"));
        let payload = format!("saga-{n}");
        let id = IssueSeed::new(lib, series, &path, payload.as_bytes(), f64::from(n))
            .with_title(format!("Saga {n}"))
            .insert(&db)
            .await;
        let added = if n <= 2 {
            now() - Duration::days(10)
        } else {
            now()
        };
        exec(
            &db,
            "UPDATE issues SET created_at = $1, year = 2020, month = 3, day = 15 WHERE id = $2",
            vec![added.into(), id.clone().into()],
        )
        .await;
        issues.push(id);
    }
    let earlier = now() - Duration::days(5);
    seed_progress_at(&db, admin.user_id, &issues[0], 19, true, earlier).await;
    seed_progress_at(&db, admin.user_id, &issues[1], 5, false, earlier).await;
    seed_progress_at(&db, admin.user_id, &issues[3], 3, false, now()).await;
    seed_progress_at(&db, reader.user_id, &issues[0], 2, false, earlier).await;
    seed_progress_at(&db, reader.user_id, &issues[2], 19, true, earlier).await;
    Fixture {
        app,
        _dir: dir,
        admin,
        reader,
        issues,
        series: series.to_string(),
        lib,
    }
}

impl Fixture {
    fn admin_expect(&self) -> Expect {
        HashMap::from([
            (self.issues[0].clone(), Some((true, 19))),
            (self.issues[1].clone(), Some((false, 5))),
            (self.issues[2].clone(), None),
            (self.issues[3].clone(), Some((false, 3))),
        ])
    }
    fn reader_expect(&self) -> Expect {
        HashMap::from([
            (self.issues[0].clone(), Some((false, 2))),
            (self.issues[1].clone(), None),
            (self.issues[2].clone(), Some((true, 19))),
            (self.issues[3].clone(), None),
        ])
    }
}

fn ids(cards: &[&Value]) -> Vec<String> {
    cards
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn series_issue_list_pages_carry_each_callers_own_progress() {
    let f = fixture().await;
    for (who, expect) in [(&f.admin, f.admin_expect()), (&f.reader, f.reader_expect())] {
        // Paged two at a time: same rows, same order, `total` first page only.
        let p1 = get(
            &f.app,
            &format!("/api/series/{}/issues?limit=2", f.series),
            who,
        )
        .await;
        assert_eq!(p1["total"].as_i64(), Some(4));
        assert_eq!(ids(&items(&p1)), f.issues[..2].to_vec());
        assert_cards("series issues p1", &items(&p1), &expect);
        let cursor = p1["next_cursor"].as_str().expect("next_cursor");
        let p2 = get(
            &f.app,
            &format!("/api/series/{}/issues?limit=2&cursor={cursor}", f.series),
            who,
        )
        .await;
        assert!(p2.get("total").is_none_or(Value::is_null), "{p2}");
        assert_eq!(ids(&items(&p2)), f.issues[2..].to_vec());
        assert!(p2["next_cursor"].is_null(), "{p2}");
        assert_cards("series issues p2", &items(&p2), &expect);

        // Search mode (`q=`) builds its rows in a different branch.
        let s = get(
            &f.app,
            &format!("/api/series/{}/issues?q=saga", f.series),
            who,
        )
        .await;
        assert_eq!(s["total"].as_i64(), Some(4));
        assert_cards("series issues q", &items(&s), &expect);
    }
}

#[tokio::test]
async fn cross_library_lists_search_and_rails_carry_the_callers_progress() {
    let f = fixture().await;
    for (who, expect) in [(&f.admin, f.admin_expect()), (&f.reader, f.reader_expect())] {
        let body = get(&f.app, "/api/issues?sort=number&order=asc", who).await;
        assert_eq!(ids(&items(&body)), f.issues);
        assert_cards("/issues", &items(&body), &expect);

        let body = get(&f.app, "/api/issues/search?q=saga", who).await;
        assert_eq!(items(&body).len(), 4);
        assert_cards("/issues/search", &items(&body), &expect);

        let body = get(&f.app, "/api/me/recent-issues", who).await;
        assert_cards("/me/recent-issues", &items(&body), &expect);

        let body = get(&f.app, "/api/me/on-this-day?month=3&day=15", who).await;
        assert_eq!(items(&body).len(), 4, "{body}");
        assert_cards("/me/on-this-day", &items(&body), &expect);

        let body = http(
            &f.app,
            Method::POST,
            "/api/me/saved-views/preview-issues",
            who,
            Some(json!({
                "filter": {"match_mode": "all", "conditions": [
                    {"field": "year", "op": "equals", "value": 2020}
                ]},
                "sort_field": "name", "sort_order": "asc", "result_limit": 50,
            })),
        )
        .await;
        assert_eq!(items(&body).len(), 4, "{body}");
        assert_cards("saved-view preview", &items(&body), &expect);
    }

    // New in series: #3 and #4 arrived after the admin started Saga and
    // the admin has not finished them — #3 unread, #4 midway.
    let body = get(&f.app, "/api/me/new-in-series", &f.admin).await;
    let mut got = ids(&items(&body));
    got.sort();
    let mut want = vec![f.issues[2].clone(), f.issues[3].clone()];
    want.sort();
    assert_eq!(got, want, "{body}");
    assert_cards("/me/new-in-series", &items(&body), &f.admin_expect());
}

#[tokio::test]
async fn collections_reading_lists_and_entity_pages_carry_the_callers_progress() {
    let f = fixture().await;
    let db = f.app.state().db.clone();
    for (who, expect) in [(&f.admin, f.admin_expect()), (&f.reader, f.reader_expect())] {
        let coll = seed_collection(&db, who.user_id, "Pile").await;
        for (pos, id) in f.issues.iter().enumerate() {
            common::seed::seed_collection_entry_issue(&db, coll, pos as i32, id).await;
        }
        let body = get(&f.app, &format!("/api/me/collections/{coll}/entries"), who).await;
        assert_eq!(ids(&nested(&body, "issue")), f.issues);
        assert_cards("collection entries", &nested(&body, "issue"), &expect);

        let list = seed_cbl_list(&db, who.user_id, "Saga order").await;
        for (pos, id) in f.issues.iter().enumerate() {
            seed_cbl_entry(&db, list, pos as i32, Some(id)).await;
        }
        // An unmatched entry has no card and gets no lookup slot.
        seed_cbl_entry(&db, list, 4, None).await;
        let body = get(&f.app, &format!("/api/me/cbl-lists/{list}/entries"), who).await;
        assert_eq!(body["total"].as_i64(), Some(5));
        assert_eq!(ids(&nested(&body, "issue")), f.issues);
        assert_cards("cbl entries", &nested(&body, "issue"), &expect);
    }

    // Entity page grid + insights' earliest issues.
    let hazel = Uuid::now_v7();
    exec(
        &db,
        "INSERT INTO character (id, slug, name, normalized_name) VALUES ($1, 'hazel', 'Hazel', 'hazel')",
        vec![hazel.into()],
    )
    .await;
    for id in &f.issues {
        exec(
            &db,
            "INSERT INTO issue_characters (issue_id, character, character_id) VALUES ($1, 'Hazel', $2)",
            vec![id.clone().into(), hazel.into()],
        )
        .await;
    }
    for (who, expect) in [(&f.admin, f.admin_expect()), (&f.reader, f.reader_expect())] {
        let body = get(&f.app, "/api/characters/hazel/issues", who).await;
        assert_eq!(items(&body).len(), 4, "{body}");
        assert_cards("entity issues", &items(&body), &expect);
        let body = get(&f.app, "/api/characters/hazel/insights", who).await;
        let earliest: Vec<&Value> = body["earliest"].as_array().unwrap().iter().collect();
        assert_cards("entity insights", &earliest, &expect);
    }
}

#[tokio::test]
async fn surfaces_that_do_not_load_card_progress_leave_the_field_out() {
    // `null` means "unread"; absent means "not loaded here" and the web
    // falls back to `GET /progress`. A surface that does not attach must
    // not claim the issue is unread. The issue's prev/next links are such
    // a surface.
    let f = fixture().await;
    let series = get(
        &f.app,
        &format!("/api/series/{}/issues?limit=1", f.series),
        &f.admin,
    )
    .await;
    let first_slug = series["items"][0]["slug"].as_str().unwrap().to_owned();
    let next = get(
        &f.app,
        &format!("/api/series/{}/issues/{first_slug}/next", f.series),
        &f.admin,
    )
    .await;
    let item = &next["items"][0];
    assert!(item.is_object(), "{next}");
    assert!(item.get("progress").is_none(), "{item}");
}

#[tokio::test]
async fn a_shared_reading_list_shows_each_reader_their_own_progress() {
    let f = fixture().await;
    let db = f.app.state().db.clone();
    // A third reader with access and no progress at all.
    let third = register(&f.app, "card-third@example.com").await;
    exec(
        &db,
        "INSERT INTO library_user_access (user_id, library_id, created_at, updated_at) \
         VALUES ($1, $2, now(), now())",
        vec![third.user_id.into(), f.lib.into()],
    )
    .await;
    // The admin owns the list and its saved view, then shares it.
    let list = seed_cbl_list(&db, f.admin.user_id, "Shared order").await;
    for (pos, id) in f.issues.iter().enumerate() {
        seed_cbl_entry(&db, list, pos as i32, Some(id)).await;
    }
    let view = Uuid::now_v7();
    exec(
        &db,
        "INSERT INTO saved_views (id, user_id, kind, name, custom_tags, cbl_list_id, auto_pin) \
         VALUES ($1, $2, 'cbl', 'Shared order', ARRAY[]::text[], $3, FALSE)",
        vec![view.into(), f.admin.user_id.into(), list.into()],
    )
    .await;
    http_expect(
        &f.app,
        Method::POST,
        &format!("/api/admin/saved-views/{view}/share"),
        &f.admin,
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    let nobody: Expect = f.issues.iter().map(|id| (id.clone(), None)).collect();
    // Readers first, so the owner's request is not what primes anything.
    for (who, expect) in [
        (&f.reader, f.reader_expect()),
        (&third, nobody),
        (&f.admin, f.admin_expect()),
    ] {
        let body = get(&f.app, &format!("/api/me/cbl-lists/{list}/entries"), who).await;
        assert_eq!(ids(&nested(&body, "issue")), f.issues);
        assert_cards("shared cbl entries", &nested(&body, "issue"), &expect);
    }
}

#[tokio::test]
async fn saved_issue_view_results_and_the_collection_add_echo_carry_the_callers_progress() {
    let f = fixture().await;
    for (who, expect) in [(&f.admin, f.admin_expect()), (&f.reader, f.reader_expect())] {
        let view = http_expect(
            &f.app,
            Method::POST,
            "/api/me/saved-views",
            who,
            Some(json!({
                "kind": "filter_issues",
                "name": "2020",
                "filter": {"match_mode": "all", "conditions": [
                    {"field": "year", "op": "equals", "value": 2020}
                ]},
                "sort_field": "name",
                "sort_order": "asc",
                "result_limit": 12,
            })),
            StatusCode::CREATED,
        )
        .await;
        let id = view["id"].as_str().unwrap();
        let body = get(
            &f.app,
            &format!("/api/me/saved-views/{id}/issue-results"),
            who,
        )
        .await;
        assert_eq!(items(&body).len(), 4, "{body}");
        assert_cards("saved-view issue-results", &items(&body), &expect);

        let coll = http_expect(
            &f.app,
            Method::POST,
            "/api/me/collections",
            who,
            Some(json!({"name": "Pile"})),
            StatusCode::CREATED,
        )
        .await;
        let coll = coll["id"].as_str().unwrap();
        for issue in &f.issues {
            let echo = http_expect(
                &f.app,
                Method::POST,
                &format!("/api/me/collections/{coll}/entries"),
                who,
                Some(json!({"entry_kind": "issue", "ref_id": issue})),
                StatusCode::CREATED,
            )
            .await;
            assert_cards("collection add echo", &[&echo["issue"]], &expect);
        }
    }
}

#[tokio::test]
async fn cross_library_issue_pages_keep_order_cursor_and_total() {
    let f = fixture().await;
    for (who, expect) in [(&f.admin, f.admin_expect()), (&f.reader, f.reader_expect())] {
        let p1 = get(&f.app, "/api/issues?sort=number&order=asc&limit=2", who).await;
        assert_eq!(p1["total"].as_i64(), Some(4), "{p1}");
        assert_eq!(ids(&items(&p1)), f.issues[..2].to_vec());
        assert_cards("/issues p1", &items(&p1), &expect);
        let cursor = p1["next_cursor"].as_str().expect("next_cursor");
        let p2 = get(
            &f.app,
            &format!("/api/issues?sort=number&order=asc&limit=2&cursor={cursor}"),
            who,
        )
        .await;
        assert!(p2.get("total").is_none_or(Value::is_null), "{p2}");
        assert_eq!(ids(&items(&p2)), f.issues[2..].to_vec());
        assert!(p2["next_cursor"].is_null(), "{p2}");
        assert_cards("/issues p2", &items(&p2), &expect);
    }
}
