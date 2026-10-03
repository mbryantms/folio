//! WP-5.5 — entity landing pages (`/characters`, `/teams`, `/arcs`,
//! `/publishers`) + their OPDS navigation feeds.
//!
//! Fixture: two libraries. Library A is granted to a restricted user with
//! a `Teen` age-rating cap; library B is not granted at all. The
//! character "Batman" appears in 3 visible issues of library A (one via
//! the provider FK, two via the scanner's name-only rows), in 1
//! `Mature 17+` issue of library A (hidden by the cap), and in library B.
//! "Joker" only appears in library B. The first registered account is
//! the admin and sees everything.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{SeriesSeed, seed_issue, seed_library};
use entity::{library_user_access, user::Entity as UserEntity};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, Database, DatabaseConnection, EntityTrait,
    QueryFilter, Set, Statement, Unchanged,
};
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
    let session = extract("__Host-comic_session=");
    let csrf = extract("__Host-comic_csrf=");
    let db = Database::connect(&app.db_url).await.unwrap();
    let user_row = UserEntity::find()
        .filter(entity::user::Column::Email.eq(email))
        .one(&db)
        .await
        .unwrap()
        .expect("user row");
    Authed {
        session,
        csrf,
        user_id: user_row.id,
    }
}

async fn get(app: &TestApp, uri: &str, user: &Authed) -> (StatusCode, Vec<u8>) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                user.session, user.csrf
            ),
        )
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

async fn get_json(app: &TestApp, uri: &str, user: &Authed) -> (StatusCode, serde_json::Value) {
    let (status, bytes) = get(app, uri, user).await;
    let v = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, v)
}

async fn get_text(app: &TestApp, uri: &str, user: &Authed) -> (StatusCode, String) {
    let (status, bytes) = get(app, uri, user).await;
    (status, String::from_utf8(bytes).unwrap())
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

async fn add_entity(db: &DatabaseConnection, table: &str, slug: &str, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    exec(
        db,
        &format!("INSERT INTO {table} (id, slug, name, normalized_name) VALUES ($1, $2, $3, $4)"),
        vec![
            id.into(),
            slug.into(),
            name.into(),
            name.trim().to_lowercase().into(),
        ],
    )
    .await;
    id
}

async fn add_character(db: &DatabaseConnection, issue_id: &str, name: &str, fk: Option<Uuid>) {
    exec(
        db,
        "INSERT INTO issue_characters (issue_id, character, character_id) VALUES ($1, $2, $3)",
        vec![issue_id.into(), name.into(), fk.into()],
    )
    .await;
}

async fn set_provenance(db: &DatabaseConnection, issue_id: &str, field: &str, set_by: &str) {
    exec(
        db,
        "INSERT INTO field_provenance (entity_type, entity_id, field, set_by, set_at) \
         VALUES ('issue', $1, $2, $3, now())",
        vec![issue_id.into(), field.into(), set_by.into()],
    )
    .await;
}

/// First column of a one-row query as `i64` (`None` for no row / NULL).
async fn scalar(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> Option<i64> {
    db.query_one_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        sql,
        values,
    ))
    .await
    .unwrap()
    .and_then(|r| r.try_get::<Option<i64>>("", "n").unwrap())
}

struct Fixture {
    _tmp: tempfile::TempDir,
    db: DatabaseConnection,
    admin: Authed,
    user: Authed,
    lib_a: Uuid,
    series_a: Uuid,
    series_b: Uuid,
    /// Visible, unrated issues of series A, in number order.
    a_issues: Vec<String>,
    /// Series-A issue rated `Mature 17+` (hidden by the Teen cap).
    a_mature: String,
    b_issues: Vec<String>,
}

async fn fixture(app: &TestApp) -> Fixture {
    let admin = register(app, "admin@example.com").await;
    let user = register(app, "reader@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
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
    // Make sure the second account is a plain user.
    entity::user::ActiveModel {
        id: Unchanged(user.user_id),
        role: Set("user".into()),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();

    let series_a = SeriesSeed::new(lib_a, "Detective Comics")
        .with_publisher("DC Comics")
        .insert(&db)
        .await;
    let series_b = SeriesSeed::new(lib_b, "Hidden Tales")
        .with_publisher("Secret Press")
        .insert(&db)
        .await;
    let mut a_issues = Vec::new();
    for n in 1..=4 {
        let p = root_a.join(format!("dc-{n}.cbz"));
        let id = seed_issue(
            &db,
            lib_a,
            series_a,
            &p,
            format!("a{n}").as_bytes(),
            n as f64,
        )
        .await;
        a_issues.push(id);
    }
    let a_mature = a_issues.pop().unwrap();
    exec(
        &db,
        "UPDATE issues SET age_rating = 'Mature 17+' WHERE id = $1",
        vec![a_mature.clone().into()],
    )
    .await;
    let mut b_issues = Vec::new();
    for n in 1..=2 {
        let p = root_b.join(format!("ht-{n}.cbz"));
        let id = seed_issue(
            &db,
            lib_b,
            series_b,
            &p,
            format!("b{n}").as_bytes(),
            n as f64,
        )
        .await;
        b_issues.push(id);
    }
    Fixture {
        _tmp: tmp,
        db,
        admin,
        user,
        lib_a,
        series_a,
        series_b,
        a_issues,
        a_mature,
        b_issues,
    }
}

#[tokio::test]
async fn characters_are_acl_and_cap_filtered_and_paginate() {
    let app = TestApp::spawn().await;
    let f = fixture(&app).await;
    let batman = add_entity(&f.db, "character", "batman", "Batman").await;
    add_entity(&f.db, "character", "joker", "Joker").await;
    // One provider-linked row (FK), two scanner name-only rows (one with
    // different casing / whitespace), the capped issue, and library B.
    add_character(&f.db, &f.a_issues[0], "Batman", Some(batman)).await;
    add_character(&f.db, &f.a_issues[1], "batman ", None).await;
    add_character(&f.db, &f.a_issues[2], "Batman", None).await;
    add_character(&f.db, &f.a_mature, "Batman", None).await;
    add_character(&f.db, &f.b_issues[0], "Batman", None).await;
    add_character(&f.db, &f.b_issues[1], "Joker", None).await;

    // ── browse index ──
    let (s, body) = get_json(&app, "/api/characters", &f.user).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let names: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Batman"], "Joker is library-B only");
    assert_eq!(body["total"], 1);
    assert_eq!(
        body["items"][0]["issue_count"], 3,
        "capped + hidden excluded"
    );
    assert_eq!(body["items"][0]["series_count"], 1);

    let (_, body) = get_json(&app, "/api/characters", &f.admin).await;
    assert_eq!(body["total"], 2);
    assert_eq!(body["items"][0]["issue_count"], 5);

    // `q` + `starts_with` filters.
    let (_, body) = get_json(&app, "/api/characters?q=jok", &f.admin).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    let (_, body) = get_json(&app, "/api/characters?starts_with=b", &f.admin).await;
    assert_eq!(body["items"][0]["slug"], "batman");
    let (s, _) = get_json(&app, "/api/characters?starts_with=zz", &f.admin).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Index pagination walks without truncation.
    let (_, p1) = get_json(&app, "/api/characters?limit=1", &f.admin).await;
    assert_eq!(p1["items"][0]["name"], "Batman");
    let cursor = p1["next_cursor"].as_str().expect("next page").to_owned();
    let (_, p2) = get_json(
        &app,
        &format!("/api/characters?limit=1&cursor={cursor}"),
        &f.admin,
    )
    .await;
    assert_eq!(p2["items"][0]["name"], "Joker");
    assert!(p2["next_cursor"].is_null());
    assert!(p2.get("total").is_none(), "total only on the first page");

    // ── detail ──
    let (s, body) = get_json(&app, "/api/characters/batman", &f.user).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["kind"], "characters");
    assert_eq!(body["issue_count"], 3);
    assert_eq!(body["series_count"], 1);
    let (s, body) = get_json(&app, "/api/characters/joker", &f.user).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "hidden-only entity must 404");
    assert_eq!(body["error"]["code"], "not_found");
    let (s, _) = get_json(&app, "/api/characters/joker", &f.admin).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = get_json(&app, "/api/characters/nobody", &f.admin).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // ── issues: cursor pagination, never leaks hidden/capped rows ──
    let (s, p1) = get_json(&app, "/api/characters/batman/issues?limit=2", &f.user).await;
    assert_eq!(s, StatusCode::OK, "{p1}");
    assert_eq!(p1["total"], 3);
    let mut seen: Vec<String> = p1["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(seen.len(), 2);
    assert_eq!(p1["items"][0]["series_name"], "Detective Comics");
    let cursor = p1["next_cursor"].as_str().expect("second page").to_owned();
    let (_, p2) = get_json(
        &app,
        &format!("/api/characters/batman/issues?limit=2&cursor={cursor}"),
        &f.user,
    )
    .await;
    assert!(p2.get("total").is_none());
    assert!(p2["next_cursor"].is_null());
    seen.extend(
        p2["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_owned()),
    );
    assert_eq!(seen, f.a_issues, "number order, no capped/hidden issues");

    let (s, _) = get_json(&app, "/api/characters/batman/issues?cursor=bogus", &f.user).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Admin sees all five issues across both libraries.
    let (_, all) = get_json(&app, "/api/characters/batman/issues", &f.admin).await;
    assert_eq!(all["total"], 5);
    assert_eq!(all["items"].as_array().unwrap().len(), 5);

    // ── series ──
    let (_, body) = get_json(&app, "/api/characters/batman/series", &f.user).await;
    let ids: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![f.series_a.to_string().as_str()]);
    assert_eq!(body["total"], 1);
    let (_, body) = get_json(&app, "/api/characters/batman/series", &f.admin).await;
    assert_eq!(body["total"], 2);
    let _ = f.series_b;
    let _ = f.lib_a;
}

#[tokio::test]
async fn teams_include_series_level_membership() {
    let app = TestApp::spawn().await;
    let f = fixture(&app).await;
    add_entity(&f.db, "team", "justice-league", "Justice League").await;
    exec(
        &f.db,
        "INSERT INTO series_teams (series_id, team) VALUES ($1, 'Justice League')",
        vec![f.series_a.into()],
    )
    .await;
    let (s, body) = get_json(&app, "/api/teams/justice-league", &f.user).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["series_count"], 1);
    assert_eq!(body["issue_count"], 0);
    let (_, body) = get_json(&app, "/api/teams/justice-league/series", &f.user).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    let (_, body) = get_json(&app, "/api/teams", &f.user).await;
    assert_eq!(body["items"][0]["slug"], "justice-league");
}

#[tokio::test]
async fn arcs_merge_fk_and_csv_rows_in_reading_order() {
    let app = TestApp::spawn().await;
    let f = fixture(&app).await;
    let arc = add_entity(&f.db, "story_arc", "knightfall", "Knightfall").await;
    // Issue 3 is part 1 via a provider apply: `issue_arcs` row + provider
    // provenance (so the rollup must leave it alone) + the rebuilt CSV.
    exec(
        &f.db,
        "INSERT INTO issue_arcs (issue_id, arc_id, position_in_arc) VALUES ($1, $2, 1)",
        vec![f.a_issues[2].clone().into(), arc.into()],
    )
    .await;
    exec(
        &f.db,
        "UPDATE issues SET story_arc = 'Knightfall' WHERE id = $1",
        vec![f.a_issues[2].clone().into()],
    )
    .await;
    set_provenance(&f.db, &f.a_issues[2], "story_arcs", "comicvine").await;
    // Issues 1 + 2 are parts 3 + 2 via the scanner CSV read-cache only.
    exec(
        &f.db,
        "UPDATE issues SET story_arc = 'Other Arc, Knightfall', story_arc_number = '3' WHERE id = $1",
        vec![f.a_issues[0].clone().into()],
    )
    .await;
    exec(
        &f.db,
        "UPDATE issues SET story_arc = 'knightfall', story_arc_number = '2' WHERE id = $1",
        vec![f.a_issues[1].clone().into()],
    )
    .await;
    // A substring-but-not-member arc name must not match; a stale
    // file-owned link (CSV no longer names the arc) must be dropped.
    exec(
        &f.db,
        "UPDATE issues SET story_arc = 'Knightfall Aftermath' WHERE id = $1",
        vec![f.a_mature.clone().into()],
    )
    .await;
    exec(
        &f.db,
        "INSERT INTO issue_arcs (issue_id, arc_id) VALUES ($1, $2)",
        vec![f.a_mature.clone().into(), arc.into()],
    )
    .await;

    // The rollup reconciles the CSV arcs into `issue_arcs` (WP-5.5).
    server::library::scanner::metadata_rollup::rollup_series_metadata(&f.db, f.series_a)
        .await
        .unwrap();
    assert_eq!(
        scalar(
            &f.db,
            "SELECT position_in_arc::int8 AS n FROM issue_arcs WHERE issue_id = $1 AND arc_id = $2",
            vec![f.a_issues[1].clone().into(), arc.into()],
        )
        .await,
        Some(2),
        "single-arc issue takes story_arc_number as its position"
    );
    assert_eq!(
        scalar(
            &f.db,
            "SELECT position_in_arc::int8 AS n FROM issue_arcs WHERE issue_id = $1",
            vec![f.a_issues[2].clone().into()],
        )
        .await,
        Some(1),
        "provider-owned link untouched"
    );

    let (s, body) = get_json(&app, "/api/arcs/knightfall/issues", &f.admin).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let ids: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec![
            f.a_issues[2].as_str(),
            f.a_issues[1].as_str(),
            f.a_issues[0].as_str()
        ]
    );
    let (_, list) = get_json(&app, "/api/arcs", &f.user).await;
    assert_eq!(list["items"][0]["issue_count"], 3);

    // OPDS arc feed is an acquisition feed with sequential nav.
    let (s, xml) = get_text(&app, "/opds/v1/arcs/knightfall", &f.user).await;
    assert_eq!(s, StatusCode::OK);
    assert!(xml.contains("<title>Knightfall</title>"), "{xml}");
    assert_eq!(xml.matches("<entry>").count(), 3);
}

#[tokio::test]
async fn publishers_match_by_fk_or_name() {
    let app = TestApp::spawn().await;
    let f = fixture(&app).await;
    add_entity(&f.db, "publisher", "dc-comics", "DC Comics").await;
    add_entity(&f.db, "publisher", "secret-press", "Secret Press").await;

    let (_, list) = get_json(&app, "/api/publishers", &f.user).await;
    let slugs: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, vec!["dc-comics"]);
    let (s, _) = get_json(&app, "/api/publishers/secret-press", &f.user).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (_, body) = get_json(&app, "/api/publishers/dc-comics/series", &f.user).await;
    assert_eq!(body["items"][0]["name"], "Detective Comics");
    // Publishers have no issue grid.
    let (s, _) = get_json(&app, "/api/publishers/dc-comics/issues", &f.user).await;
    assert_ne!(s, StatusCode::OK);

    let (s, xml) = get_text(&app, "/opds/v1/publishers/dc-comics", &f.user).await;
    assert_eq!(s, StatusCode::OK);
    assert!(xml.contains("Detective Comics"), "{xml}");
}

#[tokio::test]
async fn opds_root_links_entity_feeds_and_feeds_are_acl_filtered() {
    let app = TestApp::spawn().await;
    let f = fixture(&app).await;
    add_entity(&f.db, "character", "batman", "Batman").await;
    add_entity(&f.db, "character", "joker", "Joker").await;
    add_character(&f.db, &f.a_issues[0], "Batman", None).await;
    add_character(&f.db, &f.b_issues[0], "Joker", None).await;

    let (s, root) = get_text(&app, "/opds/v1", &f.user).await;
    assert_eq!(s, StatusCode::OK);
    for p in ["characters", "teams", "arcs", "publishers"] {
        assert!(
            root.contains(&format!("href=\"/opds/v1/{p}\"")),
            "root feed missing /opds/v1/{p}: {root}"
        );
    }

    let (s, nav) = get_text(&app, "/opds/v1/characters", &f.user).await;
    assert_eq!(s, StatusCode::OK);
    assert!(nav.contains("href=\"/opds/v1/characters/batman\""), "{nav}");
    assert!(
        !nav.contains("joker"),
        "hidden-library character leaked: {nav}"
    );

    let (s, acq) = get_text(&app, "/opds/v1/characters/batman", &f.user).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(acq.matches("<entry>").count(), 1);
    let (s, _) = get_text(&app, "/opds/v1/characters/joker", &f.user).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rollup_mints_entity_rows_and_detail_pages_expose_slugs() {
    let app = TestApp::spawn().await;
    let f = fixture(&app).await;
    // Scanner-shaped data with no entity rows behind it.
    add_character(&f.db, &f.a_issues[0], "Harley Quinn", None).await;
    exec(
        &f.db,
        "INSERT INTO issue_teams (issue_id, team) VALUES ($1, 'Gotham Sirens')",
        vec![f.a_issues[0].clone().into()],
    )
    .await;
    exec(
        &f.db,
        "UPDATE issues SET characters = 'Harley Quinn', teams = 'Gotham Sirens', \
         story_arc = 'Mad Love' WHERE id = $1",
        vec![f.a_issues[0].clone().into()],
    )
    .await;

    // A provider-linked row whose entity's name differs from the junction
    // text must keep its FK (fill-only-while-NULL).
    let the_batman = add_entity(&f.db, "character", "the-batman", "The Batman").await;
    add_entity(&f.db, "character", "batman", "Batman").await;
    add_character(&f.db, &f.a_issues[1], "Batman", Some(the_batman)).await;

    server::library::scanner::metadata_rollup::rollup_series_metadata(&f.db, f.series_a)
        .await
        .unwrap();

    // Id links filled by the rollup.
    let linked = |sql: &'static str| {
        let db = f.db.clone();
        let issue = f.a_issues[0].clone();
        async move { scalar(&db, sql, vec![issue.into()]).await }
    };
    assert_eq!(
        linked(
            "SELECT count(*) AS n FROM issue_characters j JOIN character e ON e.id = j.character_id \
             WHERE j.issue_id = $1 AND e.slug = 'harley-quinn'"
        )
        .await,
        Some(1)
    );
    assert_eq!(
        linked(
            "SELECT count(*) AS n FROM issue_teams j JOIN team e ON e.id = j.team_id \
             WHERE j.issue_id = $1 AND e.slug = 'gotham-sirens'"
        )
        .await,
        Some(1)
    );
    assert_eq!(
        linked(
            "SELECT count(*) AS n FROM issue_arcs j JOIN story_arc e ON e.id = j.arc_id \
             WHERE j.issue_id = $1 AND e.slug = 'mad-love'"
        )
        .await,
        Some(1)
    );
    assert_eq!(
        scalar(
            &f.db,
            "SELECT count(*) AS n FROM series s JOIN publisher e ON e.id = s.publisher_id \
             WHERE s.id = $1 AND e.slug = 'dc-comics'",
            vec![f.series_a.into()],
        )
        .await,
        Some(1)
    );
    assert_eq!(
        scalar(
            &f.db,
            "SELECT count(*) AS n FROM series_characters j JOIN character e ON e.id = j.character_id \
             WHERE j.series_id = $1 AND e.slug = 'harley-quinn'",
            vec![f.series_a.into()],
        )
        .await,
        Some(1),
        "series_characters rebuild carries the FK"
    );
    assert_eq!(
        scalar(
            &f.db,
            "SELECT count(*) AS n FROM issue_characters \
             WHERE issue_id = $1 AND character = 'Batman' AND character_id = $2",
            vec![f.a_issues[1].clone().into(), the_batman.into()],
        )
        .await,
        Some(1),
        "provider-set FK is never re-pointed by a name match"
    );
    // Idempotent: a second rollup inserts nothing and doesn't error.
    server::library::scanner::metadata_rollup::rollup_series_metadata(&f.db, f.series_a)
        .await
        .unwrap();

    let (s, body) = get_json(&app, "/api/characters/harley-quinn", &f.user).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (s, _) = get_json(&app, "/api/teams/gotham-sirens", &f.user).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = get_json(&app, "/api/arcs/mad-love", &f.user).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = get_json(&app, "/api/publishers/dc-comics", &f.user).await;
    assert_eq!(s, StatusCode::OK);

    // Issue detail hands the chips their slugs.
    #[derive(sea_orm::FromQueryResult)]
    struct Slugs {
        series_slug: String,
        issue_slug: String,
    }
    use sea_orm::FromQueryResult;
    let row = Slugs::find_by_statement(Statement::from_sql_and_values(
        f.db.get_database_backend(),
        "SELECT s.slug AS series_slug, i.slug AS issue_slug \
           FROM issues i JOIN series s ON s.id = i.series_id WHERE i.id = $1",
        [f.a_issues[0].clone().into()],
    ))
    .one(&f.db)
    .await
    .unwrap()
    .unwrap();
    let (s, issue) = get_json(
        &app,
        &format!("/api/series/{}/issues/{}", row.series_slug, row.issue_slug),
        &f.user,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let slugs = &issue["entity_slugs"];
    assert_eq!(
        slugs["characters"]["Harley Quinn"], "harley-quinn",
        "{issue}"
    );
    assert_eq!(slugs["teams"]["Gotham Sirens"], "gotham-sirens");
    assert_eq!(slugs["arcs"]["Mad Love"], "mad-love");

    let (s, series) = get_json(&app, &format!("/api/series/{}", row.series_slug), &f.user).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        series["entity_slugs"]["publishers"]["DC Comics"],
        "dc-comics"
    );
}

/// `m20270305` on pre-existing scanner data: entity rows minted, FKs /
/// `series.publisher_id` filled, CSV arcs linked for file-owned issues,
/// provider-owned arcs left alone. Rolled back and re-applied on seeded
/// data, the same way `migration_retire_user_edited.rs` exercises its
/// migration.
#[tokio::test]
async fn migration_backfills_entities_and_links() {
    use migration::MigratorTrait;
    let app = TestApp::spawn().await;
    let f = fixture(&app).await;
    add_character(&f.db, &f.a_issues[0], "Poison Ivy", None).await;
    exec(
        &f.db,
        "INSERT INTO issue_teams (issue_id, team) VALUES ($1, 'Birds of Prey')",
        vec![f.a_issues[0].clone().into()],
    )
    .await;
    exec(
        &f.db,
        "UPDATE issues SET story_arc = 'No Man''s Land', story_arc_number = '4' WHERE id = $1",
        vec![f.a_issues[0].clone().into()],
    )
    .await;
    // Provider-owned arcs: CSV names an arc, but provenance says a provider
    // owns the field — the migration must not add a link for it.
    exec(
        &f.db,
        "UPDATE issues SET story_arc = 'Hush' WHERE id = $1",
        vec![f.a_issues[1].clone().into()],
    )
    .await;
    set_provenance(&f.db, &f.a_issues[1], "story_arcs", "metron").await;

    let names: Vec<String> = migration::Migrator::migrations()
        .iter()
        .map(|m| m.name().to_owned())
        .collect();
    let pos = names
        .iter()
        .position(|n| n == "m20270305_000001_entity_pages")
        .expect("migration registered");
    let steps = u32::try_from(names.len() - pos).unwrap();
    migration::Migrator::down(&f.db, Some(steps)).await.unwrap();
    migration::Migrator::up(&f.db, None).await.unwrap();

    let one = |sql: &'static str, v: Vec<sea_orm::Value>| {
        let db = f.db.clone();
        async move { scalar(&db, sql, v).await }
    };
    assert_eq!(
        one(
            "SELECT count(*) AS n FROM issue_characters j JOIN character e ON e.id = j.character_id \
             WHERE j.issue_id = $1 AND e.slug = 'poison-ivy'",
            vec![f.a_issues[0].clone().into()],
        )
        .await,
        Some(1)
    );
    assert_eq!(
        one(
            "SELECT count(*) AS n FROM issue_teams j JOIN team e ON e.id = j.team_id \
             WHERE j.issue_id = $1 AND e.slug = 'birds-of-prey'",
            vec![f.a_issues[0].clone().into()],
        )
        .await,
        Some(1)
    );
    assert_eq!(
        one(
            "SELECT j.position_in_arc::int8 AS n FROM issue_arcs j JOIN story_arc e ON e.id = j.arc_id \
             WHERE j.issue_id = $1 AND e.slug = 'no-man-s-land'",
            vec![f.a_issues[0].clone().into()],
        )
        .await,
        Some(4)
    );
    assert_eq!(
        one(
            "SELECT count(*) AS n FROM issue_arcs WHERE issue_id = $1",
            vec![f.a_issues[1].clone().into()],
        )
        .await,
        Some(0),
        "provider-owned arcs are not relinked"
    );
    assert_eq!(
        one(
            "SELECT count(*) AS n FROM series s JOIN publisher e ON e.id = s.publisher_id \
             WHERE s.id = $1 AND e.slug = 'dc-comics'",
            vec![f.series_a.into()],
        )
        .await,
        Some(1)
    );
    // And the landing page resolves through the id link.
    let (s, body) = get_json(&app, "/api/arcs/no-man-s-land/issues", &f.user).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 1);
}

// ───── WP-8.4: OPDS 2.0 entity feeds ─────

/// Shape checks every OPDS 2.0 feed must pass (the same rules the
/// `opds_v2.rs` suite applies): `metadata.title`, typed links with a
/// `self`, and navigation entries / publications carrying what a client
/// needs to render and follow them.
fn assert_opds2_feed(body: &serde_json::Value) {
    assert!(body["metadata"]["title"].is_string(), "title: {body}");
    let links = body["links"].as_array().expect("links");
    for l in links {
        assert!(l["rel"].is_string() && l["href"].is_string(), "link: {l}");
        assert_eq!(l["type"], "application/opds+json", "link type: {l}");
    }
    assert!(
        links.iter().any(|l| l["rel"] == "self"),
        "self link: {body}"
    );
    for n in body["navigation"].as_array().into_iter().flatten() {
        assert!(n["title"].is_string() && n["href"].is_string(), "nav: {n}");
    }
    for p in body["publications"].as_array().into_iter().flatten() {
        assert!(p["metadata"]["title"].is_string(), "publication: {p}");
        assert!(
            p["links"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["rel"] == "http://opds-spec.org/acquisition"),
            "acquisition link: {p}"
        );
    }
}

#[tokio::test]
async fn opds_v2_entity_feeds_mirror_v1_and_are_acl_filtered() {
    let app = TestApp::spawn().await;
    let f = fixture(&app).await;
    add_entity(&f.db, "character", "batman", "Batman").await;
    add_entity(&f.db, "character", "joker", "Joker").await;
    add_character(&f.db, &f.a_issues[0], "Batman", None).await;
    add_character(&f.db, &f.b_issues[0], "Joker", None).await;
    add_entity(&f.db, "team", "gotham-sirens", "Gotham Sirens").await;
    exec(
        &f.db,
        "INSERT INTO issue_teams (issue_id, team) VALUES ($1, 'Gotham Sirens')",
        vec![f.a_issues[1].clone().into()],
    )
    .await;
    add_entity(&f.db, "publisher", "dc-comics", "DC Comics").await;

    // Root navigation links all four entity feeds.
    let (s, root) = get_json(&app, "/opds/v2", &f.user).await;
    assert_eq!(s, StatusCode::OK);
    let hrefs: Vec<&str> = root["navigation"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["href"].as_str().unwrap())
        .collect();
    for p in ["characters", "teams", "arcs", "publishers"] {
        assert!(
            hrefs.contains(&format!("/opds/v2/{p}").as_str()),
            "{hrefs:?}"
        );
    }

    // Index: navigation entries, hidden-library entities excluded.
    let (s, nav) = get_json(&app, "/opds/v2/characters", &f.user).await;
    assert_eq!(s, StatusCode::OK, "{nav}");
    assert_opds2_feed(&nav);
    let entries = nav["navigation"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{nav}");
    assert_eq!(entries[0]["href"], "/opds/v2/characters/batman");
    assert_eq!(entries[0]["title"], "Batman");
    assert_eq!(nav["metadata"]["numberOfItems"], 1);

    // Detail: the entity's issues as publications, same set as v1.
    let (s, feed) = get_json(&app, "/opds/v2/characters/batman", &f.user).await;
    assert_eq!(s, StatusCode::OK, "{feed}");
    assert_opds2_feed(&feed);
    let pubs = feed["publications"].as_array().unwrap();
    assert_eq!(pubs.len(), 1);
    assert!(
        feed["metadata"]["identifier"]
            .as_str()
            .unwrap()
            .starts_with("urn:folio:characters:"),
        "{feed}"
    );
    let (_, v1) = get_text(&app, "/opds/v1/characters/batman", &f.user).await;
    assert_eq!(v1.matches("<entry>").count(), pubs.len());
    let (s, _) = get_json(&app, "/opds/v2/characters/joker", &f.user).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let (s, team) = get_json(&app, "/opds/v2/teams/gotham-sirens", &f.user).await;
    assert_eq!(s, StatusCode::OK, "{team}");
    assert_opds2_feed(&team);
    assert_eq!(team["publications"].as_array().unwrap().len(), 1);

    // Publishers list series as navigation.
    let (s, publisher) = get_json(&app, "/opds/v2/publishers/dc-comics", &f.user).await;
    assert_eq!(s, StatusCode::OK, "{publisher}");
    assert_opds2_feed(&publisher);
    assert_eq!(publisher["navigation"][0]["title"], "Detective Comics");

    // A v1 entity path asked for as OPDS 2.0 now redirects to its twin.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/opds/v1/characters/batman")
        .header(header::ACCEPT, "application/opds+json")
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                f.user.session, f.user.csrf
            ),
        )
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        resp.headers().get(header::LOCATION).unwrap(),
        "/opds/v2/characters/batman"
    );
}

#[tokio::test]
async fn opds_v2_arc_feed_is_a_reading_order_with_sequential_links() {
    let app = TestApp::spawn().await;
    let f = fixture(&app).await;
    let arc = add_entity(&f.db, "story_arc", "knightfall", "Knightfall").await;
    for (i, pos) in [(2usize, 1), (0, 2), (1, 3)] {
        exec(
            &f.db,
            "INSERT INTO issue_arcs (issue_id, arc_id, position_in_arc) VALUES ($1, $2, $3)",
            vec![f.a_issues[i].clone().into(), arc.into(), pos.into()],
        )
        .await;
    }
    let (s, feed) = get_json(&app, "/opds/v2/arcs/knightfall", &f.user).await;
    assert_eq!(s, StatusCode::OK, "{feed}");
    assert_opds2_feed(&feed);
    assert_eq!(feed["metadata"]["title"], "Knightfall");
    let pubs = feed["publications"].as_array().unwrap();
    let ids: Vec<String> = pubs
        .iter()
        .map(|p| p["metadata"]["identifier"].as_str().unwrap().to_owned())
        .collect();
    let want: Vec<String> = [2, 0, 1]
        .iter()
        .map(|&i| format!("urn:folio:issue:{}", f.a_issues[i]))
        .collect();
    assert_eq!(ids, want, "arc reading order");
    // Reading-sequence feed: middle publication links both neighbours.
    let rels: Vec<&str> = pubs[1]["links"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| l["rel"].as_str())
        .collect();
    assert!(
        rels.contains(&"next") && rels.contains(&"previous"),
        "{rels:?}"
    );
    // Up-link to the arcs index.
    assert!(
        feed["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["rel"] == "up" && l["href"] == "/opds/v2/arcs"),
        "{feed}"
    );
}
