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
        .map(|r| {
            (
                r.from_series_id,
                r.to_series_id.expect("series edge"),
                r.kind,
            )
        })
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
        (a, b, "has_sequel".to_owned()),
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
    assert_eq!(rels[0]["kind"], "has_sequel");
    assert_eq!(rels[0]["kind_label"], "Has sequel");
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
        to_series_id: Set(Some(a)),
        to_arc_id: Set(None),
        kind: Set("see_also".into()),
        source: Set("manual".into()),
        confidence: Set(None),
        created_by: Set(None),
        created_at: Set(Utc::now().fixed_offset()),
        from_range: Set(None),
        to_range: Set(None),
        coverage: Set(None),
        qualifier: Set(None),
        note: Set(None),
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
    let reached = relationships::traverse(&db, ids[0], &[RelationshipKind::HasSequel], 50)
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
    assert!(xml.contains("Sequel to: Alpha (2020)"), "{xml}");
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

async fn row(db: &DatabaseConnection, from: Uuid, to: Uuid) -> rel::Model {
    rel::Entity::find()
        .filter(rel::Column::FromSeriesId.eq(from))
        .filter(rel::Column::ToSeriesId.eq(to))
        .one(db)
        .await
        .unwrap()
        .expect("relationship row")
}

fn error_fields(body: &serde_json::Value) -> Vec<String> {
    body["error"]["details"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|f| f["field"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn kind_catalogue_lists_every_kind_with_groups() {
    let app = TestApp::spawn().await;
    let user = register(&app, "admin@example.com").await;
    let (status, body) = call_json(&app, Method::GET, "/api/relationship-kinds", &user, None).await;
    assert_eq!(status, StatusCode::OK);
    let groups: Vec<&str> = body["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        groups,
        vec![
            "Story",
            "Publication history",
            "Editions & contents",
            "Advanced"
        ]
    );
    let kinds = body["kinds"].as_array().unwrap();
    assert_eq!(kinds.len(), RelationshipKind::ALL.len());
    let k = |name: &str| {
        kinds
            .iter()
            .find(|k| k["kind"] == name)
            .unwrap_or_else(|| panic!("{name} missing"))
            .clone()
    };
    assert_eq!(k("sequel_of")["inverse"], "has_sequel");
    assert_eq!(k("prequel_of")["inverse"], "has_prequel");
    assert_eq!(k("continues")["inverse_label"], "Continued by");
    assert_eq!(k("continues")["group"], "publication");
    assert_eq!(k("continues")["qualifiers"].as_array().unwrap().len(), 5);
    assert_eq!(k("tie_in_to")["allows_arc_target"], true);
    assert_eq!(k("collected_in")["allows_coverage"], true);
    assert_eq!(k("see_also")["symmetric"], true);
    assert_eq!(k("alternate_edition_of")["symmetric"], true);
    assert_eq!(k("reimagined_as")["group"], "advanced");
}

#[tokio::test]
async fn scope_is_validated_per_kind_and_mirrored_on_the_inverse() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = mk_library(&app, &db, "lib").await;
    let a = mk_series(&db, lib, "Alpha", "alpha").await;
    let tpb = mk_series(&db, lib, "Alpha TPB", "alpha-tpb").await;
    let b = mk_series(&db, lib, "Beta", "beta").await;
    mk_arc(&db, "Event", "event").await;

    let post = |body: serde_json::Value| {
        let app = &app;
        let admin = &admin;
        async move {
            call_json(
                app,
                Method::POST,
                "/api/series/alpha-tpb/relationships",
                admin,
                Some(body),
            )
            .await
        }
    };
    let cases = [
        (
            serde_json::json!({"target": "alpha", "kind": "sequel_of", "coverage": "full"}),
            "coverage",
        ),
        (
            serde_json::json!({"target": "alpha", "kind": "tie_in_to", "qualifier": "relaunch"}),
            "qualifier",
        ),
        (
            serde_json::json!({"target": "alpha", "kind": "see_also", "qualifier": "prelude"}),
            "qualifier",
        ),
        (
            serde_json::json!({"target": "alpha", "kind": "collects", "from_range": "1".repeat(101)}),
            "from_range",
        ),
        (
            serde_json::json!({"target": "alpha", "kind": "collects", "note": "x".repeat(501)}),
            "note",
        ),
        (
            serde_json::json!({"target_arc": "event", "kind": "sequel_of"}),
            "kind",
        ),
        (
            serde_json::json!({"target": "alpha", "target_arc": "event", "kind": "tie_in_to"}),
            "target_arc",
        ),
        (serde_json::json!({"kind": "see_also"}), "target"),
    ];
    for (body, want) in cases {
        let (status, resp) = post(body.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body} → {resp}");
        assert_eq!(resp["error"]["code"], "validation", "{resp}");
        assert_eq!(
            error_fields(&resp),
            vec![want.to_owned()],
            "{body} → {resp}"
        );
    }
    let (status, _) =
        post(serde_json::json!({"target": "alpha", "kind": "collects", "coverage": "most"})).await;
    assert!(status.is_client_error(), "unknown coverage value rejected");
    assert!(
        edges(&db).await.is_empty(),
        "nothing written by a refused POST"
    );

    // A collected edition with ranges + coverage + note.
    let (status, body) = post(serde_json::json!({
        "target": "alpha", "kind": "collects",
        "from_range": " 1 ", "to_range": "1-6,Annual 1", "coverage": "partial",
        "note": "Vol. 1", "qualifier": null
    }))
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["coverage"], "partial");
    assert_eq!(body["from_range"], "1", "ranges are trimmed");
    assert_eq!(body["group"], "editions");
    let fwd = row(&db, tpb, a).await;
    let inv = row(&db, a, tpb).await;
    assert_eq!(inv.kind, "collected_in");
    assert_eq!(inv.from_range.as_deref(), Some("1-6,Annual 1"));
    assert_eq!(inv.to_range.as_deref(), Some("1"));
    assert_eq!(inv.coverage, fwd.coverage);
    assert_eq!(inv.note.as_deref(), Some("Vol. 1"));

    // A continuation with a qualifier; the inverse keeps it.
    let (status, body) = call_json(
        &app,
        Method::POST,
        "/api/series/beta/relationships",
        &admin,
        Some(serde_json::json!({"target": "alpha", "kind": "continues", "qualifier": "relaunch"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["qualifier"], "relaunch");
    assert_eq!(body["qualifier_label"], "Relaunch");
    assert_eq!(row(&db, a, b).await.kind, "continued_by");
    assert_eq!(row(&db, a, b).await.qualifier.as_deref(), Some("relaunch"));

    // The DB CHECKs back the API up.
    for (kind, col, val) in [
        ("sequel_of", "coverage", "full"),
        ("see_also", "qualifier", "relaunch"),
        ("continues", "qualifier", "prelude"),
    ] {
        let res = exec_raw(
            &db,
            &format!(
                "INSERT INTO series_relationship (id, from_series_id, to_series_id, kind, {col}) \
                 VALUES ($1, $2, $3, $4, $5)"
            ),
            vec![
                Uuid::now_v7().into(),
                b.into(),
                tpb.into(),
                kind.into(),
                val.into(),
            ],
        )
        .await;
        assert!(res.is_err(), "CHECK must reject {kind} {col}={val}");
    }
}

#[tokio::test]
async fn patch_keeps_the_pair_in_sync_and_is_audited() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    demote_to_user(&db, user.user_id).await;
    let lib = mk_library(&app, &db, "lib").await;
    grant(&db, user.user_id, lib).await;
    let a = mk_series(&db, lib, "Alpha", "alpha").await;
    let tpb = mk_series(&db, lib, "Alpha TPB", "alpha-tpb").await;

    let (status, body) = call_json(
        &app,
        Method::POST,
        "/api/series/alpha-tpb/relationships",
        &admin,
        Some(serde_json::json!({"target": "alpha", "kind": "collects", "to_range": "1-6", "coverage": "full"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = body["id"].as_str().unwrap().to_owned();
    let path = format!("/api/series/alpha-tpb/relationships/{id}");

    // Non-admin: 403.
    let (status, _) = call_json(
        &app,
        Method::PATCH,
        &path,
        &user,
        Some(serde_json::json!({"note": "nope"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Scope edit: note added, coverage kept (omitted), to_range changed.
    let generation = app.state().similarity.generation();
    let (status, body) = call_json(
        &app,
        Method::PATCH,
        &path,
        &admin,
        Some(serde_json::json!({"note": "Deluxe", "to_range": "1-12"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], id, "a scope edit keeps the row");
    assert_eq!(body["coverage"], "full");
    assert_eq!(body["note"], "Deluxe");
    let inv = row(&db, a, tpb).await;
    assert_eq!(inv.note.as_deref(), Some("Deluxe"));
    assert_eq!(
        inv.from_range.as_deref(),
        Some("1-12"),
        "mirrored onto the inverse"
    );
    assert_eq!(inv.coverage.as_deref(), Some("full"));
    assert!(app.state().similarity.generation() > generation);
    assert_eq!(
        audit_count(&db, "admin.series.relationship.update").await,
        1
    );

    // Edit through the inverse half, from the other series' side.
    let (status, body) = call_json(
        &app,
        Method::PATCH,
        &format!("/api/series/alpha/relationships/{}", inv.id),
        &admin,
        Some(serde_json::json!({"coverage": "partial"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["kind"], "collected_in");
    assert_eq!(row(&db, tpb, a).await.coverage.as_deref(), Some("partial"));

    // Kind change into a kind the scope doesn't fit → 422, nothing changes.
    let (status, body) = call_json(
        &app,
        Method::PATCH,
        &path,
        &admin,
        Some(serde_json::json!({"kind": "sequel_of"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(error_fields(&body), vec!["coverage"]);
    assert_eq!(row(&db, tpb, a).await.id.to_string(), id);

    // Kind change collects → reprints: delete + create, scope carried.
    let (status, body) = call_json(
        &app,
        Method::PATCH,
        &path,
        &admin,
        Some(serde_json::json!({"kind": "reprints"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["kind"], "reprints");
    assert_ne!(body["id"], id, "kind change re-creates the pair");
    assert_eq!(body["note"], "Deluxe");
    let mut want = vec![
        (tpb, a, "reprints".to_owned()),
        (a, tpb, "reprinted_in".to_owned()),
    ];
    want.sort();
    assert_eq!(edges(&db).await, want);
    assert_eq!(row(&db, a, tpb).await.from_range.as_deref(), Some("1-12"));
    let new_id = body["id"].as_str().unwrap().to_owned();

    // Kind change clearing coverage → see_also.
    let (status, body) = call_json(
        &app,
        Method::PATCH,
        &format!("/api/series/alpha-tpb/relationships/{new_id}"),
        &admin,
        Some(serde_json::json!({"kind": "see_also", "coverage": null, "note": null})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["coverage"], serde_json::Value::Null);
    assert_eq!(body["note"], serde_json::Value::Null);
    let see_also_id = body["id"].as_str().unwrap().to_owned();

    // Duplicate: alpha-tpb also sequel_of alpha; turning see_also into
    // sequel_of collides → 409, the see_also pair survives (rolled back).
    relationships::create_pair(
        &db,
        tpb,
        a,
        RelationshipKind::SequelOf,
        RelationshipSource::Manual,
        None,
        None,
    )
    .await
    .unwrap();
    let (status, body) = call_json(
        &app,
        Method::PATCH,
        &format!("/api/series/alpha-tpb/relationships/{see_also_id}"),
        &admin,
        Some(serde_json::json!({"kind": "sequel_of"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(edges(&db).await.len(), 4);
    // Contradiction: see_also → has_sequel while sequel_of exists → 409.
    let (status, _) = call_json(
        &app,
        Method::PATCH,
        &format!("/api/series/alpha-tpb/relationships/{see_also_id}"),
        &admin,
        Some(serde_json::json!({"kind": "continued_by"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(edges(&db).await.len(), 4);

    // Unknown id → 404; malformed → 400.
    let (status, _) = call_json(
        &app,
        Method::PATCH,
        &format!("/api/series/alpha/relationships/{}", Uuid::now_v7()),
        &admin,
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call_json(
        &app,
        Method::PATCH,
        "/api/series/alpha/relationships/nope",
        &admin,
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        audit_count(&db, "admin.series.relationship.update").await,
        4
    );
}

#[tokio::test]
async fn arc_targets_are_one_directional_and_acl_filtered() {
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
    // `event` appears in the open library; `secret` only in the closed one.
    let event = mk_arc(&db, "Event", "event").await;
    let secret = mk_arc(&db, "Secret", "secret").await;
    series_arc(&db, a, event).await;
    series_arc(&db, hidden, secret).await;

    let tie = |slug: &'static str, arc: &'static str, extra: serde_json::Value| {
        let (app, admin) = (&app, &admin);
        async move {
            let mut body = serde_json::json!({"target_arc": arc, "kind": "tie_in_to"});
            for (k, v) in extra.as_object().unwrap() {
                body[k] = v.clone();
            }
            call_json(
                app,
                Method::POST,
                &format!("/api/series/{slug}/relationships"),
                admin,
                Some(body),
            )
            .await
        }
    };
    let (status, body) = tie(
        "alpha",
        "event",
        serde_json::json!({"qualifier": "prelude"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["kind_label"], "Prelude to");
    assert_eq!(body["arc"]["slug"], "event");
    let (status, _) = tie("alpha", "event", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK, "idempotent");
    let (status, _) = tie("beta", "event", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = tie("hidden", "event", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = tie("alpha", "secret", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = tie("alpha", "nope", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // One row per arc edge: no inverse half.
    let arc_rows = rel::Entity::find()
        .filter(rel::Column::ToArcId.is_not_null())
        .all(&db)
        .await
        .unwrap();
    assert_eq!(arc_rows.len(), 4);
    assert_eq!(rel::Entity::find().count(&db).await.unwrap(), 4);
    assert!(arc_rows.iter().all(|r| r.to_series_id.is_none()));

    // Constraints: exactly one target, arc only for tie_in_to, unique.
    let both = exec_raw(
        &db,
        "INSERT INTO series_relationship (id, from_series_id, to_series_id, to_arc_id, kind) \
         VALUES ($1, $2, $3, $4, 'tie_in_to')",
        vec![Uuid::now_v7().into(), b.into(), a.into(), event.into()],
    )
    .await;
    assert!(both.is_err(), "both targets must be rejected");
    let neither = exec_raw(
        &db,
        "INSERT INTO series_relationship (id, from_series_id, kind) VALUES ($1, $2, 'see_also')",
        vec![Uuid::now_v7().into(), b.into()],
    )
    .await;
    assert!(neither.is_err(), "no target must be rejected");
    for kind in ["see_also", "crossover_with", "tie_in_to"] {
        // see_also / crossover_with: not arc-capable; tie_in_to: duplicate.
        let res = exec_raw(
            &db,
            "INSERT INTO series_relationship (id, from_series_id, to_arc_id, kind) \
             VALUES ($1, $2, $3, $4)",
            vec![Uuid::now_v7().into(), b.into(), event.into(), kind.into()],
        )
        .await;
        assert!(res.is_err(), "{kind} arc edge must be rejected");
    }

    // The user sees `event` on alpha's page, not `secret`.
    let (status, body) = call_json(
        &app,
        Method::GET,
        "/api/series/alpha/relationships",
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let arcs: Vec<&str> = body["arcs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["arc"]["slug"].as_str().unwrap())
        .collect();
    assert_eq!(arcs, vec!["event"]);
    assert_eq!(body["arcs"][0]["qualifier"], "prelude");
    assert!(body["relationships"].as_array().unwrap().is_empty());
    let (_, body) = call_json(
        &app,
        Method::GET,
        "/api/series/alpha/relationships",
        &admin,
        None,
    )
    .await;
    assert_eq!(body["arcs"].as_array().unwrap().len(), 2, "admin sees both");

    // Arc page tie-ins: the user sees alpha + beta (not hidden), paginated.
    let (status, body) = call_json(
        &app,
        Method::GET,
        "/api/arcs/event/tie-ins?limit=1",
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 2);
    assert_eq!(body["items"][0]["series"]["slug"], "alpha");
    assert_eq!(body["items"][0]["kind_label"], "Prelude to");
    let cursor = body["next_cursor"]
        .as_str()
        .expect("second page")
        .to_owned();
    let (status, body) = call_json(
        &app,
        Method::GET,
        &format!("/api/arcs/event/tie-ins?limit=1&cursor={cursor}"),
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"][0]["series"]["slug"], "beta");
    assert!(body["next_cursor"].is_null());
    assert!(body.get("total").is_none(), "total on the first page only");
    let (_, body) = call_json(&app, Method::GET, "/api/arcs/event/tie-ins", &admin, None).await;
    assert_eq!(body["total"], 3, "admin sees the closed library too");
    // An arc the user can't see is a 404, like `/arcs/{slug}`.
    let (status, _) = call_json(&app, Method::GET, "/api/arcs/secret/tie-ins", &user, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call_json(
        &app,
        Method::GET,
        "/api/arcs/event/tie-ins?cursor=garbage",
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The helpers agree.
    assert_eq!(relationships::arc_edges(&db, a).await.unwrap().len(), 2);
    assert_eq!(
        relationships::arc_tie_ins(&db, event).await.unwrap().len(),
        3
    );
    assert!(relationships::direct(&db, a).await.unwrap().is_empty());

    // PATCH an arc edge: role changes in place; a non-arc kind is refused.
    let alpha_event = arc_rows
        .iter()
        .find(|r| r.from_series_id == a && r.to_arc_id == Some(event))
        .unwrap()
        .id;
    let (status, body) = call_json(
        &app,
        Method::PATCH,
        &format!("/api/series/alpha/relationships/{alpha_event}"),
        &admin,
        Some(serde_json::json!({"qualifier": "aftermath"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["kind_label"], "Aftermath of");
    let (status, body) = call_json(
        &app,
        Method::PATCH,
        &format!("/api/series/alpha/relationships/{alpha_event}"),
        &admin,
        Some(serde_json::json!({"kind": "see_also", "qualifier": null})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    // DELETE removes the single row; deleting the arc cascades the rest.
    let (status, _) = call(
        &app,
        Method::DELETE,
        &format!("/api/series/alpha/relationships/{alpha_event}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(rel::Entity::find().count(&db).await.unwrap(), 3);
    exec_raw(
        &db,
        "DELETE FROM story_arc WHERE id = $1",
        vec![event.into()],
    )
    .await
    .unwrap();
    assert_eq!(rel::Entity::find().count(&db).await.unwrap(), 1);
}

#[tokio::test]
async fn chain_walks_mixed_continues_and_sequels_but_not_prequels() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = mk_library(&app, &db, "lib").await;
    let v1 = mk_series(&db, lib, "Run 1", "run-1").await;
    let v2 = mk_series(&db, lib, "Run 2", "run-2").await;
    let v3 = mk_series(&db, lib, "Run 3", "run-3").await;
    let v4 = mk_series(&db, lib, "Run 4", "run-4").await;
    let origin = mk_series(&db, lib, "Origin", "origin").await;
    // v2 continues v1; v3 sequel_of v2; v4 continues v3; origin is a
    // narrative prequel of v2 (written later, set earlier).
    for (from, to, kind) in [
        (v2, v1, RelationshipKind::Continues),
        (v3, v2, RelationshipKind::SequelOf),
        (v4, v3, RelationshipKind::Continues),
        (origin, v2, RelationshipKind::PrequelOf),
    ] {
        relationships::create_pair(&db, from, to, kind, RelationshipSource::Manual, None, None)
            .await
            .unwrap();
    }
    let (status, body) = call_json(
        &app,
        Method::GET,
        "/api/series/run-2/relationships",
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        chain_slugs(&body),
        vec![
            (-1, "run-1".to_owned()),
            (0, "run-2".to_owned()),
            (1, "run-3".to_owned()),
            (2, "run-4".to_owned()),
        ],
        "mixed continues + sequel chain; the prequel stays out"
    );
    let kinds: Vec<(&str, &str)> = body["relationships"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["kind"].as_str().unwrap(), r["group"].as_str().unwrap()))
        .collect();
    assert!(kinds.contains(&("continues", "publication")), "{kinds:?}");
    assert!(kinds.contains(&("has_sequel", "story")), "{kinds:?}");
    assert!(kinds.contains(&("has_prequel", "story")), "{kinds:?}");

    // From the far end the whole run is read before it.
    let chain = relationships::chain(&db, v4).await.unwrap();
    let ids: Vec<(i32, Uuid)> = chain.iter().map(|n| (n.position, n.series_id)).collect();
    assert_eq!(ids, vec![(-3, v1), (-2, v2), (-1, v3), (0, v4)]);
    // The prequel's own chain is empty (prequel_of isn't a chain edge).
    assert!(relationships::chain(&db, origin).await.unwrap().is_empty());

    // `v2 has_sequel v1` contradicts `v2 continues v1` across families.
    let err = relationships::create_pair(
        &db,
        v2,
        v1,
        RelationshipKind::HasSequel,
        RelationshipSource::Manual,
        None,
        None,
    )
    .await;
    assert!(matches!(
        err,
        Err(relationships::PairError::Conflict {
            existing: RelationshipKind::Continues
        })
    ));
}

/// Range hygiene: a chain node whose issues a provider files under another
/// provider series carries that boundary as a sub-step — "#600–611
/// continue as Fantastic Four (2012)" — merged across providers, linked to
/// the local series matched to that provider series only when the caller
/// can see it. The local series stays one node.
#[tokio::test]
async fn chain_nodes_carry_provider_range_boundaries() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let user = register(&app, "user@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    demote_to_user(&db, user.user_id).await;
    let lib = mk_library(&app, &db, "ff").await;
    let other = mk_library(&app, &db, "relaunches").await;
    grant(&db, user.user_id, lib).await;
    let ff98 = mk_series(&db, lib, "Fantastic Four", "ff-1998").await;
    let ff01 = mk_series(&db, lib, "Fantastic Four 2001", "ff-2001").await;
    let ff12 = mk_series(&db, other, "Fantastic Four 2012", "ff-2012").await;
    relationships::create_pair(
        &db,
        ff01,
        ff98,
        RelationshipKind::Continues,
        RelationshipSource::Manual,
        None,
        None,
    )
    .await
    .unwrap();
    let dir = app._data_dir.path().join("ff");
    for n in [1.0, 2.0, 600.0, 611.0] {
        let p = dir.join(format!("ff-{n}.cbz"));
        common::seed::IssueSeed::new(lib, ff01, &p, format!("ff {n}").as_bytes(), n)
            .insert(&db)
            .await;
    }
    for (src, pid) in [("metron", "1713"), ("gcd", "9000")] {
        exec_raw(
            &db,
            "INSERT INTO series_provider_range (series_id, source, provider_series_id, provider_series_url, provider_series_name, declared_year, range_low, range_high, set_by) \
             VALUES ($1, $2, $3, $4, 'Fantastic Four', 2012, '600', '611', 'cross_reference')",
            vec![
                ff01.into(),
                src.into(),
                pid.into(),
                format!("https://example.test/{src}/{pid}").into(),
            ],
        )
        .await
        .unwrap();
    }
    exec_raw(
        &db,
        "INSERT INTO external_ids (entity_type, entity_id, source, external_id, set_by, first_set_at, last_synced_at) \
         VALUES ('series', $1, 'metron', '1713', 'metron', now(), now())",
        vec![ff12.to_string().into()],
    )
    .await
    .unwrap();

    let get = |who: &'static str| {
        let app = &app;
        let auth = if who == "admin" { &admin } else { &user };
        async move {
            let (status, body) = call_json(
                app,
                Method::GET,
                "/api/series/ff-1998/relationships",
                auth,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            body
        }
    };
    let body = get("admin").await;
    assert_eq!(
        chain_slugs(&body),
        vec![(0, "ff-1998".to_owned()), (1, "ff-2001".to_owned())],
        "still one node per local series"
    );
    let node = |body: &serde_json::Value, slug: &str| {
        body["chain"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["series"]["slug"] == slug)
            .unwrap()
            .clone()
    };
    assert_eq!(
        node(&body, "ff-1998")["provider_splits"],
        serde_json::json!([])
    );
    let splits = node(&body, "ff-2001")["provider_splits"].clone();
    assert_eq!(
        splits.as_array().unwrap().len(),
        1,
        "merged across providers: {splits}"
    );
    let s = &splits[0];
    assert_eq!(s["label"], "#600–611 continue as Fantastic Four (2012)");
    assert_eq!(s["position"], "end");
    let sources: Vec<&str> = s["providers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["source"].as_str().unwrap())
        .collect();
    assert_eq!(sources, vec!["gcd", "metron"]);
    assert_eq!(s["providers"][1]["source_label"], "Metron");
    assert_eq!(s["providers"][1]["url"], "https://example.test/metron/1713");
    assert_eq!(s["local_series"]["slug"], "ff-2012");

    // The relaunch lives in a library the user can't see: no link.
    let body = get("user").await;
    let s = node(&body, "ff-2001")["provider_splits"][0].clone();
    assert_eq!(s["label"], "#600–611 continue as Fantastic Four (2012)");
    assert!(s["local_series"].is_null(), "{s}");
}
