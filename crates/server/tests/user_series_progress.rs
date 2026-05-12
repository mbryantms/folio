//! Saved smart views — M2: integration coverage for the
//! `user_series_progress` SQL view + the helpers in
//! `server::reading::series_progress`.
//!
//! Each test stands up a fresh TestApp (real Postgres via testcontainers),
//! seeds a series with N issues, marks some finished + some not for one
//! user, and verifies the view returns the correct rollup.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::{Duration, Utc};
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library,
    progress_record::{self, ActiveModel as ProgressAM},
    reading_session::ActiveModel as ReadingSessionAM,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, Database, Set,
    sea_query::{Alias, Expr, Query},
};
use server::reading::series_progress;
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn register_user(app: &TestApp, email: &str) -> Uuid {
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
    let json = body_json(resp.into_body()).await;
    Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap()
}

/// Seed one library + one series + `count` active issues. Returns
/// `(library_id, series_id, vec_of_issue_ids)`.
async fn seed_series_with_issues(
    app: &TestApp,
    name: &str,
    count: usize,
) -> (Uuid, Uuid, Vec<String>) {
    let db = Database::connect(&app.db_url).await.unwrap();
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
    }
    .insert(&db)
    .await
    .unwrap();

    let series_id = Uuid::now_v7();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set(format!("Series {name}")),
        normalized_name: Set(normalize_name(&format!("Series {name}"))),
        year: Set(None),
        volume: Set(None),
        publisher: Set(None),
        imprint: Set(None),
        status: Set("continuing".into()),
        total_issues: Set(None),
        age_rating: Set(None),
        summary: Set(None),
        language_code: Set("en".into()),
        comicvine_id: Set(None),
        metron_id: Set(None),
        gtin: Set(None),
        series_group: Set(None),
        slug: Set(series_id.to_string()),
        alternate_names: Set(serde_json::json!([])),
        created_at: Set(now),
        updated_at: Set(now),
        folder_path: Set(None),
        last_scanned_at: Set(None),
        match_key: Set(None),
        removed_at: Set(None),
        removal_confirmed_at: Set(None),
        status_user_set_at: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    let mut issue_ids = Vec::with_capacity(count);
    for i in 0..count {
        // BLAKE3-shaped 64-char hex id; uniqueness comes from the index.
        let issue_id = format!("{:0>62}{:02x}", Uuid::now_v7().simple(), i as u8);
        IssueAM {
            id: Set(issue_id.clone()),
            library_id: Set(lib_id),
            series_id: Set(series_id),
            slug: Set(format!("{name}-{i}")),
            file_path: Set(format!("/tmp/{name}/issue-{i}.cbz")),
            file_size: Set(1),
            file_mtime: Set(now),
            state: Set("active".into()),
            content_hash: Set(issue_id.clone()),
            title: Set(None),
            sort_number: Set(Some(i as f64 + 1.0)),
            number_raw: Set(Some(format!("{}", i + 1))),
            volume: Set(None),
            year: Set(None),
            month: Set(None),
            day: Set(None),
            summary: Set(None),
            notes: Set(None),
            language_code: Set(None),
            format: Set(None),
            black_and_white: Set(None),
            manga: Set(None),
            age_rating: Set(None),
            page_count: Set(Some(20)),
            pages: Set(serde_json::json!([])),
            comic_info_raw: Set(serde_json::json!({})),
            alternate_series: Set(None),
            story_arc: Set(None),
            story_arc_number: Set(None),
            characters: Set(None),
            teams: Set(None),
            locations: Set(None),
            tags: Set(None),
            genre: Set(None),
            writer: Set(None),
            penciller: Set(None),
            inker: Set(None),
            colorist: Set(None),
            letterer: Set(None),
            cover_artist: Set(None),
            editor: Set(None),
            translator: Set(None),
            publisher: Set(None),
            imprint: Set(None),
            scan_information: Set(None),
            community_rating: Set(None),
            review: Set(None),
            web_url: Set(None),
            comicvine_id: Set(None),
            metron_id: Set(None),
            gtin: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            removed_at: Set(None),
            removal_confirmed_at: Set(None),
            superseded_by: Set(None),
            special_type: Set(None),
            hash_algorithm: Set(1),
            thumbnails_generated_at: Set(None),
            thumbnail_version: Set(0),
            thumbnails_error: Set(None),
            additional_links: Set(serde_json::json!([])),
            user_edited: Set(serde_json::json!([])),
            comicinfo_count: Set(None),
        }
        .insert(&db)
        .await
        .unwrap();
        issue_ids.push(issue_id);
    }

    (lib_id, series_id, issue_ids)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_for_series_returns_finished_count_and_percent() {
    let app = TestApp::spawn().await;
    let user_a = register_user(&app, "alice@example.com").await;
    let (_lib, series_id, issue_ids) = seed_series_with_issues(&app, "view-percent", 10).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();

    // Five finished, two in-progress, three untouched.
    for (i, issue_id) in issue_ids.iter().enumerate() {
        let finished = i < 5;
        let last_page = if i < 7 { 10 } else { 0 };
        if !finished && last_page == 0 {
            continue;
        }
        ProgressAM {
            user_id: Set(user_a),
            issue_id: Set(issue_id.clone()),
            last_page: Set(last_page),
            percent: Set(if finished { 100.0 } else { 50.0 }),
            finished: Set(finished),
            updated_at: Set(now),
            device: Set(None),
        }
        .insert(&db)
        .await
        .unwrap();
    }

    let row = series_progress::fetch_for_series(&db, user_a, series_id)
        .await
        .unwrap()
        .expect("user A has progress");
    assert_eq!(row.finished_count, 5);
    assert_eq!(row.total_count, 10);
    assert_eq!(row.percent, 50);
    assert!(row.last_read_at.is_none(), "no reading_sessions yet → null");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_read_at_tracks_max_heartbeat() {
    let app = TestApp::spawn().await;
    let user_a = register_user(&app, "bob@example.com").await;
    let (_lib, series_id, issue_ids) = seed_series_with_issues(&app, "view-heartbeat", 3).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();

    // Need at least one progress_record so the row appears in the view.
    ProgressAM {
        user_id: Set(user_a),
        issue_id: Set(issue_ids[0].clone()),
        last_page: Set(5),
        percent: Set(50.0),
        finished: Set(false),
        updated_at: Set(now),
        device: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    // Two sessions on the same series. The view should surface MAX.
    let earlier = now - Duration::hours(2);
    let later = now - Duration::minutes(5);
    for (issue_id, heartbeat) in [(&issue_ids[0], earlier), (&issue_ids[1], later)] {
        ReadingSessionAM {
            id: Set(Uuid::now_v7()),
            user_id: Set(user_a),
            issue_id: Set(issue_id.clone()),
            series_id: Set(series_id),
            client_session_id: Set(Uuid::new_v4().to_string()),
            started_at: Set(heartbeat),
            ended_at: Set(Some(heartbeat)),
            last_heartbeat_at: Set(heartbeat),
            active_ms: Set(60_000),
            distinct_pages_read: Set(5),
            page_turns: Set(5),
            start_page: Set(0),
            end_page: Set(5),
            furthest_page: Set(5),
            device: Set(None),
            view_mode: Set(None),
            client_meta: Set(serde_json::json!({})),
        }
        .insert(&db)
        .await
        .unwrap();
    }

    let row = series_progress::fetch_for_series(&db, user_a, series_id)
        .await
        .unwrap()
        .unwrap();
    let observed = row.last_read_at.expect("heartbeat surfaced");
    let drift = (observed - later).num_milliseconds().abs();
    assert!(
        drift < 1_000,
        "expected last_read_at within 1s of latest heartbeat; drift={drift}ms",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_for_user_with_no_progress_returns_none() {
    let app = TestApp::spawn().await;
    let user_b = register_user(&app, "carol@example.com").await;
    let (_lib, series_id, _issues) = seed_series_with_issues(&app, "view-none", 5).await;
    let db = Database::connect(&app.db_url).await.unwrap();

    let row = series_progress::fetch_for_series(&db, user_b, series_id)
        .await
        .unwrap();
    assert!(row.is_none(), "no progress records → no row");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subquery_left_joins_to_zero_for_unstarted_series() {
    // Verifies the M3-facing access pattern: LEFT JOIN the subquery onto
    // series, COALESCE percent to 0, then filter. Unstarted series must
    // satisfy "percent <= 50" and "percent >= 0".
    let app = TestApp::spawn().await;
    let user_b = register_user(&app, "dan@example.com").await;
    let (_lib, series_id, _issues) = seed_series_with_issues(&app, "view-leftjoin", 4).await;
    let db = Database::connect(&app.db_url).await.unwrap();

    use entity::series;
    use sea_orm::FromQueryResult;

    #[derive(Debug, FromQueryResult)]
    struct Row {
        series_id: Uuid,
        coalesced_percent: i64,
    }

    let usp = series_progress::subquery_alias();
    let stmt = Query::select()
        .expr_as(
            Expr::col((series::Entity, series::Column::Id)),
            Alias::new("series_id"),
        )
        .expr_as(
            Expr::cust("COALESCE(usp.percent, 0)"),
            Alias::new("coalesced_percent"),
        )
        .from(series::Entity)
        .join_subquery(
            sea_orm::JoinType::LeftJoin,
            series_progress::subquery_for(user_b),
            usp.clone(),
            Expr::col((usp.clone(), Alias::new("series_id")))
                .equals((series::Entity, series::Column::Id)),
        )
        .and_where(Expr::col((series::Entity, series::Column::Id)).eq(series_id))
        .to_owned();

    let backend = db.get_database_backend();
    let row = Row::find_by_statement(backend.build(&stmt))
        .one(&db)
        .await
        .unwrap()
        .expect("series row joined");
    assert_eq!(row.series_id, series_id);
    assert_eq!(row.coalesced_percent, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_fetch_keys_by_series_id() {
    let app = TestApp::spawn().await;
    let user = register_user(&app, "eve@example.com").await;
    let (_lib_a, sid_a, issues_a) = seed_series_with_issues(&app, "batch-a", 4).await;
    let (_lib_b, sid_b, _issues_b) = seed_series_with_issues(&app, "batch-b", 2).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();

    // Mark 2 of A's 4 issues finished. Series B is untouched.
    for issue_id in &issues_a[..2] {
        ProgressAM {
            user_id: Set(user),
            issue_id: Set(issue_id.clone()),
            last_page: Set(20),
            percent: Set(100.0),
            finished: Set(true),
            updated_at: Set(now),
            device: Set(None),
        }
        .insert(&db)
        .await
        .unwrap();
    }

    let map = series_progress::fetch_for_series_batch(&db, user, &[sid_a, sid_b])
        .await
        .unwrap();
    assert_eq!(map.len(), 1, "only series A appears");
    let a = map.get(&sid_a).expect("A present");
    assert_eq!(a.finished_count, 2);
    assert_eq!(a.total_count, 4);
    assert_eq!(a.percent, 50);
    assert!(!map.contains_key(&sid_b), "B has no progress → absent");
}

// Hush the unused-imports lint when adding rows directly via SeaORM.
#[allow(unused_imports)]
use progress_record as _progress;
