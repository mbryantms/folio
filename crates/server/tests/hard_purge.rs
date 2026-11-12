//! Roadmap WP-3.5 — daily hard-purge of confirmed-removed rows.
//!
//! Validates:
//!   - issues confirmed-removed longer than `soft_delete_days × multiplier`
//!     are hard-deleted, with their markers / progress / ratings /
//!     external ids cascading (FK) or cleaned (polymorphic refs)
//!   - rows inside the window, soft-deleted-but-unconfirmed rows, and
//!     active rows all survive
//!   - a confirmed-removed series is purged only once it has no issue rows
//!   - every purged entity lands a `library_events` row (`action = purged`)
//!   - multiplier `0` disables the sweep
//!   - a confirmed row whose file is back on disk is kept, loses its
//!     confirmation and gets a series rescan; a series whose folder holds
//!     archives again is kept the same way
//!   - a library whose root fails validation is skipped whole
//!   - kept (unknown) rows at the front of the order do not hide the
//!     purgeable rows behind them (keyset paging), and the per-run cap
//!     counts deletes, oldest confirmation first
//!   - every FK into `issues` / `series` is CASCADE or SET NULL, so the
//!     sweep's DELETE can never be blocked by a RESTRICT reference

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use common::TestApp;
use entity::library::ActiveModel as LibraryAM;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DbBackend, Set, Statement};
use server::jobs::hard_purge;
use server::library::scanner;
use std::io::Write;
use std::path::Path;
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
}

async fn register_user(app: &TestApp) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"purge@example.com","password":"correctly-horse-battery"}"#,
                ))
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
            .map(|c| {
                c.split(';')
                    .next()
                    .unwrap()
                    .trim_start_matches(prefix)
                    .to_owned()
            })
            .expect(prefix)
    };
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn send(
    app: &TestApp,
    auth: &Authed,
    method: Method,
    uri: &str,
    body: serde_json::Value,
) -> StatusCode {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!(
                        "__Host-comic_session={}; __Host-comic_csrf={}",
                        auth.session, auth.csrf
                    ),
                )
                .header("X-CSRF-Token", &auth.csrf)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let _ = to_bytes(resp.into_body(), usize::MAX).await;
    status
}

fn write_cbz(path: &Path, marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    zw.finish().unwrap();
}

async fn create_library(app: &TestApp, root: &Path, soft_delete_days: i32) -> Uuid {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    LibraryAM {
        id: Set(id),
        name: Set(format!("Purge Lib {id}")),
        root_path: Set(root.to_string_lossy().into_owned()),
        default_language: Set("eng".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(true),
        slug: Set(id.to_string()),
        scan_schedule_cron: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_scan_at: Set(None),
        ignore_globs: Set(serde_json::json!([])),
        report_missing_comicinfo: Set(false),
        file_watch_enabled: Set(true),
        soft_delete_days: Set(soft_delete_days),
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
    .insert(&db)
    .await
    .unwrap();
    id
}

async fn exec(db: &impl ConnectionTrait, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

async fn count(db: &impl ConnectionTrait, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get_by_index::<i64>(0).unwrap()
}

/// `(issue id, issue slug, series slug)` for the file whose path ends in
/// `file_name`.
async fn issue_by_file(db: &impl ConnectionTrait, file_name: &str) -> (String, String, String) {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT i.id, i.slug, s.slug FROM issues i JOIN series s ON s.id = i.series_id \
             WHERE i.file_path LIKE $1",
            [format!("%/{file_name}").into()],
        ))
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("no issue for {file_name}"));
    (
        row.try_get_by_index(0).unwrap(),
        row.try_get_by_index(1).unwrap(),
        row.try_get_by_index(2).unwrap(),
    )
}

/// Stamp an issue's removal timestamps (days ago; `None` = NULL).
async fn set_removal(
    db: &impl ConnectionTrait,
    table: &str,
    id_col_value: sea_orm::Value,
    removed_days_ago: Option<i64>,
    confirmed_days_ago: Option<i64>,
) {
    let now = Utc::now().fixed_offset();
    let at = |d: Option<i64>| d.map(|d| now - Duration::days(d));
    exec(
        db,
        &format!("UPDATE {table} SET removed_at = $1, removal_confirmed_at = $2 WHERE id = $3"),
        vec![
            at(removed_days_ago).into(),
            at(confirmed_days_ago).into(),
            id_col_value,
        ],
    )
    .await;
}

async fn issue_exists(db: &impl ConnectionTrait, id: &str) -> bool {
    count(
        db,
        "SELECT count(*) FROM issues WHERE id = $1",
        vec![id.into()],
    )
    .await
        == 1
}

#[tokio::test]
async fn purge_respects_window_cascades_and_writes_library_events() {
    let app = TestApp::spawn().await;
    let auth = register_user(&app).await;
    let tmp = tempfile::tempdir().unwrap();

    // Alpha: four issues in different lifecycle states. Beta: one issue,
    // the whole series gone. A third series keeps the root non-empty.
    let alpha = tmp.path().join("Alpha (2020)");
    let beta = tmp.path().join("Beta (2021)");
    let gamma = tmp.path().join("Gamma (2022)");
    for d in [&alpha, &beta, &gamma] {
        std::fs::create_dir_all(d).unwrap();
    }
    for (i, name) in [
        "Alpha 001.cbz",
        "Alpha 002.cbz",
        "Alpha 003.cbz",
        "Alpha 004.cbz",
    ]
    .iter()
    .enumerate()
    {
        write_cbz(&alpha.join(name), i as u32 + 1);
    }
    write_cbz(&beta.join("Beta 001.cbz"), 10);
    write_cbz(&gamma.join("Gamma 001.cbz"), 20);

    // soft_delete_days = 10, multiplier 2 → purge window = 20 days after
    // confirmation.
    let lib_id = create_library(&app, tmp.path(), 10).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let db = &state.db;

    let (a1, a1_slug, alpha_slug) = issue_by_file(db, "Alpha 001.cbz").await;
    let (a2, ..) = issue_by_file(db, "Alpha 002.cbz").await;
    let (a3, ..) = issue_by_file(db, "Alpha 003.cbz").await;
    let (a4, ..) = issue_by_file(db, "Alpha 004.cbz").await;
    let (b1, b1_slug, beta_slug) = issue_by_file(db, "Beta 001.cbz").await;
    let (g1, ..) = issue_by_file(db, "Gamma 001.cbz").await;

    // User data on the issues that will be purged (created while active,
    // the way it happens in real life) plus on a survivor.
    for id in [&a1, &a2, &b1] {
        assert_eq!(
            send(
                &app,
                &auth,
                Method::POST,
                "/api/me/markers",
                serde_json::json!({ "issue_id": id, "page_index": 0, "kind": "bookmark" }),
            )
            .await,
            StatusCode::CREATED
        );
        assert!(
            send(
                &app,
                &auth,
                Method::POST,
                "/api/progress",
                serde_json::json!({ "issue_id": id, "page": 0, "finished": false }),
            )
            .await
            .is_success()
        );
    }
    assert!(
        send(
            &app,
            &auth,
            Method::PUT,
            &format!("/api/series/{alpha_slug}/issues/{a1_slug}/rating"),
            serde_json::json!({ "rating": 4.0 }),
        )
        .await
        .is_success()
    );
    assert!(
        send(
            &app,
            &auth,
            Method::PUT,
            &format!("/api/series/{beta_slug}/rating"),
            serde_json::json!({ "rating": 3.0 }),
        )
        .await
        .is_success()
    );
    // Polymorphic provider id on the purged issue (no FK — the sweep must
    // clean it, or a re-import of the same book would collide on the
    // `UNIQUE (source, external_id, entity_type)` slot).
    exec(
        db,
        "INSERT INTO external_ids (entity_type, entity_id, source, external_id, set_by) \
         VALUES ('issue', $1, 'comicvine', '999001', 'provider')",
        vec![a1.clone().into()],
    )
    .await;
    let _ = b1_slug;

    // Lifecycle states (days ago):
    //   a1  removed 60 / confirmed 30  → outside window  → PURGED
    //   a2  removed 30 / confirmed 5   → inside window   → survives
    //   a3  removed 90 / unconfirmed   → never purged    → survives
    //   a4  active                                        → survives
    //   b1  removed 60 / confirmed 30  → PURGED, then series Beta (confirmed
    //       30 ago, now empty) → PURGED
    //   Alpha series confirmed 30 ago but still owns a2..a4 → survives
    set_removal(db, "issues", a1.clone().into(), Some(60), Some(30)).await;
    set_removal(db, "issues", a2.clone().into(), Some(30), Some(5)).await;
    set_removal(db, "issues", a3.clone().into(), Some(90), None).await;
    set_removal(db, "issues", b1.clone().into(), Some(60), Some(30)).await;
    let series_id = |slug: String| async move {
        let row = db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM series WHERE slug = $1",
                [slug.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        row.try_get_by_index::<Uuid>(0).unwrap()
    };
    let alpha_id = series_id(alpha_slug.clone()).await;
    let beta_id = series_id(beta_slug.clone()).await;
    set_removal(db, "series", alpha_id.into(), Some(60), Some(30)).await;
    set_removal(db, "series", beta_id.into(), Some(60), Some(30)).await;
    // An FK-less pointer at a1 from a survivor — must be cleared, not left
    // dangling.
    exec(
        db,
        "UPDATE issues SET superseded_by = $1 WHERE id = $2",
        vec![a1.clone().into(), g1.clone().into()],
    )
    .await;

    // The purged / later-purged rows' files are really gone: the sweep
    // stats every candidate and keeps one whose file is back on disk.
    for name in ["Alpha 001.cbz", "Alpha 002.cbz"] {
        std::fs::remove_file(alpha.join(name)).unwrap();
    }
    std::fs::remove_file(beta.join("Beta 001.cbz")).unwrap();

    // ── Multiplier 0 disables the sweep entirely.
    let stats = hard_purge::run_with(&state, 0, Utc::now().fixed_offset())
        .await
        .unwrap();
    assert_eq!(stats, hard_purge::PurgeStats::default());
    assert!(issue_exists(db, &a1).await);

    // ── A larger multiplier (window 40d) keeps a1 (confirmed 30d ago).
    let stats = hard_purge::run_with(&state, 4, Utc::now().fixed_offset())
        .await
        .unwrap();
    assert_eq!(
        stats.issues, 0,
        "30d-old confirmation is inside a 40d window"
    );
    assert!(issue_exists(db, &a1).await);

    // ── Default multiplier 2 (window 20d).
    let stats = hard_purge::run_with(&state, 2, Utc::now().fixed_offset())
        .await
        .unwrap();
    assert_eq!(stats.issues, 2, "a1 + b1 purged: {stats:?}");
    assert_eq!(stats.series, 1, "only the emptied Beta series: {stats:?}");
    assert_eq!(stats.libraries, 1);

    assert!(!issue_exists(db, &a1).await, "a1 outside window → purged");
    assert!(!issue_exists(db, &b1).await, "b1 outside window → purged");
    assert!(issue_exists(db, &a2).await, "a2 inside window → kept");
    assert!(issue_exists(db, &a3).await, "a3 unconfirmed → kept");
    assert!(issue_exists(db, &a4).await, "a4 active → kept");
    assert!(issue_exists(db, &g1).await, "g1 active → kept");
    assert_eq!(
        count(
            db,
            "SELECT count(*) FROM series WHERE id = $1",
            vec![beta_id.into()]
        )
        .await,
        0,
        "empty confirmed series purged"
    );
    assert_eq!(
        count(
            db,
            "SELECT count(*) FROM series WHERE id = $1",
            vec![alpha_id.into()]
        )
        .await,
        1,
        "confirmed series that still owns issues is kept"
    );

    // Cascades: the purged issues' user data is gone; the survivor's isn't.
    let purged = vec![a1.clone().into(), b1.clone().into()];
    assert_eq!(
        count(
            db,
            "SELECT count(*) FROM markers WHERE issue_id IN ($1, $2)",
            purged.clone()
        )
        .await,
        0
    );
    assert_eq!(
        count(
            db,
            "SELECT count(*) FROM progress_records WHERE issue_id IN ($1, $2)",
            purged.clone()
        )
        .await,
        0
    );
    assert_eq!(
        count(
            db,
            "SELECT count(*) FROM markers WHERE issue_id = $1",
            vec![a2.clone().into()]
        )
        .await,
        1
    );
    assert_eq!(
        count(
            db,
            "SELECT count(*) FROM progress_records WHERE issue_id = $1",
            vec![a2.clone().into()]
        )
        .await,
        1
    );
    // Polymorphic refs cleaned in the same transaction.
    assert_eq!(
        count(
            db,
            "SELECT count(*) FROM user_ratings WHERE target_id IN ($1, $2)",
            vec![a1.clone().into(), beta_id.to_string().into()]
        )
        .await,
        0
    );
    assert_eq!(
        count(
            db,
            "SELECT count(*) FROM external_ids WHERE entity_type = 'issue' AND entity_id = $1",
            vec![a1.clone().into()]
        )
        .await,
        0
    );

    assert_eq!(
        count(
            db,
            "SELECT count(*) FROM issues WHERE id = $1 AND superseded_by IS NULL",
            vec![g1.clone().into()]
        )
        .await,
        1,
        "dangling superseded_by pointer cleared"
    );

    // Library events: one `purged` row per purged entity, carrying the
    // pre-delete export counts.
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT category, entity_id, detail FROM library_events \
             WHERE library_id = $1 AND action = 'purged' ORDER BY category, entity_id",
            [lib_id.into()],
        ))
        .await
        .unwrap();
    let events: Vec<(String, String, serde_json::Value)> = rows
        .iter()
        .map(|r| {
            (
                r.try_get_by_index(0).unwrap(),
                r.try_get_by_index(1).unwrap(),
                r.try_get_by_index(2).unwrap(),
            )
        })
        .collect();
    assert_eq!(events.len(), 3, "{events:?}");
    let issue_ev = |id: &str| {
        events
            .iter()
            .find(|(c, e, _)| c == "issue" && e == id)
            .unwrap_or_else(|| panic!("no purged event for issue {id}: {events:?}"))
            .2
            .clone()
    };
    let a1_detail = issue_ev(&a1);
    assert_eq!(a1_detail["markers"], 1);
    assert_eq!(a1_detail["progress_records"], 1);
    assert_eq!(a1_detail["ratings"], 1);
    assert_eq!(a1_detail["window_days"], 20);
    assert!(
        a1_detail["file_path"]
            .as_str()
            .unwrap()
            .ends_with("Alpha 001.cbz")
    );
    let _ = issue_ev(&b1);
    let (_, _, beta_detail) = events
        .iter()
        .find(|(c, e, _)| c == "series" && *e == beta_id.to_string())
        .expect("purged event for series Beta");
    assert_eq!(beta_detail["ratings"], 1);

    // Idempotent: a second run finds nothing more.
    let stats = hard_purge::run_with(&state, 2, Utc::now().fixed_offset())
        .await
        .unwrap();
    assert_eq!(stats, hard_purge::PurgeStats::default());

    // Time passes: once a2's confirmation ages past the window it goes too;
    // a3 (never confirmed) still doesn't.
    let later = Utc::now().fixed_offset() + Duration::days(30);
    let stats = hard_purge::run_with(&state, 2, later).await.unwrap();
    assert_eq!(stats.issues, 1);
    assert!(!issue_exists(db, &a2).await);
    assert!(issue_exists(db, &a3).await);
    assert!(issue_exists(db, &a4).await);
}

/// Every FK into `issues` / `series` must be `ON DELETE CASCADE` or
/// `SET NULL`: the purge relies on the database to take the dependent rows.
/// A new `RESTRICT` / `NO ACTION` reference would make the daily sweep fail
/// for every library that has a purgeable row — decide the cascade policy
/// when adding the FK (and teach `jobs::hard_purge` if it needs code-side
/// cleanup).
#[tokio::test]
async fn fk_references_all_cascade_or_set_null() {
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT conrelid::regclass::text, confrelid::regclass::text, \
                    confdeltype::text, conname::text \
             FROM pg_constraint \
             WHERE contype = 'f' \
               AND confrelid IN ('issues'::regclass, 'series'::regclass) \
             ORDER BY 1, 4",
        ))
        .await
        .unwrap();
    assert!(!rows.is_empty(), "expected FKs referencing issues/series");
    let offenders: Vec<String> = rows
        .iter()
        .filter_map(|r| {
            let table: String = r.try_get_by_index(0).unwrap();
            let target: String = r.try_get_by_index(1).unwrap();
            let del: String = r.try_get_by_index(2).unwrap();
            let name: String = r.try_get_by_index(3).unwrap();
            // c = CASCADE, n = SET NULL, d = SET DEFAULT
            (!matches!(del.as_str(), "c" | "n" | "d"))
                .then(|| format!("{table}.{name} → {target} (confdeltype={del})"))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "FKs that would block a hard purge: {offenders:#?}"
    );
}

/// Id-shaped columns with **no** FK don't cascade, so every one must be
/// either cleaned by the sweep (`hard_purge::POLYMORPHIC_REFS` /
/// `UNCONSTRAINED_ISSUE_DELETES` / `UNCONSTRAINED_ISSUE_NULLIFIES`) or listed
/// here as deliberately left alone. A new FK-less `issue_id` column fails
/// this test until its purge policy is decided — the way `progress_records`
/// (no FK at all) would otherwise have silently kept dangling rows.
#[tokio::test]
async fn unconstrained_id_columns_are_accounted_for() {
    /// History / provider-id columns that must NOT be touched by a purge.
    const INTENTIONALLY_KEPT: &[&str] = &[
        // Keyed on entity rows (character / team / location / story_arc),
        // never an issue or series; those rows outlive any purge.
        "entity_profile.entity_id",
        // Append-only history: keeps the id of what it described.
        "audit_log.target_id",
        "library_events.entity_id",
        "scan_runs.issue_id",
        "scan_runs.series_id",
        // Provider-side ids (ComicVine / Metron), not local rows.
        "cbl_entries.cv_issue_id",
        "cbl_entries.cv_series_id",
        "cbl_entries.metron_issue_id",
        "cbl_entries.metron_series_id",
        "series_provider_range.provider_series_id",
        "series_external_relationship.provider_series_id",
    ];
    let app = TestApp::spawn().await;
    let db = &app.state().db;
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT c.table_name::text || '.' || c.column_name::text \
             FROM information_schema.columns c \
             JOIN information_schema.tables t \
               ON t.table_schema = c.table_schema AND t.table_name = c.table_name \
              AND t.table_type = 'BASE TABLE' \
             WHERE c.table_schema = 'public' \
               AND (c.column_name LIKE '%issue_id' OR c.column_name LIKE '%series_id' \
                    OR c.column_name IN ('entity_id', 'target_id', 'superseded_by')) \
               AND NOT EXISTS ( \
                   SELECT 1 FROM pg_constraint k \
                   JOIN pg_attribute a ON a.attrelid = k.conrelid AND a.attnum = ANY(k.conkey) \
                   WHERE k.contype = 'f' \
                     AND k.conrelid = (quote_ident(c.table_schema) || '.' || quote_ident(c.table_name))::regclass \
                     AND a.attname = c.column_name) \
             ORDER BY 1",
        ))
        .await
        .unwrap();
    let handled: Vec<String> = hard_purge::POLYMORPHIC_REFS
        .iter()
        .map(|(t, _, c)| format!("{t}.{c}"))
        .chain(
            hard_purge::UNCONSTRAINED_ISSUE_DELETES
                .iter()
                .chain(hard_purge::UNCONSTRAINED_ISSUE_NULLIFIES)
                .map(|(t, c)| format!("{t}.{c}")),
        )
        .collect();
    let unaccounted: Vec<String> = rows
        .iter()
        .map(|r| r.try_get_by_index::<String>(0).unwrap())
        .filter(|col| !handled.contains(col) && !INTENTIONALLY_KEPT.contains(&col.as_str()))
        .collect();
    assert!(
        unaccounted.is_empty(),
        "FK-less id columns with no hard-purge policy: {unaccounted:#?} — add them to \
         jobs::hard_purge (clean) or INTENTIONALLY_KEPT here (history / provider id)"
    );
}

async fn removal_confirmed(db: &impl ConnectionTrait, id: &str) -> bool {
    count(
        db,
        "SELECT count(*) FROM issues WHERE id = $1 AND removal_confirmed_at IS NOT NULL",
        vec![id.into()],
    )
    .await
        == 1
}

async fn series_scan_runs(db: &impl ConnectionTrait, series_id: Uuid) -> i64 {
    count(
        db,
        "SELECT count(*) FROM scan_runs WHERE series_id = $1 AND kind = 'series'",
        vec![series_id.into()],
    )
    .await
}

async fn series_id_of_issue(db: &impl ConnectionTrait, issue_id: &str) -> Uuid {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT series_id FROM issues WHERE id = $1",
            [issue_id.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get_by_index::<Uuid>(0).unwrap()
}

/// A confirmed row past the purge window whose file is on disk again is not
/// deleted: its confirmation is cleared and its series gets one rescan. One
/// whose file is absent is deleted with its dependents as before. A removed
/// series with no issue rows whose folder holds an archive again is kept the
/// same way; one whose folder is empty is purged.
#[tokio::test]
async fn purge_keeps_rows_whose_files_are_back() {
    let app = TestApp::spawn().await;
    let auth = register_user(&app).await;
    let tmp = tempfile::tempdir().unwrap();
    let delta = tmp.path().join("Delta (2020)");
    let eta = tmp.path().join("Eta (2021)");
    let theta = tmp.path().join("Theta (2022)");
    for d in [&delta, &eta, &theta] {
        std::fs::create_dir_all(d).unwrap();
    }
    write_cbz(&delta.join("Delta 001.cbz"), 1);
    write_cbz(&delta.join("Delta 002.cbz"), 2);
    write_cbz(&delta.join("Delta 003.cbz"), 3);
    write_cbz(&eta.join("Eta 001.cbz"), 4);
    write_cbz(&theta.join("Theta 001.cbz"), 5);

    let lib_id = create_library(&app, tmp.path(), 10).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let db = &state.db;

    let (back, ..) = issue_by_file(db, "Delta 001.cbz").await;
    let (gone, ..) = issue_by_file(db, "Delta 002.cbz").await;
    let (e1, _, eta_slug) = issue_by_file(db, "Eta 001.cbz").await;
    let (t1, _, theta_slug) = issue_by_file(db, "Theta 001.cbz").await;
    let delta_id = series_id_of_issue(db, &back).await;
    let eta_id = series_id_of_issue(db, &e1).await;
    let theta_id = series_id_of_issue(db, &t1).await;
    let _ = (eta_slug, theta_slug);

    // User data on the row that will be purged.
    assert!(
        send(
            &app,
            &auth,
            Method::POST,
            "/api/progress",
            serde_json::json!({ "issue_id": gone, "page": 0, "finished": false }),
        )
        .await
        .is_success()
    );

    // `back`: confirmed long ago while its share was away, file present now.
    set_removal(db, "issues", back.clone().into(), Some(60), Some(30)).await;
    // `gone`: really gone.
    std::fs::remove_file(delta.join("Delta 002.cbz")).unwrap();
    set_removal(db, "issues", gone.clone().into(), Some(60), Some(30)).await;

    // Eta and Theta: removed series whose issue rows were already purged.
    // Eta's folder holds an archive again; Theta's folder is empty.
    std::fs::remove_file(theta.join("Theta 001.cbz")).unwrap();
    exec(
        db,
        "DELETE FROM issues WHERE id IN ($1, $2)",
        vec![e1.clone().into(), t1.clone().into()],
    )
    .await;
    set_removal(db, "series", eta_id.into(), Some(60), Some(30)).await;
    set_removal(db, "series", theta_id.into(), Some(60), Some(30)).await;
    // Past the sweeps' rescan throttle.
    exec(
        db,
        "UPDATE series SET last_scanned_at = now() - interval '7 days' WHERE library_id = $1",
        vec![lib_id.into()],
    )
    .await;

    let stats = hard_purge::run_with(&state, 2, Utc::now().fixed_offset())
        .await
        .unwrap();
    assert_eq!(stats.issues, 1, "{stats:?}");
    assert_eq!(stats.series, 1, "only Theta: {stats:?}");
    assert_eq!(stats.kept_present, 2, "back + Eta: {stats:?}");
    assert_eq!(stats.rescans_queued, 2, "Delta + Eta: {stats:?}");

    assert!(issue_exists(db, &back).await, "file on disk: kept");
    assert!(
        !removal_confirmed(db, &back).await,
        "confirmation cleared so the confirm sweep re-evaluates it"
    );
    assert!(!issue_exists(db, &gone).await, "absent file: purged");
    assert_eq!(
        count(
            db,
            "SELECT count(*) FROM progress_records WHERE issue_id = $1",
            vec![gone.clone().into()]
        )
        .await,
        0,
        "dependents purged with it"
    );
    assert_eq!(series_scan_runs(db, delta_id).await, 1);

    assert_eq!(
        count(
            db,
            "SELECT count(*) FROM series WHERE id = $1 AND removal_confirmed_at IS NULL",
            vec![eta_id.into()]
        )
        .await,
        1,
        "series with an archive in its folder kept, confirmation cleared"
    );
    assert_eq!(series_scan_runs(db, eta_id).await, 1);
    assert_eq!(
        count(
            db,
            "SELECT count(*) FROM series WHERE id = $1",
            vec![theta_id.into()]
        )
        .await,
        0,
        "empty folder: series purged"
    );
    assert_eq!(
        count(
            db,
            "SELECT count(*) FROM library_events \
             WHERE library_id = $1 AND action = 'skipped'",
            vec![lib_id.into()]
        )
        .await,
        1,
        "one summary event for the kept rows"
    );

    // Nothing left to purge or keep: the kept rows are unconfirmed now.
    let stats = hard_purge::run_with(&state, 2, Utc::now().fixed_offset())
        .await
        .unwrap();
    assert_eq!(stats, hard_purge::PurgeStats::default());
}

/// A library whose root is missing is skipped whole: nothing is deleted.
#[tokio::test]
async fn purge_skips_a_library_whose_root_is_missing() {
    let app = TestApp::spawn().await;
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("lib");
    let iota = root.join("Iota (2020)");
    std::fs::create_dir_all(&iota).unwrap();
    write_cbz(&iota.join("Iota 001.cbz"), 1);
    write_cbz(&iota.join("Iota 002.cbz"), 2);

    let lib_id = create_library(&app, &root, 10).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let db = &state.db;
    let (i1, ..) = issue_by_file(db, "Iota 001.cbz").await;
    let (i2, ..) = issue_by_file(db, "Iota 002.cbz").await;
    let iota_id = series_id_of_issue(db, &i1).await;
    set_removal(db, "issues", i1.clone().into(), Some(60), Some(30)).await;
    set_removal(db, "issues", i2.clone().into(), Some(60), Some(30)).await;
    set_removal(db, "series", iota_id.into(), Some(60), Some(30)).await;

    // The share goes away: every file looks absent.
    std::fs::rename(&root, parent.path().join("lib-unmounted")).unwrap();

    let stats = hard_purge::run_with(&state, 2, Utc::now().fixed_offset())
        .await
        .unwrap();
    assert_eq!(stats.libraries_skipped, 1, "{stats:?}");
    assert_eq!(stats.issues, 0, "{stats:?}");
    assert_eq!(stats.series, 0, "{stats:?}");
    assert!(issue_exists(db, &i1).await);
    assert!(issue_exists(db, &i2).await);
    assert!(removal_confirmed(db, &i1).await, "left exactly as it was");
    assert_eq!(
        count(
            db,
            "SELECT count(*) FROM library_events \
             WHERE library_id = $1 AND action = 'skipped'",
            vec![lib_id.into()]
        )
        .await,
        1
    );
}

/// Restores a directory's mode when dropped.
struct ModeGuard(std::path::PathBuf);
impl Drop for ModeGuard {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
    }
}

/// An issue whose series folder cannot be read is unknown, not gone: the
/// purge keeps it confirmed and undeleted and queues no rescan. A file under
/// a readable folder that is really gone is still purged.
#[tokio::test]
async fn purge_keeps_rows_it_cannot_read() {
    use std::os::unix::fs::PermissionsExt;
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let locked = tmp.path().join("Kappa (2020)");
    let open = tmp.path().join("Lambda (2021)");
    for d in [&locked, &open] {
        std::fs::create_dir_all(d).unwrap();
    }
    write_cbz(&locked.join("Kappa 001.cbz"), 1);
    write_cbz(&open.join("Lambda 001.cbz"), 2);
    write_cbz(&open.join("Lambda 002.cbz"), 3);
    let lib_id = create_library(&app, tmp.path(), 10).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let db = &state.db;
    let (k1, ..) = issue_by_file(db, "Kappa 001.cbz").await;
    let (l1, ..) = issue_by_file(db, "Lambda 001.cbz").await;
    let kappa_id = series_id_of_issue(db, &k1).await;
    set_removal(db, "issues", k1.clone().into(), Some(60), Some(30)).await;
    std::fs::remove_file(open.join("Lambda 001.cbz")).unwrap();
    set_removal(db, "issues", l1.clone().into(), Some(60), Some(30)).await;

    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _guard = ModeGuard(locked.clone());
    if std::fs::read_dir(&locked).is_ok() {
        // Skipped: running with privileges (root) that ignore directory
        // modes, so the folder cannot be made unreadable.
        return;
    }

    let stats = hard_purge::run_with(&state, 2, Utc::now().fixed_offset())
        .await
        .unwrap();
    assert_eq!(stats.issues, 1, "{stats:?}");
    assert_eq!(stats.kept_unknown, 1, "{stats:?}");
    assert_eq!(stats.kept_present, 0, "{stats:?}");
    assert_eq!(stats.rescans_queued, 0, "{stats:?}");
    assert!(issue_exists(db, &k1).await, "unreadable: kept");
    assert!(
        removal_confirmed(db, &k1).await,
        "unknown keeps its confirmation"
    );
    assert!(!issue_exists(db, &l1).await, "really gone: purged");
    assert_eq!(series_scan_runs(db, kappa_id).await, 0);
}

/// A root that is an empty directory (the `RootEmpty` validation result,
/// an unmounted share's mount point) is skipped by both sweeps.
#[tokio::test]
async fn empty_root_is_skipped_by_both_sweeps() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let mu = tmp.path().join("Mu (2020)");
    std::fs::create_dir_all(&mu).unwrap();
    write_cbz(&mu.join("Mu 001.cbz"), 1);
    write_cbz(&mu.join("Mu 002.cbz"), 2);
    let lib_id = create_library(&app, tmp.path(), 10).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let db = &state.db;
    let (m1, ..) = issue_by_file(db, "Mu 001.cbz").await;
    let (m2, ..) = issue_by_file(db, "Mu 002.cbz").await;
    // m1 waits for confirmation, m2 for the purge.
    set_removal(db, "issues", m1.clone().into(), Some(60), None).await;
    set_removal(db, "issues", m2.clone().into(), Some(60), Some(30)).await;

    // Everything under the root is gone; the root itself is an empty dir.
    std::fs::remove_dir_all(&mu).unwrap();
    assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());

    let confirm = server::library::reconcile::auto_confirm_sweep(&state)
        .await
        .unwrap();
    assert_eq!(confirm.libraries_skipped, 1, "{confirm:?}");
    assert_eq!(confirm.confirmed, 0, "{confirm:?}");
    assert!(!removal_confirmed(db, &m1).await);

    let purge = hard_purge::run_with(&state, 2, Utc::now().fixed_offset())
        .await
        .unwrap();
    assert_eq!(purge.libraries_skipped, 1, "{purge:?}");
    assert_eq!(purge.issues, 0, "{purge:?}");
    assert!(issue_exists(db, &m2).await);
}

/// A library with `kept` confirmed issues at the front of the purge order
/// whose disk answer is unknown, and `gone` confirmed issues behind them
/// whose files are really gone. Returns `(state, kept ids, gone ids)`, each
/// oldest confirmation first.
///
/// The unknown answer comes from a folder link that points at itself (every
/// stat through it fails with `ELOOP`, which is not the not-found class), so
/// unlike a chmod-ed folder it holds when the tests run as root.
async fn kept_front_fixture(
    app: &TestApp,
    root: &Path,
    kept: usize,
    gone: usize,
) -> (server::state::AppState, Vec<String>, Vec<String>) {
    let nu = root.join("Nu (2020)");
    let xi = root.join("Xi (2021)");
    let omicron = root.join("Omicron (2022)");
    for d in [&nu, &xi, &omicron] {
        std::fs::create_dir_all(d).unwrap();
    }
    for k in 1..=kept {
        write_cbz(&nu.join(format!("Nu {k:03}.cbz")), 100 + k as u32);
    }
    for k in 1..=gone {
        write_cbz(&xi.join(format!("Xi {k:03}.cbz")), 200 + k as u32);
    }
    write_cbz(&omicron.join("Omicron 001.cbz"), 300);
    let lib_id = create_library(app, root, 10).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let db = &state.db;

    std::os::unix::fs::symlink("Loop", root.join("Loop")).unwrap();
    let mut kept_ids = Vec::new();
    for k in 1..=kept {
        let name = format!("Nu {k:03}.cbz");
        let (id, ..) = issue_by_file(db, &name).await;
        // Confirmed 100, 99, … days ago: ahead of every `gone` row.
        set_removal(
            db,
            "issues",
            id.clone().into(),
            Some(120),
            Some(100 - k as i64),
        )
        .await;
        exec(
            db,
            "UPDATE issues SET file_path = $1 WHERE id = $2",
            vec![
                root.join("Loop")
                    .join(&name)
                    .to_string_lossy()
                    .into_owned()
                    .into(),
                id.clone().into(),
            ],
        )
        .await;
        kept_ids.push(id);
    }
    let mut gone_ids = Vec::new();
    for k in 1..=gone {
        let name = format!("Xi {k:03}.cbz");
        let (id, ..) = issue_by_file(db, &name).await;
        std::fs::remove_file(xi.join(&name)).unwrap();
        // Confirmed 50, 49, … days ago: all past the 20-day window.
        set_removal(
            db,
            "issues",
            id.clone().into(),
            Some(60),
            Some(50 - k as i64),
        )
        .await;
        gone_ids.push(id);
    }
    (state, kept_ids, gone_ids)
}

/// More kept (unknown) candidates at the front of the order than fit in one
/// page must not hide the purgeable rows behind them: one run pages past the
/// kept rows, purges every gone row, and leaves the kept rows confirmed.
#[tokio::test]
async fn purge_pages_past_kept_rows_at_the_front() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (state, kept, gone) = kept_front_fixture(&app, tmp.path(), 7, 4).await;
    let db = &state.db;

    let limits = hard_purge::PurgeLimits {
        page: 3,
        max_issue_deletes: 100,
    };
    let stats = hard_purge::run_with_limits(&state, 2, Utc::now().fixed_offset(), limits)
        .await
        .unwrap();
    assert_eq!(
        stats.issues, 4,
        "every gone row behind the kept ones: {stats:?}"
    );
    assert_eq!(stats.kept_unknown, 7, "{stats:?}");
    assert_eq!(stats.kept_present, 0, "{stats:?}");
    for id in &gone {
        assert!(!issue_exists(db, id).await, "{id}: really gone → purged");
    }
    for id in &kept {
        assert!(issue_exists(db, id).await, "{id}: unknown → kept");
        assert!(
            removal_confirmed(db, id).await,
            "{id}: unknown keeps its confirmation"
        );
    }

    // The next run only re-examines the kept rows.
    let stats = hard_purge::run_with_limits(&state, 2, Utc::now().fixed_offset(), limits)
        .await
        .unwrap();
    assert_eq!(stats.issues, 0, "{stats:?}");
    assert_eq!(stats.kept_unknown, 7, "{stats:?}");
}

/// The per-run cap counts deletes, oldest confirmation first, and holds even
/// when it falls inside a page; kept rows do not count against it, and the
/// rest go on the next run.
#[tokio::test]
async fn purge_deletes_at_most_the_cap_per_run() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (state, kept, gone) = kept_front_fixture(&app, tmp.path(), 2, 5).await;
    let db = &state.db;

    // Page 1: two kept + gone[0..2]; page 2: gone[2..5], cut off after one.
    let limits = hard_purge::PurgeLimits {
        page: 4,
        max_issue_deletes: 3,
    };
    let log = hard_purge_log();
    let stats = hard_purge::run_with_limits(&state, 2, Utc::now().fixed_offset(), limits)
        .await
        .unwrap();
    assert_eq!(stats.issues, 3, "the cap: {stats:?}");
    assert_eq!(stats.kept_unknown, 2, "{stats:?}");
    // Issue ids are content hashes, shared with other tests' fixtures; the
    // library id (a fresh UUID) picks out this test's own export lines.
    let lib_id = library_of_issue(db, &kept[0]).await;
    let exported = log.lines_with(&[
        "hard purge: deleting confirmed-removed issue",
        &format!("library_id={lib_id}"),
    ]);
    // Positive first, so the negative checks below can never pass because
    // the capture saw nothing.
    for id in &gone[..3] {
        assert!(!issue_exists(db, id).await, "{id}: oldest first → purged");
        assert_eq!(purged_events(db, id).await, 1, "{id}: one purged event");
        assert!(
            exported.iter().any(|l| l.contains(id.as_str())),
            "{id}: exported before the delete: {exported:#?}"
        );
    }
    for id in &gone[3..] {
        assert!(issue_exists(db, id).await, "{id}: past the cap → next run");
        assert!(removal_confirmed(db, id).await, "{id}: still confirmed");
        assert_eq!(purged_events(db, id).await, 0, "{id}: no purged event");
        assert!(
            !exported.iter().any(|l| l.contains(id.as_str())),
            "{id}: past the cap, so not exported: {exported:#?}"
        );
    }
    assert_eq!(exported.len(), 3, "{exported:#?}");

    let stats = hard_purge::run_with_limits(&state, 2, Utc::now().fixed_offset(), limits)
        .await
        .unwrap();
    assert_eq!(stats.issues, 2, "the rest: {stats:?}");
    for id in &gone {
        assert!(!issue_exists(db, id).await, "{id}: purged by now");
    }
    for id in &kept {
        assert!(issue_exists(db, id).await, "{id}: unknown → kept");
        assert!(removal_confirmed(db, id).await, "{id}: still confirmed");
    }
}

/// `library_events` rows recording `issue_id` as purged.
async fn purged_events(db: &impl ConnectionTrait, issue_id: &str) -> i64 {
    count(
        db,
        "SELECT count(*) FROM library_events \
         WHERE action = 'purged' AND category = 'issue' AND entity_id = $1",
        vec![issue_id.into()],
    )
    .await
}

async fn library_of_issue(db: &impl ConnectionTrait, issue_id: &str) -> Uuid {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT library_id FROM issues WHERE id = $1",
            [issue_id.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get_by_index::<Uuid>(0).unwrap()
}

/// Every `folio::hard_purge` WARN line any test in this binary writes,
/// collected by one **process-wide** subscriber installed on first use.
///
/// Global, not a thread-local `set_default`: tracing caches each callsite's
/// interest the first time any thread reaches it, and while a single
/// dispatcher is registered it computes that interest from the *reaching
/// thread's* default (tracing-core `Dispatchers::has_just_one`). A parallel
/// test's thread with no subscriber could get to the export callsite first
/// and cache "never" for everyone. A global default is what every thread
/// sees, so the cached interest is right whoever gets there first.
///
/// If something else had already installed a global subscriber this one is
/// not set and the capture stays empty; the callers' positive assertions
/// then fail loudly rather than the negative ones passing vacuously.
fn hard_purge_log() -> &'static LogCapture {
    static LOG: std::sync::OnceLock<LogCapture> = std::sync::OnceLock::new();
    LOG.get_or_init(|| {
        let capture = LogCapture::default();
        let writer = capture.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_env_filter(tracing_subscriber::EnvFilter::new("folio::hard_purge=warn"))
            .with_writer(move || writer.clone())
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
        capture
    })
}

/// A shared in-memory log sink (one `write` per formatted event).
#[derive(Clone, Default)]
struct LogCapture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogCapture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl LogCapture {
    /// Captured lines containing every one of `needles`.
    fn lines_with(&self, needles: &[&str]) -> Vec<String> {
        String::from_utf8_lossy(&self.0.lock().unwrap())
            .lines()
            .filter(|l| needles.iter().all(|n| l.contains(n)))
            .map(str::to_owned)
            .collect()
    }
}

/// Committed deletes are counted even when a later page fails. The database
/// itself refuses to delete one row of the second page (a test-only
/// trigger), so that page's `DELETE` errors after the first page's has
/// committed: the run reports exactly the rows that are gone, counts the
/// library as aborted, and has a purged event for each of them.
#[tokio::test]
async fn purge_counts_committed_deletes_when_a_later_page_fails() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (state, _kept, gone) = kept_front_fixture(&app, tmp.path(), 0, 4).await;
    let db = &state.db;
    exec(
        db,
        &format!(
            "CREATE FUNCTION test_refuse_issue_delete() RETURNS trigger \
             LANGUAGE plpgsql AS $$ BEGIN \
               IF OLD.id = '{}' THEN RAISE EXCEPTION 'delete refused by test'; END IF; \
               RETURN OLD; END $$",
            gone[2]
        ),
        vec![],
    )
    .await;
    exec(
        db,
        "CREATE TRIGGER test_refuse_issue_delete BEFORE DELETE ON issues \
         FOR EACH ROW EXECUTE FUNCTION test_refuse_issue_delete()",
        vec![],
    )
    .await;

    let limits = hard_purge::PurgeLimits {
        page: 2,
        max_issue_deletes: 100,
    };
    let stats = hard_purge::run_with_limits(&state, 2, Utc::now().fixed_offset(), limits)
        .await
        .unwrap();
    assert_eq!(stats.issues, 2, "page 1 committed: {stats:?}");
    assert_eq!(stats.libraries, 1, "{stats:?}");
    assert_eq!(stats.libraries_aborted, 1, "{stats:?}");
    for id in &gone[..2] {
        assert!(!issue_exists(db, id).await, "{id}: page 1 → purged");
        assert_eq!(purged_events(db, id).await, 1, "{id}");
    }
    for id in &gone[2..] {
        assert!(issue_exists(db, id).await, "{id}: page 2 rolled back");
        assert!(removal_confirmed(db, id).await, "{id}: still confirmed");
        assert_eq!(purged_events(db, id).await, 0, "{id}");
    }
}

/// Candidates with the same `removal_confirmed_at` across a page boundary
/// are told apart by `id`: none is skipped and none examined twice.
#[tokio::test]
async fn purge_pages_through_equal_confirmation_times() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let (state, kept, gone) = kept_front_fixture(&app, tmp.path(), 1, 3).await;
    let db = &state.db;
    let at = Utc::now().fixed_offset() - Duration::days(40);
    for id in kept.iter().chain(&gone) {
        exec(
            db,
            "UPDATE issues SET removal_confirmed_at = $1 WHERE id = $2",
            vec![at.into(), id.clone().into()],
        )
        .await;
    }

    // Four rows, one timestamp, pages of two: the boundary falls between
    // equal times.
    let limits = hard_purge::PurgeLimits {
        page: 2,
        max_issue_deletes: 100,
    };
    let stats = hard_purge::run_with_limits(&state, 2, Utc::now().fixed_offset(), limits)
        .await
        .unwrap();
    assert_eq!(stats.issues, 3, "{stats:?}");
    assert_eq!(stats.kept_unknown, 1, "examined once: {stats:?}");
    for id in &gone {
        assert!(!issue_exists(db, id).await, "{id}: purged");
    }
    assert!(issue_exists(db, &kept[0]).await);
    assert!(removal_confirmed(db, &kept[0]).await);
}
