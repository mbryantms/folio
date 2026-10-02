//! WP-7.2: relationship suggestion engine — one fixture per evidence source,
//! canonical dedupe, rejection memory, existing-edge dedupe, accept via
//! `create_pair`, the per-run cap on a few-thousand-series library, the
//! admin gate, and audit rows.
//!
//! WP-7.3: bulk accept / reject (one audit row per batch, per-item
//! failures, one similarity invalidation), reopen, and stale marking /
//! revival.

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
use std::collections::HashMap;
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
    series_type: Option<&'static str>,
    /// `language_code`; default `en`.
    lang: Option<&'static str>,
    aliases: &'static [&'static str],
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
        language_code: Set(s.lang.unwrap_or("en").into()),
        sort_name: Set(None),
        year_end: Set(None),
        series_type: Set(s.series_type.map(str::to_owned)),
        aliases: Set(serde_json::json!(s.aliases)),
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
    month: Option<i32>,
    title: Option<&'static str>,
    special_type: Option<&'static str>,
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
           comic_info_raw, month, title, special_type) \
         VALUES ($1, $2, $3, $4, 1, now(), $1, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
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
            i.month.into(),
            i.title.map(str::to_owned).into(),
            i.special_type.map(str::to_owned).into(),
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
        .find(|r| r.from_series_id == from && r.to_series_id == Some(to) && r.kind == kind)
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
    let dd = find(&all, f.dd_2014, f.dd_2011, "continues").expect(&dump);
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
    let thor = find(&all, f.thor_2018, f.thor_2014, "continues").expect(&dump);
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

    // SeriesGroup no longer suggests pairwise same_universe (WP-7.6: it is
    // derived from universe / SeriesGroup membership instead).
    assert!(all.iter().all(|r| r.kind != "same_universe"), "{dump}");

    // Shared story arc → each series ties in to the arc (WP-7.6), not a
    // pairwise crossover. 3 vs 2 issues: no clear main, so medium/low.
    let (a, b) = ordered(f.avengers, f.spider_man);
    assert!(find(&all, a, b, "crossover_with").is_none(), "{dump}");
    let tie = |s: Uuid| {
        all.iter()
            .find(|r| r.from_series_id == s && r.to_arc_id.is_some())
            .expect(&dump)
    };
    let main = tie(f.avengers);
    assert_eq!(main.kind, "tie_in_to");
    assert_eq!(main.qualifier.as_deref(), Some("main"));
    assert_eq!(main.from_range.as_deref(), Some("1-3"));
    assert_eq!(sources(main), vec!["arc_tie_in"]);
    assert!(main.reason.contains("War of the Realms"), "{}", main.reason);
    assert_eq!(tie(f.spider_man).qualifier.as_deref(), Some("tie_in"));

    // Collected edition → collects (TPB/HC series is the subject), with the
    // cited range as scope.
    let col = find(&all, f.saga_deluxe, f.saga, "collects").expect(&dump);
    assert_eq!(col.bucket, "high");
    assert!(col.reason.contains("#1–6"), "{}", col.reason);
    assert_eq!(col.evidence["sources"][0]["issues_in_library"], 6);
    assert_eq!(col.to_range.as_deref(), Some("1-6"));
    assert_eq!(col.from_range.as_deref(), Some("1"));
    assert_eq!(col.coverage.as_deref(), Some("full"));

    // Shared provider volume with disjoint ranges → continues; different
    // names under one provider series → retitle.
    let pv = find(&all, f.usm_relaunch, f.ucsm, "continues").expect(&dump);
    assert_eq!(sources(pv), vec!["provider_volume"]);
    assert!(pv.reason.contains("424242"), "{}", pv.reason);
    assert_eq!(pv.qualifier.as_deref(), Some("retitle"));

    // Provider range → see_also.
    let (a, b) = ordered(f.ff_1998, f.ff_2012);
    let pr = find(&all, a, b, "see_also").expect(&dump);
    assert_eq!(sources(pr), vec!["provider_range"]);
    assert!(pr.reason.contains("#600–611"), "{}", pr.reason);

    // Character density is retired (WP-7.6).
    let (a, b) = ordered(f.invincible, f.guarding);
    assert!(find(&all, a, b, "same_universe").is_none());

    // Existing manual edge isn't re-suggested.
    assert!(find(&all, f.atlas_2009, f.atlas_2007, "continues").is_none());
    assert!(report.skipped_existing_edge >= 1, "{report:?}");

    // Every row is pending, canonical, and has a reason + evidence.
    for r in &all {
        assert_eq!(r.status, "pending");
        assert!(!r.reason.is_empty());
        assert!(
            r.kind.parse::<RelationshipKind>().unwrap().is_canonical(),
            "{}",
            r.kind
        );
        if ["crossover_with", "same_universe", "see_also"].contains(&r.kind.as_str()) {
            assert!(r.to_series_id.is_some_and(|t| r.from_series_id < t));
        }
        assert_eq!(
            r.to_series_id.is_some(),
            r.to_arc_id.is_none(),
            "exactly one target"
        );
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

    let thor = find(&rows(&db).await, f.thor_2018, f.thor_2014, "continues")
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
    let dd = find(&rows(&db).await, f.dd_2014, f.dd_2011, "continues")
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
    let thor_after = find(&after, f.thor_2018, f.thor_2014, "continues").unwrap();
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
    let dd_after = find(&after, f.dd_2014, f.dd_2011, "continues").unwrap();
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
    let dd = find(&all, f.dd_2014, f.dd_2011, "continues")
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
    assert_eq!(body["kind"], "continues");
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
    assert_eq!(fwd.kind, "continues");
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
    assert_eq!(inv.kind, "continued_by");
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
        find(&after, f.dd_2014, f.dd_2011, "continues")
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
    let thor = find(&rows(&db).await, f.thor_2018, f.thor_2014, "continues")
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
            assert!(
                it["to_series"]["slug"].is_string() ^ it["to_arc"]["slug"].is_string(),
                "{it}"
            );
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
            Method::POST,
            format!("/api/admin/relationship-suggestions/{id}/reopen"),
            None,
        ),
        (
            Method::POST,
            "/api/admin/relationship-suggestions/bulk-accept".to_owned(),
            Some(serde_json::json!({ "bucket": "high" })),
        ),
        (
            Method::POST,
            "/api/admin/relationship-suggestions/bulk-accept".to_owned(),
            Some(serde_json::json!({ "ids": [id] })),
        ),
        (
            Method::POST,
            "/api/admin/relationship-suggestions/bulk-reject".to_owned(),
            Some(serde_json::json!({ "ids": [id] })),
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

    // 1500 titles × 2 volumes (3000 series): 1500 consecutive-volume
    // `continues` candidates; plus 1500 "Title N Annual" series (1500
    // `annual_of` through the name-detector catalogue) and a 4500-series
    // story-arc fan-out (4500 `tie_in_to` arc candidates). The legacy
    // SeriesGroup on every series must not add anything (WP-7.6 retired it).
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
        r#"INSERT INTO series (id, library_id, name, normalized_name, slug, year, publisher,
                               status, language_code, alternate_names, aliases, series_group,
                               metadata_sync_paused, preserve_canonical_order, created_at, updated_at)
           SELECT gen_random_uuid(), $1, 'Title ' || t || ' Annual', 'title ' || t || ' annual',
                  'title-' || t || '-annual', 2006, 'Stress',
                  'continuing', 'en', '[]'::jsonb, '[]'::jsonb, 'Stress Universe',
                  false, false, now(), now()
             FROM generate_series(1, 1500) t"#,
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
    assert_eq!(report.by_source["annual"], 1500, "{report:?}");
    assert_eq!(report.by_source["arc_tie_in"], 4500, "{report:?}");
    assert!(report.failed_sources.is_empty(), "{report:?}");
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
    // Strongest first: the consecutive-volume continuations and the annuals
    // (both 0.9) fill the cap before anything weaker.
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

// ───── WP-7.3 ─────

async fn rename_series(db: &DatabaseConnection, id: Uuid, name: &str) {
    SeriesAM {
        id: Unchanged(id),
        name: Set(name.into()),
        normalized_name: Set(normalize_name(name)),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();
}

async fn status_of(db: &DatabaseConnection, id: Uuid) -> String {
    sug::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .status
}

#[tokio::test]
async fn bulk_accept_by_ids_is_one_batch_with_partial_failures() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let f = fixture(&app, &db).await;
    relationship_suggest::run(&db, f.lib).await.unwrap();
    let all = rows(&db).await;
    let dd = find(&all, f.dd_2014, f.dd_2011, "continues").unwrap().id;
    let thor = find(&all, f.thor_2018, f.thor_2014, "continues")
        .unwrap()
        .id;
    let (a, b) = ordered(f.ff_1998, f.ff_2012);
    let ff = find(&all, a, b, "see_also").unwrap().id;

    // ff: already rejected. thor: contradicted by a manual edge.
    let (status, _) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/{ff}/reject"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
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

    let missing = Uuid::now_v7();
    let generation = app.state().similarity.generation();
    let (status, body) = call_json(
        &app,
        Method::POST,
        "/api/admin/relationship-suggestions/bulk-accept",
        &admin,
        Some(serde_json::json!({ "ids": [dd, thor, ff, missing, dd] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["requested"], 4, "duplicates are processed once");
    assert_eq!(body["succeeded"], serde_json::json!([dd.to_string()]));
    assert_eq!(body["created"], 1);
    assert!(body.get("remaining").is_none(), "ids mode has no remaining");
    let failed = body["failed"].as_array().unwrap();
    let code = |id: Uuid| {
        failed
            .iter()
            .find(|f| f["id"] == id.to_string())
            .map(|f| f["code"].as_str().unwrap().to_owned())
    };
    assert_eq!(code(thor).as_deref(), Some("conflict"));
    assert_eq!(code(ff).as_deref(), Some("already_reviewed"));
    assert_eq!(code(missing).as_deref(), Some("not_found"));

    // The good item committed; the failed ones rolled back to their
    // savepoints without sinking the batch.
    assert_eq!(status_of(&db, dd).await, "accepted");
    assert_eq!(status_of(&db, thor).await, "pending");
    assert!(
        rel::Entity::find()
            .filter(rel::Column::FromSeriesId.eq(f.dd_2014))
            .filter(rel::Column::ToSeriesId.eq(f.dd_2011))
            .filter(rel::Column::Kind.eq("continues"))
            .filter(rel::Column::Source.eq("suggested"))
            .one(&db)
            .await
            .unwrap()
            .is_some(),
        "accepted via create_pair"
    );
    assert_eq!(
        app.state().similarity.generation(),
        generation + 1,
        "one invalidation per batch"
    );

    // Exactly one audit row for the batch, none per item.
    assert_eq!(
        audit_count(&db, "admin.relationship_suggestion.bulk_accept").await,
        1
    );
    assert_eq!(
        audit_count(&db, "admin.relationship_suggestion.accept").await,
        0
    );
    let audit = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("admin.relationship_suggestion.bulk_accept"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(audit.payload["mode"], "ids");
    assert_eq!(audit.payload["accepted"], 1);
    assert_eq!(audit.payload["failed"], 3);
    assert_eq!(
        audit.payload["accepted_ids"],
        serde_json::json!([dd.to_string()])
    );

    // A batch that creates nothing doesn't invalidate.
    let generation = app.state().similarity.generation();
    let (status, body) = call_json(
        &app,
        Method::POST,
        "/api/admin/relationship-suggestions/bulk-accept",
        &admin,
        Some(serde_json::json!({ "ids": [dd] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["failed"][0]["code"], "already_reviewed");
    assert_eq!(app.state().similarity.generation(), generation);
    assert_eq!(
        audit_count(&db, "admin.relationship_suggestion.bulk_accept").await,
        2
    );
}

#[tokio::test]
async fn bulk_accept_high_bucket_and_validation() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let f = fixture(&app, &db).await;
    relationship_suggest::run(&db, f.lib).await.unwrap();
    let high: Vec<Uuid> = rows(&db)
        .await
        .into_iter()
        .filter(|r| r.bucket == "high" && r.status == "pending")
        .map(|r| r.id)
        .collect();
    assert!(!high.is_empty(), "fixture has high-confidence suggestions");
    let others = rows(&db)
        .await
        .into_iter()
        .filter(|r| r.bucket != "high")
        .count();

    // Validation: 422 for neither / both / non-high bucket / library_id
    // with ids / too many ids / empty ids; 404 for an unknown library.
    for body in [
        serde_json::json!({}),
        serde_json::json!({ "ids": [high[0]], "bucket": "high" }),
        serde_json::json!({ "bucket": "medium" }),
        serde_json::json!({ "ids": [high[0]], "library_id": f.lib }),
        serde_json::json!({ "ids": [] }),
        serde_json::json!({ "ids": (0..501).map(|_| Uuid::now_v7()).collect::<Vec<_>>() }),
    ] {
        let (status, resp) = call_json(
            &app,
            Method::POST,
            "/api/admin/relationship-suggestions/bulk-accept",
            &admin,
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body} → {resp}");
        assert!(resp["error"]["details"].is_array(), "{resp}");
    }
    let (status, _) = call_json(
        &app,
        Method::POST,
        "/api/admin/relationship-suggestions/bulk-accept",
        &admin,
        Some(serde_json::json!({ "bucket": "high", "library_id": Uuid::now_v7() })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        audit_count(&db, "admin.relationship_suggestion.bulk_accept").await,
        0
    );

    let (status, body) = call_json(
        &app,
        Method::POST,
        "/api/admin/relationship-suggestions/bulk-accept",
        &admin,
        Some(serde_json::json!({ "bucket": "high", "library_id": f.lib })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["requested"], high.len() as u64);
    let succeeded = body["succeeded"].as_array().unwrap().len();
    let failed = body["failed"].as_array().unwrap().len();
    assert_eq!(succeeded + failed, high.len());
    assert_eq!(
        body["remaining"], failed as u64,
        "only refusals stay pending"
    );
    assert_eq!(
        audit_count(&db, "admin.relationship_suggestion.bulk_accept").await,
        1
    );
    // Medium / low rows untouched.
    assert_eq!(
        rows(&db)
            .await
            .iter()
            .filter(|r| r.bucket != "high" && r.status == "pending")
            .count(),
        others
    );
}

#[tokio::test]
async fn bulk_reject_writes_one_audit_row() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let f = fixture(&app, &db).await;
    relationship_suggest::run(&db, f.lib).await.unwrap();
    let ids: Vec<Uuid> = rows(&db).await.iter().take(3).map(|r| r.id).collect();
    let (status, body) = call_json(
        &app,
        Method::POST,
        "/api/admin/relationship-suggestions/bulk-reject",
        &admin,
        Some(serde_json::json!({ "ids": [ids[0], ids[1], ids[2], Uuid::now_v7()] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["succeeded"].as_array().unwrap().len(), 3);
    assert_eq!(body["failed"][0]["code"], "not_found");
    for id in &ids {
        assert_eq!(status_of(&db, *id).await, "rejected");
    }
    assert_eq!(
        audit_count(&db, "admin.relationship_suggestion.bulk_reject").await,
        1
    );
    assert_eq!(
        audit_count(&db, "admin.relationship_suggestion.reject").await,
        0
    );
    let (status, _) = call_json(
        &app,
        Method::POST,
        "/api/admin/relationship-suggestions/bulk-reject",
        &admin,
        Some(serde_json::json!({ "ids": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn reopen_clears_a_rejection() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let f = fixture(&app, &db).await;
    relationship_suggest::run(&db, f.lib).await.unwrap();
    let before = rows(&db).await.len();
    let thor = find(&rows(&db).await, f.thor_2018, f.thor_2014, "continues")
        .unwrap()
        .id;

    // Only a rejected row can be reopened.
    let (status, _) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/{thor}/reopen"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/{thor}/reject"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/{thor}/reopen"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["suggestion"]["status"], "pending");
    assert!(body["suggestion"]["reviewed_at"].is_null());
    let row = sug::Entity::find_by_id(thor)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "pending");
    assert_eq!(row.reviewed_by, None);
    assert_eq!(rows(&db).await.len(), before, "append-only: no row deleted");

    assert_eq!(
        audit_count(&db, "admin.relationship_suggestion.reopen").await,
        1
    );
    let audit = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("admin.relationship_suggestion.reopen"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(audit.payload["previous_status"], "rejected");
    assert_eq!(audit.payload["rejected_by"], admin.user_id.to_string());

    // The engine treats it as pending again (refreshed, not skipped).
    let report = relationship_suggest::run(&db, f.lib).await.unwrap();
    assert_eq!(report.skipped_reviewed, 0, "{report:?}");
    assert_eq!(status_of(&db, thor).await, "pending");

    // Unknown / malformed ids.
    let (status, _) = call_json(
        &app,
        Method::POST,
        &format!(
            "/api/admin/relationship-suggestions/{}/reopen",
            Uuid::now_v7()
        ),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call_json(
        &app,
        Method::POST,
        "/api/admin/relationship-suggestions/nope/reopen",
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn stale_rows_are_hidden_and_revive_when_produced_again() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let f = fixture(&app, &db).await;
    let first = relationship_suggest::run(&db, f.lib).await.unwrap();
    assert_eq!(first.marked_stale, 0, "{first:?}");
    let thor = find(&rows(&db).await, f.thor_2018, f.thor_2014, "continues")
        .unwrap()
        .id;
    let pending_before = rows(&db)
        .await
        .iter()
        .filter(|r| r.status == "pending")
        .count();

    // The evidence goes away: Thor (2018) is renamed, so the name
    // continuation no longer fires.
    rename_series(&db, f.thor_2018, "Mighty Thor Reborn").await;
    let report = relationship_suggest::run(&db, f.lib).await.unwrap();
    assert!(report.failed_sources.is_empty(), "{report:?}");
    assert!(report.marked_stale >= 1, "{report:?}");
    assert_eq!(status_of(&db, thor).await, "stale");

    // Hidden from the default list and from `all`; shown by `stale`.
    let listed = |body: &serde_json::Value| -> Vec<String> {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_owned())
            .collect()
    };
    for q in ["", "&status=all", "&status=pending"] {
        let (status, body) = call_json(
            &app,
            Method::GET,
            &format!(
                "/api/admin/relationship-suggestions?limit=200&library_id={}{q}",
                f.lib
            ),
            &admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(!listed(&body).contains(&thor.to_string()), "{q}");
    }
    let (_, body) = call_json(
        &app,
        Method::GET,
        &format!(
            "/api/admin/relationship-suggestions?status=stale&library_id={}",
            f.lib
        ),
        &admin,
        None,
    )
    .await;
    assert!(listed(&body).contains(&thor.to_string()));
    assert!(
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["status"] == "stale")
    );

    // A stale row can't be accepted (409) and is not rejection memory.
    let (status, body) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/{thor}/accept"),
        &admin,
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]["message"].as_str().unwrap().contains("stale"),
        "{body}"
    );

    // The evidence comes back: the same row revives to pending.
    rename_series(&db, f.thor_2018, "Thor").await;
    let report = relationship_suggest::run(&db, f.lib).await.unwrap();
    assert_eq!(report.revived, 1, "{report:?}");
    assert_eq!(report.inserted, 0, "the row is reused, not re-inserted");
    assert_eq!(report.marked_stale, 0, "{report:?}");
    assert_eq!(status_of(&db, thor).await, "pending");
    assert_eq!(
        rows(&db)
            .await
            .iter()
            .filter(|r| r.status == "pending")
            .count(),
        pending_before
    );

    // Reviewed rows never go stale.
    let (status, _) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/{thor}/reject"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    rename_series(&db, f.thor_2018, "Mighty Thor Reborn").await;
    relationship_suggest::run(&db, f.lib).await.unwrap();
    assert_eq!(status_of(&db, thor).await, "rejected");
}

/// WP-7.5: the continuation sources now emit `continues`, and an existing
/// `sequel_of` edge or a reviewed `sequel_of` row (pre-WP-7.5 rejection
/// memory) satisfies a `continues` suggestion for the same ordered pair —
/// and vice versa — so nothing is double-suggested.
#[tokio::test]
async fn sequel_of_and_continues_dedupe_each_other() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let f = fixture(&app, &db).await;
    // A legacy rejected `sequel_of` row for the Daredevil volumes.
    exec(
        &db,
        "INSERT INTO series_relationship_suggestion \
           (id, from_series_id, to_series_id, kind, confidence, bucket, reason, status) \
         VALUES ($1, $2, $3, 'sequel_of', 0.9, 'high', 'legacy', 'rejected')",
        vec![Uuid::now_v7().into(), f.dd_2014.into(), f.dd_2011.into()],
    )
    .await;
    // A manual `continues` edge between the Thor volumes, written the other
    // way round (2014 continued_by 2018).
    relationships::create_pair(
        &db,
        f.thor_2014,
        f.thor_2018,
        RelationshipKind::ContinuedBy,
        RelationshipSource::Manual,
        None,
        None,
    )
    .await
    .unwrap();

    let report = relationship_suggest::run(&db, f.lib).await.unwrap();
    let all = rows(&db).await;
    assert!(find(&all, f.dd_2014, f.dd_2011, "continues").is_none());
    assert!(find(&all, f.thor_2018, f.thor_2014, "continues").is_none());
    // The fixture's manual `sequel_of` Atlas edge covers `continues` too.
    assert!(find(&all, f.atlas_2009, f.atlas_2007, "continues").is_none());
    assert!(report.skipped_reviewed >= 1, "{report:?}");
    assert!(report.skipped_existing_edge >= 2, "{report:?}");
    // Other continuations are still proposed, as `continues`.
    let pv = find(&all, f.usm_relaunch, f.ucsm, "continues").expect("provider volume");
    assert_eq!(pv.status, "pending");
    assert!(
        all.iter()
            .all(|r| r.kind != "sequel_of" || r.status != "pending")
    );

    // Accepting a `continues` suggestion while a `has_sequel` edge already
    // runs the other way is a contradiction (409), across the two kinds.
    relationships::create_pair(
        &db,
        f.usm_relaunch,
        f.ucsm,
        RelationshipKind::HasSequel,
        RelationshipSource::Manual,
        None,
        None,
    )
    .await
    .unwrap();
    let err = server::relationships::suggestions::accept(&db, pv.id, Uuid::now_v7(), None).await;
    assert!(
        matches!(
            err,
            Err(server::relationships::suggestions::ReviewError::Pair(
                relationships::PairError::Conflict { .. }
            ))
        ),
        "{err:?}"
    );
}

// ───── WP-7.6 detectors ─────

/// Series with issues `numbers` in `year` (each `month` 1 unless given).
async fn mk_run(
    db: &DatabaseConnection,
    lib: Uuid,
    s: S,
    numbers: std::ops::RangeInclusive<i32>,
    year: i32,
) -> Uuid {
    let id = mk_series(db, lib, s).await;
    for n in numbers {
        mk_issue(
            db,
            lib,
            id,
            I {
                number: f64::from(n),
                year: Some(year),
                ..Default::default()
            },
        )
        .await;
    }
    id
}

fn s(name: &'static str, year: i32) -> S {
    S {
        name,
        year: Some(year),
        ..Default::default()
    }
}

fn dump(all: &[sug::Model]) -> String {
    all.iter()
        .map(|r| {
            format!(
                "{} {:.2} {:?} {:?}/{:?}/{:?} {}",
                r.kind, r.confidence, r.qualifier, r.from_range, r.to_range, r.coverage, r.reason
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn link_arc(db: &DatabaseConnection, issue: &str, arc: Uuid) {
    exec(
        db,
        "INSERT INTO issue_arcs (issue_id, arc_id) VALUES ($1, $2)",
        vec![issue.into(), arc.into()],
    )
    .await;
}

async fn arc_issue(
    db: &DatabaseConnection,
    lib: Uuid,
    series: Uuid,
    arc: Uuid,
    number: f64,
    (year, month): (i32, i32),
    title: Option<&'static str>,
) {
    let id = mk_issue(
        db,
        lib,
        series,
        I {
            number,
            year: Some(year),
            month: Some(month),
            title,
            ..Default::default()
        },
    )
    .await;
    link_arc(db, &id, arc).await;
}

fn arc_row(all: &[sug::Model], from: Uuid, arc: Uuid) -> Option<&sug::Model> {
    all.iter()
        .find(|r| r.from_series_id == from && r.to_arc_id == Some(arc))
}

#[tokio::test]
async fn annual_detector_picks_the_volume_whose_years_contain_it() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = mk_library(&app, &db, "annuals").await;
    // Two Avengers volumes; the 2012 annual sits inside the first.
    let av_2010 = mk_run(&db, lib, s("Avengers", 2010), 1..=3, 2010).await;
    exec(
        &db,
        "UPDATE issues SET year = 2012 WHERE series_id = $1 AND sort_number = 3",
        vec![av_2010.into()],
    )
    .await;
    let av_2013 = mk_run(&db, lib, s("Avengers", 2013), 1..=3, 2013).await;
    let av_annual = mk_run(&db, lib, s("Avengers Annual", 2012), 1..=1, 2012).await;
    // "The" is ignored when matching.
    let asm = mk_run(&db, lib, s("Amazing Spider-Man", 2018), 1..=5, 2018).await;
    let asm_annual = mk_run(
        &db,
        lib,
        s("The Amazing Spider-Man Annual", 2019),
        1..=1,
        2019,
    )
    .await;
    // Issues formatted Annual, no "Annual" in the name.
    let iron_man = mk_run(&db, lib, s("Iron Man", 1998), 1..=40, 1998).await;
    exec(
        &db,
        "UPDATE issues SET year = 2001 WHERE series_id = $1 AND sort_number > 20",
        vec![iron_man.into()],
    )
    .await;
    let im_1999 = mk_series(&db, lib, s("Iron Man 1999", 1999)).await;
    mk_issue(
        &db,
        lib,
        im_1999,
        I {
            number: 1.0,
            year: Some(1999),
            special_type: Some("Annual"),
            ..Default::default()
        },
    )
    .await;
    // Metron "Annual Series" series type.
    let thor = mk_run(&db, lib, s("Thor", 2007), 1..=12, 2007).await;
    let thor_annual = mk_run(
        &db,
        lib,
        S {
            series_type: Some("Annual Series"),
            ..s("Thor", 2008)
        },
        1..=1,
        2008,
    )
    .await;
    // Different publisher: no suggestion.
    mk_run(
        &db,
        lib,
        S {
            publisher: Some("DC Comics"),
            ..s("Hawkman Annual", 2010)
        },
        1..=1,
        2010,
    )
    .await;
    mk_run(&db, lib, s("Hawkman", 2010), 1..=6, 2010).await;
    // Two equally fitting volumes: lower confidence.
    let x_a = mk_run(&db, lib, s("X-Factor", 2005), 1..=3, 2005).await;
    let x_b = mk_run(&db, lib, s("X-Factor", 2005), 4..=6, 2005).await;
    let x_annual = mk_run(&db, lib, s("X-Factor Annual", 2005), 1..=1, 2005).await;

    relationship_suggest::run(&db, lib).await.unwrap();
    let all = rows(&db).await;
    let d = dump(&all);

    let a = find(&all, av_annual, av_2010, "annual_of").expect(&d);
    assert_eq!(a.bucket, "high", "{d}");
    assert!((a.confidence - 0.9).abs() < 1e-6, "{d}");
    assert!(
        a.reason.contains("within that volume's 2010–2012"),
        "{}",
        a.reason
    );
    assert!(find(&all, av_annual, av_2013, "annual_of").is_none(), "{d}");
    assert_eq!(sources(a), vec!["annual"]);

    assert!(find(&all, asm_annual, asm, "annual_of").is_some(), "{d}");
    let im = find(&all, im_1999, iron_man, "annual_of").expect(&d);
    assert!(
        find(&all, im_1999, iron_man, "continues").is_none(),
        "an annual is not the next volume: {d}"
    );
    assert_eq!(im.evidence["sources"][0]["signal"], "format");
    assert_eq!(im.bucket, "high", "{d}");
    let th = find(&all, thor_annual, thor, "annual_of").expect(&d);
    assert_eq!(th.evidence["sources"][0]["signal"], "seriestype");
    assert!(
        all.iter()
            .all(|r| r.kind != "annual_of" || !r.reason.contains("Hawkman")),
        "{d}"
    );
    let xf = all
        .iter()
        .find(|r| r.from_series_id == x_annual && r.kind == "annual_of")
        .expect(&d);
    assert!(xf.to_series_id == Some(x_a) || xf.to_series_id == Some(x_b));
    assert!(xf.confidence < 0.8, "ambiguous volume → not high: {d}");
}

#[tokio::test]
async fn arc_tie_ins_assign_roles_and_co_mains_cross_over() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = mk_library(&app, &db, "arcs").await;

    // Main by name: the ComicVine reading-list arc `"Secret Wars"
    // Battleworld` matches the "Secret Wars" series.
    let sw_arc = mk_arc(&db, "\"Secret Wars\" Battleworld").await;
    let sw = mk_series(&db, lib, s("Secret Wars", 2015)).await;
    for (n, m) in [(1.0, 5), (2.0, 6), (3.0, 7)] {
        arc_issue(&db, lib, sw, sw_arc, n, (2015, m), None).await;
    }
    let avengers = mk_series(&db, lib, s("Avengers", 2015)).await;
    arc_issue(&db, lib, avengers, sw_arc, 1.0, (2015, 6), None).await;
    arc_issue(&db, lib, avengers, sw_arc, 2.0, (2015, 7), None).await;
    // Prelude by series name, aftermath by issue title, prelude by date.
    let prelude = mk_series(&db, lib, s("Secret Wars Prelude", 2015)).await;
    arc_issue(&db, lib, prelude, sw_arc, 1.0, (2015, 5), None).await;
    let xmen = mk_series(&db, lib, s("X-Men", 2015)).await;
    arc_issue(
        &db,
        lib,
        xmen,
        sw_arc,
        7.0,
        (2015, 6),
        Some("Aftermath, Part 1"),
    )
    .await;
    let thor = mk_series(&db, lib, s("Thor", 2014)).await;
    arc_issue(&db, lib, thor, sw_arc, 8.0, (2015, 1), None).await;
    let later = mk_series(&db, lib, s("Squadron Supreme", 2016)).await;
    arc_issue(&db, lib, later, sw_arc, 1.0, (2016, 2), None).await;

    // Two co-main titles (3 + 3 issues) and a one-issue tie-in.
    let sv_arc = mk_arc(&db, "Spider-Verse").await;
    let spidey = mk_series(&db, lib, s("Spider-Man", 2014)).await;
    let woman = mk_series(&db, lib, s("Spider-Woman", 2014)).await;
    for n in 1..=3 {
        arc_issue(&db, lib, spidey, sv_arc, f64::from(n), (2014, 10 + n), None).await;
        arc_issue(&db, lib, woman, sv_arc, f64::from(n), (2014, 10 + n), None).await;
    }
    let scarlet = mk_series(&db, lib, s("Scarlet Spiders", 2014)).await;
    arc_issue(&db, lib, scarlet, sv_arc, 1.0, (2014, 11), None).await;

    // An arc inside one series suggests nothing.
    let solo_arc = mk_arc(&db, "Solo").await;
    let solo = mk_series(&db, lib, s("Daredevil", 2014)).await;
    arc_issue(&db, lib, solo, solo_arc, 1.0, (2014, 1), None).await;

    relationship_suggest::run(&db, lib).await.unwrap();
    let all = rows(&db).await;
    let d = dump(&all);
    let role = |s: Uuid, arc: Uuid| {
        let r = arc_row(&all, s, arc).expect(&d);
        assert_eq!(r.kind, "tie_in_to");
        (r.qualifier.clone().unwrap(), r.bucket.clone())
    };
    assert_eq!(role(sw, sw_arc), ("main".into(), "high".into()));
    assert_eq!(
        arc_row(&all, sw, sw_arc).unwrap().from_range.as_deref(),
        Some("1-3")
    );
    assert_eq!(role(avengers, sw_arc), ("tie_in".into(), "high".into()));
    assert_eq!(role(prelude, sw_arc).0, "prelude");
    assert_eq!(role(xmen, sw_arc).0, "aftermath");
    assert_eq!(
        role(thor, sw_arc).0,
        "prelude",
        "cover-dated before the main span"
    );
    assert_eq!(role(later, sw_arc).0, "aftermath", "cover-dated after");
    assert!(
        arc_row(&all, thor, sw_arc)
            .unwrap()
            .reason
            .contains("cover-dated before"),
        "{d}"
    );

    assert_eq!(role(spidey, sv_arc), ("main".into(), "medium".into()));
    assert_eq!(role(woman, sv_arc).0, "main");
    assert_eq!(role(scarlet, sv_arc), ("tie_in".into(), "medium".into()));
    let (a, b) = ordered(spidey, woman);
    let cross = find(&all, a, b, "crossover_with").expect(&d);
    assert_eq!(sources(cross), vec!["arc_crossover"]);
    // No pairwise crossover for plain tie-ins.
    assert_eq!(
        all.iter().filter(|r| r.kind == "crossover_with").count(),
        1,
        "{d}"
    );
    assert!(all.iter().all(|r| r.to_arc_id != Some(solo_arc)), "{d}");
}

#[tokio::test]
async fn reprint_rollup_and_citations_carry_ranges_and_coverage() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = mk_library(&app, &db, "reprints").await;
    let saga = mk_series(&db, lib, s("Saga", 2012)).await;
    let mut saga_issues = HashMap::new();
    for n in 1..=9 {
        let id = mk_issue(
            &db,
            lib,
            saga,
            I {
                number: f64::from(n),
                ..Default::default()
            },
        )
        .await;
        saga_issues.insert(n, id);
    }
    let reprint = |db: &DatabaseConnection, issue: String, of: String| {
        let db = db.clone();
        async move {
            exec(
                &db,
                "INSERT INTO issue_reprints (id, issue_id, reprinted_issue_id) VALUES ($1, $2, $3)",
                vec![Uuid::now_v7().into(), issue.into(), of.into()],
            )
            .await;
        }
    };
    // A TPB reprinting #1-6: collects, full.
    let tpb = mk_series(&db, lib, s("Saga Vol. 1 TPB", 2012)).await;
    let tpb1 = mk_issue(
        &db,
        lib,
        tpb,
        I {
            number: 1.0,
            format: Some("TPB"),
            ..Default::default()
        },
    )
    .await;
    for n in 1..=6 {
        reprint(&db, tpb1.clone(), saga_issues[&n].clone()).await;
    }
    // A singles reprint series covering #1-6 and #9 (7, 8 are in the
    // library but not reprinted): reprints, partial.
    let classic = mk_series(&db, lib, s("Saga Classic", 2020)).await;
    let c1 = mk_issue(
        &db,
        lib,
        classic,
        I {
            number: 1.0,
            ..Default::default()
        },
    )
    .await;
    let c2 = mk_issue(
        &db,
        lib,
        classic,
        I {
            number: 2.0,
            ..Default::default()
        },
    )
    .await;
    for n in 1..=6 {
        reprint(&db, c1.clone(), saga_issues[&n].clone()).await;
    }
    reprint(&db, c2.clone(), saga_issues[&9].clone()).await;
    // A label-only reprint row is ignored.
    exec(
        &db,
        "INSERT INTO issue_reprints (id, issue_id, reprinted_label) VALUES ($1, $2, 'Saga #7')",
        vec![Uuid::now_v7().into(), c2.clone().into()],
    )
    .await;

    // Citations: an HC series whose two issues cite #1-6 and #9-10 of
    // "Paper Girls": one suggestion, merged ranges, partial (gap 7-8).
    let pg = mk_series(&db, lib, s("Paper Girls", 2015)).await;
    for n in 1..=10 {
        mk_issue(
            &db,
            lib,
            pg,
            I {
                number: f64::from(n),
                ..Default::default()
            },
        )
        .await;
    }
    let pg_hc = mk_series(&db, lib, s("Paper Girls Deluxe HC", 2017)).await;
    for (n, note) in [
        (1.0, "Collects Paper Girls #1-6."),
        (2.0, "Collects Paper Girls #9-10."),
    ] {
        mk_issue(
            &db,
            lib,
            pg_hc,
            I {
                number: n,
                format: Some("Hardcover"),
                notes: Some(note),
                ..Default::default()
            },
        )
        .await;
    }

    relationship_suggest::run(&db, lib).await.unwrap();
    let all = rows(&db).await;
    let d = dump(&all);
    let col = find(&all, tpb, saga, "collects").expect(&d);
    assert_eq!(col.to_range.as_deref(), Some("1-6"), "{d}");
    assert_eq!(col.from_range.as_deref(), Some("1"), "{d}");
    assert_eq!(col.coverage.as_deref(), Some("full"), "{d}");
    assert_eq!(col.bucket, "high");
    assert_eq!(sources(col), vec!["reprint_rollup"]);
    let rp = find(&all, classic, saga, "reprints").expect(&d);
    assert_eq!(rp.to_range.as_deref(), Some("1-6,9"), "{d}");
    assert_eq!(rp.from_range.as_deref(), Some("1-2"), "{d}");
    assert_eq!(rp.coverage.as_deref(), Some("partial"), "{d}");
    assert_eq!(rp.bucket, "medium");
    assert_eq!(rp.evidence["sources"][0]["linked_issues"], 7);

    let cite = find(&all, pg_hc, pg, "collects").expect(&d);
    assert_eq!(cite.to_range.as_deref(), Some("1-6,9-10"), "{d}");
    assert_eq!(cite.from_range.as_deref(), Some("1-2"), "{d}");
    assert_eq!(cite.coverage.as_deref(), Some("partial"), "{d}");
    assert_eq!(
        all.iter()
            .filter(|r| r.from_series_id == pg_hc && r.kind == "collects")
            .count(),
        1,
        "citations roll up per pair"
    );
}

#[tokio::test]
async fn alternate_editions_facsimiles_supplements() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = mk_library(&app, &db, "editions").await;
    let hom = mk_run(&db, lib, s("House of M", 2005), 1..=8, 2005).await;
    let hom_dc = mk_run(&db, lib, s("House of M Director's Cut", 2005), 1..=1, 2005).await;
    // Content overlap only (years apart): The Walking Dead Deluxe.
    let twd = mk_run(&db, lib, s("The Walking Dead", 2003), 1..=12, 2003).await;
    let twd_deluxe = mk_run(&db, lib, s("The Walking Dead Deluxe", 2020), 1..=6, 2020).await;
    // "Absolute" singles are an event / a new line, not an edition.
    mk_run(&db, lib, s("Carnage", 2016), 1..=10, 2016).await;
    mk_run(&db, lib, s("Absolute Carnage", 2016), 1..=5, 2016).await;
    // Facsimile → reprints that one issue.
    let rom = mk_run(&db, lib, s("ROM", 1979), 1..=3, 1979).await;
    let rom_fax = mk_run(&db, lib, s("ROM #1 Facsimile Edition", 2019), 1..=1, 2019).await;
    // Supplements.
    let annihilation = mk_run(&db, lib, s("Annihilation", 2006), 1..=6, 2006).await;
    let saga = mk_run(&db, lib, s("Annihilation Saga", 2007), 1..=1, 2007).await;
    let ff = mk_run(&db, lib, s("Fantastic Four", 2018), 1..=12, 2018).await;
    let wedding = mk_run(
        &db,
        lib,
        s("Fantastic Four: Wedding Special", 2018),
        1..=1,
        2018,
    )
    .await;
    let marvel_u = mk_run(&db, lib, s("Marvel Universe", 1998), 1..=7, 1998).await;
    let ohotmu = mk_run(
        &db,
        lib,
        s("Official Handbook of the Marvel Universe", 2004),
        1..=1,
        2004,
    )
    .await;
    // "Saga" alone is a title, not a supplement.
    mk_run(&db, lib, s("Saga", 2012), 1..=3, 2012).await;

    relationship_suggest::run(&db, lib).await.unwrap();
    let all = rows(&db).await;
    let d = dump(&all);

    let (a, b) = ordered(hom, hom_dc);
    let alt = find(&all, a, b, "alternate_edition_of").expect(&d);
    assert_eq!(alt.bucket, "medium", "{d}");
    assert!(alt.reason.contains("Director's Cut"), "{}", alt.reason);
    let (a, b) = ordered(twd, twd_deluxe);
    assert!(find(&all, a, b, "alternate_edition_of").is_some(), "{d}");
    assert!(
        all.iter()
            .filter(|r| r.kind == "alternate_edition_of")
            .count()
            == 2,
        "Absolute Carnage is not an edition: {d}"
    );

    let fax = find(&all, rom_fax, rom, "reprints").expect(&d);
    assert_eq!(fax.to_range.as_deref(), Some("1"));
    assert_eq!(fax.coverage.as_deref(), Some("full"));
    assert_eq!(sources(fax), vec!["facsimile"]);
    let (a, b) = ordered(rom, rom_fax);
    assert!(find(&all, a, b, "alternate_edition_of").is_none());

    assert!(
        find(&all, saga, annihilation, "supplement_to").is_some(),
        "{d}"
    );
    assert!(find(&all, wedding, ff, "supplement_to").is_some(), "{d}");
    let hb = find(&all, ohotmu, marvel_u, "supplement_to").expect(&d);
    assert!(hb.reason.contains("handbook"), "{}", hb.reason);
    assert_eq!(
        all.iter().filter(|r| r.kind == "supplement_to").count(),
        3,
        "{d}"
    );
}

#[tokio::test]
async fn translation_detector_points_at_the_original() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = mk_library(&app, &db, "translations").await;
    // Same title, shared writer, French original (eng/fre codes fold).
    let fr = mk_run(
        &db,
        lib,
        S {
            lang: Some("fre"),
            ..s("Asterix", 1961)
        },
        1..=2,
        1961,
    )
    .await;
    let en = mk_run(
        &db,
        lib,
        S {
            lang: Some("eng"),
            ..s("Asterix", 1969)
        },
        1..=2,
        1969,
    )
    .await;
    for sid in [fr, en] {
        exec(
            &db,
            "INSERT INTO issue_credits (issue_id, role, person) \
             SELECT id, 'writer', 'René Goscinny' FROM issues WHERE series_id = $1",
            vec![sid.into()],
        )
        .await;
    }
    // Alias evidence: the English edition lists the original title.
    let schtroumpfs = mk_run(
        &db,
        lib,
        S {
            lang: Some("fr"),
            ..s("Les Schtroumpfs", 1959)
        },
        1..=1,
        1959,
    )
    .await;
    let smurfs = mk_run(
        &db,
        lib,
        S {
            lang: Some("en"),
            aliases: &["Les Schtroumpfs"],
            ..s("The Smurfs", 2010)
        },
        1..=1,
        2010,
    )
    .await;
    // Provider evidence: both claim ComicVine volume 777.
    let ll = mk_series(
        &db,
        lib,
        S {
            lang: Some("fr"),
            ..s("Lucky Luke", 1949)
        },
    )
    .await;
    let ll_en = mk_series(
        &db,
        lib,
        S {
            lang: Some("en"),
            ..s("Lucky Luke Adventures", 2006)
        },
    )
    .await;
    for (sid, n, y) in [(ll, 1.0, 1949), (ll_en, 2.0, 2006)] {
        mk_issue(
            &db,
            lib,
            sid,
            I {
                number: n,
                year: Some(y),
                cv_series: Some(777),
                ..Default::default()
            },
        )
        .await;
    }
    // Same title, same language: a continuation, not a translation.
    mk_run(&db, lib, s("Blacksad", 2000), 1..=1, 2000).await;
    mk_run(&db, lib, s("Blacksad", 2010), 1..=1, 2010).await;

    relationship_suggest::run(&db, lib).await.unwrap();
    let all = rows(&db).await;
    let d = dump(&all);
    let t = find(&all, en, fr, "translation_of").expect(&d);
    assert!(t.reason.contains("shared creator"), "{}", t.reason);
    assert!(
        find(&all, en, fr, "continues").is_none(),
        "language splits the continuation partition: {d}"
    );
    let a = find(&all, smurfs, schtroumpfs, "translation_of").expect(&d);
    assert_eq!(a.evidence["sources"][0]["evidence"], "alias");
    let p = find(&all, ll_en, ll, "translation_of").expect(&d);
    assert_eq!(p.evidence["sources"][0]["evidence"], "provider_series");
    assert!(
        find(&all, ll_en, ll, "continues").is_none() && find(&all, ll_en, ll, "see_also").is_none(),
        "provider volume skips cross-language claimants: {d}"
    );
    assert!(
        all.iter()
            .filter(|r| r.kind == "translation_of")
            .all(|r| r.bucket != "high"),
        "translations are medium or low: {d}"
    );
    assert_eq!(
        all.iter().filter(|r| r.kind == "translation_of").count(),
        3,
        "{d}"
    );
}

#[tokio::test]
async fn continuation_qualifiers() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib = mk_library(&app, &db, "qualifiers").await;
    // relaunch: restarts at #1 after the previous run ended.
    let hk_a = mk_run(&db, lib, s("Hawkeye", 2012), 1..=22, 2012).await;
    let hk_b = mk_run(&db, lib, s("Hawkeye", 2016), 1..=16, 2016).await;
    // numbering: the later volume's numbers continue the old ones.
    let thor_a = mk_run(&db, lib, s("Thor", 2007), 1..=12, 2007).await;
    let thor_b = mk_run(&db, lib, s("Thor", 2008), 600..=614, 2008).await;
    // split: a provider records the run as split (series_provider_range).
    let ff_a = mk_run(&db, lib, s("Fantastic Four", 1998), 1..=70, 1998).await;
    let ff_b = mk_run(&db, lib, s("Fantastic Four", 2013), 1..=16, 2013).await;
    exec(
        &db,
        "INSERT INTO series_provider_range (series_id, source, provider_series_id, range_low, range_high, set_by) \
         VALUES ($1, 'metron', '1713', '500', '611', 'cross_reference')",
        vec![ff_a.into()],
    )
    .await;
    // unsure: no issue numbers → no qualifier.
    let nova_a = mk_series(&db, lib, s("Nova", 2013)).await;
    let nova_b = mk_series(&db, lib, s("Nova", 2015)).await;
    // retitle: two names under one provider series.
    let usm = mk_series(&db, lib, s("Ultimate Spider-Man", 2000)).await;
    let ucsm = mk_series(&db, lib, s("Ultimate Comics Spider-Man", 2009)).await;
    for (sid, nums) in [(usm, 1..=3), (ucsm, 4..=6)] {
        for n in nums {
            mk_issue(
                &db,
                lib,
                sid,
                I {
                    number: f64::from(n),
                    cv_series: Some(5151),
                    ..Default::default()
                },
            )
            .await;
        }
    }

    relationship_suggest::run(&db, lib).await.unwrap();
    let all = rows(&db).await;
    let d = dump(&all);
    let q = |from: Uuid, to: Uuid| {
        find(&all, from, to, "continues")
            .expect(&d)
            .qualifier
            .clone()
    };
    assert_eq!(q(hk_b, hk_a).as_deref(), Some("relaunch"), "{d}");
    assert_eq!(q(thor_b, thor_a).as_deref(), Some("numbering"), "{d}");
    assert_eq!(q(ff_b, ff_a).as_deref(), Some("split"), "{d}");
    assert_eq!(q(nova_b, nova_a), None, "{d}");
    assert_eq!(q(ucsm, usm).as_deref(), Some("retitle"), "{d}");
}

/// WP-7.6 retired the pairwise `same_universe` sources (SeriesGroup star,
/// publisher + character density) and the pairwise arc → `crossover_with`
/// source. Their pending rows go stale on the next run through the normal
/// stale mechanism; reviewed rows and accepted edges are untouched.
#[tokio::test]
async fn retired_pairwise_suggestions_go_stale() {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let f = fixture(&app, &db).await;
    let legacy = |from: Uuid,
                  to: Uuid,
                  kind: &'static str,
                  status: &'static str,
                  src: &'static str| {
        let db = db.clone();
        async move {
            let (a, b) = ordered(from, to);
            let id = Uuid::now_v7();
            exec(
                &db,
                "INSERT INTO series_relationship_suggestion \
                   (id, from_series_id, to_series_id, kind, confidence, bucket, reason, evidence, status) \
                 VALUES ($1, $2, $3, $4, 0.6, 'medium', 'legacy', $5::jsonb, $6)",
                vec![
                    id.into(),
                    a.into(),
                    b.into(),
                    kind.into(),
                    serde_json::json!({"sources": [{"source": src}]}).to_string().into(),
                    status.into(),
                ],
            )
            .await;
            id
        }
    };
    let group = legacy(
        f.batman,
        f.nightwing,
        "same_universe",
        "pending",
        "series_group",
    )
    .await;
    let density = legacy(
        f.invincible,
        f.guarding,
        "same_universe",
        "pending",
        "character_density",
    )
    .await;
    let arc_cross = legacy(
        f.avengers,
        f.spider_man,
        "crossover_with",
        "pending",
        "story_arc",
    )
    .await;
    let accepted = legacy(
        f.saga,
        f.invincible,
        "same_universe",
        "accepted",
        "series_group",
    )
    .await;
    relationships::create_pair(
        &db,
        f.saga,
        f.invincible,
        RelationshipKind::SameUniverse,
        RelationshipSource::Suggested,
        Some(0.6),
        None,
    )
    .await
    .unwrap();

    let report = relationship_suggest::run(&db, f.lib).await.unwrap();
    assert!(report.failed_sources.is_empty(), "{report:?}");
    assert!(report.marked_stale >= 3, "{report:?}");
    for id in [group, density, arc_cross] {
        assert_eq!(status_of(&db, id).await, "stale");
    }
    assert_eq!(status_of(&db, accepted).await, "accepted");
    assert!(
        rel::Entity::find()
            .filter(rel::Column::FromSeriesId.eq(f.saga))
            .filter(rel::Column::Kind.eq("same_universe"))
            .one(&db)
            .await
            .unwrap()
            .is_some(),
        "accepted same_universe edges stay"
    );
    // The engine no longer produces any of these kinds pairwise.
    assert!(
        rows(&db)
            .await
            .iter()
            .filter(|r| r.status == "pending")
            .all(|r| r.kind != "same_universe"),
    );
}

#[tokio::test]
async fn accepting_scoped_and_arc_suggestions_creates_the_right_edges() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let f = fixture(&app, &db).await;
    let lib = f.lib;
    let hk_a = mk_run(&db, lib, s("Hawkeye", 2012), 1..=22, 2012).await;
    let hk_b = mk_run(&db, lib, s("Hawkeye", 2016), 1..=16, 2016).await;
    relationship_suggest::run(&db, lib).await.unwrap();
    let all = rows(&db).await;
    let arc_id = all
        .iter()
        .find(|r| r.from_series_id == f.avengers && r.to_arc_id.is_some())
        .unwrap()
        .to_arc_id
        .unwrap();
    let arc_sug = arc_row(&all, f.avengers, arc_id).unwrap().clone();
    let spidey_sug = arc_row(&all, f.spider_man, arc_id).unwrap().clone();
    let col = find(&all, f.saga_deluxe, f.saga, "collects")
        .unwrap()
        .clone();
    let hk = find(&all, hk_b, hk_a, "continues").unwrap().clone();

    // The list view exposes the arc target and the scope.
    let (status, body) = call_json(
        &app,
        Method::GET,
        &format!("/api/admin/relationship-suggestions?limit=200&library_id={lib}"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let item = |id: Uuid| {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == id.to_string())
            .cloned()
            .unwrap()
    };
    let v = item(arc_sug.id);
    assert!(v["to_series"].is_null());
    assert_eq!(v["to_arc"]["name"], "War of the Realms");
    assert_eq!(v["qualifier"], "main");
    assert_eq!(v["kind_label"], "Main story of");
    assert_eq!(v["from_range"], "1-3");
    let v = item(col.id);
    assert_eq!(v["to_range"], "1-6");
    assert_eq!(v["coverage"], "full");
    assert_eq!(item(hk.id)["qualifier_label"], "Relaunch");

    // Arc accept → one series → arc edge with the role and range; no
    // inverse.
    let (status, body) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/{}/accept", arc_sug.id),
        &admin,
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["inverse_id"].is_null());
    assert_eq!(body["created"], true);
    let edge = rel::Entity::find()
        .filter(rel::Column::FromSeriesId.eq(f.avengers))
        .filter(rel::Column::ToArcId.eq(arc_id))
        .one(&db)
        .await
        .unwrap()
        .expect("arc edge");
    assert_eq!(edge.kind, "tie_in_to");
    assert_eq!(edge.qualifier.as_deref(), Some("main"));
    assert_eq!(edge.from_range.as_deref(), Some("1-3"));
    assert_eq!(edge.source, "suggested");

    // Overriding an arc suggestion with a kind that can't target an arc → 422.
    let (status, body) = call_json(
        &app,
        Method::POST,
        &format!(
            "/api/admin/relationship-suggestions/{}/accept",
            spidey_sug.id
        ),
        &admin,
        Some(serde_json::json!({ "kind": "see_also" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(status_of(&db, spidey_sug.id).await, "pending");

    // Scoped series accept → both halves, the inverse mirrored.
    let (status, body) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/{}/accept", col.id),
        &admin,
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let fwd = rel::Entity::find()
        .filter(rel::Column::FromSeriesId.eq(f.saga_deluxe))
        .filter(rel::Column::ToSeriesId.eq(f.saga))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            fwd.kind.as_str(),
            fwd.from_range.as_deref(),
            fwd.to_range.as_deref(),
            fwd.coverage.as_deref()
        ),
        ("collects", Some("1"), Some("1-6"), Some("full"))
    );
    let inv = rel::Entity::find()
        .filter(rel::Column::FromSeriesId.eq(f.saga))
        .filter(rel::Column::ToSeriesId.eq(f.saga_deluxe))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            inv.kind.as_str(),
            inv.from_range.as_deref(),
            inv.to_range.as_deref(),
            inv.coverage.as_deref()
        ),
        ("collected_in", Some("1-6"), Some("1"), Some("full"))
    );

    // Edit kind: a qualifier the new kind doesn't take is dropped, not an
    // error.
    let (status, body) = call_json(
        &app,
        Method::POST,
        &format!("/api/admin/relationship-suggestions/{}/accept", hk.id),
        &admin,
        Some(serde_json::json!({ "kind": "sequel_of" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["suggestion"]["status"], "modified");
    let e = rel::Entity::find()
        .filter(rel::Column::FromSeriesId.eq(hk_b))
        .filter(rel::Column::ToSeriesId.eq(hk_a))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e.kind, "sequel_of");
    assert_eq!(e.qualifier, None);

    // Bulk accept handles arc suggestions too.
    let (status, body) = call_json(
        &app,
        Method::POST,
        "/api/admin/relationship-suggestions/bulk-accept",
        &admin,
        Some(serde_json::json!({ "ids": [spidey_sug.id] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["created"], 1, "{body}");
    assert!(
        rel::Entity::find()
            .filter(rel::Column::FromSeriesId.eq(f.spider_man))
            .filter(rel::Column::ToArcId.eq(arc_id))
            .filter(rel::Column::Qualifier.eq("tie_in"))
            .one(&db)
            .await
            .unwrap()
            .is_some()
    );

    // A rerun doesn't re-propose accepted arc edges.
    let report = relationship_suggest::run(&db, lib).await.unwrap();
    assert_eq!(report.inserted, 0, "{report:?}");
    assert!(report.skipped_existing_edge >= 2, "{report:?}");
}
