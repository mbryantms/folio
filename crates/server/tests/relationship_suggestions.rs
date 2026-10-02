//! WP-7.2: relationship suggestion engine — one fixture per evidence source,
//! canonical dedupe, rejection memory, existing-edge dedupe, accept via
//! `create_pair`, the per-run cap on a few-thousand-series library, the
//! admin gate, and audit rows.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    library,
    series::{ActiveModel as SeriesAM, normalize_name},
    series_relationship as rel, series_relationship_suggestion as sug,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, Database, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, Set, Unchanged,
};
use server::jobs::relationship_suggest;
use server::relationships::suggestions::MAX_SUGGESTIONS_PER_RUN;
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
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = entity::user::Entity::find()
        .filter(entity::user::Column::Email.eq(email))
        .one(&db)
        .await
        .unwrap()
        .expect("user row");
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id: user.id,
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

async fn call_json(
    app: &TestApp,
    method: Method,
    path: &str,
    user: &Authed,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
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
    let v = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
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

#[derive(Default, Clone)]
struct S {
    name: &'static str,
    year: Option<i32>,
    volume: Option<i32>,
    publisher: Option<&'static str>,
    group: Option<&'static str>,
}

async fn mk_series(db: &DatabaseConnection, lib_id: Uuid, s: S) -> Uuid {
    let now = Utc::now().fixed_offset();
    let id = Uuid::now_v7();
    let slug = format!(
        "{}-{}-{}",
        normalize_name(s.name).replace(' ', "-"),
        s.year.unwrap_or(0),
        &id.simple().to_string()[24..]
    );
    SeriesAM {
        id: Set(id),
        library_id: Set(lib_id),
        name: Set(s.name.into()),
        normalized_name: Set(normalize_name(s.name)),
        year: Set(s.year),
        volume: Set(s.volume),
        publisher: Set(Some(s.publisher.unwrap_or("Marvel").into())),
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
        series_group: Set(s.group.map(str::to_owned)),
        slug: Set(slug),
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

#[derive(Default)]
struct I {
    number: f64,
    year: Option<i32>,
    alternate_series: Option<&'static str>,
    format: Option<&'static str>,
    notes: Option<&'static str>,
    cv_series: Option<i64>,
}

async fn mk_issue(db: &DatabaseConnection, lib: Uuid, series: Uuid, i: I) -> String {
    let id = Uuid::now_v7().simple().to_string();
    let raw = match i.cv_series {
        Some(cv) => serde_json::json!({ "comicvine_series_id": cv }),
        None => serde_json::json!({}),
    };
    db.execute_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO issues (id, library_id, series_id, file_path, file_size, file_mtime, \
           content_hash, slug, sort_number, number_raw, year, alternate_series, format, notes, \
           comic_info_raw) \
         VALUES ($1, $2, $3, $4, 1, now(), $1, $5, $6, $7, $8, $9, $10, $11, $12)",
        [
            id.clone().into(),
            lib.into(),
            series.into(),
            format!("/fixture/{id}.cbz").into(),
            format!("i-{id}").into(),
            i.number.into(),
            format!("{}", i.number).into(),
            i.year.into(),
            i.alternate_series.map(str::to_owned).into(),
            i.format.map(str::to_owned).into(),
            i.notes.map(str::to_owned).into(),
            raw.into(),
        ],
    ))
    .await
    .unwrap();
    id
}

async fn mk_arc(db: &DatabaseConnection, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    db.execute_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO story_arc (id, slug, name, normalized_name) VALUES ($1, $2, $3, $4)",
        [
            id.into(),
            format!("arc-{}", id.simple()).into(),
            name.into(),
            normalize_name(name).into(),
        ],
    ))
    .await
    .unwrap();
    id
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}

async fn audit_count(db: &DatabaseConnection, action: &str) -> u64 {
    entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq(action))
        .count(db)
        .await
        .unwrap()
}

/// Every suggestion row as `(from, to, kind)`.
async fn rows(db: &DatabaseConnection) -> Vec<sug::Model> {
    sug::Entity::find().all(db).await.unwrap()
}

fn find<'a>(rows: &'a [sug::Model], from: Uuid, to: Uuid, kind: &str) -> Option<&'a sug::Model> {
    rows.iter()
        .find(|r| r.from_series_id == from && r.to_series_id == to && r.kind == kind)
}

fn sources(r: &sug::Model) -> Vec<String> {
    r.evidence["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["source"].as_str().unwrap().to_owned())
        .collect()
}

/// The series of the fixture library, one cluster per evidence source.
struct Fixture {
    lib: Uuid,
    dd_2011: Uuid,
    dd_2014: Uuid,
    thor_2014: Uuid,
    thor_2018: Uuid,
    civil_war: Uuid,
    iron_man: Uuid,
    batman: Uuid,
    nightwing: Uuid,
    avengers: Uuid,
    spider_man: Uuid,
    saga: Uuid,
    saga_deluxe: Uuid,
    ucsm: Uuid,
    usm_relaunch: Uuid,
    ff_1998: Uuid,
    ff_2012: Uuid,
    invincible: Uuid,
    guarding: Uuid,
    atlas_2007: Uuid,
    atlas_2009: Uuid,
}

async fn fixture(app: &TestApp, db: &DatabaseConnection) -> Fixture {
    let lib = mk_library(app, db, "fixture").await;
    let s = |name, year| S {
        name,
        year: Some(year),
        ..Default::default()
    };

    // Name continuation: consecutive volumes (high) and a plain year gap.
    let dd_2011 = mk_series(
        db,
        lib,
        S {
            volume: Some(3),
            ..s("Daredevil", 2011)
        },
    )
    .await;
    let dd_2014 = mk_series(
        db,
        lib,
        S {
            volume: Some(4),
            ..s("Daredevil", 2014)
        },
    )
    .await;
    let thor_2014 = mk_series(db, lib, s("Thor", 2014)).await;
    let thor_2018 = mk_series(db, lib, s("Thor", 2018)).await;

    // AlternateSeries: Iron Man issues cite "Civil War"; a Civil War issue
    // cites Iron Man back (canonical dedupe of the self-inverse kind).
    let civil_war = mk_series(db, lib, s("Civil War", 2006)).await;
    let iron_man = mk_series(db, lib, s("Iron Man", 2005)).await;
    for n in 13..=15 {
        mk_issue(
            db,
            lib,
            iron_man,
            I {
                number: f64::from(n),
                year: Some(2006),
                alternate_series: Some("Civil War"),
                ..Default::default()
            },
        )
        .await;
    }
    mk_issue(
        db,
        lib,
        civil_war,
        I {
            number: 1.0,
            year: Some(2006),
            alternate_series: Some("Iron Man"),
            ..Default::default()
        },
    )
    .await;

    // SeriesGroup.
    let batman = mk_series(
        db,
        lib,
        S {
            group: Some("Batman Family"),
            publisher: Some("DC Comics"),
            ..s("Batman", 2016)
        },
    )
    .await;
    let nightwing = mk_series(
        db,
        lib,
        S {
            group: Some("Batman Family"),
            publisher: Some("DC Comics"),
            ..s("Nightwing", 2016)
        },
    )
    .await;

    // Shared story arc.
    let avengers = mk_series(db, lib, s("Avengers", 2018)).await;
    let spider_man = mk_series(db, lib, s("Spider-Man", 2019)).await;
    let arc = mk_arc(db, "War of the Realms").await;
    for (series, n) in [
        (avengers, 1.0),
        (avengers, 2.0),
        (avengers, 3.0),
        (spider_man, 1.0),
        (spider_man, 2.0),
    ] {
        let issue = mk_issue(
            db,
            lib,
            series,
            I {
                number: n,
                year: Some(2019),
                ..Default::default()
            },
        )
        .await;
        exec(
            db,
            "INSERT INTO issue_arcs (issue_id, arc_id) VALUES ($1, $2)",
            vec![issue.into(), arc.into()],
        )
        .await;
    }

    // Collected edition citing a range of another series.
    let saga = mk_series(
        db,
        lib,
        S {
            publisher: Some("Image"),
            ..s("Saga", 2012)
        },
    )
    .await;
    for n in 1..=6 {
        mk_issue(
            db,
            lib,
            saga,
            I {
                number: f64::from(n),
                ..Default::default()
            },
        )
        .await;
    }
    let saga_deluxe = mk_series(
        db,
        lib,
        S {
            publisher: Some("Image"),
            ..s("Saga Deluxe Edition", 2014)
        },
    )
    .await;
    mk_issue(
        db,
        lib,
        saga_deluxe,
        I {
            number: 1.0,
            format: Some("Hardcover"),
            notes: Some("Collects Saga #1-6."),
            ..Default::default()
        },
    )
    .await;

    // Two local series claiming one ComicVine volume, disjoint ranges.
    let ucsm = mk_series(db, lib, s("Ultimate Comics Spider-Man", 2011)).await;
    let usm_relaunch = mk_series(db, lib, s("Ultimate Spider-Man Relaunch", 2013)).await;
    for n in 1..=3 {
        mk_issue(
            db,
            lib,
            ucsm,
            I {
                number: f64::from(n),
                cv_series: Some(424_242),
                ..Default::default()
            },
        )
        .await;
    }
    for n in 4..=6 {
        mk_issue(
            db,
            lib,
            usm_relaunch,
            I {
                number: f64::from(n),
                cv_series: Some(424_242),
                ..Default::default()
            },
        )
        .await;
    }

    // series_provider_range: Metron files FF #600–611 under series 1713,
    // which the local "FF" series is matched to.
    let ff_1998 = mk_series(db, lib, s("Fantastic Four", 1998)).await;
    let ff_2012 = mk_series(db, lib, s("FF", 2012)).await;
    exec(
        db,
        "INSERT INTO series_provider_range (series_id, source, provider_series_id, provider_series_name, range_low, range_high, set_by) \
         VALUES ($1, 'metron', '1713', 'Fantastic Four', '600', '611', 'cross_reference')",
        vec![ff_1998.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO external_ids (entity_type, entity_id, source, external_id, set_by) \
         VALUES ('series', $1, 'metron', '1713', 'user')",
        vec![ff_2012.to_string().into()],
    )
    .await;

    // Character density: same publisher, five shared uncommon characters.
    let invincible = mk_series(
        db,
        lib,
        S {
            publisher: Some("Image"),
            ..s("Invincible", 2003)
        },
    )
    .await;
    let guarding = mk_series(
        db,
        lib,
        S {
            publisher: Some("Image"),
            ..s("Guarding the Globe", 2010)
        },
    )
    .await;
    for who in [
        "Omni-Man",
        "Atom Eve",
        "Robot",
        "Rex Splode",
        "Allen the Alien",
    ] {
        for sid in [invincible, guarding] {
            exec(
                db,
                "INSERT INTO series_characters (series_id, character) VALUES ($1, $2)",
                vec![sid.into(), who.into()],
            )
            .await;
        }
    }

    // A pair that already has a manual sequel_of edge.
    let atlas_2007 = mk_series(db, lib, s("Agents of Atlas", 2007)).await;
    let atlas_2009 = mk_series(db, lib, s("Agents of Atlas", 2009)).await;
    let txn = sea_orm::TransactionTrait::begin(db).await.unwrap();
    relationships::create_pair(
        &txn,
        atlas_2009,
        atlas_2007,
        RelationshipKind::SequelOf,
        RelationshipSource::Manual,
        None,
        None,
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    Fixture {
        lib,
        dd_2011,
        dd_2014,
        thor_2014,
        thor_2018,
        civil_war,
        iron_man,
        batman,
        nightwing,
        avengers,
        spider_man,
        saga,
        saga_deluxe,
        ucsm,
        usm_relaunch,
        ff_1998,
        ff_2012,
        invincible,
        guarding,
        atlas_2007,
        atlas_2009,
    }
}

fn ordered(a: Uuid, b: Uuid) -> (Uuid, Uuid) {
    if a < b { (a, b) } else { (b, a) }
}

// ───── tests ─────

#[tokio::test]
async fn fixture_library_yields_one_suggestion_per_source() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let f = fixture(&app, &db).await;

    let report = relationship_suggest::run(&db, f.lib).await.unwrap();
    let all = rows(&db).await;
    let dump = all
        .iter()
        .map(|r| format!("{} {:.2} {}", r.kind, r.confidence, r.reason))
        .collect::<Vec<_>>()
        .join("\n");

    // Name continuation: consecutive volumes → high.
    let dd = find(&all, f.dd_2014, f.dd_2011, "sequel_of").expect(&dump);
    assert_eq!(dd.bucket, "high");
    assert!((dd.confidence - 0.9).abs() < 1e-6);
    assert!(
        dd.reason
            .contains("Daredevil vol. 4 (2014) follows Daredevil vol. 3 (2011)"),
        "{}",
        dd.reason
    );
    assert_eq!(sources(dd), vec!["name_continuation"]);
    // Year gap only → medium.
    let thor = find(&all, f.thor_2018, f.thor_2014, "sequel_of").expect(&dump);
    assert_eq!(thor.bucket, "medium");
    assert!(thor.reason.contains("4 years later"), "{}", thor.reason);

    // AlternateSeries: both directions collapse onto one canonical row.
    let (a, b) = ordered(f.civil_war, f.iron_man);
    let cw = find(&all, a, b, "crossover_with").expect(&dump);
    assert!(
        find(&all, b, a, "crossover_with").is_none(),
        "canonical dedupe"
    );
    assert_eq!(sources(cw), vec!["alternate_series"]);
    assert!(cw.reason.contains("\"Civil War\""), "{}", cw.reason);
    assert!(cw.confidence >= 0.8, "{}", cw.confidence);

    // SeriesGroup → same_universe, high for a small group.
    let (a, b) = ordered(f.batman, f.nightwing);
    let grp = find(&all, a, b, "same_universe").expect(&dump);
    assert_eq!(grp.bucket, "high");
    assert!(grp.reason.contains("Batman Family"));

    // Shared story arc → crossover_with.
    let (a, b) = ordered(f.avengers, f.spider_man);
    let arc = find(&all, a, b, "crossover_with").expect(&dump);
    assert_eq!(sources(arc), vec!["story_arc"]);
    assert!(arc.reason.contains("War of the Realms"), "{}", arc.reason);

    // Collected edition → collects (TPB/HC series is the subject).
    let col = find(&all, f.saga_deluxe, f.saga, "collects").expect(&dump);
    assert_eq!(col.bucket, "high");
    assert!(col.reason.contains("#1–6"), "{}", col.reason);
    assert_eq!(col.evidence["sources"][0]["issues_in_library"], 6);

    // Shared provider volume with disjoint ranges → sequel_of.
    let pv = find(&all, f.usm_relaunch, f.ucsm, "sequel_of").expect(&dump);
    assert_eq!(sources(pv), vec!["provider_volume"]);
    assert!(pv.reason.contains("424242"), "{}", pv.reason);

    // Provider range → see_also.
    let (a, b) = ordered(f.ff_1998, f.ff_2012);
    let pr = find(&all, a, b, "see_also").expect(&dump);
    assert_eq!(sources(pr), vec!["provider_range"]);
    assert!(pr.reason.contains("#600–611"), "{}", pr.reason);

    // Character density → same_universe, always low.
    let (a, b) = ordered(f.invincible, f.guarding);
    let dens = find(&all, a, b, "same_universe").expect(&dump);
    assert_eq!(dens.bucket, "low");
    assert!(dens.confidence <= 0.5);
    assert_eq!(dens.evidence["sources"][0]["shared_features"], 5);

    // Existing manual edge isn't re-suggested.
    assert!(find(&all, f.atlas_2009, f.atlas_2007, "sequel_of").is_none());
    assert!(report.skipped_existing_edge >= 1, "{report:?}");

    // Every row is pending, canonical, and has a reason + evidence.
    for r in &all {
        assert_eq!(r.status, "pending");
        assert!(!r.reason.is_empty());
        assert!(!["prequel_of", "has_spin_off", "collected_in"].contains(&r.kind.as_str()));
        if ["crossover_with", "same_universe", "see_also"].contains(&r.kind.as_str()) {
            assert!(r.from_series_id < r.to_series_id);
        }
    }
    assert_eq!(report.inserted, all.len());

    // Rerun: nothing new, nothing changed.
    let again = relationship_suggest::run(&db, f.lib).await.unwrap();
    assert_eq!(again.inserted, 0, "{again:?}");
    assert_eq!(again.updated, 0, "{again:?}");
    assert_eq!(rows(&db).await.len(), all.len());

    // One library_events row for the first run (the idle rerun adds none).
    let events = entity::library_event::Entity::find()
        .filter(entity::library_event::Column::LibraryId.eq(f.lib))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        events[0].detail.as_ref().unwrap()["kind"],
        "relationship_suggestions"
    );
}

#[tokio::test]
async fn rejected_suggestions_never_reappear_and_pending_ones_refresh() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let f = fixture(&app, &db).await;
    relationship_suggest::run(&db, f.lib).await.unwrap();

    let thor = find(&rows(&db).await, f.thor_2018, f.thor_2014, "sequel_of")
        .unwrap()
        .clone();
    let (status, body) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/{}/reject", thor.id),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "rejected");
    assert_eq!(
        audit_count(&db, "admin.relationship_suggestion.reject").await,
        1
    );

    // Tamper with a pending row so the rerun has something to refresh.
    let dd = find(&rows(&db).await, f.dd_2014, f.dd_2011, "sequel_of")
        .unwrap()
        .clone();
    sug::ActiveModel {
        id: Unchanged(dd.id),
        confidence: Set(0.1),
        bucket: Set("low".into()),
        ..Default::default()
    }
    .update(&db)
    .await
    .unwrap();

    let report = relationship_suggest::run(&db, f.lib).await.unwrap();
    assert_eq!(report.skipped_reviewed, 1, "{report:?}");
    assert_eq!(report.inserted, 0);
    assert_eq!(report.updated, 1);

    let after = rows(&db).await;
    let thor_after = find(&after, f.thor_2018, f.thor_2014, "sequel_of").unwrap();
    assert_eq!(thor_after.status, "rejected");
    assert_eq!(
        thor_after.updated_at,
        sug::Entity::find_by_id(thor.id)
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .updated_at
    );
    let dd_after = find(&after, f.dd_2014, f.dd_2011, "sequel_of").unwrap();
    assert!(
        (dd_after.confidence - 0.9).abs() < 1e-6,
        "pending row refreshed"
    );
    assert_eq!(dd_after.bucket, "high");

    // Rejecting twice is a conflict.
    let (status, _) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/{}/reject", thor.id),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn accept_creates_the_pair_and_is_never_resuggested() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let f = fixture(&app, &db).await;
    relationship_suggest::run(&db, f.lib).await.unwrap();
    let all = rows(&db).await;

    // Plain accept.
    let dd = find(&all, f.dd_2014, f.dd_2011, "sequel_of")
        .unwrap()
        .clone();
    let generation = app.state().similarity.generation();
    let (status, body) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/{}/accept", dd.id),
        &admin,
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["suggestion"]["status"], "accepted");
    assert_eq!(body["kind"], "sequel_of");
    assert_eq!(body["created"], true);
    assert!(
        app.state().similarity.generation() > generation,
        "accepting an edge invalidates the WP-7.4 similarity cache"
    );
    assert_eq!(
        body["suggestion"]["from_series"]["id"],
        f.dd_2014.to_string()
    );
    let fwd = rel::Entity::find()
        .filter(rel::Column::FromSeriesId.eq(f.dd_2014))
        .filter(rel::Column::ToSeriesId.eq(f.dd_2011))
        .one(&db)
        .await
        .unwrap()
        .expect("forward edge");
    assert_eq!(fwd.kind, "sequel_of");
    assert_eq!(fwd.source, "suggested");
    assert!((fwd.confidence.unwrap() - 0.9).abs() < 1e-6);
    assert_eq!(fwd.created_by, Some(admin.user_id));
    let inv = rel::Entity::find()
        .filter(rel::Column::FromSeriesId.eq(f.dd_2011))
        .filter(rel::Column::ToSeriesId.eq(f.dd_2014))
        .one(&db)
        .await
        .unwrap()
        .expect("inverse edge");
    assert_eq!(inv.kind, "prequel_of");
    assert_eq!(
        audit_count(&db, "admin.relationship_suggestion.accept").await,
        1
    );

    // Accept with a kind override → modified.
    let (a, b) = ordered(f.ff_1998, f.ff_2012);
    let pr = find(&all, a, b, "see_also").unwrap().clone();
    let (status, body) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/{}/accept", pr.id),
        &admin,
        Some(serde_json::json!({ "kind": "spin_off_of" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["suggestion"]["status"], "modified");
    assert_eq!(body["suggestion"]["accepted_kind"], "spin_off_of");
    assert_eq!(body["kind"], "spin_off_of");
    assert!(
        rel::Entity::find()
            .filter(rel::Column::FromSeriesId.eq(a))
            .filter(rel::Column::ToSeriesId.eq(b))
            .filter(rel::Column::Kind.eq("spin_off_of"))
            .one(&db)
            .await
            .unwrap()
            .is_some()
    );

    // Second accept → 409; unknown id → 404; bad id → 400.
    let (status, _) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/{}/accept", dd.id),
        &admin,
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call_json(
        &app,
        Method::POST,
        &format!(
            "/api/admin/relationship-suggestions/{}/accept",
            Uuid::now_v7()
        ),
        &admin,
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call_json(
        &app,
        Method::POST,
        "/api/admin/relationship-suggestions/nope/accept",
        &admin,
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Rerun: the accepted / modified rows stay as they are and nothing new
    // appears for those pairs.
    let report = relationship_suggest::run(&db, f.lib).await.unwrap();
    assert_eq!(report.inserted, 0, "{report:?}");
    let after = rows(&db).await;
    assert_eq!(after.len(), all.len());
    assert_eq!(
        find(&after, f.dd_2014, f.dd_2011, "sequel_of")
            .unwrap()
            .status,
        "accepted"
    );
}

#[tokio::test]
async fn accept_refuses_a_contradicting_edge() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let f = fixture(&app, &db).await;
    relationship_suggest::run(&db, f.lib).await.unwrap();
    let thor = find(&rows(&db).await, f.thor_2018, f.thor_2014, "sequel_of")
        .unwrap()
        .clone();
    // An admin meanwhile linked them the other way round.
    let txn = sea_orm::TransactionTrait::begin(&db).await.unwrap();
    relationships::create_pair(
        &txn,
        f.thor_2014,
        f.thor_2018,
        RelationshipKind::SequelOf,
        RelationshipSource::Manual,
        None,
        None,
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    let (status, body) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/{}/accept", thor.id),
        &admin,
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    // Still pending; no audit row.
    let row = sug::Entity::find_by_id(thor.id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "pending");
    assert_eq!(
        audit_count(&db, "admin.relationship_suggestion.accept").await,
        0
    );
}

#[tokio::test]
async fn list_paginates_filters_and_reports_counts() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let f = fixture(&app, &db).await;
    relationship_suggest::run(&db, f.lib).await.unwrap();
    let total = rows(&db).await.len();

    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let url = match &cursor {
            Some(c) => format!("/api/admin/relationship-suggestions?limit=3&cursor={c}"),
            None => format!(
                "/api/admin/relationship-suggestions?limit=3&library_id={}",
                f.lib
            ),
        };
        let (status, body) = call_json(&app, Method::GET, &url, &admin, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        if pages == 0 {
            assert_eq!(body["total"], total as u64);
            let c = &body["bucket_counts"];
            assert_eq!(
                c["high"].as_u64().unwrap()
                    + c["medium"].as_u64().unwrap()
                    + c["low"].as_u64().unwrap(),
                total as u64
            );
        } else {
            assert!(body.get("total").is_none(), "total only on the first page");
        }
        let items = body["items"].as_array().unwrap();
        for it in items {
            assert!(it["from_series"]["slug"].is_string());
            assert!(it["to_series"]["slug"].is_string());
            seen.push((
                it["confidence"].as_f64().unwrap(),
                it["id"].as_str().unwrap().to_owned(),
            ));
        }
        pages += 1;
        cursor = body["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), total);
    assert!(pages > 1);
    assert!(seen.windows(2).all(|w| w[0].0 >= w[1].0), "confidence desc");

    // Bucket filter.
    let (_, body) = call_json(
        &app,
        Method::GET,
        "/api/admin/relationship-suggestions?bucket=low&limit=200",
        &admin,
        None,
    )
    .await;
    assert!(
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["bucket"] == "low")
    );
    // Bad cursor → 400.
    let (status, _) = call_json(
        &app,
        Method::GET,
        "/api/admin/relationship-suggestions?cursor=%%%",
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Per-series list: everything touching Saga.
    let saga_slug = entity::series::Entity::find_by_id(f.saga)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .slug;
    let (status, body) = call_json(
        &app,
        Method::GET,
        &format!("/api/series/{saga_slug}/relationship-suggestions"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["kind"], "collects");
    assert_eq!(items[0]["kind_label"], "Collects");
}

#[tokio::test]
async fn non_admins_get_403_everywhere() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let user = register(&app, "reader@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    demote_to_user(&db, user.user_id).await;
    let f = fixture(&app, &db).await;
    relationship_suggest::run(&db, f.lib).await.unwrap();
    let id = rows(&db).await[0].id;
    let saga_slug = entity::series::Entity::find_by_id(f.saga)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .slug;

    for (method, path, body) in [
        (
            Method::GET,
            "/api/admin/relationship-suggestions".to_owned(),
            None,
        ),
        (
            Method::POST,
            format!("/api/admin/relationship-suggestions/{id}/accept"),
            Some(serde_json::json!({})),
        ),
        (
            Method::POST,
            format!("/api/admin/relationship-suggestions/{id}/reject"),
            None,
        ),
        (
            Method::POST,
            "/api/admin/relationship-suggestions/run".to_owned(),
            None,
        ),
        (
            Method::GET,
            format!("/api/series/{saga_slug}/relationship-suggestions"),
            None,
        ),
    ] {
        let (status, _) = call_json(&app, method.clone(), &path, &user, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
    }
    assert!(rows(&db).await.iter().all(|r| r.status == "pending"));
}

#[tokio::test]
async fn run_endpoint_enqueues_dedupes_and_audits() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = mk_library(&app, &db, "runlib").await;

    let (status, body) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/run?library_id={lib}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["enqueued"], serde_json::json!([lib.to_string()]));
    // A second trigger while queued is coalesced.
    let (status, body) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/run?library_id={lib}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["already_queued"], serde_json::json!([lib.to_string()]));
    assert_eq!(
        audit_count(&db, "admin.relationship_suggestion.run").await,
        2
    );

    let (status, _) = call_json(
        &app,
        Method::POST,
        &format!(
            "/api/admin/relationship-suggestions/run?library_id={}",
            Uuid::now_v7()
        ),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// The stress fixture: a few thousand series with far more than
/// [`MAX_SUGGESTIONS_PER_RUN`] potential suggestions. The run must finish
/// quickly and write exactly the cap, strongest first.
#[tokio::test]
async fn cap_is_respected_on_a_large_library() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = mk_library(&app, &db, "stress").await;

    // 1500 titles × 2 volumes (3000 series): 1500 consecutive-volume sequel
    // candidates, plus one 3000-member SeriesGroup (a star → 2999 more), plus
    // a 1500-pair story-arc fan-out.
    exec(
        &db,
        r#"INSERT INTO series (id, library_id, name, normalized_name, slug, year, volume, publisher,
                               status, language_code, alternate_names, aliases, series_group,
                               metadata_sync_paused, preserve_canonical_order, created_at, updated_at)
           SELECT gen_random_uuid(), $1, 'Title ' || t, 'title ' || t,
                  'title-' || t || '-' || v, 2000 + v * 5, v, 'Stress',
                  'continuing', 'en', '[]'::jsonb, '[]'::jsonb, 'Stress Universe',
                  false, false, now(), now()
             FROM generate_series(1, 1500) t, generate_series(1, 2) v"#,
        vec![lib.into()],
    )
    .await;
    exec(
        &db,
        r#"INSERT INTO issues (id, library_id, series_id, file_path, file_size, file_mtime,
                               content_hash, slug, sort_number, number_raw, year)
           SELECT md5(s.id::text), $1, s.id, '/stress/' || s.id, 1, now(), md5(s.id::text),
                  'i-1', 1, '1', s.year
             FROM series s WHERE s.library_id = $1"#,
        vec![lib.into()],
    )
    .await;
    let arc = mk_arc(&db, "Stress Event").await;
    exec(
        &db,
        "INSERT INTO issue_arcs (issue_id, arc_id) SELECT i.id, $2 FROM issues i WHERE i.library_id = $1",
        vec![lib.into(), arc.into()],
    )
    .await;

    let started = std::time::Instant::now();
    let report = relationship_suggest::run(&db, lib).await.unwrap();
    let elapsed = started.elapsed();
    assert!(report.proposals > MAX_SUGGESTIONS_PER_RUN, "{report:?}");
    assert_eq!(report.inserted, MAX_SUGGESTIONS_PER_RUN, "{report:?}");
    assert_eq!(
        report.capped,
        report.proposals
            - MAX_SUGGESTIONS_PER_RUN
            - report.skipped_existing_edge
            - report.skipped_reviewed
    );
    assert_eq!(
        sug::Entity::find().count(&db).await.unwrap(),
        MAX_SUGGESTIONS_PER_RUN as u64
    );
    // Strongest first: the 1500 consecutive-volume sequels (0.9) fill the
    // cap before anything weaker.
    let min_written = sug::Entity::find()
        .all(&db)
        .await
        .unwrap()
        .iter()
        .map(|r| r.confidence)
        .fold(f32::MAX, f32::min);
    assert!(min_written >= 0.85, "lowest written {min_written}");
    assert!(
        elapsed < std::time::Duration::from_secs(30),
        "run took {elapsed:?}"
    );

    // A rerun stays bounded and doesn't grow past the cap of new rows.
    let again = relationship_suggest::run(&db, lib).await.unwrap();
    assert!(again.inserted <= MAX_SUGGESTIONS_PER_RUN);
}
