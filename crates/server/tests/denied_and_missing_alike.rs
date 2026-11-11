//! To a caller who may not see an object, that object does not exist:
//! every route answers a hidden library, series or issue exactly as it
//! answers an id that was never issued (same status, same body).
//! Soft-removed content is not secret: a removed series is not-found for
//! a non-admin on every series route, a removed issue in a live series
//! keeps its reads, and nobody makes new writes on removed content.
//!
//! Callers, each compared byte for byte with the same request carrying a
//! random unknown id:
//!
//! - `nogrant`: no grant on the library at all;
//! - `capped`: a `Teen` cap, facing a `Mature 17+` series;
//! - `reader`: a full grant, facing a soft-removed series and a
//!   soft-removed issue (writes, and series routes).
//!
//! The admin (first registered account) still gets through where an admin
//! is meant to.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{CblListSeed, IssueSeed, LibrarySeed, SeriesSeed, seed_cbl_entry};
use entity::{library_user_access, marker, saved_view, series, user::Entity as UserEntity};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set, Unchanged,
};
use serde_json::{Value, json};
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
        .filter_map(|v| v.to_str().ok())
        .map(str::to_owned)
        .collect();
    let extract = |prefix: &str| -> String {
        cookies
            .iter()
            .find(|c| c.starts_with(prefix))
            .and_then(|c| c.split(';').next())
            .map(|kv| kv.split_once('=').map(|(_, v)| v.to_owned()).unwrap())
            .expect("cookie")
    };
    let user_row = UserEntity::find()
        .filter(entity::user::Column::Email.eq(email))
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("user row");
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id: user_row.id,
    }
}

async fn call(
    app: &TestApp,
    user: &Authed,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder().method(method.clone()).uri(path).header(
        header::COOKIE,
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            user.session, user.csrf
        ),
    );
    if method != Method::GET {
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
    let bytes = to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

fn json_of(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

/// The hidden request and the unknown-id request answer byte for byte
/// alike, and neither is a success.
#[allow(clippy::too_many_arguments)]
async fn assert_alike(
    app: &TestApp,
    user: &Authed,
    who: &str,
    method: Method,
    hidden_path: &str,
    hidden_body: Option<Value>,
    unknown_path: &str,
    unknown_body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
    let hidden = call(app, user, method.clone(), hidden_path, hidden_body).await;
    let unknown = call(app, user, method.clone(), unknown_path, unknown_body).await;
    assert_eq!(
        hidden.0,
        unknown.0,
        "{who}: {method} {hidden_path} status differs from the unknown id ({}, {})",
        String::from_utf8_lossy(&hidden.1),
        String::from_utf8_lossy(&unknown.1),
    );
    assert_eq!(
        String::from_utf8_lossy(&hidden.1),
        String::from_utf8_lossy(&unknown.1),
        "{who}: {method} {hidden_path} body differs from the unknown id"
    );
    hidden
}

/// One hidden target per caller.
struct Hidden {
    series_id: Uuid,
    series_slug: String,
    issue_id: String,
    issue_slug: String,
    /// The slug of the series the issue lives in (for issue routes).
    issue_series_slug: String,
}

struct Fixture {
    admin: Authed,
    nogrant: Authed,
    capped: Authed,
    reader: Authed,
    lib: Uuid,
    mature: Hidden,
    live: Hidden,
    removed: Hidden,
    /// An issue whose parent series is soft-removed (the issue itself is not).
    in_removed_series: String,
    in_removed_series_slug: String,
    _dir: tempfile::TempDir,
}

async fn issue_slug(db: &DatabaseConnection, id: &str) -> String {
    entity::issue::Entity::find_by_id(id.to_owned())
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .slug
}

async fn series_slug(db: &DatabaseConnection, id: Uuid) -> String {
    series::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .slug
}

async fn grant(db: &DatabaseConnection, lib: Uuid, user: Uuid, cap: Option<&str>) {
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        library_id: Set(lib),
        user_id: Set(user),
        age_rating_max: Set(cap.map(str::to_owned)),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn setup() -> (TestApp, Fixture) {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let nogrant = register(&app, "nogrant@example.com").await;
    let capped = register(&app, "capped@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    let db = app.state().db.clone();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let lib = LibrarySeed::new(root).insert(&db).await;

    // Mature series + issue (hidden from the capped reader).
    let mature_series = SeriesSeed::new(lib, "Mature Series").insert(&db).await;
    series::ActiveModel {
        id: Unchanged(mature_series),
        age_rating: Set(Some("Mature 17+".into())),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();
    let mature_issue = IssueSeed::new(lib, mature_series, &root.join("m1.cbz"), b"mature-1", 1.0)
        .insert(&db)
        .await;

    // A live series with a live issue and a soft-removed issue.
    let live_series = SeriesSeed::new(lib, "Live Series").insert(&db).await;
    let live_issue = IssueSeed::new(lib, live_series, &root.join("l1.cbz"), b"live-1", 1.0)
        .insert(&db)
        .await;
    let removed_issue = IssueSeed::new(lib, live_series, &root.join("l2.cbz"), b"live-2", 2.0)
        .insert(&db)
        .await;
    let now = Utc::now().fixed_offset();
    entity::issue::ActiveModel {
        id: Unchanged(removed_issue.clone()),
        removed_at: Set(Some(now)),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();

    // A soft-removed series whose issue is not itself marked.
    let removed_series = SeriesSeed::new(lib, "Removed Series").insert(&db).await;
    let in_removed_series =
        IssueSeed::new(lib, removed_series, &root.join("r1.cbz"), b"removed-1", 1.0)
            .insert(&db)
            .await;
    series::ActiveModel {
        id: Unchanged(removed_series),
        removed_at: Set(Some(now)),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();

    grant(&db, lib, capped.user_id, Some("Teen")).await;
    grant(&db, lib, reader.user_id, None).await;

    let mature = Hidden {
        series_id: mature_series,
        series_slug: series_slug(&db, mature_series).await,
        issue_slug: issue_slug(&db, &mature_issue).await,
        issue_id: mature_issue,
        issue_series_slug: series_slug(&db, mature_series).await,
    };
    let live = Hidden {
        series_id: live_series,
        series_slug: series_slug(&db, live_series).await,
        issue_slug: issue_slug(&db, &live_issue).await,
        issue_id: live_issue,
        issue_series_slug: series_slug(&db, live_series).await,
    };
    let removed = Hidden {
        series_id: removed_series,
        series_slug: series_slug(&db, removed_series).await,
        issue_slug: issue_slug(&db, &removed_issue).await,
        issue_id: removed_issue,
        issue_series_slug: series_slug(&db, live_series).await,
    };
    (
        app,
        Fixture {
            admin,
            nogrant,
            capped,
            reader,
            lib,
            mature,
            live,
            removed,
            in_removed_series_slug: issue_slug(&db, &in_removed_series).await,
            in_removed_series,
            _dir: dir,
        },
    )
}

/// `(caller label, caller, target)` for writes: hidden by access, and
/// soft-removed content (no new writes on it, for any role).
fn cases(f: &Fixture) -> [(&'static str, &Authed, &Hidden); 3] {
    [
        ("no grant", &f.nogrant, &f.live),
        ("capped", &f.capped, &f.mature),
        ("removed", &f.reader, &f.removed),
    ]
}

/// The two cases hidden by access, for reads.
fn access_cases(f: &Fixture) -> [(&'static str, &Authed, &Hidden); 2] {
    [
        ("no grant", &f.nogrant, &f.live),
        ("capped", &f.capped, &f.mature),
    ]
}

/// The error code of an error envelope.
fn code(bytes: &[u8]) -> String {
    json_of(bytes)["error"]["code"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn unknown_issue_id() -> String {
    format!("{:0>64}", Uuid::now_v7().simple())
}

fn unknown_slug() -> String {
    format!("no-such-{}", Uuid::now_v7())
}

// ───────── a. progress ─────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn progress_answers_hidden_like_unknown() {
    let (app, f) = setup().await;
    for (who, user, h) in cases(&f) {
        let unknown = unknown_issue_id();
        let (status, body) = assert_alike(
            &app,
            user,
            who,
            Method::POST,
            "/api/me/progress/bulk",
            Some(json!({ "issue_ids": [h.issue_id], "finished": true })),
            "/api/me/progress/bulk",
            Some(json!({ "issue_ids": [unknown], "finished": true })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{who}");
        let body = json_of(&body);
        assert_eq!(body["not_found"], 1, "{who}: {body}");
        assert_eq!(body["forbidden"], 0, "{who}: {body}");
        assert_eq!(body["updated"], 0, "{who}: {body}");

        let (_, body) = assert_alike(
            &app,
            user,
            who,
            Method::POST,
            "/api/me/progress/series-bulk",
            Some(json!({ "series_ids": [h.series_id], "finished": true })),
            "/api/me/progress/series-bulk",
            Some(json!({ "series_ids": [Uuid::now_v7()], "finished": true })),
        )
        .await;
        let body = json_of(&body);
        assert_eq!(body["not_found_series"], 1, "{who}: {body}");
        assert_eq!(body["forbidden_series"], 0, "{who}: {body}");

        for suffix in ["progress", "progress-matching"] {
            let (status, _) = assert_alike(
                &app,
                user,
                who,
                Method::POST,
                &format!("/api/series/{}/{suffix}", h.series_slug),
                Some(json!({ "finished": true })),
                &format!("/api/series/{}/{suffix}", unknown_slug()),
                Some(json!({ "finished": true })),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{who} {suffix}");
        }

        let (status, _) = assert_alike(
            &app,
            user,
            who,
            Method::POST,
            "/api/progress",
            Some(json!({ "issue_id": h.issue_id, "page": 0 })),
            "/api/progress",
            Some(json!({ "issue_id": unknown_issue_id(), "page": 0 })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who}");
    }

    // An issue whose series was soft-removed is gone for writes too.
    assert_alike(
        &app,
        &f.reader,
        "removed parent",
        Method::POST,
        "/api/progress",
        Some(json!({ "issue_id": f.in_removed_series, "page": 0 })),
        "/api/progress",
        Some(json!({ "issue_id": unknown_issue_id(), "page": 0 })),
    )
    .await;

    // The admin still writes progress on content above any cap.
    let (status, body) = call(
        &app,
        &f.admin,
        Method::POST,
        "/api/me/progress/bulk",
        Some(json!({ "issue_ids": [f.mature.issue_id], "finished": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_of(&body)["updated"], 1);
    // And the reader writes on what they can see.
    let (status, _) = call(
        &app,
        &f.reader,
        Method::POST,
        "/api/progress",
        Some(json!({ "issue_id": f.live.issue_id, "page": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

// ───────── b. markers ─────────

async fn insert_marker(app: &TestApp, owner: Uuid, series_id: Uuid, issue_id: &str) -> Uuid {
    let now = Utc::now().fixed_offset();
    let id = Uuid::now_v7();
    marker::ActiveModel {
        id: Set(id),
        user_id: Set(owner),
        series_id: Set(series_id),
        issue_id: Set(issue_id.to_owned()),
        page_index: Set(0),
        kind: Set(marker::MarkerKind::Bookmark),
        is_favorite: Set(false),
        tags: Set(Vec::new()),
        region: Set(None),
        selection: Set(None),
        body: Set(None),
        color: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        hidden_from_log: Set(false),
        page_hash: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
    id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn markers_answer_hidden_like_unknown() {
    let (app, f) = setup().await;
    for (who, user, h) in cases(&f) {
        let (status, _) = assert_alike(
            &app,
            user,
            who,
            Method::POST,
            "/api/me/markers",
            Some(json!({ "issue_id": h.issue_id, "page_index": 0, "kind": "bookmark" })),
            "/api/me/markers",
            Some(json!({ "issue_id": unknown_issue_id(), "page_index": 0, "kind": "bookmark" })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who}");
        if who != "removed" {
            let (status, body) = assert_alike(
                &app,
                user,
                who,
                Method::GET,
                &format!("/api/me/issues/{}/markers", h.issue_id),
                None,
                &format!("/api/me/issues/{}/markers", unknown_issue_id()),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{who}");
            assert_eq!(code(&body), "not_found", "{who}");
        }

        // The caller's own marker on content now hidden from them: left
        // out of the list and the count, reading or editing it answers
        // like an unknown marker, deleting it still works.
        let own = insert_marker(&app, user.user_id, h.series_id, &h.issue_id).await;
        let (_, list) = call(&app, user, Method::GET, "/api/me/markers", None).await;
        assert!(
            !String::from_utf8_lossy(&list).contains(&own.to_string()),
            "{who}: hidden marker listed"
        );
        let (_, count) = call(&app, user, Method::GET, "/api/me/markers/count", None).await;
        assert_eq!(json_of(&count)["total"], 0, "{who}: hidden marker counted");
        let (status, _) = assert_alike(
            &app,
            user,
            who,
            Method::PATCH,
            &format!("/api/me/markers/{own}"),
            Some(json!({ "is_favorite": true })),
            &format!("/api/me/markers/{}", Uuid::now_v7()),
            Some(json!({ "is_favorite": true })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who}");
        let (status, _) = call(
            &app,
            user,
            Method::DELETE,
            &format!("/api/me/markers/{own}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{who}: own marker delete");
    }

    // A removed issue is history: the reader's overlay for it still reads,
    // with the caller's own markers on it.
    let own = insert_marker(
        &app,
        f.reader.user_id,
        f.live.series_id,
        &f.removed.issue_id,
    )
    .await;
    let (status, body) = call(
        &app,
        &f.reader,
        Method::GET,
        &format!("/api/me/issues/{}/markers", f.removed.issue_id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains(&own.to_string()));

    // The permalink of the caller's own marker on content hidden by access
    // answers like an unknown marker; on a removed issue it still opens.
    for (who, user, h) in access_cases(&f) {
        let own = insert_marker(&app, user.user_id, h.series_id, &h.issue_id).await;
        let (status, _) = assert_alike(
            &app,
            user,
            who,
            Method::GET,
            &format!("/markers/{own}"),
            None,
            &format!("/markers/{}", Uuid::now_v7()),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who} permalink");
    }
    let (status, _) = call(
        &app,
        &f.reader,
        Method::GET,
        &format!("/markers/{own}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "removed issue permalink");

    // Marker restore: a snapshot on hidden or removed content is skipped,
    // exactly like one on an unknown issue.
    for (who, user, h) in cases(&f) {
        let snap = |issue: &str| {
            json!({ "markers": [{
                "id": Uuid::now_v7(),
                "issue_id": issue,
                "page_index": 0,
                "kind": "bookmark"
            }] })
        };
        let hidden = call(
            &app,
            user,
            Method::POST,
            "/api/me/markers/restore",
            Some(snap(&h.issue_id)),
        )
        .await;
        let unknown = call(
            &app,
            user,
            Method::POST,
            "/api/me/markers/restore",
            Some(snap(&unknown_issue_id())),
        )
        .await;
        assert_eq!(
            hidden.0,
            StatusCode::OK,
            "{who}: {}",
            String::from_utf8_lossy(&hidden.1)
        );
        assert_eq!(hidden.0, unknown.0, "{who}");
        assert_eq!(json_of(&hidden.1), json_of(&unknown.1), "{who}");
        assert_eq!(json_of(&hidden.1)["skipped"], 1, "{who}");
    }

    // Another user's marker answers like an unknown one, for read-free
    // edits and deletes alike.
    let theirs = insert_marker(&app, f.reader.user_id, f.live.series_id, &f.live.issue_id).await;
    for method in [Method::PATCH, Method::DELETE] {
        let body = (method == Method::PATCH).then(|| json!({ "is_favorite": true }));
        let (status, _) = assert_alike(
            &app,
            &f.capped,
            "other user's marker",
            method,
            &format!("/api/me/markers/{theirs}"),
            body.clone(),
            &format!("/api/me/markers/{}", Uuid::now_v7()),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // The admin still creates a marker on content above any cap.
    let (status, _) = call(
        &app,
        &f.admin,
        Method::POST,
        "/api/me/markers",
        Some(json!({ "issue_id": f.mature.issue_id, "page_index": 0, "kind": "bookmark" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

/// A closing note on `issue_id` for `owner`.
async fn insert_note(app: &TestApp, owner: Uuid, issue_id: &str, body: &str) {
    let now = Utc::now().fixed_offset();
    entity::user_issue_note::ActiveModel {
        user_id: Set(owner),
        issue_id: Set(issue_id.to_owned()),
        body: Set(body.to_owned()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
}

/// The notes export keeps every marker and closing note the user owns.
/// Removed content keeps its identity, flagged unavailable; content hidden
/// by access sits in one anonymous group that names nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notes_export_flags_removed_and_names_nothing_hidden() {
    let (app, f) = setup().await;
    // The capped reader: one marker on a live issue, one on a removed
    // issue, one marker and a closing note on the Mature issue above the cap.
    let user = &f.capped;
    let visible = insert_marker(&app, user.user_id, f.live.series_id, &f.live.issue_id).await;
    let on_removed = insert_marker(&app, user.user_id, f.live.series_id, &f.removed.issue_id).await;
    let on_hidden = insert_marker(&app, user.user_id, f.mature.series_id, &f.mature.issue_id).await;
    insert_note(
        &app,
        user.user_id,
        &f.mature.issue_id,
        "hidden closing note",
    )
    .await;

    let (status, body) = call(
        &app,
        user,
        Method::GET,
        "/api/me/markers/export?format=json",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let doc = json_of(&body);
    assert_eq!(doc["total"], 3, "{doc}");
    assert_eq!(doc["closing_notes"], 1, "{doc}");
    let text = doc.to_string();
    // Nothing about the hidden content is named.
    assert!(!text.contains("Mature Series"), "{text}");
    assert!(!text.contains(&f.mature.issue_id), "{text}");
    assert!(!text.contains(&f.mature.series_id.to_string()), "{text}");

    let markers: Vec<Value> = doc["series"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["issues"].as_array().unwrap().clone())
        .flat_map(|i| i["pages"].as_array().unwrap().clone())
        .flat_map(|p| p["markers"].as_array().unwrap().clone())
        .collect();
    let find = |id: Uuid| {
        markers
            .iter()
            .find(|m| m["id"] == id.to_string())
            .unwrap_or_else(|| panic!("marker {id} exported: {doc}"))
            .clone()
    };
    let m = find(visible);
    assert_eq!(m["available"], true);
    assert!(m["jump_url"].is_string());
    // Removed: identity kept, flagged, no link.
    let m = find(on_removed);
    assert_eq!(m["available"], false);
    assert!(m.get("jump_url").is_none());
    assert_eq!(m["issue"]["issue_id"], json!(f.removed.issue_id));
    assert_eq!(m["issue"]["series_name"], "Live Series");
    // Hidden by access: blank issue reference, flagged, no link.
    let m = find(on_hidden);
    assert_eq!(m["available"], false);
    assert!(m.get("jump_url").is_none());
    assert_eq!(m["issue"]["issue_id"], "");
    assert!(m["issue"]["series_name"].is_null());
    assert!(m["issue"]["content_hash"].is_null());

    // The anonymous group: last, nil id, no year or library, no titles,
    // and the closing note without a rating.
    let groups = doc["series"].as_array().unwrap();
    let anon = groups.last().unwrap();
    assert_eq!(anon["series"]["series_name"], "No longer available");
    assert_eq!(anon["series"]["series_id"], Uuid::nil().to_string());
    assert!(anon["series"]["series_year"].is_null());
    assert!(anon["series"]["library_slug"].is_null());
    for i in anon["issues"].as_array().unwrap() {
        assert!(i["issue_title"].is_null(), "{i}");
        assert_eq!(i["issue"]["issue_id"], "", "{i}");
    }
    let note = anon["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|i| i.get("closing_note").cloned())
        .expect("hidden closing note exported");
    assert_eq!(note["body"], "hidden closing note");
    assert!(note["rating"].is_null());

    // Markdown: the removed marker reads "(no longer available)" under its
    // series; the hidden one sits under the anonymous heading.
    let (status, md) = call(&app, user, Method::GET, "/api/me/markers/export", None).await;
    assert_eq!(status, StatusCode::OK);
    let md = String::from_utf8_lossy(&md);
    assert!(md.contains("## Live Series"), "{md}");
    assert!(md.contains("(no longer available)"), "{md}");
    assert!(md.contains("## No longer available"), "{md}");
    assert!(!md.contains("Mature Series"), "{md}");
}

/// The account export keeps every row the user owns. Rows on content
/// hidden by access keep ids and the content hash but no names; removed
/// content keeps its names.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn account_export_names_nothing_hidden() {
    let (app, f) = setup().await;
    let user = &f.capped;
    insert_marker(&app, user.user_id, f.mature.series_id, &f.mature.issue_id).await;
    insert_marker(&app, user.user_id, f.live.series_id, &f.removed.issue_id).await;
    insert_note(
        &app,
        user.user_id,
        &f.mature.issue_id,
        "hidden closing note",
    )
    .await;
    let (status, body) = call(&app, user, Method::GET, "/api/me/export", None).await;
    assert_eq!(status, StatusCode::OK);
    let doc = json_of(&body);
    let text = doc.to_string();
    assert!(!text.contains("Mature Series"), "{text}");
    let markers = doc["sections"]["markers"].as_array().unwrap();
    let hidden = markers
        .iter()
        .find(|m| m["issue"]["issue_id"] == json!(f.mature.issue_id))
        .expect("hidden marker kept");
    assert!(hidden["issue"]["content_hash"].is_string(), "{hidden}");
    assert!(hidden["issue"]["series_name"].is_null(), "{hidden}");
    assert!(hidden["issue"]["series_year"].is_null(), "{hidden}");
    assert!(hidden["issue"]["issue_number"].is_null(), "{hidden}");
    assert!(hidden["issue"]["library_slug"].is_null(), "{hidden}");
    let removed = markers
        .iter()
        .find(|m| m["issue"]["issue_id"] == json!(f.removed.issue_id))
        .expect("removed marker kept");
    assert_eq!(removed["issue"]["series_name"], "Live Series");
    assert!(removed["issue"]["library_slug"].is_string());
    // The admin's export is unaffected by any cap.
    insert_marker(
        &app,
        f.admin.user_id,
        f.mature.series_id,
        &f.mature.issue_id,
    )
    .await;
    let (_, body) = call(&app, &f.admin, Method::GET, "/api/me/export", None).await;
    assert!(String::from_utf8_lossy(&body).contains("Mature Series"));
}

// ───────── c. reader preferences ─────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reader_prefs_answer_hidden_like_unknown() {
    let (app, f) = setup().await;
    for (who, user, h) in cases(&f) {
        let unknown = Uuid::now_v7();
        let (status, _) = assert_alike(
            &app,
            user,
            who,
            Method::PUT,
            &format!("/api/me/series/{}/reader-prefs", h.series_id),
            Some(json!({ "cover_solo": true })),
            &format!("/api/me/series/{unknown}/reader-prefs"),
            Some(json!({ "cover_solo": true })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who}");
        let (status, _) = assert_alike(
            &app,
            user,
            who,
            Method::GET,
            &format!("/api/me/series/{}/reader-prefs", h.series_id),
            None,
            &format!("/api/me/series/{unknown}/reader-prefs"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who}");
    }
    // Visible series: stored and read back.
    let path = format!("/api/me/series/{}/reader-prefs", f.live.series_id);
    let (status, _) = call(
        &app,
        &f.reader,
        Method::PUT,
        &path,
        Some(json!({ "cover_solo": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(&app, &f.reader, Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_of(&body)["cover_solo"], true);
    // The admin, like the series page, still opens a removed series, but
    // stores nothing new on it.
    let (status, _) = call(
        &app,
        &f.admin,
        Method::GET,
        &format!("/api/me/series/{}/reader-prefs", f.removed.series_id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(
        &app,
        &f.admin,
        Method::PUT,
        &format!("/api/me/series/{}/reader-prefs", f.removed.series_id),
        Some(json!({ "cover_solo": true })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "not_found");
    // Completion overrides: same rule; clearing one stays possible.
    let path = format!("/api/me/series/{}/completion-prefs", f.removed.series_id);
    let (status, _) = call(&app, &f.admin, Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(
        &app,
        &f.admin,
        Method::PUT,
        &path,
        Some(json!({ "count_specials": "include" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "not_found");
    let (status, _) = call(
        &app,
        &f.admin,
        Method::PUT,
        &path,
        Some(json!({ "count_specials": "inherit" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

// ───────── d. metadata, external ids, provider ranges ─────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_routes_answer_hidden_like_unknown() {
    let (app, f) = setup().await;
    let series_gets = [
        "external-ids",
        "provider-ranges",
        "provider-coverage",
        "metadata/status",
        "metadata/candidates",
        "metadata/overview",
        "metadata/issues",
    ];
    for (who, user, h) in cases(&f) {
        let unknown = unknown_slug();
        for suffix in series_gets {
            let (status, _) = assert_alike(
                &app,
                user,
                who,
                Method::GET,
                &format!("/api/series/{}/{suffix}", h.series_slug),
                None,
                &format!("/api/series/{unknown}/{suffix}"),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{who} {suffix}");
        }
        let (status, _) = assert_alike(
            &app,
            user,
            who,
            Method::POST,
            &format!("/api/series/{}/metadata/search", h.series_slug),
            None,
            &format!("/api/series/{unknown}/metadata/search"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who} search");
        if who == "removed" {
            continue;
        }
        let (status, _) = assert_alike(
            &app,
            user,
            who,
            Method::POST,
            &format!(
                "/api/series/{}/issues/{}/metadata/search",
                h.issue_series_slug, h.issue_slug
            ),
            None,
            &format!(
                "/api/series/{unknown}/issues/{}/metadata/search",
                unknown_slug()
            ),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who} issue search");
        for suffix in ["external-ids", "metadata/candidates"] {
            let (status, _) = assert_alike(
                &app,
                user,
                who,
                Method::GET,
                &format!(
                    "/api/series/{}/issues/{}/{suffix}",
                    h.issue_series_slug, h.issue_slug
                ),
                None,
                &format!("/api/series/{unknown}/issues/{}/{suffix}", unknown_slug()),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{who} issue {suffix}");
        }
    }
    // A removed issue in a live series still reads; an issue in a removed
    // series answers like an unknown one.
    let (status, _) = call(
        &app,
        &f.reader,
        Method::GET,
        &format!(
            "/api/series/{}/issues/{}/external-ids",
            f.removed.issue_series_slug, f.removed.issue_slug
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "removed issue external ids");
    for suffix in ["external-ids", "metadata/candidates"] {
        let (status, body) = assert_alike(
            &app,
            &f.reader,
            "issue in a removed series",
            Method::GET,
            &format!(
                "/api/series/{}/issues/{}/{suffix}",
                f.removed.series_slug, f.in_removed_series_slug
            ),
            None,
            &format!(
                "/api/series/{}/issues/{}/{suffix}",
                unknown_slug(),
                unknown_slug()
            ),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{suffix}");
        assert_eq!(code(&body), "issue.not_found", "{suffix}");
    }
    // The admin sees past the cap and into a removed series.
    for slug in [&f.mature.series_slug, &f.removed.series_slug] {
        let (status, _) = call(
            &app,
            &f.admin,
            Method::GET,
            &format!("/api/series/{slug}/external-ids"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    // A visible series answers 200 for the reader.
    let (status, _) = call(
        &app,
        &f.reader,
        Method::GET,
        &format!("/api/series/{}/external-ids", f.live.series_slug),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

// ───────── e. saved views over a reading list ─────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cbl_view_may_reference_only_own_or_shared_lists() {
    let (app, f) = setup().await;
    let db = app.state().db.clone();
    let theirs = CblListSeed::new(f.capped.user_id, "Their Secret List")
        .insert(&db)
        .await;
    let (status, body) = assert_alike(
        &app,
        &f.reader,
        "other user's list",
        Method::POST,
        "/api/me/saved-views",
        Some(json!({ "kind": "cbl", "name": "Mine", "cbl_list_id": theirs })),
        "/api/me/saved-views",
        Some(json!({ "kind": "cbl", "name": "Mine", "cbl_list_id": Uuid::now_v7() })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!String::from_utf8_lossy(&body).contains("Secret"));
    // The list itself answers like an unknown one.
    assert_alike(
        &app,
        &f.reader,
        "other user's list",
        Method::GET,
        &format!("/api/me/cbl-lists/{theirs}"),
        None,
        &format!("/api/me/cbl-lists/{}", Uuid::now_v7()),
        None,
    )
    .await;

    // Own list: accepted.
    let mine = CblListSeed::new(f.reader.user_id, "My List")
        .insert(&db)
        .await;
    let (status, _) = call(
        &app,
        &f.reader,
        Method::POST,
        "/api/me/saved-views",
        Some(json!({ "kind": "cbl", "name": "Mine", "cbl_list_id": mine })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_view_over_another_users_list_reads_like_a_gone_list() {
    let (app, f) = setup().await;
    let db = app.state().db.clone();
    let theirs = CblListSeed::new(f.capped.user_id, "Their Secret List")
        .insert(&db)
        .await;
    seed_cbl_entry(&db, theirs, 1, Some(&f.live.issue_id)).await;
    // A row written before creation checked the list's owner.
    let now = Utc::now().fixed_offset();
    let view_id = Uuid::now_v7();
    saved_view::ActiveModel {
        id: Set(view_id),
        user_id: Set(Some(f.reader.user_id)),
        kind: Set(entity::saved_view::SavedViewKind::Cbl),
        system_key: Set(None),
        name: Set("Borrowed".into()),
        description: Set(None),
        custom_year_start: Set(Some(1987)),
        custom_year_end: Set(Some(1993)),
        custom_tags: Set(Vec::new()),
        match_mode: Set(None),
        conditions: Set(None),
        sort_field: Set(None),
        sort_order: Set(None),
        result_limit: Set(None),
        cbl_list_id: Set(Some(theirs)),
        auto_pin: Set(false),
        preserve_canonical_order: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();

    // The view list: no year range copied from the list.
    let (status, body) = call(&app, &f.reader, Method::GET, "/api/me/saved-views", None).await;
    assert_eq!(status, StatusCode::OK);
    let body = json_of(&body);
    let view = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == view_id.to_string())
        .expect("legacy view listed")
        .clone();
    assert!(view["custom_year_start"].is_null(), "{view}");
    assert!(view["custom_year_end"].is_null(), "{view}");
    assert!(view["cbl_list_id"].is_null(), "{view}");
    let text = body.to_string();
    assert!(!text.contains("Secret"), "{text}");

    // No covers drawn from the list's items.
    let (status, body) = call(&app, &f.reader, Method::GET, "/api/me/view-covers", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !String::from_utf8_lossy(&body).contains(&view_id.to_string()),
        "{}",
        String::from_utf8_lossy(&body)
    );

    // Where the issue appears: not in the borrowed list.
    let (status, body) = call(
        &app,
        &f.reader,
        Method::GET,
        &format!(
            "/api/series/{}/issues/{}/appearances",
            f.live.issue_series_slug, f.live.issue_slug
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!String::from_utf8_lossy(&body).contains("Secret"));
    assert!(!String::from_utf8_lossy(&body).contains(&theirs.to_string()));

    // Up next through the view: answered as through a view that does not
    // exist.
    let next = |cbl: Uuid| {
        format!(
            "/api/series/{}/issues/{}/next?cbl={cbl}",
            f.live.issue_series_slug, f.live.issue_slug
        )
    };
    assert_alike(
        &app,
        &f.reader,
        "up next through a legacy view",
        Method::GET,
        &next(view_id),
        None,
        &next(Uuid::now_v7()),
        None,
    )
    .await;

    // The list itself answers like an unknown id.
    assert_alike(
        &app,
        &f.reader,
        "legacy view list",
        Method::GET,
        &format!("/api/me/cbl-lists/{theirs}/entries"),
        None,
        &format!("/api/me/cbl-lists/{}/entries", Uuid::now_v7()),
        None,
    )
    .await;

    // The list's owner still sees it whole.
    let (status, body) = call(
        &app,
        &f.capped,
        Method::GET,
        &format!("/api/me/cbl-lists/{theirs}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains("Secret"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_users_view_and_collection_answer_like_unknown() {
    let (app, f) = setup().await;
    let (status, body) = call(
        &app,
        &f.reader,
        Method::POST,
        "/api/me/collections",
        Some(json!({ "name": "Private" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let coll = json_of(&body)["id"].as_str().unwrap().to_owned();
    for (method, suffix, body) in [
        (Method::PATCH, "", Some(json!({ "name": "Mine now" }))),
        (Method::DELETE, "", None),
        (Method::GET, "/entries", None),
    ] {
        assert_alike(
            &app,
            &f.capped,
            "other user's collection",
            method,
            &format!("/api/me/collections/{coll}{suffix}"),
            body.clone(),
            &format!("/api/me/collections/{}{suffix}", Uuid::now_v7()),
            body,
        )
        .await;
    }
    assert_alike(
        &app,
        &f.capped,
        "other user's view",
        Method::PATCH,
        &format!("/api/me/saved-views/{coll}"),
        Some(json!({ "name": "x" })),
        &format!("/api/me/saved-views/{}", Uuid::now_v7()),
        Some(json!({ "name": "x" })),
    )
    .await;
    assert_alike(
        &app,
        &f.capped,
        "other user's view",
        Method::DELETE,
        &format!("/api/me/saved-views/{coll}"),
        None,
        &format!("/api/me/saved-views/{}", Uuid::now_v7()),
        None,
    )
    .await;
}

// ───────── f. writes on hidden and soft-removed content ─────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn personal_writes_answer_hidden_and_removed_like_unknown() {
    let (app, f) = setup().await;
    for (who, user, h) in cases(&f) {
        let unknown = unknown_slug();
        let (status, _) = assert_alike(
            &app,
            user,
            who,
            Method::PUT,
            &format!("/api/series/{}/rating", h.series_slug),
            Some(json!({ "rating": 4.0 })),
            &format!("/api/series/{unknown}/rating"),
            Some(json!({ "rating": 4.0 })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who} series rating");
        for suffix in ["rating", "note"] {
            let body = if suffix == "rating" {
                json!({ "rating": 4.0 })
            } else {
                json!({ "body": "a note" })
            };
            let (status, _) = assert_alike(
                &app,
                user,
                who,
                Method::PUT,
                &format!(
                    "/api/series/{}/issues/{}/{suffix}",
                    h.issue_series_slug, h.issue_slug
                ),
                Some(body.clone()),
                &format!("/api/series/{unknown}/issues/{}/{suffix}", unknown_slug()),
                Some(body),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{who} issue {suffix}");
        }

        // Collections (Want to Read is one): the single add and the bulk add.
        let (status, body) = call(
            &app,
            user,
            Method::POST,
            "/api/me/collections",
            Some(json!({ "name": format!("C {who}") })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{who}");
        let coll = json_of(&body)["id"].as_str().unwrap().to_owned();
        for (kind, hidden, unknown_ref) in [
            ("issue", h.issue_id.clone(), unknown_issue_id()),
            (
                "series",
                h.series_id.to_string(),
                Uuid::now_v7().to_string(),
            ),
        ] {
            assert_alike(
                &app,
                user,
                who,
                Method::POST,
                &format!("/api/me/collections/{coll}/entries"),
                Some(json!({ "entry_kind": kind, "ref_id": hidden })),
                &format!("/api/me/collections/{coll}/entries"),
                Some(json!({ "entry_kind": kind, "ref_id": unknown_ref })),
            )
            .await;
            assert_alike(
                &app,
                user,
                who,
                Method::POST,
                &format!("/api/me/collections/{coll}/members/bulk-add"),
                Some(json!({ "members": [{ "entry_kind": kind, "ref_id": hidden }] })),
                &format!("/api/me/collections/{coll}/members/bulk-add"),
                Some(json!({ "members": [{ "entry_kind": kind, "ref_id": unknown_ref }] })),
            )
            .await;
            // Rail dismissals.
            assert_alike(
                &app,
                user,
                who,
                Method::POST,
                "/api/me/rail-dismissals",
                Some(json!({ "target_kind": kind, "target_id": hidden })),
                "/api/me/rail-dismissals",
                Some(json!({ "target_kind": kind, "target_id": unknown_ref })),
            )
            .await;
        }

        // Page overrides and reading sessions key on an issue id too.
        let (status, body) = assert_alike(
            &app,
            user,
            who,
            Method::PUT,
            &format!("/api/me/issues/{}/page-overrides", h.issue_id),
            Some(json!({ "shift_pairing": true })),
            &format!("/api/me/issues/{}/page-overrides", unknown_issue_id()),
            Some(json!({ "shift_pairing": true })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who} page overrides");
        assert_eq!(code(&body), "not_found", "{who}");
        // Deleting one's own overrides answers 204 for any id.
        let (status, _) = assert_alike(
            &app,
            user,
            who,
            Method::DELETE,
            &format!("/api/me/issues/{}/page-overrides", h.issue_id),
            None,
            &format!("/api/me/issues/{}/page-overrides", unknown_issue_id()),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{who} page overrides delete"
        );
        let session = |issue: &str| {
            json!({
                "client_session_id": format!("s-{who}"),
                "issue_id": issue,
                "started_at": Utc::now().to_rfc3339(),
                "active_ms": 60_000,
                "distinct_pages_read": 1,
                "page_turns": 1,
                "start_page": 0,
                "end_page": 0,
                "device": "desktop"
            })
        };
        let (status, body) = assert_alike(
            &app,
            user,
            who,
            Method::POST,
            "/api/me/reading-sessions",
            Some(session(&h.issue_id)),
            "/api/me/reading-sessions",
            Some(session(&unknown_issue_id())),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who} reading session");
        assert_eq!(code(&body), "not_found", "{who}");
    }

    // A removed issue's page overrides still read (history).
    let (status, _) = call(
        &app,
        &f.reader,
        Method::GET,
        &format!("/api/me/issues/{}/page-overrides", f.removed.issue_id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // A rating on a visible issue still works.
    let (status, _) = call(
        &app,
        &f.reader,
        Method::PUT,
        &format!(
            "/api/series/{}/issues/{}/rating",
            f.live.issue_series_slug, f.live.issue_slug
        ),
        Some(json!({ "rating": 4.0 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // The admin rates a series above any cap.
    let (status, _) = call(
        &app,
        &f.admin,
        Method::PUT,
        &format!("/api/series/{}/rating", f.mature.series_slug),
        Some(json!({ "rating": 4.0 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let _ = f.lib;
}

// ───────── protocol progress shims ─────────

/// OPDS progress, OPDS Progression, KOReader sync and the Komga shim are
/// writes: hidden and removed content answer like an unknown id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protocol_progress_answers_hidden_like_unknown() {
    let (app, f) = setup().await;
    // The Komga shim answers only once its compat mode is on.
    let (status, body) = call(
        &app,
        &f.admin,
        Method::PATCH,
        "/api/admin/settings",
        Some(json!({ "compat.opds_panels_mode": "komga" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    for (who, user, h) in cases(&f) {
        let routes: [(Method, String, String, Value); 4] = [
            (
                Method::PUT,
                format!("/opds/v1/issues/{}/progress", h.issue_id),
                "/opds/v1/issues/{}/progress".to_owned(),
                json!({ "page": 0 }),
            ),
            (
                Method::PUT,
                format!("/opds/v1/progression/{}", h.issue_id),
                "/opds/v1/progression/{}".to_owned(),
                json!({
                    "modified": "2026-05-19T12:00:00Z",
                    "device": { "id": "d-1", "name": "Reader" },
                    "progression": 0.5
                }),
            ),
            (
                Method::PUT,
                format!("/opds/v1/syncs/progress/{}", h.issue_id),
                "/opds/v1/syncs/progress/{}".to_owned(),
                json!({ "progress": "p", "percentage": 0.5, "device": "KOReader", "device_id": "k-1" }),
            ),
            (
                Method::PATCH,
                format!("/api/v1/books/{}/read-progress", h.issue_id),
                "/api/v1/books/{}/read-progress".to_owned(),
                json!({ "page": 1 }),
            ),
        ];
        for (method, hidden, template, body) in routes {
            let unknown = template.replace("{}", &unknown_issue_id());
            let (status, _) = assert_alike(
                &app,
                user,
                who,
                method,
                &hidden,
                Some(body.clone()),
                &unknown,
                Some(body),
            )
            .await;
            assert!(!status.is_success(), "{who}: {hidden} answered {status}");
        }
    }
    // A removed issue's Komga book still reads; only the write is refused.
    let (status, _) = call(
        &app,
        &f.reader,
        Method::GET,
        &format!("/api/v1/books/{}", f.removed.issue_id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "removed Komga book read");
}

// ───────── filter options, batches, other users' views and lists ─────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn filter_options_answer_a_hidden_library_like_unknown() {
    let (app, f) = setup().await;
    for picker in ["genres", "publishers", "characters"] {
        let (status, body) = assert_alike(
            &app,
            &f.nogrant,
            "no grant",
            Method::GET,
            &format!("/api/filter-options/{picker}?library={}", f.lib),
            None,
            &format!("/api/filter-options/{picker}?library={}", Uuid::now_v7()),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{picker}");
        assert_eq!(code(&body), "not_found", "{picker}");
    }
    let (status, _) = call(
        &app,
        &f.reader,
        Method::GET,
        &format!("/api/filter-options/genres?library={}", f.lib),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// A metadata batch is readable by admins and its creator only: a library
/// grant does not open someone else's batch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_batch_is_admin_and_creator_only() {
    let (app, f) = setup().await;
    let now = Utc::now().fixed_offset();
    let batch = Uuid::now_v7();
    entity::metadata_batch::ActiveModel {
        id: Set(batch),
        library_id: Set(Some(f.lib)),
        scope: Set("series".into()),
        trigger_kind: Set("manual".into()),
        status: Set("running".into()),
        items_total: Set(0),
        created_by: Set(Some(f.admin.user_id)),
        created_at: Set(now),
        ended_at: Set(None),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
    for (who, user) in [
        ("reader", &f.reader),
        ("capped", &f.capped),
        ("no grant", &f.nogrant),
    ] {
        let (status, body) = assert_alike(
            &app,
            user,
            who,
            Method::GET,
            &format!("/api/metadata/batch/{batch}"),
            None,
            &format!("/api/metadata/batch/{}", Uuid::now_v7()),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who}");
        assert_eq!(code(&body), "not_found", "{who}");
    }
    let (status, _) = call(
        &app,
        &f.admin,
        Method::GET,
        &format!("/api/metadata/batch/{batch}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_users_view_sub_routes_answer_like_unknown() {
    let (app, f) = setup().await;
    let (status, body) = call(
        &app,
        &f.reader,
        Method::POST,
        "/api/me/collections",
        Some(json!({ "name": "Private" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let view = json_of(&body)["id"].as_str().unwrap().to_owned();
    let other = Uuid::now_v7().to_string();
    for (method, suffix, body) in [
        (Method::POST, "/pin", Some(json!({}))),
        (Method::POST, "/sidebar?show=true", None),
        (Method::POST, "/icon", Some(json!({ "icon": "star" }))),
        (Method::GET, "/results", None),
        (Method::GET, "/issue-results", None),
    ] {
        let (status, resp) = assert_alike(
            &app,
            &f.capped,
            "other user's view",
            method,
            &format!("/api/me/saved-views/{view}{suffix}"),
            body.clone(),
            &format!("/api/me/saved-views/{other}{suffix}"),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{suffix}");
        assert_eq!(code(&resp), "not_found", "{suffix}");
    }
}

/// Managing a reading list: another user's private list answers like an
/// unknown id (404); a shared list is visible, so a reader's write is 403.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reading_list_manage_routes_hide_private_lists() {
    let (app, f) = setup().await;
    let db = app.state().db.clone();
    let private = CblListSeed::new(f.capped.user_id, "Private List")
        .insert(&db)
        .await;
    let shared = CblListSeed::new(f.admin.user_id, "Shared List")
        .insert(&db)
        .await;
    entity::cbl_list::ActiveModel {
        id: Unchanged(shared),
        owner_user_id: Set(None),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();
    for (method, suffix, body) in [
        (Method::PATCH, "", Some(json!({ "name": "Renamed" }))),
        (Method::DELETE, "", None),
        (Method::POST, "/refresh", None),
    ] {
        let (status, resp) = assert_alike(
            &app,
            &f.reader,
            "private list",
            method.clone(),
            &format!("/api/me/cbl-lists/{private}{suffix}"),
            body.clone(),
            &format!("/api/me/cbl-lists/{}{suffix}", Uuid::now_v7()),
            body.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {suffix}");
        assert_eq!(code(&resp), "not_found", "{method} {suffix}");
        let (status, resp) = call(
            &app,
            &f.reader,
            method.clone(),
            &format!("/api/me/cbl-lists/{shared}{suffix}"),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {suffix} shared");
        assert_eq!(code(&resp), "forbidden", "{method} {suffix} shared");
    }
}

// ───────── series sub-routes for a removed series ─────────

/// Every series-slug read answers a series hidden by access, and a
/// soft-removed series for a non-admin, like an unknown slug; the admin
/// still opens the removed series.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_sub_routes_answer_hidden_and_removed_series_like_unknown() {
    let (app, f) = setup().await;
    let suffixes = [
        "",
        "/collection",
        "/issues",
        "/resume",
        "/similar",
        "/relationships",
        "/appearances",
        "/same-universe",
        "/metadata/overview",
    ];
    let callers = [
        ("no grant", &f.nogrant, &f.live.series_slug),
        ("capped", &f.capped, &f.mature.series_slug),
        ("removed", &f.reader, &f.removed.series_slug),
    ];
    for (who, user, slug) in callers {
        let unknown = unknown_slug();
        for suffix in suffixes {
            let (status, body) = assert_alike(
                &app,
                user,
                who,
                Method::GET,
                &format!("/api/series/{slug}{suffix}"),
                None,
                &format!("/api/series/{unknown}{suffix}"),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{who} {suffix}");
            assert_eq!(code(&body), "not_found", "{who} {suffix}");
        }
    }
    for suffix in suffixes {
        let (status, body) = call(
            &app,
            &f.admin,
            Method::GET,
            &format!("/api/series/{}{suffix}", f.removed.series_slug),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "admin {suffix}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    // The reader still opens a live series that has a removed issue.
    let (status, _) = call(
        &app,
        &f.reader,
        Method::GET,
        &format!("/api/series/{}/issues", f.live.series_slug),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
