//! Integration coverage for `PATCH /series/{series_slug}/issues/{issue_slug}`
//! and `GET /series/{series_slug}/issues/{issue_slug}/next`.
//!
//! Verifies:
//!   - The full ComicRack-derived edit set lands in the DB and is tracked
//!     in `user_edited` so the scanner skips them on rescan.
//!   - Validation rejects nonsensical inputs (invalid manga, out-of-range
//!     year/month).
//!   - The "next in series" endpoint returns siblings in `sort_number`
//!     order and excludes the current row.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    issue::ActiveModel as IssueAM,
    library,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{ActiveModelTrait, Set};
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

struct Authed {
    session: String,
    csrf: String,
}

impl Authed {
    fn cookie(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
}

async fn register_admin(app: &TestApp) -> Authed {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"admin@example.com","password":"correctly-horse-battery-staple"}"#,
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
    let extract = |prefix: &str| {
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

#[derive(Default)]
struct IssueSeed<'a> {
    slug: &'a str,
    sort_number: Option<f64>,
    number_raw: Option<&'a str>,
}

/// Seeds `library` + `series` + N issues. Returns
/// `(library_id, series_id, series_slug, [issue_id, ...])`.
async fn seed(
    app: &TestApp,
    series_slug: &str,
    issues: &[IssueSeed<'_>],
) -> (Uuid, Uuid, String, Vec<String>) {
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let lib_id = Uuid::now_v7();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("Lib {series_slug}")),
        root_path: Set(format!("/tmp/{series_slug}-{lib_id}")),
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
        name: Set(format!("Series {series_slug}")),
        normalized_name: Set(normalize_name(&format!("Series {series_slug}"))),
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
        slug: Set(series_slug.to_owned()),
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

    let mut ids = Vec::with_capacity(issues.len());
    for (idx, seed) in issues.iter().enumerate() {
        let issue_id = format!(
            "{:0>64}",
            format!("{:x}{:x}", lib_id.as_u128(), idx as u128)
        );
        IssueAM {
            id: Set(issue_id.clone()),
            library_id: Set(lib_id),
            series_id: Set(series_id),
            file_path: Set(format!("/tmp/{series_slug}/{idx}.cbz")),
            file_size: Set(1),
            file_mtime: Set(now),
            state: Set("active".into()),
            content_hash: Set(issue_id.clone()),
            title: Set(None),
            sort_number: Set(seed.sort_number),
            number_raw: Set(seed.number_raw.map(str::to_owned)),
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
            slug: Set(seed.slug.to_owned()),
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
        ids.push(issue_id);
    }
    (lib_id, series_id, series_slug.to_owned(), ids)
}

async fn patch(
    app: &TestApp,
    auth: &Authed,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookie())
                .header("X-CSRF-Token", auth.csrf.clone())
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn get(app: &TestApp, auth: &Authed, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::COOKIE, auth.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_full_field_set_persists() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (_lib, _series_id, series_slug, ids) = seed(
        &app,
        "edit-fields",
        &[IssueSeed {
            slug: "issue-1",
            sort_number: Some(1.0),
            number_raw: Some("1"),
        }],
    )
    .await;
    let _ = ids;

    let body = serde_json::json!({
        "title": "The Beginning",
        "number": "1.5",
        "volume": 3,
        "year": 1999,
        "month": 6,
        "day": 15,
        "summary": "A summary.",
        "notes": "Editor's pick.",
        "publisher": "Acme",
        "imprint": "Acme Imprint",
        "writer": "A. Writer",
        "penciller": "P. Penciller",
        "inker": "I. Inker",
        "colorist": "C. Colorist",
        "letterer": "L. Letterer",
        "cover_artist": "Cover Artist",
        "editor": "E. Editor",
        "translator": "T. Translator",
        "characters": "Hero, Sidekick",
        "teams": "Team A",
        "locations": "City",
        "alternate_series": "Reprint",
        "story_arc": "Origins",
        "story_arc_number": "1",
        "genre": "Action",
        "tags": "tag-1",
        "language_code": "en",
        "age_rating": "Teen",
        "format": "One-Shot",
        "black_and_white": true,
        "manga": "YesAndRightToLeft",
        "sort_number": 1.5,
        "web_url": "https://example.com/issue/1",
        "gtin": "9781234567890",
    });
    let (status, json) = patch(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/issue-1"),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {json}");

    // Round-trip: GET should now return the new values.
    let (gs, gj) = get(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/issue-1"),
    )
    .await;
    assert_eq!(gs, StatusCode::OK);
    assert_eq!(gj["title"], "The Beginning");
    assert_eq!(gj["number"], "1.5");
    assert_eq!(gj["volume"], 3);
    assert_eq!(gj["year"], 1999);
    assert_eq!(gj["editor"], "E. Editor");
    assert_eq!(gj["translator"], "T. Translator");
    assert_eq!(gj["imprint"], "Acme Imprint");
    assert_eq!(gj["alternate_series"], "Reprint");
    assert_eq!(gj["manga"], "YesAndRightToLeft");
    assert_eq!(gj["black_and_white"], true);
    assert_eq!(gj["web_url"], "https://example.com/issue/1");

    // Every touched column should appear in user_edited so the scanner skips
    // it on rescan. number_raw is the entity-side name for the API's `number`.
    let edited: Vec<String> = gj["user_edited"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    for col in [
        "title",
        "number_raw",
        "volume",
        "year",
        "month",
        "day",
        "summary",
        "notes",
        "publisher",
        "imprint",
        "writer",
        "penciller",
        "inker",
        "colorist",
        "letterer",
        "cover_artist",
        "editor",
        "translator",
        "characters",
        "teams",
        "locations",
        "alternate_series",
        "story_arc",
        "story_arc_number",
        "genre",
        "tags",
        "language_code",
        "age_rating",
        "format",
        "black_and_white",
        "manga",
        "sort_number",
        "web_url",
        "gtin",
    ] {
        assert!(
            edited.contains(&col.to_owned()),
            "missing {col} in {edited:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_validation_rejects_bad_input() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (_lib, _sid, series_slug, _ids) = seed(
        &app,
        "validation",
        &[IssueSeed {
            slug: "issue-1",
            sort_number: Some(1.0),
            number_raw: Some("1"),
        }],
    )
    .await;

    // Out-of-range year.
    let (s, _) = patch(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/issue-1"),
        serde_json::json!({ "year": 1500 }),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Invalid month.
    let (s, _) = patch(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/issue-1"),
        serde_json::json!({ "month": 13 }),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Manga must be one of the canonical values.
    let (s, _) = patch(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/issue-1"),
        serde_json::json!({ "manga": "Maybe" }),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn next_in_series_returns_upcoming_in_order() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (_lib, _sid, series_slug, _ids) = seed(
        &app,
        "next",
        &[
            IssueSeed {
                slug: "issue-1",
                sort_number: Some(1.0),
                number_raw: Some("1"),
            },
            IssueSeed {
                slug: "issue-2",
                sort_number: Some(2.0),
                number_raw: Some("2"),
            },
            IssueSeed {
                slug: "issue-3",
                sort_number: Some(3.0),
                number_raw: Some("3"),
            },
            IssueSeed {
                slug: "issue-4",
                sort_number: Some(4.0),
                number_raw: Some("4"),
            },
        ],
    )
    .await;

    // From issue-2, expect [issue-3, issue-4] in that order.
    let (status, json) = get(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/issue-2/next?limit=5"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {json}");
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["slug"], "issue-3");
    assert_eq!(items[1]["slug"], "issue-4");

    // The current issue is excluded, and limit is honored.
    let (_, json) = get(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/issue-1/next?limit=2"),
    )
    .await;
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["slug"], "issue-2");
    assert_eq!(items[1]["slug"], "issue-3");

    // From the last issue, returns empty.
    let (_, json) = get(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/issue-4/next"),
    )
    .await;
    assert!(json["items"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn patch_external_ids_persist_and_round_trip() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (_lib, _sid, series_slug, _ids) = seed(
        &app,
        "ext-ids",
        &[IssueSeed {
            slug: "issue-1",
            sort_number: Some(1.0),
            number_raw: Some("1"),
        }],
    )
    .await;

    let (status, _) = patch(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/issue-1"),
        serde_json::json!({
            "comicvine_id": 381432,
            "metron_id": 12345,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, json) = get(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/issue-1"),
    )
    .await;
    assert_eq!(json["comicvine_id"], 381432);
    assert_eq!(json["metron_id"], 12345);
    let edited: Vec<String> = json["user_edited"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(edited.contains(&"comicvine_id".to_owned()));
    assert!(edited.contains(&"metron_id".to_owned()));

    // Sending null clears.
    let (status, _) = patch(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/issue-1"),
        serde_json::json!({ "comicvine_id": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, json) = get(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/issue-1"),
    )
    .await;
    assert!(json["comicvine_id"].is_null());
    // metron_id was not in this body, so it's untouched.
    assert_eq!(json["metron_id"], 12345);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_status_and_external_ids_editable() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (_lib, _sid, series_slug, _ids) = seed(
        &app,
        "series-edit",
        &[IssueSeed {
            slug: "issue-1",
            sort_number: Some(1.0),
            number_raw: Some("1"),
        }],
    )
    .await;

    // Update series status + external IDs in one request.
    let (status, _) = patch(
        &app,
        &auth,
        &format!("/api/series/{series_slug}"),
        serde_json::json!({
            "status": "ended",
            "comicvine_id": 49901,
            "metron_id": 1234,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, json) = get(&app, &auth, &format!("/api/series/{series_slug}")).await;
    assert_eq!(json["status"], "ended");
    assert_eq!(json["comicvine_id"], 49901);
    assert_eq!(json["metron_id"], 1234);

    // Bogus status is rejected.
    let (status, _) = patch(
        &app,
        &auth,
        &format!("/api/series/{series_slug}"),
        serde_json::json!({ "status": "wat" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_summary_editable_via_patch() {
    // Genre / tag overrides on series no longer exist (M1 saved-views
    // refactor): series-level facets are pure aggregations of their issues'
    // junction-table rows. Summary is the only PATCH-able free-text field
    // remaining on the series row.
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (_lib, _sid, series_slug, _ids) = seed(
        &app,
        "series-fields",
        &[IssueSeed {
            slug: "issue-1",
            sort_number: Some(1.0),
            number_raw: Some("1"),
        }],
    )
    .await;

    // Drop a summary on the first issue so the fallback has something to
    // surface when the series-level summary is null.
    use entity::issue;
    use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    let row = issue::Entity::find()
        .filter(issue::Column::Slug.eq("issue-1"))
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: issue::ActiveModel = row.into();
    am.summary = Set(Some("Issue summary fallback".into()));
    am.update(&db).await.unwrap();

    // Series with no summary should surface the issue's summary on GET.
    let (_, json) = get(&app, &auth, &format!("/api/series/{series_slug}")).await;
    assert_eq!(json["summary"], "Issue summary fallback");

    let (status, _) = patch(
        &app,
        &auth,
        &format!("/api/series/{series_slug}"),
        serde_json::json!({ "summary": "Series-level summary" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, json) = get(&app, &auth, &format!("/api/series/{series_slug}")).await;
    assert_eq!(json["summary"], "Series-level summary");

    // Clearing summary lets the issue-fallback resurface.
    let (status, _) = patch(
        &app,
        &auth,
        &format!("/api/series/{series_slug}"),
        serde_json::json!({ "summary": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, json) = get(&app, &auth, &format!("/api/series/{series_slug}")).await;
    assert_eq!(json["summary"], "Issue summary fallback");
}

async fn put_json(
    app: &TestApp,
    auth: &Authed,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, auth.cookie())
                .header("X-CSRF-Token", auth.csrf.clone())
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_and_issue_ratings_round_trip() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (_lib, _sid, series_slug, _ids) = seed(
        &app,
        "ratings",
        &[IssueSeed {
            slug: "issue-1",
            sort_number: Some(1.0),
            number_raw: Some("1"),
        }],
    )
    .await;

    // Initial GETs: no rating.
    let (_, json) = get(&app, &auth, &format!("/api/series/{series_slug}")).await;
    assert!(json["user_rating"].is_null());
    let (_, json) = get(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/issue-1"),
    )
    .await;
    assert!(json["user_rating"].is_null());

    // Series rating round-trip.
    let (status, body) = put_json(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/rating"),
        serde_json::json!({ "rating": 4.5 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["rating"], 4.5);

    let (_, json) = get(&app, &auth, &format!("/api/series/{series_slug}")).await;
    assert_eq!(json["user_rating"], 4.5);

    // Half-step precision is enforced.
    let (status, _) = put_json(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/rating"),
        serde_json::json!({ "rating": 3.7 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Issue rating round-trip.
    let (status, _) = put_json(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/issue-1/rating"),
        serde_json::json!({ "rating": 3.0 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, json) = get(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/issues/issue-1"),
    )
    .await;
    assert_eq!(json["user_rating"], 3.0);

    // Null clears.
    let (status, _) = put_json(
        &app,
        &auth,
        &format!("/api/series/{series_slug}/rating"),
        serde_json::json!({ "rating": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, json) = get(&app, &auth, &format!("/api/series/{series_slug}")).await;
    assert!(json["user_rating"].is_null());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_view_includes_progress_summary_and_year_range() {
    use entity::{issue, progress_record};
    use sea_orm::{ActiveModelTrait, Set};

    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let (_lib, _sid, series_slug, ids) = seed(
        &app,
        "progress-and-years",
        &[
            IssueSeed {
                slug: "issue-1",
                sort_number: Some(1.0),
                number_raw: Some("1"),
            },
            IssueSeed {
                slug: "issue-2",
                sort_number: Some(2.0),
                number_raw: Some("2"),
            },
            IssueSeed {
                slug: "issue-3",
                sort_number: Some(3.0),
                number_raw: Some("3"),
            },
        ],
    )
    .await;

    // Stamp years 2012, 2015, 2018 across the three issues so we exercise
    // the earliest/latest aggregation.
    let db = sea_orm::Database::connect(&app.db_url).await.unwrap();
    for (idx, year) in [2012, 2015, 2018].iter().enumerate() {
        let row = issue::Entity::find_by_id(ids[idx].clone())
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        let mut am: issue::ActiveModel = row.into();
        am.year = Set(Some(*year));
        am.update(&db).await.unwrap();
    }

    // Pull the user_id from the auth cookie indirectly: the only user
    // exists, so just grab it.
    use entity::user;
    use sea_orm::EntityTrait;
    let user_row = user::Entity::find().one(&db).await.unwrap().unwrap();

    // Mark issue-1 finished and issue-2 in-progress.
    let now = chrono::Utc::now().fixed_offset();
    progress_record::ActiveModel {
        user_id: Set(user_row.id),
        issue_id: Set(ids[0].clone()),
        last_page: Set(19),
        percent: Set(100.0),
        finished: Set(true),
        updated_at: Set(now),
        device: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
    progress_record::ActiveModel {
        user_id: Set(user_row.id),
        issue_id: Set(ids[1].clone()),
        last_page: Set(8),
        percent: Set(50.0),
        finished: Set(false),
        updated_at: Set(now),
        device: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    let (status, json) = get(&app, &auth, &format!("/api/series/{series_slug}")).await;
    assert_eq!(status, StatusCode::OK);
    let summary = &json["progress_summary"];
    assert_eq!(summary["total"], 3, "body: {json}");
    assert_eq!(summary["finished"], 1);
    assert_eq!(summary["in_progress"], 1);
    // `seed()` stamps every issue with `page_count = 20`, so finishing one
    // issue contributes 20 pages to the aggregate. The series-page Reading
    // Load stat divides this from `total_page_count` to estimate remaining
    // minutes — regression-protect that wiring here.
    assert_eq!(summary["finished_pages"], 20);
    assert_eq!(json["earliest_year"], 2012);
    assert_eq!(json["latest_year"], 2018);
}
