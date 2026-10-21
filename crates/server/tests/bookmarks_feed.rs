//! Integration tests for `crates/server/src/api/bookmarks_feed.rs` — the
//! unified `/me/bookmarks` feed (markers + issue-level closing notes),
//! its sort orders and exact cursor, the per-kind counts, the series
//! list, and the closing-note section of the notes export.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use common::TestApp;
use common::seed::{seed_issue, seed_library, seed_series};
use sea_orm::Database;
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
}

impl Authed {
    fn cookies(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
}

fn extract_cookie(resp: &Response<Body>, name: &str) -> String {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|s| {
            let prefix = format!("{name}=");
            s.split(';')
                .next()
                .and_then(|kv| kv.strip_prefix(&prefix))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| panic!("expected cookie {name}"))
}

async fn body_text(b: Body) -> String {
    String::from_utf8(to_bytes(b, usize::MAX).await.unwrap().to_vec()).unwrap()
}

async fn body_json(b: Body) -> serde_json::Value {
    serde_json::from_str(&body_text(b).await).unwrap()
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
    let session = extract_cookie(&resp, "__Host-comic_session");
    let csrf = extract_cookie(&resp, "__Host-comic_csrf");
    let json: serde_json::Value = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session,
        csrf,
        user_id,
    }
}

async fn send(
    app: &TestApp,
    auth: &Authed,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> Response<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, auth.cookies())
        .header("X-CSRF-Token", &auth.csrf);
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    app.router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap()
}

async fn get_json(app: &TestApp, auth: &Authed, uri: &str) -> serde_json::Value {
    let resp = send(app, auth, Method::GET, uri, None).await;
    assert_eq!(resp.status(), StatusCode::OK, "GET {uri}");
    body_json(resp.into_body()).await
}

struct Seeded {
    auth: Authed,
    /// (series_slug, issue_slug, issue_id) for Saga #1, Saga #2, Alpha #1.
    saga1: (String, String, String),
    saga2: (String, String, String),
    alpha1: (String, String, String),
}

/// Two series ("Saga" with #1, #2; "Alpha Flight" with #1) in one library
/// the registered user can read.
async fn seed(app: &TestApp, tmp: &std::path::Path) -> Seeded {
    use entity::{issue, library_user_access, series};
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};

    let auth = register(app, "feed@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = seed_library(&db, tmp).await;
    let saga = seed_series(&db, lib_id, "Saga").await;
    let alpha = seed_series(&db, lib_id, "Alpha Flight").await;
    let s1 = seed_issue(&db, lib_id, saga, &tmp.join("s1.cbz"), b"saga-1", 1.0).await;
    let s2 = seed_issue(&db, lib_id, saga, &tmp.join("s2.cbz"), b"saga-2", 2.0).await;
    let a1 = seed_issue(&db, lib_id, alpha, &tmp.join("a1.cbz"), b"alpha-1", 1.0).await;
    let now = chrono::Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(auth.user_id),
        library_id: Set(lib_id),
        age_rating_max: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    let slugs = |series_id: Uuid, issue_id: String| {
        let db = db.clone();
        async move {
            let s = series::Entity::find_by_id(series_id)
                .one(&db)
                .await
                .unwrap()
                .unwrap();
            let i = issue::Entity::find_by_id(issue_id.clone())
                .one(&db)
                .await
                .unwrap()
                .unwrap();
            (s.slug, i.slug, issue_id)
        }
    };
    Seeded {
        saga1: slugs(saga, s1).await,
        saga2: slugs(saga, s2).await,
        alpha1: slugs(alpha, a1).await,
        auth,
    }
}

async fn add_marker(app: &TestApp, auth: &Authed, body: serde_json::Value) -> String {
    let resp = send(
        app,
        auth,
        Method::POST,
        "/api/me/markers",
        Some(body.clone()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "create {body}");
    body_json(resp.into_body()).await["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn set_note(app: &TestApp, auth: &Authed, slugs: &(String, String, String), body: &str) {
    let uri = format!("/api/series/{}/issues/{}/note", slugs.0, slugs.1);
    let resp = send(
        app,
        auth,
        Method::PUT,
        &uri,
        Some(serde_json::json!({ "body": body })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

async fn set_rating(app: &TestApp, auth: &Authed, slugs: &(String, String, String), r: f64) {
    let uri = format!("/api/series/{}/issues/{}/rating", slugs.0, slugs.1);
    let resp = send(
        app,
        auth,
        Method::PUT,
        &uri,
        Some(serde_json::json!({ "rating": r })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

/// `(type, label)` per entry: `("marker", "<kind>@<page>")` or
/// `("closing_note", "<issue_number>")`.
fn shape(items: &serde_json::Value) -> Vec<(String, String)> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let t = e["type"].as_str().unwrap().to_owned();
            let label = if t == "marker" {
                format!(
                    "{}@{}",
                    e["marker"]["kind"].as_str().unwrap(),
                    e["marker"]["page_index"]
                )
            } else {
                e["note"]["issue_number"].as_str().unwrap_or("?").to_owned()
            };
            (t, label)
        })
        .collect()
}

/// Walk every page of `uri` (which must not already carry `cursor`).
async fn walk(app: &TestApp, auth: &Authed, uri: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..20 {
        let full = match &cursor {
            Some(c) => format!("{uri}&cursor={c}"),
            None => uri.to_owned(),
        };
        let page = get_json(app, auth, &full).await;
        out.extend(shape(&page["items"]));
        match page["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => return out,
        }
    }
    panic!("cursor never terminated");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn feed_unions_markers_and_closing_notes_in_every_sort() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let s = seed(&app, tmp.path()).await;
    let a = &s.auth;

    // Markers: Saga #1 pages 3 and 1, Saga #2 page 5, Alpha #1 page 2.
    add_marker(&app, a, serde_json::json!({"issue_id": s.saga1.2, "page_index": 3, "kind": "note", "body": "Alana's line lands", "tags": ["plot"]})).await;
    add_marker(
        &app,
        a,
        serde_json::json!({"issue_id": s.saga1.2, "page_index": 1, "kind": "bookmark"}),
    )
    .await;
    add_marker(&app, a, serde_json::json!({"issue_id": s.saga2.2, "page_index": 5, "kind": "highlight", "is_favorite": true, "region": {"x": 10.0, "y": 10.0, "w": 40.0, "h": 20.0}})).await;
    add_marker(
        &app,
        a,
        serde_json::json!({"issue_id": s.alpha1.2, "page_index": 2, "kind": "favorite"}),
    )
    .await;
    // Closing notes on Saga #2 (also rated) and Alpha #1.
    set_rating(&app, a, &s.saga2, 4.0).await;
    set_note(&app, a, &s.saga2, "The arc closer.").await;
    set_note(&app, a, &s.alpha1, "Slow start, great art.").await;

    // Newest first: the last write (Alpha note) leads; every entry present.
    let newest = get_json(&app, a, "/api/me/bookmarks").await;
    let items = shape(&newest["items"]);
    assert_eq!(items.len(), 6);
    assert_eq!(items[0], ("closing_note".into(), "1".into()));
    assert_eq!(items[1], ("closing_note".into(), "2".into()));
    assert!(newest["next_cursor"].is_null());
    // The note entry carries its identity + rating.
    let note = newest["items"][1]["note"].clone();
    assert_eq!(note["series_name"].as_str(), Some("Saga"));
    assert_eq!(note["rating"].as_f64(), Some(4.0));
    assert_eq!(note["body"].as_str(), Some("The arc closer."));
    assert!(
        note["cover_url"]
            .as_str()
            .unwrap()
            .ends_with("/pages/0/thumb")
    );

    // Oldest first is the exact reverse.
    let oldest = shape(&get_json(&app, a, "/api/me/bookmarks?sort=oldest").await["items"]);
    let mut rev = items.clone();
    rev.reverse();
    assert_eq!(oldest, rev);

    // Reading order: series by name, issue by number, closing note before
    // page 0, then pages ascending.
    let reading = walk(&app, a, "/api/me/bookmarks?sort=reading&limit=2").await;
    assert_eq!(
        reading,
        vec![
            ("closing_note".to_owned(), "1".to_owned()), // Alpha Flight #1
            ("marker".to_owned(), "favorite@2".to_owned()),
            ("marker".to_owned(), "bookmark@1".to_owned()), // Saga #1
            ("marker".to_owned(), "note@3".to_owned()),
            ("closing_note".to_owned(), "2".to_owned()), // Saga #2
            ("marker".to_owned(), "highlight@5".to_owned()),
        ]
    );
    // A tiny page size through the newest sort walks every entry once.
    let paged = walk(&app, a, "/api/me/bookmarks?sort=newest&limit=1").await;
    assert_eq!(paged, items);

    // Bad cursor is a 400, not a crash.
    let resp = send(&app, a, Method::GET, "/api/me/bookmarks?cursor=zzz", None).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn feed_filters_counts_and_series() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let s = seed(&app, tmp.path()).await;
    let a = &s.auth;
    add_marker(&app, a, serde_json::json!({"issue_id": s.saga1.2, "page_index": 3, "kind": "note", "body": "Alana's line lands", "tags": ["plot"]})).await;
    add_marker(&app, a, serde_json::json!({"issue_id": s.saga1.2, "page_index": 1, "kind": "bookmark", "is_favorite": true})).await;
    add_marker(
        &app,
        a,
        serde_json::json!({"issue_id": s.alpha1.2, "page_index": 2, "kind": "favorite"}),
    )
    .await;
    set_note(&app, a, &s.saga2, "The arc closer — Alana again.").await;
    set_note(&app, a, &s.alpha1, "Slow start.").await;

    // kind=closing_note → notes only; kind=note → page notes only.
    let only_notes =
        shape(&get_json(&app, a, "/api/me/bookmarks?kind=closing_note").await["items"]);
    assert_eq!(only_notes.len(), 2);
    assert!(only_notes.iter().all(|(t, _)| t == "closing_note"));
    let page_notes = shape(&get_json(&app, a, "/api/me/bookmarks?kind=note").await["items"]);
    assert_eq!(page_notes, vec![("marker".to_owned(), "note@3".to_owned())]);

    // Favorites: the legacy flag and the standalone kind, never notes.
    let favs = shape(&get_json(&app, a, "/api/me/bookmarks?is_favorite=true").await["items"]);
    assert_eq!(favs.len(), 2);
    assert!(favs.iter().all(|(t, _)| t == "marker"));

    // Tags exclude notes; the series filter and search span both.
    let tagged = shape(&get_json(&app, a, "/api/me/bookmarks?tags=plot").await["items"]);
    assert_eq!(tagged.len(), 1);
    let saga_uri = format!(
        "/api/me/bookmarks?series_id={}",
        series_id_of(&app, &s.saga1.2).await
    );
    let saga = shape(&get_json(&app, a, &saga_uri).await["items"]);
    assert_eq!(saga.len(), 3);
    let alana = shape(&get_json(&app, a, "/api/me/bookmarks?q=alana").await["items"]);
    assert_eq!(
        alana.len(),
        2,
        "search matches a page note and a closing note"
    );
    assert!(alana.iter().any(|(t, _)| t == "closing_note"));

    // Counts: per kind under the other filters, plus the unfiltered total.
    let counts = get_json(&app, a, "/api/me/bookmarks/counts").await;
    assert_eq!(counts["all"].as_u64(), Some(5));
    assert_eq!(counts["total"].as_u64(), Some(5));
    assert_eq!(counts["by_kind"]["note"].as_u64(), Some(1));
    assert_eq!(counts["by_kind"]["bookmark"].as_u64(), Some(1));
    assert_eq!(counts["by_kind"]["closing_note"].as_u64(), Some(2));
    // Favorites = standalone kind (1) + legacy-flagged bookmark (1).
    assert_eq!(counts["by_kind"]["favorite"].as_u64(), Some(2));
    let counts_q = get_json(&app, a, "/api/me/bookmarks/counts?q=alana").await;
    assert_eq!(counts_q["total"].as_u64(), Some(2));
    assert_eq!(counts_q["all"].as_u64(), Some(5));
    assert_eq!(counts_q["by_kind"]["closing_note"].as_u64(), Some(1));

    // Series list: both series, name order, with entry counts.
    let series = get_json(&app, a, "/api/me/bookmarks/series").await;
    let names: Vec<(String, u64)> = series["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["name"].as_str().unwrap().to_owned(),
                r["count"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        names,
        vec![("Alpha Flight".to_owned(), 2), ("Saga".to_owned(), 3)]
    );

    // Another user sees nothing of this.
    let other = register(&app, "other@example.com").await;
    let theirs = get_json(&app, &other, "/api/me/bookmarks").await;
    assert_eq!(theirs["items"].as_array().unwrap().len(), 0);
}

async fn series_id_of(app: &TestApp, issue_id: &str) -> String {
    use sea_orm::EntityTrait;
    let db = Database::connect(&app.db_url).await.unwrap();
    entity::issue::Entity::find_by_id(issue_id.to_owned())
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .series_id
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notes_export_carries_closing_notes() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let s = seed(&app, tmp.path()).await;
    let a = &s.auth;
    add_marker(&app, a, serde_json::json!({"issue_id": s.saga1.2, "page_index": 3, "kind": "note", "body": "Alana's line lands"})).await;
    set_rating(&app, a, &s.saga2, 4.5).await;
    // Saga #2 has a closing note and no markers: it must still appear.
    set_note(&app, a, &s.saga2, "The arc closer.").await;

    let json = get_json(&app, a, "/api/me/markers/export?format=json").await;
    assert_eq!(json["total"].as_u64(), Some(1));
    assert_eq!(json["closing_notes"].as_u64(), Some(1));
    let saga = json["series"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["series"]["series_name"] == "Saga")
        .expect("Saga group");
    let issues = saga["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 2);
    assert!(issues[0]["closing_note"].is_null());
    assert_eq!(
        issues[1]["closing_note"]["body"].as_str(),
        Some("The arc closer.")
    );
    assert_eq!(issues[1]["closing_note"]["rating"].as_f64(), Some(4.5));
    assert_eq!(issues[1]["pages"].as_array().unwrap().len(), 0);

    let resp = send(
        &app,
        a,
        Method::GET,
        "/api/me/markers/export?format=md",
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let md = body_text(resp.into_body()).await;
    assert!(md.contains("· 1 marker · 1 closing note"), "{md}");
    assert!(
        md.contains("**Closing note** · ★★★★½ (4.5/5)\n\nThe arc closer.\n"),
        "{md}"
    );
}
