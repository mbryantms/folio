//! WP-7.1: series relationships — inverse pairs, idempotency, validation,
//! admin gate + audit, ACL-filtered reads, cycle safety, and the CTE depth
//! cap, plus the OPDS `rel="related"` links.

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
    series_relationship as rel,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Database, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, Set, Unchanged,
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

async fn edges(db: &DatabaseConnection) -> Vec<(Uuid, Uuid, String)> {
    let mut v: Vec<_> = rel::Entity::find()
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.from_series_id, r.to_series_id, r.kind))
        .collect();
    v.sort();
    v
}

async fn audit_count(db: &DatabaseConnection, action: &str) -> u64 {
    entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq(action))
        .count(db)
        .await
        .unwrap()
}

fn chain_slugs(body: &serde_json::Value) -> Vec<(i64, String)> {
    body["chain"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["position"].as_i64().unwrap(),
                e["series"]["slug"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

// ───── tests ─────

#[tokio::test]
async fn create_writes_inverse_pair_and_delete_removes_both() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = mk_library(&app, &db, "lib").await;
    let a = mk_series(&db, lib, "Alpha", "alpha").await;
    let b = mk_series(&db, lib, "Beta", "beta").await;

    let (status, body) = call_json(
        &app,
        Method::POST,
        "/api/series/beta/relationships",
        &admin,
        Some(serde_json::json!({"target": "alpha", "kind": "sequel_of"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["kind"], "sequel_of");
    assert_eq!(body["source"], "manual");
    assert_eq!(body["series"]["slug"], "alpha");
    let fwd_id = body["id"].as_str().unwrap().to_owned();

    // Both halves exist.
    let mut expect = vec![
        (b, a, "sequel_of".to_owned()),
        (a, b, "prequel_of".to_owned()),
    ];
    expect.sort();
    assert_eq!(edges(&db).await, expect);
    let fwd = rel::Entity::find_by_id(Uuid::parse_str(&fwd_id).unwrap())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fwd.created_by, Some(admin.user_id));

    // The other side sees the inverse.
    let (status, body) = call_json(
        &app,
        Method::GET,
        "/api/series/alpha/relationships",
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rels = body["relationships"].as_array().unwrap();
    assert_eq!(rels.len(), 1);
    assert_eq!(rels[0]["kind"], "prequel_of");
    assert_eq!(rels[0]["kind_label"], "Prequel of");
    assert_eq!(rels[0]["series"]["slug"], "beta");
    assert_eq!(
        chain_slugs(&body),
        vec![(0, "alpha".to_owned()), (1, "beta".to_owned())]
    );
    assert_eq!(
        audit_count(&db, "admin.series.relationship.create").await,
        1
    );

    // Delete via the inverse half's id (from alpha's page) removes both.
    let inverse_id = rels[0]["id"].as_str().unwrap();
    let (status, _) = call(
        &app,
        Method::DELETE,
        &format!("/api/series/alpha/relationships/{inverse_id}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(edges(&db).await.is_empty());
    assert_eq!(
        audit_count(&db, "admin.series.relationship.delete").await,
        1
    );

    // Deleting again → 404.
    let (status, _) = call(
        &app,
        Method::DELETE,
        &format!("/api/series/alpha/relationships/{inverse_id}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn self_inverse_kinds_store_same_kind_both_ways() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = mk_library(&app, &db, "lib").await;
    let a = mk_series(&db, lib, "Alpha", "alpha").await;
    let b = mk_series(&db, lib, "Beta", "beta").await;
    let (status, _) = call_json(
        &app,
        Method::POST,
        "/api/series/alpha/relationships",
        &admin,
        Some(serde_json::json!({"target": b.to_string(), "kind": "crossover_with"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let mut expect = vec![
        (a, b, "crossover_with".to_owned()),
        (b, a, "crossover_with".to_owned()),
    ];
    expect.sort();
    assert_eq!(edges(&db).await, expect);
}

#[tokio::test]
async fn create_is_idempotent_and_validates() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = mk_library(&app, &db, "lib").await;
    mk_series(&db, lib, "Alpha", "alpha").await;
    mk_series(&db, lib, "Beta", "beta").await;

    let req = serde_json::json!({"target": "alpha", "kind": "collected_in"});
    let (s1, b1) = call_json(
        &app,
        Method::POST,
        "/api/series/beta/relationships",
        &admin,
        Some(req.clone()),
    )
    .await;
    assert_eq!(s1, StatusCode::CREATED);
    let (s2, b2) = call_json(
        &app,
        Method::POST,
        "/api/series/beta/relationships",
        &admin,
        Some(req),
    )
    .await;
    assert_eq!(s2, StatusCode::OK, "duplicate returns existing with 200");
    assert_eq!(b1["id"], b2["id"]);
    assert_eq!(edges(&db).await.len(), 2);
    // Only the real create is audited.
    assert_eq!(
        audit_count(&db, "admin.series.relationship.create").await,
        1
    );

    // The same pair from the other side is also a no-op (it IS the inverse).
    let (s3, _) = call_json(
        &app,
        Method::POST,
        "/api/series/alpha/relationships",
        &admin,
        Some(serde_json::json!({"target": "beta", "kind": "collects"})),
    )
    .await;
    assert_eq!(s3, StatusCode::OK);
    assert_eq!(edges(&db).await.len(), 2);

    // Contradiction: beta collects alpha while beta collected_in alpha.
    let (s4, b4) = call_json(
        &app,
        Method::POST,
        "/api/series/beta/relationships",
        &admin,
        Some(serde_json::json!({"target": "alpha", "kind": "collects"})),
    )
    .await;
    assert_eq!(s4, StatusCode::CONFLICT, "{b4}");
    assert_eq!(b4["error"]["code"], "conflict");

    // Self edge → 422.
    let (s5, b5) = call_json(
        &app,
        Method::POST,
        "/api/series/beta/relationships",
        &admin,
        Some(serde_json::json!({"target": "beta", "kind": "see_also"})),
    )
    .await;
    assert_eq!(s5, StatusCode::UNPROCESSABLE_ENTITY, "{b5}");
    assert_eq!(b5["error"]["code"], "validation");

    // Unknown target → 404; unknown kind → 422 (serde) ; blank target → 422.
    let (s6, _) = call_json(
        &app,
        Method::POST,
        "/api/series/beta/relationships",
        &admin,
        Some(serde_json::json!({"target": "nope", "kind": "see_also"})),
    )
    .await;
    assert_eq!(s6, StatusCode::NOT_FOUND);
    let (s7, _) = call_json(
        &app,
        Method::POST,
        "/api/series/beta/relationships",
        &admin,
        Some(serde_json::json!({"target": "alpha", "kind": "sequel"})),
    )
    .await;
    assert!(s7.is_client_error(), "bad kind rejected, got {s7}");
    let (s8, _) = call_json(
        &app,
        Method::POST,
        "/api/series/beta/relationships",
        &admin,
        Some(serde_json::json!({"target": "  ", "kind": "see_also"})),
    )
    .await;
    assert_eq!(s8, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(edges(&db).await.len(), 2);
}

#[tokio::test]
async fn create_pair_helper_is_idempotent_and_db_rejects_self_edges() {
    let app = TestApp::spawn().await;
    register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = mk_library(&app, &db, "lib").await;
    let a = mk_series(&db, lib, "Alpha", "alpha").await;
    let b = mk_series(&db, lib, "Beta", "beta").await;

    let first = relationships::create_pair(
        &db,
        a,
        b,
        RelationshipKind::SpinOffOf,
        RelationshipSource::Suggested,
        Some(0.8),
        None,
    )
    .await
    .unwrap();
    assert!(first.created);
    assert_eq!(first.inverse.kind, "has_spin_off");
    assert_eq!(first.forward.source, "suggested");
    assert_eq!(first.forward.confidence, Some(0.8));
    let again = relationships::create_pair(
        &db,
        a,
        b,
        RelationshipKind::SpinOffOf,
        RelationshipSource::Manual,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(!again.created);
    assert_eq!(again.forward.id, first.forward.id);

    assert!(matches!(
        relationships::create_pair(
            &db,
            a,
            a,
            RelationshipKind::SeeAlso,
            RelationshipSource::Manual,
            None,
            None
        )
        .await,
        Err(relationships::PairError::SelfEdge)
    ));
    assert!(matches!(
        relationships::create_pair(
            &db,
            a,
            b,
            RelationshipKind::SeeAlso,
            RelationshipSource::Suggested,
            Some(1.5),
            None
        )
        .await,
        Err(relationships::PairError::InvalidConfidence)
    ));
    // The DB CHECK backs up the helper.
    let raw = rel::ActiveModel {
        id: Set(Uuid::now_v7()),
        from_series_id: Set(a),
        to_series_id: Set(a),
        kind: Set("see_also".into()),
        source: Set("manual".into()),
        confidence: Set(None),
        created_by: Set(None),
        created_at: Set(Utc::now().fixed_offset()),
    }
    .insert(&db)
    .await;
    assert!(raw.is_err(), "CHECK (from <> to) must reject self edges");

    // Deleting a series cascades both halves.
    entity::series::Entity::delete_by_id(b)
        .exec(&db)
        .await
        .unwrap();
    assert!(edges(&db).await.is_empty());
}

#[tokio::test]
async fn non_admin_cannot_write() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    demote_to_user(&db, user.user_id).await;
    let lib = mk_library(&app, &db, "lib").await;
    grant(&db, user.user_id, lib).await;
    mk_series(&db, lib, "Alpha", "alpha").await;
    mk_series(&db, lib, "Beta", "beta").await;

    let (status, _) = call_json(
        &app,
        Method::POST,
        "/api/series/beta/relationships",
        &user,
        Some(serde_json::json!({"target": "alpha", "kind": "sequel_of"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(edges(&db).await.is_empty());

    let (status, body) = call_json(
        &app,
        Method::POST,
        "/api/series/beta/relationships",
        &admin,
        Some(serde_json::json!({"target": "alpha", "kind": "sequel_of"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = body["id"].as_str().unwrap();
    let (status, _) = call(
        &app,
        Method::DELETE,
        &format!("/api/series/beta/relationships/{id}"),
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(edges(&db).await.len(), 2);

    // ...but the user can read.
    let (status, body) = call_json(
        &app,
        Method::GET,
        "/api/series/beta/relationships",
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["relationships"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn get_filters_by_library_acl_and_prunes_hidden_chain_links() {
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
    let d = mk_series(&db, open, "Delta", "delta").await;

    // alpha see_also beta (visible); alpha sequel_of hidden; hidden
    // sequel_of delta. Chain from alpha: delta(-2) → hidden(-1) → alpha.
    for (from, to, kind) in [
        (a, b, RelationshipKind::SeeAlso),
        (a, hidden, RelationshipKind::SequelOf),
        (hidden, d, RelationshipKind::SequelOf),
    ] {
        relationships::create_pair(&db, from, to, kind, RelationshipSource::Manual, None, None)
            .await
            .unwrap();
    }

    let (status, body) = call_json(
        &app,
        Method::GET,
        "/api/series/alpha/relationships",
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["relationships"].as_array().unwrap().len(), 2);
    assert_eq!(
        chain_slugs(&body),
        vec![
            (-2, "delta".to_owned()),
            (-1, "hidden".to_owned()),
            (0, "alpha".to_owned())
        ]
    );

    let (status, body) = call_json(
        &app,
        Method::GET,
        "/api/series/alpha/relationships",
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rels = body["relationships"].as_array().unwrap();
    assert_eq!(rels.len(), 1, "hidden-library series filtered: {body}");
    assert_eq!(rels[0]["series"]["slug"], "beta");
    // delta is visible but only reachable through the hidden series, so the
    // chain collapses (nothing but alpha left → empty).
    assert!(body["chain"].as_array().unwrap().is_empty(), "{body}");

    // A series the user can't see answers 404.
    let (status, _) = call_json(
        &app,
        Method::GET,
        "/api/series/hidden/relationships",
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cycles_terminate_and_each_node_appears_once() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = mk_library(&app, &db, "lib").await;
    let a = mk_series(&db, lib, "Alpha", "alpha").await;
    let b = mk_series(&db, lib, "Beta", "beta").await;
    let c = mk_series(&db, lib, "Gamma", "gamma").await;
    // a sequel_of b sequel_of c sequel_of a — a 3-cycle (plus inverses).
    for (from, to) in [(a, b), (b, c), (c, a)] {
        relationships::create_pair(
            &db,
            from,
            to,
            RelationshipKind::SequelOf,
            RelationshipSource::Manual,
            None,
            None,
        )
        .await
        .unwrap();
    }
    // Also a self-inverse see_also triangle on top.
    for (from, to) in [(a, b), (b, c), (c, a)] {
        relationships::create_pair(
            &db,
            from,
            to,
            RelationshipKind::SeeAlso,
            RelationshipSource::Manual,
            None,
            None,
        )
        .await
        .unwrap();
    }

    let all = relationships::traverse(&db, a, &RelationshipKind::ALL, 6)
        .await
        .unwrap();
    let mut ids: Vec<Uuid> = all.iter().map(|n| n.series_id).collect();
    ids.sort();
    let mut expect = vec![b, c];
    expect.sort();
    assert_eq!(ids, expect, "start excluded, each node once");
    assert!(all.iter().all(|n| n.depth == 1));

    let (status, body) = call_json(
        &app,
        Method::GET,
        "/api/series/alpha/relationships",
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let chain = chain_slugs(&body);
    let mut slugs: Vec<&str> = chain.iter().map(|(_, s)| s.as_str()).collect();
    slugs.sort();
    assert_eq!(slugs, vec!["alpha", "beta", "gamma"], "{body}");
}

#[tokio::test]
async fn traversal_depth_is_capped_at_six() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = mk_library(&app, &db, "lib").await;
    // s0 ← s1 ← … ← s9 (each s(i+1) sequel_of s(i)).
    let mut ids = Vec::new();
    for i in 0..10 {
        ids.push(mk_series(&db, lib, &format!("Vol {i}"), &format!("vol-{i}")).await);
    }
    for w in ids.windows(2) {
        relationships::create_pair(
            &db,
            w[1],
            w[0],
            RelationshipKind::SequelOf,
            RelationshipSource::Manual,
            None,
            None,
        )
        .await
        .unwrap();
    }

    // Asking for more than the cap is clamped to 6.
    let reached = relationships::traverse(&db, ids[0], &[RelationshipKind::PrequelOf], 50)
        .await
        .unwrap();
    assert_eq!(reached.len(), 6);
    assert_eq!(reached.last().unwrap().depth, 6);
    assert_eq!(reached.last().unwrap().series_id, ids[6]);
    assert_eq!(reached[0].parent_id, ids[0]);

    // Chain from the middle: 6 back is limited by the start, 6 forward by
    // the cap.
    let (status, body) = call_json(
        &app,
        Method::GET,
        "/api/series/vol-0/relationships",
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let chain = chain_slugs(&body);
    assert_eq!(chain.len(), 7, "vol-0 plus six sequels: {chain:?}");
    assert_eq!(chain.first().unwrap(), &(0, "vol-0".to_owned()));
    assert_eq!(chain.last().unwrap(), &(6, "vol-6".to_owned()));

    let (_, body) = call_json(
        &app,
        Method::GET,
        "/api/series/vol-9/relationships",
        &admin,
        None,
    )
    .await;
    let chain = chain_slugs(&body);
    assert_eq!(chain.first().unwrap(), &(-6, "vol-3".to_owned()));
    assert_eq!(chain.last().unwrap(), &(0, "vol-9".to_owned()));
}

#[tokio::test]
async fn opds_series_feeds_carry_related_links() {
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
    relationships::create_pair(
        &db,
        b,
        a,
        RelationshipKind::SequelOf,
        RelationshipSource::Manual,
        None,
        None,
    )
    .await
    .unwrap();
    relationships::create_pair(
        &db,
        b,
        hidden,
        RelationshipKind::SeeAlso,
        RelationshipSource::Manual,
        None,
        None,
    )
    .await
    .unwrap();

    let (status, xml) = call(
        &app,
        Method::GET,
        &format!("/opds/v1/series/{b}"),
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{xml}");
    assert!(
        xml.contains(&format!(
            r#"<link rel="related" href="/opds/v1/series/{a}""#
        )),
        "{xml}"
    );
    assert!(xml.contains("Sequel of: Alpha (2020)"), "{xml}");
    assert!(!xml.contains(&hidden.to_string()), "hidden series leaked");

    let (status, body) = call_json(
        &app,
        Method::GET,
        &format!("/opds/v2/series/{b}"),
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
    assert_eq!(related[0]["href"], format!("/opds/v2/series/{a}"));

    // Admin sees both.
    let (_, xml) = call(
        &app,
        Method::GET,
        &format!("/opds/v1/series/{b}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(xml.matches(r#"rel="related""#).count(), 2, "{xml}");
}
