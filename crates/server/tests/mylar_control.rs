//! Add-to-Mylar (add-to-mylar 1.0): queuing missing reading-list entries
//! and the sweep that sends them to Mylar.
//!
//! Covers:
//! - the endpoints are admin-only, and refuse (409) until a Mylar address
//!   and key are stored;
//! - a request queues one row per series and (when asked) per issue,
//!   skips series the snapshot already shows in Mylar, counts entries
//!   with no ComicVine id, ignores matched entries, and is audited;
//! - asking twice queues nothing new;
//! - the sweep adds a series Mylar lacks, marks an importable issue
//!   Wanted, and leaves an issue Mylar does not know yet waiting;
//! - a refusal from Mylar fails the row with its reason, and asking again
//!   re-queues it;
//! - Mylar being down records nothing against any request;
//! - the list status reports each target's state;
//! - the wanted check finds issues Mylar wants that the library holds,
//!   and counts reading-list issues Mylar is not looking for;
//! - search now, recheck, refresh and un-want are admin-only, audited,
//!   refuse what Mylar does not have, and un-want only a Wanted issue.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{cbl_entry, cbl_list, mylar_add_request, mylar_watch};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde_json::{Value, json};
use server::mylar_control::client::MylarClient;
use server::mylar_control::{requests, sweep};
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REQUESTS: &str = "/api/admin/integrations/mylar/requests";
const TEST: &str = "/api/admin/integrations/mylar/test";

struct Authed {
    session: String,
    csrf: String,
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
            .map(|kv| kv.split_once('=').map(|(_, v)| v.to_owned()).unwrap())
            .unwrap()
    };
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn call(
    app: &TestApp,
    m: Method,
    uri: &str,
    user: &Authed,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(m.clone()).uri(uri).header(
        header::COOKIE,
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            user.session, user.csrf
        ),
    );
    if m != Method::GET {
        req = req.header("X-CSRF-Token", &user.csrf);
    }
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Store a Mylar address and key. The address refuses connections, so the
/// sweep a handler wakes finds Mylar down and records nothing.
fn configure(app: &TestApp) {
    let mut cfg = (*app.state().cfg()).clone();
    cfg.integrations_mylar_url = Some("http://127.0.0.1:9/".into());
    cfg.integrations_mylar_api_key = Some("secret-key".into());
    app.state().replace_cfg(cfg);
}

/// `(series name, number, cv series id, cv issue id, missing?)`
type EntrySpec = (&'static str, &'static str, Option<i32>, Option<i32>, bool);

async fn seed_list(app: &TestApp, entries: &[EntrySpec]) -> (Uuid, Vec<Uuid>) {
    let db = app.state().db.clone();
    let now = Utc::now().fixed_offset();
    let list_id = Uuid::now_v7();
    cbl_list::ActiveModel {
        id: Set(list_id),
        owner_user_id: Set(None),
        source_kind: Set("upload".into()),
        raw_sha256: Set(vec![0u8; 32]),
        raw_xml: Set("<ReadingList />".into()),
        parsed_name: Set("Event".into()),
        parsed_matchers_present: Set(false),
        num_issues_declared: Set(Some(entries.len() as i32)),
        imported_at: Set(now),
        created_at: Set(now),
        updated_at: Set(now),
        preserve_canonical_order: Set(false),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    let mut ids = Vec::new();
    for (pos, (name, number, cv_series, cv_issue, missing)) in entries.iter().enumerate() {
        let id = Uuid::now_v7();
        ids.push(id);
        cbl_entry::ActiveModel {
            id: Set(id),
            cbl_list_id: Set(list_id),
            position: Set(pos as i32),
            series_name: Set((*name).into()),
            issue_number: Set((*number).into()),
            volume: Set(None),
            year: Set(None),
            cv_series_id: Set(*cv_series),
            cv_issue_id: Set(*cv_issue),
            metron_series_id: Set(None),
            metron_issue_id: Set(None),
            matched_issue_id: Set(None),
            match_status: Set(if *missing { "missing" } else { "matched" }.into()),
            match_method: Set(None),
            match_confidence: Set(None),
            ambiguous_candidates: Set(None),
            user_resolved_at: Set(None),
            matched_at: Set(None),
        }
        .insert(&db)
        .await
        .unwrap();
    }
    (list_id, ids)
}

async fn watch(app: &TestApp, cv_series_id: i64) {
    mylar_watch::ActiveModel {
        cv_series_id: Set(cv_series_id),
        status: Set("Active".into()),
        continuing: Set(true),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
}

async fn rows(app: &TestApp) -> Vec<mylar_add_request::Model> {
    mylar_add_request::Entity::find()
        .all(&app.state().db)
        .await
        .unwrap()
}

async fn row(app: &TestApp, series: i64, issue: Option<i64>) -> mylar_add_request::Model {
    let mut q =
        mylar_add_request::Entity::find().filter(mylar_add_request::Column::CvSeriesId.eq(series));
    q = match issue {
        Some(i) => q.filter(mylar_add_request::Column::CvIssueId.eq(i)),
        None => q.filter(mylar_add_request::Column::CvIssueId.is_null()),
    };
    q.one(&app.state().db).await.unwrap().expect("request row")
}

const STANDARD: &[EntrySpec] = &[
    ("Alpha", "1", Some(100), Some(1001), true),
    ("Alpha", "2", Some(100), Some(1002), true),
    ("Beta", "1", Some(200), Some(2001), true),
    ("Gamma", "1", None, None, true),
    ("Delta", "1", Some(300), Some(3001), false),
];

/// A Mylar that answers one command for one id.
async fn answer(server: &MockServer, cmd: &str, id: i64, body: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path("/api"))
        .and(query_param("apikey", "secret-key"))
        .and(query_param("cmd", cmd))
        .and(query_param("id", id.to_string()))
        .respond_with(body)
        .mount(server)
        .await;
}

/// Point add-to-Mylar at a mock Mylar.
fn configure_at(app: &TestApp, mylar: &MockServer) {
    let mut cfg = (*app.state().cfg()).clone();
    cfg.integrations_mylar_url = Some(mylar.uri());
    cfg.integrations_mylar_api_key = Some("secret-key".into());
    app.state().replace_cfg(cfg);
}

async fn audited(app: &TestApp, action: &str) -> usize {
    entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq(action))
        .all(&app.state().db)
        .await
        .unwrap()
        .len()
}

fn ok_json(v: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(v)
}

// ────────────────────── endpoints ──────────────────────

#[tokio::test]
async fn endpoints_are_admin_only_and_need_a_stored_address_and_key() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    let (list_id, _) = seed_list(&app, STANDARD).await;
    let body = json!({ "cbl_list_id": list_id });
    let status_uri = format!("{REQUESTS}?cbl_list_id={list_id}");

    for (m, uri, b) in [
        (Method::POST, REQUESTS, Some(body.clone())),
        (Method::GET, status_uri.as_str(), None),
        (Method::POST, TEST, None),
    ] {
        let (status, _) = call(&app, m, uri, &reader, b).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }

    let (status, resp) = call(&app, Method::POST, REQUESTS, &admin, Some(body.clone())).await;
    assert_eq!(status, StatusCode::CONFLICT, "{resp}");
    let (status, _) = call(&app, Method::POST, TEST, &admin, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, resp) = call(&app, Method::GET, &status_uri, &admin, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resp["configured"], false);
    assert_eq!(resp["missing"], 4);
    assert!(rows(&app).await.is_empty());

    configure(&app);
    let missing_list = json!({ "cbl_list_id": Uuid::now_v7() });
    let (status, _) = call(&app, Method::POST, REQUESTS, &admin, Some(missing_list)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let empty = json!({ "cbl_list_id": list_id, "entry_ids": [] });
    let (status, _) = call(&app, Method::POST, REQUESTS, &admin, Some(empty)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn a_request_queues_series_and_issues_once_and_is_audited() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    configure(&app);
    let (list_id, _) = seed_list(&app, STANDARD).await;
    watch(&app, 200).await;

    let body = json!({ "cbl_list_id": list_id, "want_issues": true });
    let (status, resp) = call(&app, Method::POST, REQUESTS, &admin, Some(body.clone())).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{resp}");
    assert_eq!(
        resp,
        json!({
            "series_requested": 1,
            "issues_requested": 3,
            "series_in_mylar": 1,
            "already_requested": 0,
            "without_comicvine_id": 1,
        })
    );
    let all = rows(&app).await;
    assert_eq!(all.len(), 4, "the matched entry (300) is never queued");
    assert!(all.iter().all(|r| r.cv_series_id != 300));
    assert!(all.iter().all(|r| r.status == "pending"));
    assert_eq!(row(&app, 100, None).await.series_name, "Alpha");

    let (status, resp) = call(&app, Method::POST, REQUESTS, &admin, Some(body)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(resp["series_requested"], 0);
    assert_eq!(resp["issues_requested"], 0);
    assert_eq!(resp["already_requested"], 4);
    assert_eq!(rows(&app).await.len(), 4);

    let audited = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("admin.integration.mylar.add"))
        .all(&app.state().db)
        .await
        .unwrap();
    assert_eq!(audited.len(), 2);
}

#[tokio::test]
async fn chosen_entries_only_and_series_only_by_default() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    configure(&app);
    let (list_id, ids) = seed_list(&app, STANDARD).await;

    // Beta #1 and the matched Delta #1: only Beta is missing.
    let body = json!({ "cbl_list_id": list_id, "entry_ids": [ids[2], ids[4]] });
    let (status, resp) = call(&app, Method::POST, REQUESTS, &admin, Some(body)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{resp}");
    assert_eq!(resp["series_requested"], 1);
    assert_eq!(resp["issues_requested"], 0);
    let all = rows(&app).await;
    assert_eq!(all.len(), 1);
    assert_eq!((all[0].cv_series_id, all[0].cv_issue_id), (200, None));
}

// ────────────────────── sweep ──────────────────────

#[tokio::test]
async fn the_sweep_adds_series_wants_issues_and_waits_for_imports() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let (list_id, _) = seed_list(&app, STANDARD).await;
    watch(&app, 200).await;
    let db = app.state().db.clone();
    let actor = entity::user::Entity::find()
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    requests::create(&db, actor.id, list_id, None, true)
        .await
        .unwrap();

    let mylar = MockServer::start().await;
    // Alpha (100) is not in Mylar yet: added, its issues unknown for now.
    answer(
        &mylar,
        "getComic",
        100,
        ok_json(json!({"success": true, "data": {"comic": [], "issues": []}})),
    )
    .await;
    answer(
        &mylar,
        "addComic",
        100,
        ok_json(json!({"success": true, "data": "Successfully queued up addding id: 100"})),
    )
    .await;
    let unknown = json!({"success": false, "error": {"message": "No issue found with that ID"}});
    answer(&mylar, "getIssueInfo", 1001, ok_json(unknown.clone())).await;
    answer(&mylar, "getIssueInfo", 1002, ok_json(unknown)).await;
    // Beta (200) is in Mylar; its issue is Skipped, so it gets queued.
    answer(
        &mylar,
        "getIssueInfo",
        2001,
        ok_json(json!({"success": true, "data": [{"status": "Skipped"}]})),
    )
    .await;
    answer(
        &mylar,
        "queueIssue",
        2001,
        ResponseTemplate::new(200).set_body_string("OK"),
    )
    .await;

    let client = MylarClient::new(&mylar.uri(), "secret-key").unwrap();
    let tick = sweep::run_with(&db, &client).await;
    assert_eq!((tick.done, tick.waiting, tick.failed), (2, 2, 0));
    assert_eq!(row(&app, 100, None).await.status, "done");
    assert_eq!(row(&app, 200, Some(2001)).await.status, "done");
    let waiting = row(&app, 100, Some(1001)).await;
    assert_eq!(waiting.status, "pending");
    assert_eq!(waiting.attempts, 1);
    assert!(waiting.next_attempt_at > Utc::now().fixed_offset());

    // Nothing is due again yet.
    let tick = sweep::run_with(&db, &client).await;
    assert_eq!(tick, sweep::Tick::default());

    configure(&app);
    let uri = format!("{REQUESTS}?cbl_list_id={list_id}");
    let (status, resp) = call(&app, Method::GET, &uri, &admin, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resp["configured"], true);
    assert_eq!(resp["without_comicvine_id"], 1);
    assert_eq!(
        resp["series"],
        json!([
            {"cv_id": 100, "state": "in_mylar", "error": null},
            {"cv_id": 200, "state": "in_mylar", "error": null},
        ])
    );
    assert_eq!(
        resp["issues"],
        json!([
            {"cv_id": 1001, "state": "requested", "error": null},
            {"cv_id": 1002, "state": "requested", "error": null},
            {"cv_id": 2001, "state": "in_mylar", "error": null},
        ])
    );
}

#[tokio::test]
async fn a_refusal_fails_the_row_and_asking_again_requeues_it() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let (list_id, _) = seed_list(&app, &STANDARD[..1]).await;
    let db = app.state().db.clone();
    let actor = entity::user::Entity::find()
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    requests::create(&db, actor.id, list_id, None, false)
        .await
        .unwrap();

    let mylar = MockServer::start().await;
    answer(
        &mylar,
        "getComic",
        100,
        ok_json(json!({"success": true, "data": {"comic": []}})),
    )
    .await;
    answer(
        &mylar,
        "addComic",
        100,
        ok_json(json!({"success": false, "error": {"message": "Invalid ComicID"}})),
    )
    .await;
    let client = MylarClient::new(&mylar.uri(), "secret-key").unwrap();
    let tick = sweep::run_with(&db, &client).await;
    assert_eq!(tick.failed, 1);
    let failed = row(&app, 100, None).await;
    assert_eq!(failed.status, "failed");
    assert_eq!(
        failed.last_error.as_deref(),
        Some("Mylar refused: Invalid ComicID")
    );
    let status = requests::list_status(&db, list_id).await.unwrap();
    assert_eq!(status.series[0].state, requests::TargetState::Failed);

    let again = requests::create(&db, actor.id, list_id, None, false)
        .await
        .unwrap();
    assert_eq!(again.series_requested, 1);
    let requeued = row(&app, 100, None).await;
    assert_eq!(requeued.id, failed.id, "the same row, reset");
    assert_eq!(
        (requeued.status.as_str(), requeued.attempts),
        ("pending", 0)
    );
    assert_eq!(requeued.last_error, None);

    // Accepted just now: asking again sends nothing. Accepted long ago
    // (the series may be gone from Mylar): asking again re-sends it.
    let mut done: mylar_add_request::ActiveModel = requeued.into();
    done.status = Set("done".into());
    let done = done.update(&db).await.unwrap();
    let again = requests::create(&db, actor.id, list_id, None, false)
        .await
        .unwrap();
    assert_eq!((again.series_requested, again.already_requested), (0, 1));
    let mut old: mylar_add_request::ActiveModel = done.into();
    old.updated_at = Set(Utc::now().fixed_offset() - chrono::Duration::hours(25));
    old.update(&db).await.unwrap();
    let again = requests::create(&db, actor.id, list_id, None, false)
        .await
        .unwrap();
    assert_eq!(again.series_requested, 1);
    assert_eq!(row(&app, 100, None).await.status, "pending");
}

#[tokio::test]
async fn mylar_being_down_counts_against_no_request() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let (list_id, _) = seed_list(&app, STANDARD).await;
    let db = app.state().db.clone();
    let actor = entity::user::Entity::find()
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    requests::create(&db, actor.id, list_id, None, true)
        .await
        .unwrap();

    let mylar = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&mylar)
        .await;
    let client = MylarClient::new(&mylar.uri(), "secret-key").unwrap();
    let tick = sweep::run_with(&db, &client).await;
    assert_eq!(tick, sweep::Tick::default());
    assert_eq!(mylar.received_requests().await.unwrap().len(), 1);
    assert!(
        rows(&app)
            .await
            .iter()
            .all(|r| r.status == "pending" && r.attempts == 0)
    );
}

#[tokio::test]
async fn a_bad_key_is_reported_by_the_connection_test() {
    let mylar = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("cmd", "getVersion"))
        .respond_with(ok_json(
            json!({"success": false, "error": {"message": "Incorrect API key"}}),
        ))
        .mount(&mylar)
        .await;
    let client = MylarClient::new(&mylar.uri(), "wrong").unwrap();
    let err = client.version().await.unwrap_err();
    assert_eq!(err.to_string(), "Mylar refused: Incorrect API key");
}

// ────────────────────── wanted check + actions ──────────────────────

#[tokio::test]
async fn the_wanted_check_finds_what_the_library_already_holds() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = common::seed::seed_library(&db, tmp.path()).await;
    let series = common::seed::seed_series(&db, lib, "Alpha").await;
    let held =
        common::seed::seed_issue(&db, lib, series, &tmp.path().join("a1.cbz"), b"a1", 1.0).await;
    let now = Utc::now().fixed_offset();
    entity::external_id::ActiveModel {
        entity_type: Set("issue".into()),
        entity_id: Set(held),
        source: Set("comicvine".into()),
        external_id: Set("1001".into()),
        external_url: Set(None),
        set_by: Set("user".into()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    // Reading lists miss 1001, 1002 and 2001; Mylar wants 1001 and 1002.
    seed_list(&app, STANDARD).await;

    let mylar = MockServer::start().await;
    Mock::given(method("GET"))
        .and(query_param("cmd", "getWanted"))
        .respond_with(ok_json(json!({
            "issues": [
                {"ComicName": "Alpha", "Issue_Number": "1", "ComicID": "100", "IssueID": "1001", "Status": "Wanted"},
                {"ComicName": "Alpha", "Issue_Number": "2", "ComicID": "100", "IssueID": "1002", "Status": "Wanted"},
                {"ComicName": "One-off", "Issue_Number": "1", "ComicID": "S123", "IssueID": "x", "Status": "Wanted"},
            ],
            "annuals": [
                {"ComicName": "Alpha Annual", "Issue_Number": "1", "ComicID": 100, "IssueID": 1500, "Status": "Wanted"},
            ],
        })))
        .mount(&mylar)
        .await;
    configure_at(&app, &mylar);

    const CHECK: &str = "/api/admin/integrations/mylar/wanted-check";
    let (status, _) = call(&app, Method::GET, CHECK, &reader, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, resp) = call(&app, Method::GET, CHECK, &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["wanted"], 3, "the row without numeric ids is left out");
    assert_eq!(resp["owned_issues"], 1);
    assert_eq!(resp["owned_series_total"], 1);
    assert_eq!(
        resp["owned"],
        json!([{
            "cv_series_id": 100,
            "series": "Alpha",
            "issues": [{"cv_issue_id": 1001, "issue_number": "1"}],
        }])
    );
    assert_eq!(resp["list_missing"], 3);
    assert_eq!(resp["list_missing_not_wanted"], 1);
}

#[tokio::test]
async fn the_actions_are_admin_only_audited_and_refuse_what_mylar_lacks() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    const SEARCH: &str = "/api/admin/integrations/mylar/search";
    let recheck = |id: i64| format!("/api/admin/integrations/mylar/series/{id}/recheck");
    let refresh = |id: i64| format!("/api/admin/integrations/mylar/series/{id}/refresh");
    let unwant = |id: i64| format!("/api/admin/integrations/mylar/issues/{id}/unwant");

    for uri in [SEARCH.to_owned(), recheck(100), refresh(100), unwant(1001)] {
        let (status, _) = call(&app, Method::POST, &uri, &reader, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        let (status, _) = call(&app, Method::POST, &uri, &admin, None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{uri}: not configured");
    }

    let mylar = MockServer::start().await;
    configure_at(&app, &mylar);
    Mock::given(method("GET"))
        .and(query_param("cmd", "forceSearch"))
        .respond_with(ResponseTemplate::new(200).set_body_string("OK"))
        .mount(&mylar)
        .await;
    // Mylar has series 100, not 200.
    answer(
        &mylar,
        "getComic",
        100,
        ok_json(json!({"success": true, "data": {"comic": [{"id": "100"}]}})),
    )
    .await;
    answer(
        &mylar,
        "getComic",
        200,
        ok_json(json!({"success": true, "data": {"comic": []}})),
    )
    .await;
    // recheckFiles answers `null`; refreshComic queues, then breaks on
    // its own success message (a 500).
    answer(
        &mylar,
        "recheckFiles",
        100,
        ResponseTemplate::new(200).set_body_string("null"),
    )
    .await;
    answer(&mylar, "refreshComic", 100, ResponseTemplate::new(500)).await;

    let (status, resp) = call(&app, Method::POST, SEARCH, &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["outcome"], "done");
    let (status, resp) = call(&app, Method::POST, &recheck(100), &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["outcome"], "done");
    let (status, resp) = call(&app, Method::POST, &refresh(100), &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["outcome"], "started");
    for uri in [recheck(200), refresh(200)] {
        let (status, _) = call(&app, Method::POST, &uri, &admin, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
    let sent = mylar.received_requests().await.unwrap();
    let sent_cmd = |cmd: &str, id: &str| {
        sent.iter().any(|r| {
            let q = r.url.query().unwrap_or_default();
            q.contains(&format!("cmd={cmd}")) && q.contains(&format!("id={id}"))
        })
    };
    assert!(!sent_cmd("recheckFiles", "200") && !sent_cmd("refreshComic", "200"));

    for action in ["search", "recheck", "refresh"] {
        let name = format!("admin.integration.mylar.{action}");
        assert_eq!(audited(&app, &name).await, 1, "{name}");
    }
}

#[tokio::test]
async fn un_want_only_touches_an_issue_mylar_has_as_wanted() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let (list_id, _) = seed_list(&app, STANDARD).await;
    let db = app.state().db.clone();
    let actor = entity::user::Entity::find()
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    requests::create(&db, actor.id, list_id, None, true)
        .await
        .unwrap();
    let unwant = |id: i64| format!("/api/admin/integrations/mylar/issues/{id}/unwant");

    let mylar = MockServer::start().await;
    configure_at(&app, &mylar);
    let status_of = |s: &str| ok_json(json!({"success": true, "data": [{"status": s}]}));
    answer(&mylar, "getIssueInfo", 1001, status_of("Wanted")).await;
    answer(&mylar, "getIssueInfo", 1002, status_of("Downloaded")).await;
    answer(
        &mylar,
        "getIssueInfo",
        9999,
        ok_json(json!({"success": false, "error": {"message": "No issue found with that ID"}})),
    )
    .await;
    answer(
        &mylar,
        "unqueueIssue",
        1001,
        ResponseTemplate::new(200).set_body_string("OK"),
    )
    .await;

    let (status, resp) = call(&app, Method::POST, &unwant(1002), &admin, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{resp}");
    assert_eq!(
        resp["error"]["message"],
        "Mylar has this issue as Downloaded, not Wanted"
    );
    let (status, _) = call(&app, Method::POST, &unwant(9999), &admin, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, resp) = call(&app, Method::POST, &unwant(1001), &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let left = rows(&app).await;
    assert!(left.iter().all(|r| r.cv_issue_id != Some(1001)));
    assert!(left.iter().any(|r| r.cv_issue_id == Some(1002)));
    assert_eq!(audited(&app, "admin.integration.mylar.unwant").await, 1);

    let unqueued: Vec<String> = mylar
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter_map(|r| r.url.query().map(str::to_owned))
        .filter(|q| q.contains("cmd=unqueueIssue"))
        .collect();
    assert_eq!(unqueued.len(), 1, "{unqueued:?}");
}
