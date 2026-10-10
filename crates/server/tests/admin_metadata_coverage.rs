//! `GET /admin/metadata/coverage` — the Coverage tab's snapshot: identifier
//! coverage per provider, completeness tiers, per-library and per-series
//! gaps, tag sources, tool state and fill-cost estimates. Admin only,
//! cached in Redis, `?refresh=true` recomputes.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::{TestApp, seed};
use sea_orm::{ActiveModelTrait, Set};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn register(app: &TestApp, email: &str) -> String {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"email": email, "password": "correctly-horse-battery"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|c| c.split(';').next())
        .filter(|c| c.starts_with("__Host-comic_session=") || c.starts_with("__Host-comic_csrf="))
        .collect::<Vec<_>>()
        .join("; ")
}

async fn get(app: &TestApp, cookie: &str, path: &str) -> (StatusCode, Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(path)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn external_id(
    db: &sea_orm::DatabaseConnection,
    entity_type: &str,
    entity_id: &str,
    source: &str,
) {
    let now = chrono::Utc::now().fixed_offset();
    entity::external_id::ActiveModel {
        entity_type: Set(entity_type.into()),
        entity_id: Set(entity_id.to_owned()),
        source: Set(source.into()),
        // Unique per (source, external_id, entity_type).
        external_id: Set(format!("{entity_id}-{source}")),
        external_url: Set(None),
        set_by: Set("user".into()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

/// Give an issue the core fields the completeness tier needs (the seed
/// already sets `page_count`).
async fn fill_core(db: &sea_orm::DatabaseConnection, id: &str, summary: bool) {
    entity::issue::ActiveModel {
        id: Set(id.to_owned()),
        year: Set(Some(1990)),
        summary: Set(summary.then(|| "A summary.".to_owned())),
        writer: Set(Some("Someone".into())),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();
}

fn by_source<'a>(rows: &'a Value, source: &str) -> &'a Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r["source"] == source)
        .unwrap_or_else(|| panic!("no row for {source}: {rows}"))
}

#[tokio::test]
async fn coverage_snapshot_counts_ids_tiers_and_gaps() {
    let app = TestApp::spawn().await;
    let cookie = register(&app, "admin@example.com").await;
    let db = &app.state().db;
    let dir = tempfile::tempdir().unwrap();
    let lib = seed::seed_library(db, dir.path()).await;
    let linked = seed::seed_series(db, lib, "Linked Run").await;
    let lonely = seed::seed_series(db, lib, "Lonely Run").await;
    external_id(db, "series", &linked.to_string(), "comicvine").await;

    // Linked #1: all three ids + every core field → complete.
    let a1 = seed::seed_issue(db, lib, linked, &dir.path().join("a1.cbz"), b"a1", 1.0).await;
    for src in ["comicvine", "metron", "gcd"] {
        external_id(db, "issue", &a1, src).await;
    }
    fill_core(db, &a1, true).await;
    // Linked #2: no id at all → needs metadata.
    let a2 = seed::seed_issue(db, lib, linked, &dir.path().join("a2.cbz"), b"a2", 2.0).await;
    fill_core(db, &a2, true).await;
    // Lonely #1: a Metron id, no summary → partial.
    let b1 = seed::seed_issue(db, lib, lonely, &dir.path().join("b1.cbz"), b"b1", 1.0).await;
    external_id(db, "issue", &b1, "metron").await;
    fill_core(db, &b1, false).await;

    let (st, body) = get(&app, &cookie, "/api/admin/metadata/coverage").await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["cached"], false);
    assert_eq!(body["series_total"], 2);
    assert_eq!(body["issues_total"], 3);
    assert_eq!(body["libraries_total"], 1);

    let ids = &body["identifiers"];
    assert_eq!(by_source(ids, "comicvine")["series_with_id"], 1);
    assert_eq!(by_source(ids, "comicvine")["issues_with_id"], 1);
    assert_eq!(by_source(ids, "metron")["series_with_id"], 0);
    assert_eq!(by_source(ids, "metron")["issues_with_id"], 2);
    assert_eq!(by_source(ids, "gcd")["issues_with_id"], 1);
    assert_eq!(body["all_sources_series"], 0);
    assert_eq!(body["all_sources_issues"], 1);

    assert_eq!(body["tiers"]["complete"], 1, "{}", body["tiers"]);
    assert_eq!(body["tiers"]["partial"], 1, "{}", body["tiers"]);
    assert_eq!(body["tiers"]["needs_metadata"], 1, "{}", body["tiers"]);
    assert_eq!(body["tiers"]["accepted"], 0);
    // Among the two incomplete: one lacks an id, one lacks a summary.
    let missing = &body["missing_core"];
    let field = |k: &str| {
        missing
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["field"] == k)
            .map(|r| r["issues"].as_i64().unwrap())
            .unwrap_or_else(|| panic!("no missing_core row {k}: {missing}"))
    };
    assert_eq!(field("external_id"), 1);
    assert_eq!(field("summary"), 1);
    assert_eq!(field("credits"), 0);
    assert_eq!(field("page_count"), 0);

    // Per library + per series.
    let libs = body["by_library"].as_array().unwrap();
    assert_eq!(libs.len(), 1);
    assert_eq!(libs[0]["issues_total"], 3);
    assert_eq!(libs[0]["without_comicvine"], 2);
    assert_eq!(libs[0]["without_metron"], 1);
    assert_eq!(libs[0]["without_gcd"], 2);
    let top_cv = by_source(&body["top_series"], "comicvine")["series"]
        .as_array()
        .unwrap()
        .clone();
    let names: Vec<&str> = top_cv.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["Linked Run", "Lonely Run"], "{top_cv:?}");
    assert_eq!(top_cv[0]["issues_without_id"], 1);

    // Tag sources partition every active issue.
    let tagged: i64 = body["tag_sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["issues"].as_i64().unwrap())
        .sum();
    assert_eq!(tagged, 3, "{}", body["tag_sources"]);

    assert_eq!(body["tools"]["libraries_total"], 1);
    assert_eq!(body["tools"]["auto_apply_libraries"], 0);
    assert_eq!(body["tools"]["weekly_refresh_enabled"], false);

    let cost = by_source(&body["fill_cost"], "metron");
    assert_eq!(cost["issues_without_id"], 1);
    assert_eq!(cost["series_without_id"], 2);
    // 2 series searches + one list page per series.
    assert_eq!(cost["via_lists_requests"], 4);
    assert_eq!(cost["via_detail_requests"], 1);
    assert_eq!(cost["list_page_size"], 100);
    assert_eq!(by_source(&body["fill_cost"], "gcd")["list_page_size"], 50);
    assert_eq!(
        by_source(&body["fill_cost"], "comicvine")["daily_budget"],
        4800
    );
    // Live provider views ride along (one per provider, none configured).
    assert_eq!(body["providers"].as_array().map(Vec::len), Some(3));
    assert!(
        body["providers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["configured"] == false)
    );

    // Second read is the cached snapshot; `refresh` recomputes.
    let (st, again) = get(&app, &cookie, "/api/admin/metadata/coverage").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(again["cached"], true);
    assert_eq!(again["computed_at"], body["computed_at"]);
    let (_, fresh) = get(&app, &cookie, "/api/admin/metadata/coverage?refresh=true").await;
    assert_eq!(fresh["cached"], false);
}

#[tokio::test]
async fn coverage_is_admin_only() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "admin@example.com").await;
    let member = register(&app, "member@example.com").await;
    let (st, _) = get(&app, &member, "/api/admin/metadata/coverage").await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}
