//! WP-7.7: server reads behind the series page's Related tab — the
//! derived same-universe list (ACL, age cap, removed, paging),
//! `relationship_count` on the series detail, OPDS links for arc edges,
//! admin visibility + role grouping on the arc tie-in list, and the tie-in
//! role in similar-series reasons.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    library, library_user_access,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Database, DatabaseConnection, EntityTrait, QueryFilter, Set,
    Unchanged,
};
use server::relationships::{self, RelationshipKind, RelationshipSource};
use tower::ServiceExt;
use uuid::Uuid;

// ───── scaffolding ─────

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
    let user_row = entity::user::Entity::find()
        .filter(entity::user::Column::Email.eq(email))
        .one(&db)
        .await
        .unwrap()
        .expect("user row by email");
    Authed {
        session,
        csrf,
        user_id: user_row.id,
    }
}

async fn demote_to_user(db: &DatabaseConnection, user_id: Uuid) {
    entity::user::ActiveModel {
        id: Unchanged(user_id),
        role: Set("user".into()),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();
}

async fn call(
    app: &TestApp,
    method: Method,
    path: &str,
    user: &Authed,
    body: Option<serde_json::Value>,
) -> (StatusCode, String) {
    let mut req = Request::builder().method(method.clone()).uri(path);
    req = req.header(
        header::COOKIE,
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            user.session, user.csrf
        ),
    );
    if method != Method::GET {
        req = req.header("X-CSRF-Token", &user.csrf);
    }
    let body_bytes = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = app
        .router
        .clone()
        .oneshot(req.body(body_bytes).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn call_json(
    app: &TestApp,
    method: Method,
    path: &str,
    user: &Authed,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let (status, text) = call(app, method, path, user, body).await;
    let v = if text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
    };
    (status, v)
}

async fn mk_library(app: &TestApp, db: &DatabaseConnection, name: &str) -> Uuid {
    let now = Utc::now().fixed_offset();
    let lib_id = Uuid::now_v7();
    let root = app._data_dir.path().join(name);
    std::fs::create_dir_all(&root).unwrap();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set(name.into()),
        root_path: Set(root.to_string_lossy().into_owned()),
        default_language: Set("en".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(true),
        slug: Set(lib_id.to_string()),
        scan_schedule_cron: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_scan_at: Set(None),
        ignore_globs: Set(serde_json::json!([])),
        report_missing_comicinfo: Set(false),
        file_watch_enabled: Set(true),
        soft_delete_days: Set(30),
        thumbnails_enabled: Set(true),
        thumbnail_format: Set("webp".to_owned()),
        thumbnail_cover_quality: Set(server::library::thumbnails::DEFAULT_COVER_QUALITY as i32),
        thumbnail_page_quality: Set(server::library::thumbnails::DEFAULT_STRIP_QUALITY as i32),
        generate_page_thumbs_on_scan: Set(false),
        allow_archive_writeback: Set(false),
        metadata_writeback_enabled: Set(false),
        archive_backup_retain_count: Set(1),
        archive_backup_retain_days: Set(30),
        archive_writeback_jpeg_quality: Set(92),
        cbr_convert_confirmed_at: Set(None),
        metadata_publisher_blacklist: Set(serde_json::json!([])),
        filename_ignore_leading_numbers: Set(false),
        filename_assume_issue_one: Set(false),
        metadata_auto_apply_strong_matches: Set(false),
        auto_convert_cbr_on_scan: Set(false),
        auto_convert_cb7_on_scan: Set(false),
        trust_fingerprint_on_first_import: Set(false),
    }
    .insert(db)
    .await
    .unwrap();
    lib_id
}

async fn mk_series(db: &DatabaseConnection, lib_id: Uuid, name: &str, slug: &str) -> Uuid {
    let now = Utc::now().fixed_offset();
    let id = Uuid::now_v7();
    SeriesAM {
        id: Set(id),
        library_id: Set(lib_id),
        name: Set(name.into()),
        normalized_name: Set(normalize_name(name)),
        year: Set(Some(2020)),
        volume: Set(None),
        publisher: Set(Some("Rel Comics".into())),
        imprint: Set(None),
        status: Set("continuing".into()),
        total_issues: Set(None),
        age_rating: Set(None),
        summary: Set(None),
        language_code: Set("en".into()),
        sort_name: Set(None),
        year_end: Set(None),
        series_type: Set(None),
        aliases: Set(serde_json::json!([])),
        deck: Set(None),
        publisher_id: Set(None),
        imprint_id: Set(None),
        last_metadata_sync_at: Set(None),
        metadata_sync_paused: Set(false),
        series_json_present: Set(None),
        series_group: Set(None),
        slug: Set(slug.into()),
        alternate_names: Set(serde_json::json!([])),
        created_at: Set(now),
        updated_at: Set(now),
        folder_path: Set(None),
        last_scanned_at: Set(None),
        match_key: Set(None),
        removed_at: Set(None),
        removal_confirmed_at: Set(None),
        status_user_set_at: Set(None),
        reading_direction: Set(None),
        text_language: Set(None),
        preserve_canonical_order: Set(false),
    }
    .insert(db)
    .await
    .unwrap();
    id
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

// ───── WP-7.5: taxonomy, scope, PATCH, arc targets, mixed chain ─────

async fn exec_raw(
    db: &DatabaseConnection,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<(), sea_orm::DbErr> {
    use sea_orm::ConnectionTrait;
    db.execute_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .map(|_| ())
}

async fn mk_arc(db: &DatabaseConnection, name: &str, slug: &str) -> Uuid {
    let id = Uuid::now_v7();
    exec_raw(
        db,
        "INSERT INTO story_arc (id, slug, name, normalized_name) VALUES ($1, $2, $3, $4)",
        vec![
            id.into(),
            slug.into(),
            name.into(),
            normalize_name(name).into(),
        ],
    )
    .await
    .unwrap();
    id
}

async fn series_arc(db: &DatabaseConnection, series: Uuid, arc: Uuid) {
    exec_raw(
        db,
        "INSERT INTO series_arcs (series_id, arc_id) VALUES ($1, $2)",
        vec![series.into(), arc.into()],
    )
    .await
    .unwrap();
}

async fn set_raw(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    exec_raw(db, sql, values).await.unwrap();
}

async fn mk_universe(db: &DatabaseConnection, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    set_raw(
        db,
        "INSERT INTO universe (id, slug, name, normalized_name) VALUES ($1, $2, $3, $4)",
        vec![
            id.into(),
            normalize_name(name).replace(' ', "-").into(),
            name.into(),
            normalize_name(name).into(),
        ],
    )
    .await;
    id
}

async fn in_universe(db: &DatabaseConnection, series: Uuid, universe: Uuid) {
    set_raw(
        db,
        "INSERT INTO series_universes (series_id, universe_id) VALUES ($1, $2)",
        vec![series.into(), universe.into()],
    )
    .await;
}

fn slugs(body: &serde_json::Value) -> Vec<String> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["series"]["slug"].as_str().unwrap().to_owned())
        .collect()
}

// ───── tests ─────

#[tokio::test]
async fn same_universe_is_derived_acl_filtered_and_paginated() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    demote_to_user(&db, user.user_id).await;
    let open = mk_library(&app, &db, "open").await;
    let closed = mk_library(&app, &db, "closed").await;
    grant(&db, user.user_id, open).await;
    // Cap the user's grant at Teen.
    set_raw(
        &db,
        "UPDATE library_user_access SET age_rating_max = 'Teen' WHERE user_id = $1",
        vec![user.user_id.into()],
    )
    .await;

    let src = mk_series(&db, open, "Source", "source").await;
    let b = mk_series(&db, open, "Bravo", "bravo").await;
    let c = mk_series(&db, open, "Charlie", "charlie").await;
    let grp = mk_series(&db, open, "Echo", "echo").await;
    let closed_s = mk_series(&db, closed, "Delta", "delta").await;
    let removed = mk_series(&db, open, "Foxtrot", "foxtrot").await;
    let mature = mk_series(&db, open, "Golf", "golf").await;
    let unrelated = mk_series(&db, open, "Hotel", "hotel").await;
    let u = mk_universe(&db, "Mignolaverse").await;
    let other_u = mk_universe(&db, "Elsewhere").await;
    for s in [src, b, c, closed_s, removed, mature] {
        in_universe(&db, s, u).await;
    }
    in_universe(&db, unrelated, other_u).await;
    // Series groups: split on `,` / `;`, trimmed, case-insensitive. Bravo
    // shares both the universe and a group.
    set_raw(
        &db,
        "UPDATE series SET series_group = 'Hellboy Universe, B.P.R.D.' WHERE id = $1",
        vec![src.into()],
    )
    .await;
    set_raw(
        &db,
        "UPDATE series SET series_group = '  b.p.r.d. ;Other' WHERE id = $1",
        vec![grp.into()],
    )
    .await;
    set_raw(
        &db,
        "UPDATE series SET series_group = 'hellboy universe' WHERE id = $1",
        vec![b.into()],
    )
    .await;
    set_raw(
        &db,
        "UPDATE series SET removed_at = now() WHERE id = $1",
        vec![removed.into()],
    )
    .await;
    set_raw(
        &db,
        "UPDATE series SET age_rating = 'Adults Only 18+' WHERE id = $1",
        vec![mature.into()],
    )
    .await;

    let (status, body) = call_json(
        &app,
        Method::GET,
        "/api/series/source/same-universe",
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(slugs(&body), ["bravo", "charlie", "echo"]);
    assert_eq!(body["total"], 3);
    // What is shared: universes first, then groups (as this series spells
    // the group).
    assert_eq!(
        body["items"][0]["shared"],
        serde_json::json!([
            {"via": "universe", "name": "Mignolaverse"},
            {"via": "series_group", "name": "Hellboy Universe"},
        ])
    );
    assert_eq!(
        body["items"][2]["shared"],
        serde_json::json!([{"via": "series_group", "name": "B.P.R.D."}])
    );

    // Paging: limit 2 → 2 + cursor (total on page 1 only), then the rest.
    let (_, p1) = call_json(
        &app,
        Method::GET,
        "/api/series/source/same-universe?limit=2",
        &user,
        None,
    )
    .await;
    assert_eq!(slugs(&p1), ["bravo", "charlie"]);
    assert_eq!(p1["total"], 3);
    let cursor = p1["next_cursor"].as_str().expect("next_cursor").to_owned();
    let (_, p2) = call_json(
        &app,
        Method::GET,
        &format!("/api/series/source/same-universe?limit=2&cursor={cursor}"),
        &user,
        None,
    )
    .await;
    assert_eq!(slugs(&p2), ["echo"]);
    assert!(p2["next_cursor"].is_null());
    assert!(p2.get("total").is_none_or(serde_json::Value::is_null));

    // Admin: every library, no cap, removed series included.
    let (_, body) = call_json(
        &app,
        Method::GET,
        "/api/series/source/same-universe",
        &admin,
        None,
    )
    .await;
    assert_eq!(
        slugs(&body),
        ["bravo", "charlie", "delta", "echo", "foxtrot", "golf"]
    );

    // A series the caller can't see is a 404; a bad cursor is a 400.
    let (status, _) = call_json(
        &app,
        Method::GET,
        "/api/series/delta/same-universe",
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call_json(
        &app,
        Method::GET,
        "/api/series/source/same-universe?cursor=nope",
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A series sharing nothing gets an empty first page.
    let (status, body) = call_json(
        &app,
        Method::GET,
        "/api/series/hotel/same-universe",
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(slugs(&body), Vec::<String>::new());
    assert_eq!(body["total"], 0);
}

#[tokio::test]
async fn series_detail_carries_the_visible_relationship_count() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    demote_to_user(&db, user.user_id).await;
    let open = mk_library(&app, &db, "open").await;
    let closed = mk_library(&app, &db, "closed").await;
    grant(&db, user.user_id, open).await;
    let a = mk_series(&db, open, "Alpha", "alpha").await;
    let b = mk_series(&db, open, "Beta", "beta").await;
    let hidden = mk_series(&db, closed, "Hidden", "hidden").await;
    mk_series(&db, open, "Lonely", "lonely").await;
    for (to, kind) in [
        (b, RelationshipKind::SequelOf),
        (hidden, RelationshipKind::SeeAlso),
    ] {
        relationships::create_pair(&db, a, to, kind, RelationshipSource::Manual, None, None)
            .await
            .unwrap();
    }
    // Arc edges: `event` is visible to the user (appears in alpha),
    // `secret` only in the closed library.
    let event = mk_arc(&db, "Event", "event").await;
    let secret = mk_arc(&db, "Secret", "secret").await;
    series_arc(&db, a, event).await;
    series_arc(&db, hidden, secret).await;
    for arc in ["event", "secret"] {
        let (status, body) = call_json(
            &app,
            Method::POST,
            "/api/series/alpha/relationships",
            &admin,
            Some(serde_json::json!({"target_arc": arc, "kind": "tie_in_to"})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    let count = |who: &'static str, slug: &'static str| {
        let app = &app;
        let caller = if who == "admin" { &admin } else { &user };
        async move {
            let (status, body) = call_json(
                app,
                Method::GET,
                &format!("/api/series/{slug}"),
                caller,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            body["relationship_count"].as_i64()
        }
    };
    // User: beta + event (hidden series and secret arc filtered out).
    assert_eq!(count("user", "alpha").await, Some(2));
    // Admin: everything.
    assert_eq!(count("admin", "alpha").await, Some(4));
    assert_eq!(count("user", "lonely").await, Some(0));
    assert_eq!(count("user", "beta").await, Some(1));

    // List payloads stay lean: no count on `/series`.
    let (_, list) = call_json(&app, Method::GET, "/api/series?limit=50", &user, None).await;
    let items = list["items"].as_array().unwrap();
    assert!(!items.is_empty());
    assert!(items.iter().all(|i| i.get("relationship_count").is_none()));
}

#[tokio::test]
async fn opds_series_feeds_link_arc_edges_to_the_arc_feed() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    demote_to_user(&db, user.user_id).await;
    let open = mk_library(&app, &db, "open").await;
    let closed = mk_library(&app, &db, "closed").await;
    grant(&db, user.user_id, open).await;
    let a = mk_series(&db, open, "Alpha", "alpha").await;
    let hidden = mk_series(&db, closed, "Hidden", "hidden").await;
    let event = mk_arc(&db, "Secret Wars", "secret-wars").await;
    let secret = mk_arc(&db, "Classified", "classified").await;
    series_arc(&db, a, event).await;
    series_arc(&db, hidden, secret).await;
    for (arc, qualifier) in [("secret-wars", Some("prelude")), ("classified", None)] {
        let (status, body) = call_json(
            &app,
            Method::POST,
            "/api/series/alpha/relationships",
            &admin,
            Some(serde_json::json!({
                "target_arc": arc, "kind": "tie_in_to", "qualifier": qualifier,
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    let (status, xml) = call(
        &app,
        Method::GET,
        &format!("/opds/v1/series/{a}"),
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{xml}");
    assert!(
        xml.contains(
            r#"<link rel="related" href="/opds/v1/arcs/secret-wars" type="application/atom+xml;profile=opds-catalog;kind=acquisition" title="Prelude to: Secret Wars"/>"#
        ),
        "{xml}"
    );
    assert!(!xml.contains("classified"), "invisible arc leaked: {xml}");
    // The linked arc feed exists for this user.
    let (status, _) = call(&app, Method::GET, "/opds/v1/arcs/secret-wars", &user, None).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call_json(
        &app,
        Method::GET,
        &format!("/opds/v2/series/{a}"),
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let related: Vec<&serde_json::Value> = body["links"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["rel"] == "related")
        .collect();
    assert_eq!(related.len(), 1, "{body}");
    // WP-8.4: the v2 feed links the arc's OPDS 2.0 feed, not the 1.x one.
    assert_eq!(related[0]["href"], "/opds/v2/arcs/secret-wars");
    assert_eq!(related[0]["title"], "Prelude to: Secret Wars");
    assert_eq!(related[0]["type"], "application/opds+json");
    assert_eq!(related[0]["properties"]["folio:relationship"], "tie_in_to");
    // …and that feed exists for this user.
    let (status, arc) =
        call_json(&app, Method::GET, "/opds/v2/arcs/secret-wars", &user, None).await;
    assert_eq!(status, StatusCode::OK, "{arc}");
    assert_eq!(arc["metadata"]["title"], "Secret Wars");

    // Admin sees both arc links.
    let (_, xml) = call(
        &app,
        Method::GET,
        &format!("/opds/v1/series/{a}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(xml.matches("/opds/v1/arcs/").count(), 2, "{xml}");
}

#[tokio::test]
async fn arc_tie_ins_group_by_role_and_show_admins_removed_series() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    demote_to_user(&db, user.user_id).await;
    let open = mk_library(&app, &db, "open").await;
    grant(&db, user.user_id, open).await;
    let event = mk_arc(&db, "Event", "event").await;
    // Created in this order; roles shuffle them.
    let specs = [
        ("aftermath-one", Some("aftermath")),
        ("tie-one", None),
        ("main-one", Some("main")),
        ("prelude-one", Some("prelude")),
        ("tie-two", Some("tie_in")),
        ("gone", None),
    ];
    for (slug, qualifier) in specs {
        let id = mk_series(&db, open, slug, slug).await;
        series_arc(&db, id, event).await;
        let (status, body) = call_json(
            &app,
            Method::POST,
            &format!("/api/series/{slug}/relationships"),
            &admin,
            Some(serde_json::json!({
                "target_arc": "event", "kind": "tie_in_to", "qualifier": qualifier,
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    set_raw(
        &db,
        "UPDATE series SET removed_at = now() WHERE slug = 'gone'",
        vec![],
    )
    .await;

    // Walk every page at limit 2 so the role-keyed cursor is exercised.
    let walk = |caller: &'static str| {
        let app = &app;
        let who = if caller == "admin" { &admin } else { &user };
        async move {
            let mut out: Vec<(String, String)> = Vec::new();
            let mut cursor: Option<String> = None;
            loop {
                let path = match &cursor {
                    Some(c) => format!("/api/arcs/event/tie-ins?limit=2&cursor={c}"),
                    None => "/api/arcs/event/tie-ins?limit=2".to_owned(),
                };
                let (status, body) = call_json(app, Method::GET, &path, who, None).await;
                assert_eq!(status, StatusCode::OK, "{body}");
                for i in body["items"].as_array().unwrap() {
                    out.push((
                        i["series"]["slug"].as_str().unwrap().to_owned(),
                        i["kind_label"].as_str().unwrap().to_owned(),
                    ));
                }
                match body["next_cursor"].as_str() {
                    Some(c) => cursor = Some(c.to_owned()),
                    None => break,
                }
            }
            out
        }
    };
    let user_view = walk("user").await;
    assert_eq!(
        user_view,
        [
            ("prelude-one".to_owned(), "Prelude to".to_owned()),
            ("main-one".to_owned(), "Main story of".to_owned()),
            ("tie-one".to_owned(), "Tie-in to".to_owned()),
            ("tie-two".to_owned(), "Tie-in to".to_owned()),
            ("aftermath-one".to_owned(), "Aftermath of".to_owned()),
        ]
    );
    let admin_view: Vec<String> = walk("admin").await.into_iter().map(|(s, _)| s).collect();
    assert_eq!(
        admin_view,
        [
            "prelude-one",
            "main-one",
            "tie-one",
            "tie-two",
            "gone",
            "aftermath-one"
        ]
    );
    let (_, first) = call_json(&app, Method::GET, "/api/arcs/event/tie-ins", &user, None).await;
    assert_eq!(first["total"], 5);
    let (_, first) = call_json(&app, Method::GET, "/api/arcs/event/tie-ins", &admin, None).await;
    assert_eq!(first["total"], 6);
}

#[tokio::test]
async fn similar_reasons_fold_the_tie_in_role_into_the_label() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let open = mk_library(&app, &db, "open").await;
    mk_series(&db, open, "Event Main", "event-main").await;
    mk_series(&db, open, "Lead In", "lead-in").await;
    let (status, body) = call_json(
        &app,
        Method::POST,
        "/api/series/lead-in/relationships",
        &admin,
        Some(serde_json::json!({
            "target": "event-main", "kind": "tie_in_to", "qualifier": "prelude",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // On the main series' page, the lead-in reads "prelude to Event Main".
    let (status, body) = call_json(
        &app,
        Method::GET,
        "/api/series/event-main/similar",
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let item = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["series"]["slug"] == "lead-in")
        .expect("lead-in listed");
    let reason = item["because"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "relationship")
        .expect("relationship reason");
    assert_eq!(reason["role"], "tie_in_to");
    assert_eq!(reason["label"], "prelude to");
    assert_eq!(reason["name"], "Event Main (2020)");

    // And the reverse page: the main series "has prelude Lead In".
    let (_, body) = call_json(
        &app,
        Method::GET,
        "/api/series/lead-in/similar",
        &admin,
        None,
    )
    .await;
    let reason = body["items"][0]["because"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "relationship")
        .expect("relationship reason");
    assert_eq!(reason["label"], "has prelude", "{body}");
}
