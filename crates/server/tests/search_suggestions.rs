//! U-11: a series search (`/api/series?q=`) whose first page finds nothing
//! carries `suggestions` — the closest series names the caller can see,
//! from the `series_normalized_name_trgm` index.
//!
//! Visibility is the hard requirement: a suggestion passes the same
//! library-grant, age-cap and soft-removal rules the list itself applies,
//! and a caller with nothing suggestible gets the same body as a search
//! where nothing similar exists.

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
    user::Entity as UserEntity,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, Database, DatabaseConnection, DbBackend,
    EntityTrait, QueryFilter, QueryTrait, Set, TransactionTrait, Unchanged,
};
use server::api::series::{
    SUGGESTION_LIMIT, SUGGESTION_WORD_SIMILARITY_SQL, apply_series_visibility,
    series_suggestion_select,
};
use server::library::access::VisibleLibraries;
use std::collections::{HashMap, HashSet};
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
        .expect("user row by email");
    Authed {
        session,
        csrf,
        user_id: user_row.id,
    }
}

async fn demote_to_user(app: &TestApp, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    entity::user::ActiveModel {
        id: Unchanged(user_id),
        role: Set(entity::user::UserRole::User),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();
}

async fn get_json(app: &TestApp, path: &str, user: &Authed) -> serde_json::Value {
    let req = Request::builder()
        .method(Method::GET)
        .uri(path)
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
    assert_eq!(resp.status(), StatusCode::OK, "GET {path}");
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// `/api/series?q=<q>` with the query URL-encoded.
fn search_uri(q: &str) -> String {
    format!("/api/series?q={}", q.replace(' ', "%20"))
}

fn suggestion_names(body: &serde_json::Value) -> Vec<String> {
    body["suggestions"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| s["name"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

async fn seed_library(db: &DatabaseConnection, name: &str) -> Uuid {
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("Lib {name}")),
        root_path: Set(format!("/tmp/{name}-{lib_id}")),
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
        auto_search_new_series: Set(false),
        show_in_sidebar_by_default: Set(true),
    }
    .insert(db)
    .await
    .unwrap();
    lib_id
}

fn series_am(lib_id: Uuid, name: &str, rating: Option<&str>, removed: bool) -> SeriesAM {
    let now = Utc::now().fixed_offset();
    let id = Uuid::now_v7();
    SeriesAM {
        id: Set(id),
        library_id: Set(lib_id),
        name: Set(name.into()),
        normalized_name: Set(normalize_name(name)),
        year: Set(Some(2020)),
        volume: Set(None),
        publisher: Set(None),
        imprint: Set(None),
        status: Set("continuing".into()),
        total_issues: Set(None),
        age_rating: Set(rating.map(str::to_owned)),
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
        slug: Set(format!("s-{id}")),
        alternate_names: Set(serde_json::json!([])),
        created_at: Set(now),
        updated_at: Set(now),
        folder_path: Set(None),
        last_scanned_at: Set(None),
        match_key: Set(None),
        removed_at: Set(removed.then_some(now)),
        removal_confirmed_at: Set(None),
        status_user_set_at: Set(None),
        reading_direction: Set(None),
        text_language: Set(None),
        preserve_canonical_order: Set(false),
    }
}

async fn seed_series(
    db: &DatabaseConnection,
    lib_id: Uuid,
    name: &str,
    rating: Option<&str>,
    removed: bool,
) -> Uuid {
    series_am(lib_id, name, rating, removed)
        .insert(db)
        .await
        .unwrap()
        .id
}

async fn grant(db: &DatabaseConnection, user_id: Uuid, library_id: Uuid, cap: Option<&str>) {
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        user_id: Set(user_id),
        library_id: Set(library_id),
        age_rating_max: Set(cap.map(str::to_owned)),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

/// A one-letter typo the main search misses yields the series; nonsense
/// yields no `suggestions` key at all.
#[tokio::test]
async fn typo_suggests_the_series_and_nonsense_suggests_nothing() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@suggest.test").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = seed_library(&db, "typo").await;
    seed_series(
        &db,
        lib,
        "Batman The Long Halloween Absolute Edition",
        None,
        false,
    )
    .await;
    seed_series(&db, lib, "The Sandman Overture Deluxe Edition", None, false).await;

    // One word of a long name, one letter dropped.
    let body = get_json(&app, &search_uri("sandmn"), &admin).await;
    assert_eq!(body["items"], serde_json::json!([]), "main search misses");
    assert_eq!(body["total"], 0);
    assert_eq!(
        suggestion_names(&body),
        vec!["The Sandman Overture Deluxe Edition"]
    );

    // One letter substituted.
    let body = get_json(&app, &search_uri("batnan"), &admin).await;
    assert_eq!(body["total"], 0, "main search misses: {body}");
    assert_eq!(
        suggestion_names(&body),
        vec!["Batman The Long Halloween Absolute Edition"]
    );

    // Nonsense: no suggestions field, the plain empty page.
    let body = get_json(&app, &search_uri("zzzzqq"), &admin).await;
    assert_eq!(
        body,
        serde_json::json!({"items": [], "next_cursor": null, "total": 0})
    );
}

/// Shape: the list is objects with only `name`, capped at
/// `SUGGESTION_LIMIT`, deduplicated by name, and absent on any search that
/// found something.
#[tokio::test]
async fn suggestions_are_bounded_deduplicated_and_only_on_zero_hits() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@shape.test").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_a = seed_library(&db, "shape-a").await;
    let lib_b = seed_library(&db, "shape-b").await;
    for (lib, name) in [
        (lib_a, "Hellboy in Hell Library Edition Volume One"),
        // Same name in a second library: one suggestion, not two.
        (lib_b, "Hellboy in Hell Library Edition Volume One"),
        (lib_a, "Hellboy and the BPRD Library Edition Volume Two"),
        (lib_a, "Hellboy The Crooked Man and Others Collection"),
        (lib_a, "Hellboy Universe Essentials Collection Volume Four"),
        (lib_a, "Hellboy Weird Tales Omnibus Collection Volume Five"),
    ] {
        seed_series(&db, lib, name, None, false).await;
    }

    let body = get_json(&app, &search_uri("helboy"), &admin).await;
    assert_eq!(body["total"], 0, "main search misses: {body}");
    let suggestions = body["suggestions"].as_array().expect("suggestions");
    assert_eq!(suggestions.len() as u64, SUGGESTION_LIMIT);
    for s in suggestions {
        let obj = s.as_object().unwrap();
        assert_eq!(obj.len(), 1, "only `name`: {s}");
        assert!(obj["name"].as_str().unwrap().starts_with("Hellboy"));
    }
    let names: HashSet<String> = suggestion_names(&body).into_iter().collect();
    assert_eq!(names.len(), suggestions.len(), "deduplicated");

    // A search that finds something carries no suggestions.
    let body = get_json(&app, &search_uri("hellboy"), &admin).await;
    assert!(body["total"].as_i64().unwrap() > 0);
    assert!(body.get("suggestions").is_none(), "{body}");
}

/// One- and two-character queries never ask for suggestions, even when the
/// trigram match would find one.
#[tokio::test]
async fn short_queries_get_no_suggestions() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@floor.test").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = seed_library(&db, "floor").await;
    seed_series(
        &db,
        lib,
        "Xavier Institute Alumni Yearbook Special",
        None,
        false,
    )
    .await;

    // word_similarity("xa", "xavier …") is 2/3, over the threshold — only
    // the length floor keeps it out.
    for q in ["x", "xa", "x-a"] {
        let body = get_json(&app, &search_uri(q), &admin).await;
        assert_eq!(body["total"], 0, "{q}: main search misses: {body}");
        assert!(body.get("suggestions").is_none(), "{q}: {body}");
    }
}

/// A reader with no grant on library B is never offered B's series, and
/// gets exactly the body a search with nothing similar gets.
#[tokio::test]
async fn suggestions_respect_library_grants() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@acl.test").await;
    let reader = register(&app, "reader@acl.test").await;
    demote_to_user(&app, reader.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_a = seed_library(&db, "acl-a").await;
    let lib_b = seed_library(&db, "acl-b").await;
    grant(&db, reader.user_id, lib_a, None).await;
    seed_series(
        &db,
        lib_a,
        "Batman The Long Halloween Absolute Edition",
        None,
        false,
    )
    .await;
    seed_series(
        &db,
        lib_b,
        "The Invisible Kingdom Deluxe Edition",
        None,
        false,
    )
    .await;

    // Positive control: the admin is offered it.
    let body = get_json(&app, &search_uri("invisble"), &admin).await;
    assert_eq!(body["total"], 0, "main search misses: {body}");
    assert_eq!(
        suggestion_names(&body),
        vec!["The Invisible Kingdom Deluxe Edition"]
    );

    let hidden = get_json(&app, &search_uri("invisble"), &reader).await;
    let nothing = get_json(&app, &search_uri("zzzzqq"), &reader).await;
    assert_eq!(
        hidden,
        serde_json::json!({"items": [], "next_cursor": null, "total": 0})
    );
    assert_eq!(hidden, nothing, "indistinguishable from nothing similar");

    // The reader is still offered what they can see.
    let body = get_json(&app, &search_uri("batnan"), &reader).await;
    assert_eq!(
        suggestion_names(&body),
        vec!["Batman The Long Halloween Absolute Edition"]
    );

    // A reader with no grant at all gets the plain empty page.
    let stranger = register(&app, "stranger@acl.test").await;
    demote_to_user(&app, stranger.user_id).await;
    let body = get_json(&app, &search_uri("batnan"), &stranger).await;
    assert_eq!(
        body,
        serde_json::json!({"items": [], "next_cursor": null, "total": 0})
    );
}

/// A capped reader is never offered a series rated above the cap.
#[tokio::test]
async fn suggestions_respect_the_age_rating_cap() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@cap.test").await;
    let capped = register(&app, "capped@cap.test").await;
    demote_to_user(&app, capped.user_id).await;
    let uncapped = register(&app, "uncapped@cap.test").await;
    demote_to_user(&app, uncapped.user_id).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = seed_library(&db, "cap").await;
    grant(&db, capped.user_id, lib, Some("Teen")).await;
    grant(&db, uncapped.user_id, lib, None).await;
    seed_series(
        &db,
        lib,
        "Wolverine Origins Ultimate Collection",
        Some("Mature 17+"),
        false,
    )
    .await;
    seed_series(
        &db,
        lib,
        "Teen Titans Go Big Collection",
        Some("Teen"),
        false,
    )
    .await;

    let body = get_json(&app, &search_uri("wolverne"), &uncapped).await;
    assert_eq!(body["total"], 0, "main search misses: {body}");
    assert_eq!(
        suggestion_names(&body),
        vec!["Wolverine Origins Ultimate Collection"]
    );

    let hidden = get_json(&app, &search_uri("wolverne"), &capped).await;
    let nothing = get_json(&app, &search_uri("zzzzqq"), &capped).await;
    assert_eq!(hidden, nothing, "indistinguishable from nothing similar");
    assert!(hidden.get("suggestions").is_none());

    // Within the cap the capped reader is still offered names.
    let body = get_json(&app, &search_uri("titnas"), &capped).await;
    assert_eq!(body["total"], 0, "main search misses: {body}");
    assert_eq!(
        suggestion_names(&body),
        vec!["Teen Titans Go Big Collection"]
    );
}

/// A soft-removed series is never suggested; restoring it brings it back.
#[tokio::test]
async fn removed_series_are_not_suggested() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@removed.test").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = seed_library(&db, "removed").await;
    let id = seed_series(&db, lib, "Daredevil Born Again Collection", None, true).await;

    let body = get_json(&app, &search_uri("daredevl"), &admin).await;
    assert_eq!(
        body,
        serde_json::json!({"items": [], "next_cursor": null, "total": 0})
    );

    entity::series::ActiveModel {
        id: Unchanged(id),
        removed_at: Set(None),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();
    let body = get_json(&app, &search_uri("daredevl"), &admin).await;
    assert_eq!(body["total"], 0, "main search misses: {body}");
    assert_eq!(
        suggestion_names(&body),
        vec!["Daredevil Born Again Collection"]
    );
}

/// EXPLAIN guard at stress-library size: the suggestion query answers from
/// the `series_normalized_name_trgm` GIN index and never seq-scans
/// `series`.
///
/// What it guards: the exact SQL the handler runs for a request with no
/// facet filters — `series_suggestion_select` over the handler's own
/// visibility step (`apply_series_visibility`: active series, the grant's
/// `library_id IN (…)` or `?library=`, the age-cap predicate), for an
/// unrestricted caller, a capped reader, and a capped reader with
/// `?library=`. That is every all-results `/search` request and the
/// search sheet. What it does not guard: requests that add facet filters
/// (the series category grid's sheet); those subquery predicates join the
/// same `WHERE` and leave the trigram condition as the index entry point,
/// but their plans are not asserted here.
#[tokio::test]
async fn suggestion_query_uses_the_trigram_index() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = seed_library(&db, "explain").await;
    // Syllable-built names: a varied trigram population, like a real
    // library's, instead of a handful of repeated words.
    const SYL: [&str; 20] = [
        "ka", "lo", "mi", "ra", "zu", "be", "do", "fi", "gu", "ne", "so", "ti", "va", "we", "yo",
        "ci", "ha", "je", "pu", "xo",
    ];
    let mut batch = vec![series_am(lib, "Phantom Stranger Showcase", None, false)];
    for i in 0..2_600usize {
        let name = format!(
            "{}{}{} {}{} {i}",
            SYL[i % 20],
            SYL[(i / 20) % 20],
            SYL[(i / 400) % 20],
            SYL[(i / 7) % 20],
            SYL[(i / 3) % 20],
        );
        let rating = (i % 5 == 0).then_some("Mature 17+");
        let mut row = series_am(lib, &name, rating, i % 50 == 0);
        // A real series row carries a summary (and the `search_doc` built
        // from it); without one the table is a few dozen pages and any
        // plan is a seq scan.
        row.summary = Set(Some(format!(
            "{name}: {}",
            SYL.iter()
                .cycle()
                .skip(i % 20)
                .take(240)
                .copied()
                .collect::<Vec<_>>()
                .join(" ")
        )));
        batch.push(row);
        if batch.len() == 500 {
            entity::series::Entity::insert_many(std::mem::take(&mut batch))
                .exec(&db)
                .await
                .unwrap();
        }
    }
    if !batch.is_empty() {
        entity::series::Entity::insert_many(batch)
            .exec(&db)
            .await
            .unwrap();
    }
    db.execute_unprepared("ANALYZE series").await.unwrap();

    let capped = VisibleLibraries {
        unrestricted: false,
        allowed: HashSet::from([lib]),
        caps: HashMap::from([(lib, "Teen".to_owned())]),
    };
    let unrestricted = VisibleLibraries::unrestricted();
    let find = entity::series::Entity::find;
    let bases = [
        (
            "unrestricted",
            apply_series_visibility(find(), &unrestricted, None),
        ),
        (
            "capped reader",
            apply_series_visibility(find(), &capped, None),
        ),
        (
            "capped reader, ?library=",
            apply_series_visibility(find(), &capped, Some(lib)),
        ),
    ];
    for (label, base) in bases {
        let base = base.expect("visible");
        let stmt =
            series_suggestion_select(base, &normalize_name("phantm")).build(DbBackend::Postgres);
        let txn = db.begin().await.unwrap();
        txn.execute_unprepared(SUGGESTION_WORD_SIMILARITY_SQL)
            .await
            .unwrap();
        let mut explain = stmt.clone();
        explain.sql = format!("EXPLAIN {}", stmt.sql);
        let rows = txn.query_all_raw(explain).await.unwrap();
        let plan: Vec<String> = rows
            .iter()
            .map(|r| r.try_get_by_index::<String>(0).unwrap())
            .collect();
        let plan = plan.join("\n");
        assert!(
            plan.contains("series_normalized_name_trgm"),
            "{label}: trigram index not used:\n{plan}"
        );
        assert!(
            !plan.contains("Seq Scan on series"),
            "{label}: seq scan on series:\n{plan}"
        );

        // And the query itself finds the typo'd word.
        let found = txn.query_all_raw(stmt).await.unwrap();
        assert_eq!(found.len(), 1, "{label}: one suggestion for the typo");
        txn.rollback().await.unwrap();
    }
}

/// The request's own scope reaches the suggestion query: a search narrowed
/// to library A, or to another status, is never offered a name that the
/// same narrowed search would not find.
#[tokio::test]
async fn suggestions_respect_the_requests_library_and_facets() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@facet.test").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_a = seed_library(&db, "facet-a").await;
    let lib_b = seed_library(&db, "facet-b").await;
    seed_series(&db, lib_a, "Saga", None, false).await;
    seed_series(
        &db,
        lib_b,
        "The Sandman Overture Deluxe Edition",
        None,
        false,
    )
    .await;

    // Positive control: unscoped, the admin is offered it.
    let body = get_json(&app, &search_uri("sandmn"), &admin).await;
    assert_eq!(
        suggestion_names(&body),
        vec!["The Sandman Overture Deluxe Edition"]
    );
    // `?library=` A: the only close name lives in B.
    let body = get_json(
        &app,
        &format!("{}&library={lib_a}", search_uri("sandmn")),
        &admin,
    )
    .await;
    assert_eq!(body["total"], 0);
    assert!(body.get("suggestions").is_none(), "{body}");
    // `?library=` B still offers it.
    let body = get_json(
        &app,
        &format!("{}&library={lib_b}", search_uri("sandmn")),
        &admin,
    )
    .await;
    assert_eq!(
        suggestion_names(&body),
        vec!["The Sandman Overture Deluxe Edition"]
    );
    // A facet: the close series is `continuing`, the search asks for `ended`.
    let body = get_json(
        &app,
        &format!("{}&status=ended", search_uri("sandmn")),
        &admin,
    )
    .await;
    assert_eq!(body["total"], 0);
    assert!(body.get("suggestions").is_none(), "{body}");
    // Its own status still offers it.
    let body = get_json(
        &app,
        &format!("{}&status=continuing", search_uri("sandmn")),
        &admin,
    )
    .await;
    assert_eq!(
        suggestion_names(&body),
        vec!["The Sandman Overture Deluxe Edition"]
    );
}

/// The lowered threshold is `SET LOCAL`: it never outlives the suggestion
/// transaction on a pooled connection. And only the first page of a search
/// asks for suggestions.
#[tokio::test]
async fn threshold_stays_local_and_cursor_pages_carry_no_suggestions() {
    // One pooled connection: the suggestion transaction and the check
    // below necessarily run on the same session.
    let app = TestApp::spawn_with_pool(std::path::PathBuf::from("/tmp/library"), 1, 1).await;
    let admin = register(&app, "admin@local.test").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = seed_library(&db, "local").await;
    seed_series(&db, lib, "The Sandman Overture Deluxe Edition", None, false).await;

    let body = get_json(&app, &search_uri("sandmn"), &admin).await;
    assert_eq!(suggestion_names(&body).len(), 1);

    // The pool's only connection: the one that ran the `SET LOCAL`.
    let state = app.state();
    let txn = tokio::time::timeout(std::time::Duration::from_secs(10), state.db.begin())
        .await
        .expect("the pool's connection")
        .unwrap();
    let row = txn
        .query_one_raw(sea_orm::Statement::from_string(
            DbBackend::Postgres,
            "SELECT current_setting('pg_trgm.word_similarity_threshold')",
        ))
        .await
        .unwrap()
        .unwrap();
    let threshold: String = row.try_get_by_index(0).unwrap();
    assert_eq!(
        threshold, "0.6",
        "the pooled connection kept the lowered threshold"
    );
    txn.rollback().await.unwrap();

    // A cursor page (the offset cursor of a search) never carries them.
    use base64::Engine;
    let cursor = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("offset:50");
    let body = get_json(
        &app,
        &format!("{}&cursor={cursor}", search_uri("sandmn")),
        &admin,
    )
    .await;
    assert_eq!(body["items"], serde_json::json!([]));
    assert!(body.get("total").is_none(), "{body}");
    assert!(body.get("suggestions").is_none(), "{body}");
}

/// The figures the `SUGGESTION_WORD_SIMILARITY_SQL` comment and
/// docs/dev/search.md quote, measured on the server's own pg_trgm.
/// `word_similarity` is the best score of the query against one contiguous
/// run of the name's trigrams (shared / (query + run - shared)), so what a
/// typo costs depends on the intact run around it, not on word length.
#[tokio::test]
async fn documented_word_similarity_scores() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let cases: [(&str, &str, f64, bool); 13] = [
        // (query, normalized name, score, clears 0.4)
        (
            "sandmn",
            "the sandman overture deluxe edition",
            0.7143,
            true,
        ),
        (
            "helboy",
            "hellboy in hell library edition volume one",
            0.6667,
            true,
        ),
        (
            "invisble",
            "the invisible kingdom deluxe edition",
            0.5833,
            true,
        ),
        (
            "wolverne",
            "wolverine origins ultimate collection",
            0.6667,
            true,
        ),
        ("daredevl", "daredevil born again collection", 0.7778, true),
        (
            "batnan",
            "batman the long halloween absolute edition",
            0.4286,
            true,
        ),
        ("titnas", "teen titans go big collection", 0.4286, true),
        // Short words: a typo at the end keeps a run, one near the start
        // does not.
        ("hulc", "hulk", 0.6, true),
        ("spawm", "spawn", 0.6667, true),
        // The exact boundary, not a typical case: 2 shared / 5 = 0.4. It
        // passes because pg_trgm compares with `>=` and the float4 score
        // 0.4 is 0.4000000059…, just above the float8 threshold 0.4.
        ("thur", "thor", 0.4, true),
        ("sega", "saga", 0.25, false),
        // Noise.
        ("zzzzqq", "zuzu", 0.1667, false),
        (
            "qwrtpz",
            "batman the long halloween absolute edition",
            0.0,
            false,
        ),
    ];
    for (q, name, want, clears) in cases {
        let txn = db.begin().await.unwrap();
        txn.execute_unprepared(SUGGESTION_WORD_SIMILARITY_SQL)
            .await
            .unwrap();
        let row = txn
            .query_one_raw(sea_orm::Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT word_similarity($1, $2)::float8, $2 %> $1",
                [q.into(), name.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        txn.rollback().await.unwrap();
        let got: f64 = row.try_get_by_index(0).unwrap();
        let passes: bool = row.try_get_by_index(1).unwrap();
        assert!((got - want).abs() < 0.001, "{q} vs {name}: {got}");
        assert_eq!(passes, clears, "{q} vs {name} at the suggestion threshold");
    }
}
