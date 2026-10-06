//! WP-7.4 — explainable "similar series".
//!
//! `GET /series/{slug}/similar` + `GET /me/similar-series`: ranking by
//! shared-entity overlap, the "because" list matches the overlap, library
//! ACL + age cap, hidden (rail-dismissed) series, keyset paging, and the
//! neighbour cache being dropped by scans, metadata applies and issue
//! edits.
//!
//! Fixtures write the series-level junctions directly (the scanner's
//! rollup output) — the similarity read path is what's under test; the
//! scan test at the bottom drives the real scanner end-to-end.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{LibrarySeed, SeriesSeed, seed_issue, seed_library, seed_progress};
use entity::{library_user_access, user::Entity as UserEntity};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    Set, Statement, Unchanged,
};
use serde_json::Value;
use std::io::Write;
use std::path::Path;
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
}

impl Authed {
    fn cookie(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
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

async fn send(
    app: &TestApp,
    method: Method,
    uri: &str,
    user: &Authed,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, user.cookie())
        .header("X-CSRF-Token", user.csrf.clone());
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
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v)
}

async fn get(app: &TestApp, uri: &str, user: &Authed) -> (StatusCode, Value) {
    send(app, Method::GET, uri, user, None).await
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

async fn credit(db: &DatabaseConnection, series: Uuid, role: &str, person: &str) {
    exec(
        db,
        "INSERT INTO series_credits (series_id, role, person) VALUES ($1, $2, $3)",
        vec![series.into(), role.into(), person.into()],
    )
    .await;
}

async fn character(db: &DatabaseConnection, series: Uuid, name: &str) {
    exec(
        db,
        r#"INSERT INTO series_characters (series_id, "character") VALUES ($1, $2)"#,
        vec![series.into(), name.into()],
    )
    .await;
}

async fn genre(db: &DatabaseConnection, series: Uuid, name: &str) {
    exec(
        db,
        "INSERT INTO series_genres (series_id, genre) VALUES ($1, $2)",
        vec![series.into(), name.into()],
    )
    .await;
}

async fn arc(db: &DatabaseConnection, series: Uuid, name: &str) {
    exec(
        db,
        "INSERT INTO story_arc (id, slug, name, normalized_name) \
         VALUES ($1, $2, $3, $4) ON CONFLICT (normalized_name) DO NOTHING",
        vec![
            Uuid::now_v7().into(),
            name.to_lowercase().replace(' ', "-").into(),
            name.into(),
            name.to_lowercase().into(),
        ],
    )
    .await;
    exec(
        db,
        "INSERT INTO series_arcs (series_id, arc_id) \
         SELECT $1, id FROM story_arc WHERE normalized_name = $2",
        vec![series.into(), name.to_lowercase().into()],
    )
    .await;
}

async fn series(db: &DatabaseConnection, lib: Uuid, name: &str) -> Uuid {
    SeriesSeed::new(lib, name).insert(db).await
}

/// `n` unrelated series so the IDF has a population to normalise over
/// (each carries the ubiquitous genre "Superhero").
async fn filler(db: &DatabaseConnection, lib: Uuid, n: usize) {
    for i in 0..n {
        let s = series(db, lib, &format!("Filler {i}")).await;
        genre(db, s, "Superhero").await;
        credit(db, s, "writer", &format!("Filler Writer {i}")).await;
    }
}

fn ids(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["series"]["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn ranks_by_overlap_and_because_matches_the_overlap() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    filler(&db, lib, 8).await;

    let target = series(&db, lib, "Captain America").await;
    credit(&db, target, "writer", "Ed Brubaker").await;
    credit(&db, target, "penciller", "Steve Epting").await;
    character(&db, target, "Bucky Barnes").await;
    character(&db, target, "Steve Rogers").await;
    arc(&db, target, "Winter Soldier").await;
    genre(&db, target, "Superhero").await;

    // Strong: same writer + a shared character + the arc.
    let strong = series(&db, lib, "Winter Soldier").await;
    credit(&db, strong, "writer", "Ed Brubaker").await;
    credit(&db, strong, "penciller", "Butch Guice").await;
    character(&db, strong, "bucky barnes ").await; // name normalisation
    arc(&db, strong, "Winter Soldier").await;
    genre(&db, strong, "Superhero").await;

    // Medium: same writer only.
    let medium = series(&db, lib, "Criminal").await;
    credit(&db, medium, "writer", "Ed Brubaker").await;

    // Noise: only the ubiquitous genre — below MIN_SCORE.
    let noise = series(&db, lib, "Generic Hero").await;
    genre(&db, noise, "Superhero").await;

    let (status, body) = get(&app, &format!("/api/series/{target}/similar"), &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        ids(&body),
        vec![strong.to_string(), medium.to_string()],
        "{body}"
    );
    assert_eq!(body["total"], 2);
    assert!(body["next_cursor"].is_null());

    let items = body["items"].as_array().unwrap();
    assert!(items[0]["score"].as_f64().unwrap() > items[1]["score"].as_f64().unwrap());

    // The because list is exactly the shared entities (strongest first),
    // never something the two series don't share.
    let because: Vec<(String, Option<String>, String)> = items[0]["because"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["kind"].as_str().unwrap().to_owned(),
                r["role"].as_str().map(str::to_owned),
                r["name"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let mut sorted = because.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec![
            ("arc".into(), None, "Winter Soldier".into()),
            ("character".into(), None, "Bucky Barnes".into()),
            (
                "creator".into(),
                Some("writer".into()),
                "Ed Brubaker".into()
            ),
            // "Superhero" is shared too, but by most of the library — a
            // stop-entity that never explains a match.
        ],
        "{body}"
    );
    let weights: Vec<f64> = items[0]["because"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["weight"].as_f64().unwrap())
        .collect();
    assert!(weights.windows(2).all(|w| w[0] >= w[1]), "{weights:?}");

    let medium_because = items[1]["because"].as_array().unwrap();
    assert_eq!(medium_because.len(), 1);
    assert_eq!(medium_because[0]["kind"], "creator");
    assert_eq!(medium_because[0]["role"], "writer");
    assert_eq!(medium_because[0]["name"], "Ed Brubaker");

    // Hydrated as a normal series card.
    assert_eq!(items[0]["series"]["name"], "Winter Soldier");

    // Unknown series → 404.
    let (status, _) = get(&app, "/api/series/no-such-series/similar", &admin).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn respects_library_acl_and_age_cap() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let user = register(&app, "reader@example.com").await;
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let root_a = tmp.path().join("a");
    let root_b = tmp.path().join("b");
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();
    let lib_a = seed_library(&db, &root_a).await;
    let lib_b = seed_library(&db, &root_b).await;
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        library_id: Set(lib_a),
        user_id: Set(user.user_id),
        age_rating_max: Set(Some("Teen".into())),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    entity::user::ActiveModel {
        id: Unchanged(user.user_id),
        role: Set("user".into()),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();
    filler(&db, lib_a, 6).await;

    let target = series(&db, lib_a, "Target").await;
    let visible = series(&db, lib_a, "Visible").await;
    let other_lib = series(&db, lib_b, "Other Library").await;
    let mature = series(&db, lib_a, "Mature").await;
    exec(
        &db,
        "UPDATE series SET age_rating = 'Mature 17+' WHERE id = $1",
        vec![mature.into()],
    )
    .await;
    for s in [target, visible, other_lib, mature] {
        credit(&db, s, "writer", "Shared Writer").await;
    }

    // Admin sees all three neighbours.
    let (status, body) = get(&app, &format!("/api/series/{target}/similar"), &admin).await;
    assert_eq!(status, StatusCode::OK);
    let mut all = ids(&body);
    all.sort();
    let mut expected = vec![
        visible.to_string(),
        other_lib.to_string(),
        mature.to_string(),
    ];
    expected.sort();
    assert_eq!(all, expected);

    // The restricted reader sees only the granted, under-cap one — from
    // the same cached list (the cache is unfiltered).
    let (status, body) = get(&app, &format!("/api/series/{target}/similar"), &user).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&body), vec![visible.to_string()], "{body}");
    assert_eq!(body["total"], 1);

    // A series in an ungranted library is a 404, not an empty list.
    let (status, _) = get(&app, &format!("/api/series/{other_lib}/similar"), &user).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Same for one above the cap.
    let (status, _) = get(&app, &format!("/api/series/{mature}/similar"), &user).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn hidden_and_removed_series_are_excluded() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    filler(&db, lib, 6).await;
    let target = series(&db, lib, "Target").await;
    let keep = series(&db, lib, "Keep").await;
    let hide = series(&db, lib, "Hide").await;
    let removed = series(&db, lib, "Removed").await;
    exec(
        &db,
        "UPDATE series SET removed_at = now() WHERE id = $1",
        vec![removed.into()],
    )
    .await;
    for s in [target, keep, hide, removed] {
        credit(&db, s, "writer", "Shared Writer").await;
    }

    // Hide via the real dismissal endpoint.
    let (status, body) = send(
        &app,
        Method::POST,
        "/api/me/rail-dismissals",
        &admin,
        Some(serde_json::json!({"target_kind": "series", "target_id": hide.to_string()})),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");

    let (status, body) = get(&app, &format!("/api/series/{target}/similar"), &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&body), vec![keep.to_string()], "{body}");

    // Un-hiding brings it back (per-request filter, cache untouched).
    let (status, _) = send(
        &app,
        Method::DELETE,
        &format!("/api/me/rail-dismissals/series/{hide}"),
        &admin,
        None,
    )
    .await;
    assert!(status.is_success());
    let (_, body) = get(&app, &format!("/api/series/{target}/similar"), &admin).await;
    let mut got = ids(&body);
    got.sort();
    let mut want = vec![keep.to_string(), hide.to_string()];
    want.sort();
    assert_eq!(got, want);
}

#[tokio::test]
async fn cursor_paging_walks_every_neighbour_once() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    filler(&db, lib, 30).await;
    let target = series(&db, lib, "Target").await;
    credit(&db, target, "writer", "Writer A").await;
    for i in 0..5 {
        character(&db, target, &format!("Hero {i}")).await;
    }
    // 25 neighbours, each sharing one rare character with the target
    // (so every one clears MIN_SCORE) plus the writer and 0–4 common
    // characters — scores tie in groups, so the keyset must break ties
    // by id.
    let mut neighbours = Vec::new();
    for i in 0..25 {
        character(&db, target, &format!("Link {i}")).await;
        let s = series(&db, lib, &format!("Neighbour {i}")).await;
        character(&db, s, &format!("Link {i}")).await;
        credit(&db, s, "writer", "Writer A").await;
        for c in 0..(i % 5) {
            character(&db, s, &format!("Hero {c}")).await;
        }
        neighbours.push(s.to_string());
    }

    let mut seen: Vec<String> = Vec::new();
    let mut scores: Vec<f64> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let uri = match &cursor {
            Some(c) => format!("/api/series/{target}/similar?limit=10&cursor={c}"),
            None => format!("/api/series/{target}/similar?limit=10"),
        };
        let (status, body) = get(&app, &uri, &admin).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if pages == 0 {
            assert_eq!(body["total"], 25);
        } else {
            assert!(body.get("total").is_none(), "total only on page 1");
        }
        for item in body["items"].as_array().unwrap() {
            seen.push(item["series"]["id"].as_str().unwrap().to_owned());
            scores.push(item["score"].as_f64().unwrap());
        }
        pages += 1;
        match body["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen.len(), 25);
    let mut dedup = seen.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(dedup.len(), 25, "no repeats across pages");
    let mut want = neighbours.clone();
    want.sort();
    assert_eq!(dedup, want);
    assert!(scores.windows(2).all(|w| w[0] >= w[1]), "{scores:?}");

    let (status, _) = get(
        &app,
        &format!("/api/series/{target}/similar?cursor=not-a-cursor"),
        &admin,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn home_rail_seeds_from_latest_read_and_skips_started_series() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    filler(&db, lib, 6).await;

    // Nothing read yet → empty rail, no seed.
    let (status, body) = get(&app, "/api/me/similar-series", &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["seed"].is_null());
    assert_eq!(body["items"].as_array().unwrap().len(), 0);

    let seed = series(&db, lib, "Seed").await;
    let unread = series(&db, lib, "Unread Neighbour").await;
    let started = series(&db, lib, "Started Neighbour").await;
    for s in [seed, unread, started] {
        credit(&db, s, "writer", "Shared Writer").await;
    }
    let seed_issue_id = seed_issue(
        &db,
        lib,
        seed,
        &tmp.path().join("seed-1.cbz"),
        b"seed-1",
        1.0,
    )
    .await;
    let started_issue_id = seed_issue(
        &db,
        lib,
        started,
        &tmp.path().join("started-1.cbz"),
        b"started-1",
        1.0,
    )
    .await;
    seed_progress(&db, admin.user_id, &started_issue_id, 3, 0.2, false).await;
    // The seed is the most recent read.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    seed_progress(&db, admin.user_id, &seed_issue_id, 5, 0.3, false).await;

    let (status, body) = get(&app, "/api/me/similar-series", &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["seed"]["id"], seed.to_string());
    assert_eq!(body["seed"]["name"], "Seed");
    assert_eq!(ids(&body), vec![unread.to_string()], "{body}");
    assert_eq!(body["items"][0]["because"][0]["name"], "Shared Writer");
}

// ───── cache invalidation ─────

fn write_cbz(path: &Path, comic_info: &str, marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    zw.start_file("ComicInfo.xml", opts).unwrap();
    zw.write_all(comic_info.as_bytes()).unwrap();
    zw.finish().unwrap();
}

fn add_series_folder(root: &Path, name: &str, writer: &str, marker: u32) {
    let folder = root.join(format!("{name} (2010)"));
    std::fs::create_dir_all(&folder).unwrap();
    write_cbz(
        &folder.join(format!("{name} 001.cbz")),
        &format!(
            r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>{name}</Series>
  <Number>1</Number>
  <Year>2010</Year>
  <Writer>{writer}</Writer>
  <Characters>Character {marker}</Characters>
</ComicInfo>"#
        ),
        marker,
    );
}

async fn series_id_by_name(db: &DatabaseConnection, name: &str) -> Uuid {
    entity::series::Entity::find()
        .filter(entity::series::Column::Name.eq(name))
        .one(db)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("series {name}"))
        .id
}

#[tokio::test]
async fn scan_invalidates_the_cached_neighbours() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let state = app.state();
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..6 {
        add_series_folder(
            tmp.path(),
            &format!("Filler {i}"),
            &format!("Solo {i}"),
            100 + i,
        );
    }
    add_series_folder(tmp.path(), "Alpha", "Shared Writer", 1);
    add_series_folder(tmp.path(), "Beta", "Shared Writer", 2);
    let lib = LibrarySeed::new(tmp.path()).insert(&state.db).await;
    server::library::scanner::scan_library(&state, lib)
        .await
        .unwrap();

    let alpha = series_id_by_name(&state.db, "Alpha").await;
    let beta = series_id_by_name(&state.db, "Beta").await;
    let uri = format!("/api/series/{alpha}/similar");
    let (_, body) = get(&app, &uri, &admin).await;
    assert_eq!(ids(&body), vec![beta.to_string()], "{body}");
    assert!(
        !state.similarity.is_empty(),
        "first read populates the cache"
    );

    // A write that bypasses every hook is NOT seen: the list is cached.
    // (Its own library, so the rescans below don't reconcile it away.)
    let other_root = tempfile::tempdir().unwrap();
    let other_lib = common::seed::seed_library(&state.db, other_root.path()).await;
    let sneaky = series(&state.db, other_lib, "Sneaky").await;
    credit(&state.db, sneaky, "writer", "Shared Writer").await;
    let (_, body) = get(&app, &uri, &admin).await;
    assert_eq!(ids(&body), vec![beta.to_string()], "served from cache");

    // A no-op rescan leaves the cache warm …
    server::library::scanner::scan_library(&state, lib)
        .await
        .unwrap();
    assert!(!state.similarity.is_empty(), "no-op scan keeps the cache");

    // … a scan that ingests something drops it, and the next read sees
    // both the scanned series and the earlier direct write.
    add_series_folder(tmp.path(), "Gamma", "Shared Writer", 3);
    let stats = server::library::scanner::scan_library(&state, lib)
        .await
        .unwrap();
    assert_eq!(stats.files_added, 1, "{stats:?}");
    assert!(
        state.similarity.is_empty(),
        "mutating scan clears the cache"
    );
    let gamma = series_id_by_name(&state.db, "Gamma").await;
    let (_, body) = get(&app, &uri, &admin).await;
    let mut got = ids(&body);
    got.sort();
    let mut want = vec![beta.to_string(), gamma.to_string(), sneaky.to_string()];
    want.sort();
    assert_eq!(got, want, "{body}");
}

#[tokio::test]
async fn issue_edit_and_metadata_apply_invalidate_the_cache() {
    let app = TestApp::spawn_with_comicvine("cv-test-key", true).await;
    let admin = register(&app, "admin@example.com").await;
    let state = app.state();
    let db = state.db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    filler(&db, lib, 4).await;
    let target = series(&db, lib, "Target").await;
    let other = series(&db, lib, "Other").await;
    for s in [target, other] {
        credit(&db, s, "writer", "Shared Writer").await;
    }
    let issue_id = seed_issue(&db, lib, target, &tmp.path().join("t-1.cbz"), b"t-1", 1.0).await;
    let uri = format!("/api/series/{target}/similar");

    // ── Issue PATCH (manual edit) ──
    let (status, _) = get(&app, &uri, &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!state.similarity.is_empty());
    let issue_slug = entity::issue::Entity::find_by_id(issue_id.clone())
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .slug;
    let (status, body) = send(
        &app,
        Method::PATCH,
        &format!("/api/series/{target}/issues/{issue_slug}"),
        &admin,
        Some(serde_json::json!({"title": "Edited"})),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
    assert!(state.similarity.is_empty(), "issue edit clears the cache");

    // ── Metadata apply (DB-direct path) ──
    let (status, _) = get(&app, &uri, &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!state.similarity.is_empty());
    use server::metadata::cache;
    use server::metadata::identifier::{Identifier, Source};
    let prefilled = server::metadata::provider::GenericMetadata {
        series_name: Some("Target".into()),
        publisher: Some("Image Comics".into()),
        identifiers: vec![Identifier::with_canonical_url(
            Source::ComicVine,
            "777",
            "series",
        )],
        source_provider: Some(Source::ComicVine),
        source_external_id: Some("777".into()),
        ..Default::default()
    };
    cache::put(
        &db,
        Source::ComicVine,
        cache::CacheEntity::Series,
        "777",
        &prefilled,
    )
    .await
    .unwrap();
    let run_id = seed_series_run(&db, target, "777").await;
    server::jobs::metadata_apply::apply_series_inline(
        &state,
        target,
        server::metadata::apply::ApplyArgs {
            run_id,
            ordinal: 0,
            mode: server::metadata::apply::ApplyMode::FillMissing,
            apply_cover: false,
            cover_overwrite_policy: server::metadata::writers::CoverOverwritePolicy::WhenMissing,
            override_user_edits: false,
            actor_id: None,
            selected_fields: None,
            override_external_id_sources: std::collections::HashSet::new(),
        },
    )
    .await
    .expect("apply_series");
    assert!(
        state.similarity.is_empty(),
        "metadata apply clears the cache"
    );
}

async fn seed_series_run(db: &DatabaseConnection, series_id: Uuid, cv_id: &str) -> Uuid {
    let now = Utc::now().fixed_offset();
    let run_id = Uuid::now_v7();
    entity::metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("series".into()),
        scope_entity_id: Set(Some(series_id.to_string())),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["comicvine".into()]),
        status: Set("completed".into()),
        started_at: Set(now),
        finished_at: Set(Some(now)),
        items_total: Set(1),
        items_matched_high: Set(1),
        items_matched_medium: Set(0),
        items_matched_low: Set(0),
        items_no_match: Set(0),
        items_applied: Set(0),
        items_skipped: Set(0),
        items_failed: Set(0),
        error_summary: Set(None),
        resume_after: Set(None),
        batch_id: Set(None),
        provider_status: Set(None),
        partial_results: Set(None),
        query: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    entity::metadata_run_candidate::ActiveModel {
        run_id: Set(run_id),
        ordinal: Set(0),
        source: Set("comicvine".into()),
        external_id: Set(cv_id.into()),
        bucket: Set("high".into()),
        score: Set(95.0),
        score_breakdown: Set(serde_json::json!({})),
        candidate: Set(serde_json::json!({
            "kind": "series",
            "source": "comicvine",
            "external_id": cv_id,
            "external_url": null,
            "name": "Target",
            "year": 2020,
            "publisher": "Image Comics",
            "issue_count": 1,
            "cover_image_url": null,
            "deck": null,
        })),
        applied_at: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    run_id
}

#[tokio::test]
async fn accepted_relationships_are_a_signal() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let state = app.state();
    let db = state.db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    filler(&db, lib, 4).await;
    // No shared metadata at all — only the curated link can relate them.
    let relaunch = series(&db, lib, "Daredevil 2014").await;
    let original = series(&db, lib, "Daredevil 2011").await;

    let (status, body) = get(&app, &format!("/api/series/{relaunch}/similar"), &admin).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&body), Vec::<String>::new());

    // "Daredevil 2014 is a sequel of Daredevil 2011" (WP-7.1 stores the
    // inverse edge too). Creating it drops the cached empty list.
    let (status, body) = send(
        &app,
        Method::POST,
        &format!("/api/series/{relaunch}/relationships"),
        &admin,
        Some(serde_json::json!({"target": original.to_string(), "kind": "sequel_of"})),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");

    let (_, body) = get(&app, &format!("/api/series/{relaunch}/similar"), &admin).await;
    assert_eq!(ids(&body), vec![original.to_string()], "{body}");
    let reason = &body["items"][0]["because"][0];
    assert_eq!(reason["kind"], "relationship");
    // Read from the neighbour's side: 2011 "has sequel" 2014 (WP-7.5:
    // `has_sequel` is the inverse of `sequel_of`), with the label carried.
    assert_eq!(reason["role"], "has_sequel");
    assert_eq!(reason["label"], "has sequel");
    assert_eq!(reason["name"], "Daredevil 2014 (2020)");

    let (_, body) = get(&app, &format!("/api/series/{original}/similar"), &admin).await;
    assert_eq!(ids(&body), vec![relaunch.to_string()], "{body}");
    assert_eq!(body["items"][0]["because"][0]["role"], "sequel_of");
    assert_eq!(body["items"][0]["because"][0]["label"], "sequel to");
    assert_eq!(
        body["items"][0]["because"][0]["name"],
        "Daredevil 2011 (2020)"
    );

    // Deleting the edge drops it again.
    let (_, rels) = get(
        &app,
        &format!("/api/series/{relaunch}/relationships"),
        &admin,
    )
    .await;
    let rel_id = rels["relationships"][0]["id"]
        .as_str()
        .expect("relationship id")
        .to_owned();
    let (status, _) = send(
        &app,
        Method::DELETE,
        &format!("/api/series/{relaunch}/relationships/{rel_id}"),
        &admin,
        None,
    )
    .await;
    assert!(status.is_success(), "{status}");
    let (_, body) = get(&app, &format!("/api/series/{relaunch}/similar"), &admin).await;
    assert_eq!(ids(&body), Vec::<String>::new(), "{body}");
}

/// WP-8.2: two series with accepted `tie_in_to` edges to the same story
/// arc are similar ("both tie in to …"), and the signal doesn't stack on
/// the `issue_arcs` / `series_arcs` arc signal for the same arc.
#[tokio::test]
async fn accepted_arc_tie_ins_are_a_signal_without_double_counting() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let state = app.state();
    let db = state.db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    filler(&db, lib, 8).await;
    let journal = series(&db, lib, "Secret Wars Journal").await;
    let hulk = series(&db, lib, "Planet Hulk").await;
    let gauntlet = series(&db, lib, "Infinity Gauntlet").await;
    // "Secret Wars": tagged on both series AND accepted tie-ins on both.
    arc(&db, journal, "Secret Wars").await;
    arc(&db, hulk, "Secret Wars").await;
    // "Infinity": accepted tie-ins only (no issue tagging).
    arc(&db, gauntlet, "Infinity").await;
    exec(
        &db,
        "DELETE FROM series_arcs WHERE series_id = $1",
        vec![gauntlet.into()],
    )
    .await;
    let arc_id = |name: &'static str| {
        let db = db.clone();
        async move {
            entity::story_arc::Entity::find()
                .filter(entity::story_arc::Column::NormalizedName.eq(name.to_lowercase()))
                .one(&db)
                .await
                .unwrap()
                .unwrap()
                .id
        }
    };
    let secret_wars = arc_id("Secret Wars").await;
    let infinity = arc_id("Infinity").await;
    for (s, a) in [
        (journal, secret_wars),
        (hulk, secret_wars),
        (journal, infinity),
        (gauntlet, infinity),
    ] {
        server::relationships::create_arc_edge(
            &db,
            s,
            a,
            server::relationships::RelationshipKind::TieInTo,
            server::relationships::RelationshipSource::Suggested,
            Some(0.8),
            None,
            &server::relationships::Scope::default(),
        )
        .await
        .unwrap();
    }

    let (status, body) = get(&app, &format!("/api/series/{journal}/similar"), &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().unwrap();
    let item = |id: Uuid| {
        items
            .iter()
            .find(|i| i["series"]["id"] == id.to_string())
            .unwrap_or_else(|| panic!("{id} listed: {body}"))
            .clone()
    };
    let g = item(gauntlet);
    let reason = &g["because"][0];
    assert_eq!(reason["kind"], "arc");
    assert_eq!(reason["name"], "Infinity");
    assert_eq!(reason["label"], "both tie in to", "{g}");
    // Planet Hulk shares "Secret Wars" through tagging AND tie-ins: it
    // counts once (the max), so it scores exactly what Infinity Gauntlet's
    // single tie-in arc scores (same document frequency, 2).
    let h = item(hulk);
    assert_eq!(h["because"].as_array().unwrap().len(), 1, "{h}");
    assert_eq!(h["score"], g["score"], "no double count: {h} vs {g}");
}

/// WP-8.2: promoting an external ("not in library") link to a series
/// relationship drops the similar-series cache — from the external-ids
/// endpoint that matched the target, and from the suggestion job's
/// promotion pass.
#[tokio::test]
async fn promoting_an_external_link_invalidates_the_cache() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let state = app.state();
    let db = state.db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    filler(&db, lib, 4).await;
    let alpha = series(&db, lib, "Alpha").await;
    let beta = series(&db, lib, "Beta").await;
    let gamma = series(&db, lib, "Gamma").await;
    let uri = format!("/api/series/{alpha}/similar");
    for (pid, name) in [("4242", "Beta"), ("5151", "Gamma")] {
        let (status, body) = send(
            &app,
            Method::POST,
            &format!("/api/series/{alpha}/external-relationships"),
            &admin,
            Some(serde_json::json!({
                "kind": "see_also",
                "source": "metron",
                "provider_series_id": pid,
                "name": name,
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let (_, body) = get(&app, &uri, &admin).await;
    assert_eq!(ids(&body), Vec::<String>::new());
    assert!(!state.similarity.is_empty(), "cached");

    // (1) The external-ids endpoint matches Beta to Metron 4242: the
    // writer's promotion hook creates the pair, the handler drops the
    // cache.
    let (status, body) = send(
        &app,
        Method::POST,
        &format!("/api/series/{beta}/external-ids"),
        &admin,
        Some(serde_json::json!({ "source": "metron", "external_id": "4242" })),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
    assert!(state.similarity.is_empty(), "promotion clears the cache");
    let (_, body) = get(&app, &uri, &admin).await;
    assert_eq!(ids(&body), vec![beta.to_string()], "{body}");

    // (2) Gamma's id lands without the hook (a raw insert, as a missed
    // hook); the suggestion job's promotion pass creates the pair and the
    // job drops the cache.
    exec(
        &db,
        "INSERT INTO external_ids (entity_type, entity_id, source, external_id, set_by) \
         VALUES ('series', $1, 'metron', '5151', 'user')",
        vec![gamma.to_string().into()],
    )
    .await;
    let (_, body) = get(&app, &uri, &admin).await;
    assert_eq!(ids(&body), vec![beta.to_string()], "still cached");
    let generation = state.similarity.generation();
    let report = server::jobs::relationship_suggest::run_with_state(&state, lib)
        .await
        .unwrap();
    assert_eq!(report.promoted_pairs, 1, "{report:?}");
    assert!(state.similarity.generation() > generation);
    let (_, body) = get(&app, &uri, &admin).await;
    let mut got = ids(&body);
    got.sort();
    let mut want = vec![beta.to_string(), gamma.to_string()];
    want.sort();
    assert_eq!(got, want, "{body}");

    // A run that promotes nothing leaves the cache alone.
    let generation = state.similarity.generation();
    let report = server::jobs::relationship_suggest::run_with_state(&state, lib)
        .await
        .unwrap();
    assert_eq!(report.promoted_pairs, 0);
    assert_eq!(state.similarity.generation(), generation);
}
