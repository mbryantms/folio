//! `/filter-options/*` endpoints — saved-views M5.
//!
//! Seeds two libraries' worth of series + issue genre/tag/credit
//! junction rows, then verifies the distinct-value lookup endpoints
//! filter by library scope and prefix.

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
    series_credit::ActiveModel as SeriesCreditAM,
    series_genre::ActiveModel as SeriesGenreAM,
    series_tag::ActiveModel as SeriesTagAM,
    user::Entity as UserEntity,
};
use sea_orm::{ActiveModelTrait, Database, EntityTrait, Set};
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
    if bytes.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap()
}

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
            .map(|c| {
                c.split(';')
                    .next()
                    .unwrap()
                    .trim_start_matches(prefix)
                    .to_owned()
            })
            .expect(prefix)
    };
    let json = body_json(resp.into_body()).await;
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id,
    }
}

async fn promote_to_admin(app: &TestApp, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = UserEntity::find_by_id(user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::user::ActiveModel = user.into();
    am.role = Set("admin".into());
    am.update(&db).await.unwrap();
}

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: Option<&Authed>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method.clone()).uri(uri);
    if let Some(a) = auth {
        builder = builder
            .header(
                header::COOKIE,
                format!(
                    "__Host-comic_session={}; __Host-comic_csrf={}",
                    a.session, a.csrf
                ),
            )
            .header("X-CSRF-Token", &a.csrf);
    }
    let req = builder.body(Body::empty()).unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

/// Insert a library with one series carrying the supplied genres/tags/
/// credits in the corresponding junction tables.
async fn seed_library_with_metadata(
    app: &TestApp,
    lib_name: &str,
    series_name: &str,
    genres: &[&str],
    tags: &[&str],
    writers: &[&str],
) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("Lib {lib_name}")),
        root_path: Set(format!("/tmp/{lib_name}-{lib_id}")),
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
        name: Set(series_name.into()),
        normalized_name: Set(normalize_name(series_name)),
        year: Set(Some(2020)),
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
        slug: Set(format!("{lib_name}-{series_name}")),
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

    // One placeholder issue — required by the FK / state constraints
    // but its junctions aren't what the options endpoint reads.
    let issue_id = format!("{:0>62}{:02x}", series_id.simple(), 0u8);
    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(format!("{series_name}-1")),
        file_path: Set(format!("/tmp/{lib_name}/{series_name}.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.clone()),
        title: Set(None),
        sort_number: Set(Some(1.0)),
        number_raw: Set(Some("1".into())),
        volume: Set(None),
        year: Set(Some(2020)),
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

    for g in genres {
        SeriesGenreAM {
            series_id: Set(series_id),
            genre: Set((*g).into()),
        }
        .insert(&db)
        .await
        .unwrap();
    }
    for t in tags {
        SeriesTagAM {
            series_id: Set(series_id),
            tag: Set((*t).into()),
        }
        .insert(&db)
        .await
        .unwrap();
    }
    for w in writers {
        SeriesCreditAM {
            series_id: Set(series_id),
            role: Set("writer".into()),
            person: Set((*w).into()),
        }
        .insert(&db)
        .await
        .unwrap();
    }

    lib_id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn genres_returns_distinct_values_across_visible_libraries() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    seed_library_with_metadata(
        &app,
        "horror",
        "Hellboy",
        &["Horror", "Action"],
        &["dark", "occult"],
        &["Mike Mignola"],
    )
    .await;
    seed_library_with_metadata(
        &app,
        "scifi",
        "Saga",
        &["Sci-Fi"],
        &["space"],
        &["Brian K. Vaughan"],
    )
    .await;

    let (status, json) = http(&app, Method::GET, "/filter-options/genres", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    let values: Vec<String> = json["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(values.contains(&"Horror".to_owned()));
    assert!(values.contains(&"Action".to_owned()));
    assert!(values.contains(&"Sci-Fi".to_owned()));
    // Ordering is alphabetic.
    let mut sorted = values.clone();
    sorted.sort();
    assert_eq!(values, sorted, "values should be sorted ascending");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn library_scope_restricts_options() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    let horror_lib = seed_library_with_metadata(
        &app,
        "horror",
        "Hellboy",
        &["Horror"],
        &["dark"],
        &["Mike Mignola"],
    )
    .await;
    let _scifi_lib = seed_library_with_metadata(
        &app,
        "scifi",
        "Saga",
        &["Sci-Fi"],
        &["space"],
        &["Brian K. Vaughan"],
    )
    .await;

    let url = format!("/filter-options/genres?library={horror_lib}");
    let (status, json) = http(&app, Method::GET, &url, Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    let values: Vec<String> = json["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(values, vec!["Horror".to_owned()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefix_filter_narrows_results() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    seed_library_with_metadata(
        &app,
        "tags-lib",
        "Hellboy",
        &["Horror"],
        &["dark", "demon", "occult", "action"],
        &[],
    )
    .await;

    let (status, json) = http(&app, Method::GET, "/filter-options/tags?q=de", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    let values: Vec<String> = json["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(values, vec!["demon".to_owned()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn credits_endpoint_filters_by_role() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    seed_library_with_metadata(
        &app,
        "credits-lib",
        "Hellboy",
        &["Horror"],
        &[],
        &["Mike Mignola", "John Byrne"],
    )
    .await;

    let (status, json) = http(
        &app,
        Method::GET,
        "/filter-options/credits/writer",
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let values: Vec<String> = json["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        values,
        vec!["John Byrne".to_owned(), "Mike Mignola".to_owned()]
    );

    // Unknown role 400s.
    let (status, _body) = http(
        &app,
        Method::GET,
        "/filter-options/credits/bogus",
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// Library with `n` series, each carrying the supplied publisher (one
/// per slot). `Some` populates `series.publisher`; `None` leaves it
/// NULL so we can assert NULL/empty exclusion.
async fn seed_library_with_publishers(
    app: &TestApp,
    lib_name: &str,
    publishers: &[Option<&str>],
) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let lib_id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set(format!("Lib {lib_name}")),
        root_path: Set(format!("/tmp/{lib_name}-{lib_id}")),
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

    for (i, p) in publishers.iter().enumerate() {
        let series_id = Uuid::now_v7();
        let name = format!("{lib_name}-s{i}");
        SeriesAM {
            id: Set(series_id),
            library_id: Set(lib_id),
            name: Set(name.clone()),
            normalized_name: Set(normalize_name(&name)),
            year: Set(Some(2020)),
            volume: Set(None),
            publisher: Set(p.map(str::to_owned)),
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
            slug: Set(format!("{lib_name}-{i}")),
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
    }
    lib_id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publishers_returns_distinct_non_null_values() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    seed_library_with_publishers(
        &app,
        "main",
        &[
            Some("Image"),
            Some("Image"), // dedup
            Some("Marvel"),
            None,     // NULL excluded
            Some(""), // empty excluded
        ],
    )
    .await;

    let (status, json) = http(&app, Method::GET, "/filter-options/publishers", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK, "json: {json:#?}");
    let values: Vec<String> = json["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(values, vec!["Image".to_owned(), "Marvel".to_owned()]);
}

/// Insert a single series in `lib_id` with the supplied facets. Used
/// by the `/series` filter tests below to seed deliberately-shaped
/// rows without going through the scanner.
async fn seed_one_series(
    app: &TestApp,
    lib_id: Uuid,
    name: &str,
    status: &str,
    year: Option<i32>,
    publisher: Option<&str>,
    genres: &[&str],
) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let series_id = Uuid::now_v7();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set(name.into()),
        normalized_name: Set(normalize_name(name)),
        year: Set(year),
        volume: Set(None),
        publisher: Set(publisher.map(str::to_owned)),
        imprint: Set(None),
        status: Set(status.into()),
        total_issues: Set(None),
        age_rating: Set(None),
        summary: Set(None),
        language_code: Set("en".into()),
        comicvine_id: Set(None),
        metron_id: Set(None),
        gtin: Set(None),
        series_group: Set(None),
        slug: Set(format!("series-{}", series_id.simple())),
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
    for g in genres {
        SeriesGenreAM {
            series_id: Set(series_id),
            genre: Set((*g).into()),
        }
        .insert(&db)
        .await
        .unwrap();
    }
    series_id
}

/// `/series` filter tests live alongside `/filter-options/*` because
/// they exercise the same junction-table machinery and the seed
/// helpers are already here.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_filter_status_narrows_results() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let lib = seed_library_with_publishers(&app, "lib", &[]).await;
    seed_one_series(&app, lib, "Continuing One", "continuing", None, None, &[]).await;
    seed_one_series(&app, lib, "Ended One", "ended", None, None, &[]).await;
    seed_one_series(&app, lib, "Cancelled One", "cancelled", None, None, &[]).await;

    let (status, json) = http(&app, Method::GET, "/series?status=ended", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<String> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, vec!["Ended One".to_owned()]);

    // Unknown enum is a 400, not silently empty.
    let (status, _) = http(&app, Method::GET, "/series?status=bogus", Some(&auth)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_filter_year_range_inclusive_bounds() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let lib = seed_library_with_publishers(&app, "lib", &[]).await;
    seed_one_series(&app, lib, "S 2018", "continuing", Some(2018), None, &[]).await;
    seed_one_series(&app, lib, "S 2020", "continuing", Some(2020), None, &[]).await;
    seed_one_series(&app, lib, "S 2024", "continuing", Some(2024), None, &[]).await;
    seed_one_series(&app, lib, "S null", "continuing", None, None, &[]).await;

    let (status, json) = http(
        &app,
        Method::GET,
        "/series?year_from=2019&year_to=2022",
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<String> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, vec!["S 2020".to_owned()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_filter_publisher_csv_includes_any() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let lib = seed_library_with_publishers(&app, "lib", &[]).await;
    seed_one_series(&app, lib, "Image A", "continuing", None, Some("Image"), &[]).await;
    seed_one_series(
        &app,
        lib,
        "Marvel A",
        "continuing",
        None,
        Some("Marvel"),
        &[],
    )
    .await;
    seed_one_series(&app, lib, "DC A", "continuing", None, Some("DC"), &[]).await;

    let (status, json) = http(&app, Method::GET, "/series?publisher=Image,DC", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    let mut names: Vec<String> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["DC A".to_owned(), "Image A".to_owned()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_filter_genres_csv_includes_any() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let lib = seed_library_with_publishers(&app, "lib", &[]).await;
    seed_one_series(
        &app,
        lib,
        "Horror Series",
        "continuing",
        None,
        None,
        &["Horror"],
    )
    .await;
    seed_one_series(
        &app,
        lib,
        "Sci-Fi Series",
        "continuing",
        None,
        None,
        &["Sci-Fi"],
    )
    .await;
    seed_one_series(
        &app,
        lib,
        "Drama Series",
        "continuing",
        None,
        None,
        &["Drama"],
    )
    .await;

    let (status, json) = http(
        &app,
        Method::GET,
        "/series?genres=Horror,Drama",
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut names: Vec<String> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["Drama Series".to_owned(), "Horror Series".to_owned()]
    );
}

/// Like `seed_one_series` but exposes the language/age_rating columns
/// and lets the caller seed credit junction rows + a user rating in
/// one shot. Used by the /series filter tests below for language /
/// age rating / credits / user-rating dimensions.
#[allow(clippy::too_many_arguments)]
async fn seed_series_full(
    app: &TestApp,
    lib_id: Uuid,
    name: &str,
    language_code: &str,
    age_rating: Option<&str>,
    credits: &[(&str, &str)],
    user_rating: Option<(Uuid, f64)>,
) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let series_id = Uuid::now_v7();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set(name.into()),
        normalized_name: Set(normalize_name(name)),
        year: Set(Some(2020)),
        volume: Set(None),
        publisher: Set(None),
        imprint: Set(None),
        status: Set("continuing".into()),
        total_issues: Set(None),
        age_rating: Set(age_rating.map(str::to_owned)),
        summary: Set(None),
        language_code: Set(language_code.into()),
        comicvine_id: Set(None),
        metron_id: Set(None),
        gtin: Set(None),
        series_group: Set(None),
        slug: Set(format!("series-{}", series_id.simple())),
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
    for (role, person) in credits {
        SeriesCreditAM {
            series_id: Set(series_id),
            role: Set((*role).into()),
            person: Set((*person).into()),
        }
        .insert(&db)
        .await
        .unwrap();
    }
    if let Some((user_id, rating)) = user_rating {
        entity::user_rating::ActiveModel {
            user_id: Set(user_id),
            target_type: Set("series".into()),
            target_id: Set(series_id.to_string()),
            rating: Set(rating),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(&db)
        .await
        .unwrap();
    }
    series_id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_filter_language_csv() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let lib = seed_library_with_publishers(&app, "lib", &[]).await;
    seed_series_full(&app, lib, "EN One", "en", None, &[], None).await;
    seed_series_full(&app, lib, "FR One", "fr", None, &[], None).await;
    seed_series_full(&app, lib, "JA One", "ja", None, &[], None).await;

    let (status, json) = http(&app, Method::GET, "/series?language=en,fr", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    let mut names: Vec<String> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["EN One".to_owned(), "FR One".to_owned()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_filter_credits_writers_includes_any() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let lib = seed_library_with_publishers(&app, "lib", &[]).await;
    seed_series_full(
        &app,
        lib,
        "BKV Series",
        "en",
        None,
        &[("writer", "Brian K. Vaughan")],
        None,
    )
    .await;
    seed_series_full(
        &app,
        lib,
        "Hickman Series",
        "en",
        None,
        &[("writer", "Jonathan Hickman")],
        None,
    )
    .await;
    seed_series_full(
        &app,
        lib,
        "Other Series",
        "en",
        None,
        &[("penciller", "Jamie McKelvie")],
        None,
    )
    .await;

    // includes-any: BKV or Hickman matches; Other has no writer matches.
    let (status, json) = http(
        &app,
        Method::GET,
        "/series?writers=Brian%20K.%20Vaughan,Jonathan%20Hickman",
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut names: Vec<String> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["BKV Series".to_owned(), "Hickman Series".to_owned()]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_filter_user_rating_excludes_unrated_when_bounds_set() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let lib = seed_library_with_publishers(&app, "lib", &[]).await;
    seed_series_full(
        &app,
        lib,
        "Rated 5",
        "en",
        None,
        &[],
        Some((auth.user_id, 5.0)),
    )
    .await;
    seed_series_full(
        &app,
        lib,
        "Rated 3",
        "en",
        None,
        &[],
        Some((auth.user_id, 3.0)),
    )
    .await;
    seed_series_full(&app, lib, "Unrated", "en", None, &[], None).await;

    let (status, json) = http(
        &app,
        Method::GET,
        "/series?user_rating_min=4&user_rating_max=5",
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<String> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, vec!["Rated 5".to_owned()]);

    // Bad input: max < min.
    let (status, _) = http(
        &app,
        Method::GET,
        "/series?user_rating_min=4&user_rating_max=2",
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Bad input: out-of-range.
    let (status, _) = http(&app, Method::GET, "/series?user_rating_min=-1", Some(&auth)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn languages_options_endpoint() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let lib = seed_library_with_publishers(&app, "lib", &[]).await;
    seed_series_full(&app, lib, "S1", "en", None, &[], None).await;
    seed_series_full(&app, lib, "S2", "fr", None, &[], None).await;
    seed_series_full(&app, lib, "S3", "en", None, &[], None).await;

    let (status, json) = http(&app, Method::GET, "/filter-options/languages", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    let values: Vec<String> = json["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(values, vec!["en".to_owned(), "fr".to_owned()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn age_ratings_options_endpoint_excludes_null_and_empty() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let lib = seed_library_with_publishers(&app, "lib", &[]).await;
    seed_series_full(&app, lib, "S1", "en", Some("Teen"), &[], None).await;
    seed_series_full(&app, lib, "S2", "en", Some("Mature"), &[], None).await;
    seed_series_full(&app, lib, "S3", "en", Some(""), &[], None).await;
    seed_series_full(&app, lib, "S4", "en", None, &[], None).await;

    let (status, json) = http(
        &app,
        Method::GET,
        "/filter-options/age_ratings",
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let values: Vec<String> = json["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(values, vec!["Mature".to_owned(), "Teen".to_owned()]);
}

/// Insert a series with one issue carrying the supplied CSV cast/
/// setting columns. Used to exercise the `/series?characters=...` and
/// `/filter-options/characters` endpoints, which scan the issues
/// table directly (no junction table for cast metadata yet).
#[allow(clippy::too_many_arguments)]
async fn seed_series_with_issue_csv(
    app: &TestApp,
    lib_id: Uuid,
    name: &str,
    characters: Option<&str>,
    teams: Option<&str>,
    locations: Option<&str>,
) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let series_id = Uuid::now_v7();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set(name.into()),
        normalized_name: Set(normalize_name(name)),
        year: Set(Some(2020)),
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
        slug: Set(format!("series-{}", series_id.simple())),
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

    let issue_id = format!("{:0>62}{:02x}", series_id.simple(), 0u8);
    IssueAM {
        id: Set(issue_id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        slug: Set(format!("{name}-1")),
        file_path: Set(format!("/tmp/{name}-{series_id}.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(issue_id.clone()),
        title: Set(None),
        sort_number: Set(Some(1.0)),
        number_raw: Set(Some("1".into())),
        volume: Set(None),
        year: Set(Some(2020)),
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
        characters: Set(characters.map(str::to_owned)),
        teams: Set(teams.map(str::to_owned)),
        locations: Set(locations.map(str::to_owned)),
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
    series_id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_filter_characters_includes_any_case_insensitive() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let lib = seed_library_with_publishers(&app, "lib", &[]).await;
    seed_series_with_issue_csv(
        &app,
        lib,
        "Spidey Series",
        Some("Spider-Man, Mary Jane"),
        None,
        None,
    )
    .await;
    seed_series_with_issue_csv(
        &app,
        lib,
        "Avengers Series",
        Some("iron man; Thor"),
        None,
        None,
    )
    .await;
    seed_series_with_issue_csv(&app, lib, "Empty Series", None, None, None).await;

    // Case-insensitive match across both `,` and `;` separators.
    let (status, json) = http(
        &app,
        Method::GET,
        "/series?characters=spider-man,Iron%20Man",
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut names: Vec<String> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["Avengers Series".to_owned(), "Spidey Series".to_owned()]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn characters_options_endpoint_dedupes_and_sorts() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let lib = seed_library_with_publishers(&app, "lib", &[]).await;
    seed_series_with_issue_csv(&app, lib, "S1", Some("Spider-Man, Mary Jane"), None, None).await;
    seed_series_with_issue_csv(&app, lib, "S2", Some("spider-man;Aunt May"), None, None).await;

    let (status, json) = http(&app, Method::GET, "/filter-options/characters", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    let values: Vec<String> = json["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    // Three distinct names (spider-man casings dedup), alphabetical.
    // The endpoint picks `min(trim(piece))` for the display value, so
    // "Spider-Man" wins over "spider-man" (uppercase < lowercase).
    assert_eq!(
        values,
        vec![
            "Aunt May".to_owned(),
            "Mary Jane".to_owned(),
            "Spider-Man".to_owned(),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn locations_options_endpoint_excludes_empty() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let lib = seed_library_with_publishers(&app, "lib", &[]).await;
    seed_series_with_issue_csv(&app, lib, "S1", None, None, Some("Gotham, Metropolis")).await;
    seed_series_with_issue_csv(&app, lib, "S2", None, None, Some("")).await;
    seed_series_with_issue_csv(&app, lib, "S3", None, None, None).await;

    let (status, json) = http(&app, Method::GET, "/filter-options/locations", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    let values: Vec<String> = json["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(values, vec!["Gotham".to_owned(), "Metropolis".to_owned()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publishers_scoped_to_library_when_query_param_set() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;

    let img_lib = seed_library_with_publishers(&app, "img", &[Some("Image")]).await;
    let _marvel_lib = seed_library_with_publishers(&app, "marvel", &[Some("Marvel")]).await;

    let url = format!("/filter-options/publishers?library={img_lib}");
    let (status, json) = http(&app, Method::GET, &url, Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    let values: Vec<String> = json["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(values, vec!["Image".to_owned()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn series_sort_by_year_descending() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let lib = seed_library_with_publishers(&app, "lib", &[]).await;
    seed_one_series(&app, lib, "Old", "continuing", Some(2010), None, &[]).await;
    seed_one_series(&app, lib, "Mid", "continuing", Some(2020), None, &[]).await;
    seed_one_series(&app, lib, "New", "continuing", Some(2024), None, &[]).await;
    seed_one_series(&app, lib, "Undated", "continuing", None, None, &[]).await;

    let (status, json) = http(&app, Method::GET, "/series?sort=year", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<String> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect();
    // Default order for year is DESC; Undated (NULL) sorts last.
    assert_eq!(
        names,
        vec![
            "New".to_owned(),
            "Mid".to_owned(),
            "Old".to_owned(),
            "Undated".to_owned(),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issues_cross_library_filter_by_writer() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let lib = seed_library_with_publishers(&app, "lib", &[]).await;
    let s1 = seed_one_series(&app, lib, "Series A", "continuing", Some(2020), None, &[]).await;
    let s2 = seed_one_series(&app, lib, "Series B", "continuing", Some(2021), None, &[]).await;
    seed_issue_in_series(&app, lib, s1, "iss-a1", Some("Brian K. Vaughan"), None).await;
    seed_issue_in_series(&app, lib, s2, "iss-b1", Some("Other Writer"), None).await;

    let (status, json) = http(
        &app,
        Method::GET,
        "/issues?writers=Brian%20K.%20Vaughan",
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"].as_str(), Some("iss-a1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issues_cross_library_sort_by_year_then_page_count() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let lib = seed_library_with_publishers(&app, "lib", &[]).await;
    let s1 = seed_one_series(&app, lib, "S1", "continuing", Some(2020), None, &[]).await;
    seed_issue_in_year(&app, lib, s1, "Old", Some(2010), Some(20)).await;
    seed_issue_in_year(&app, lib, s1, "Mid", Some(2020), Some(50)).await;
    seed_issue_in_year(&app, lib, s1, "New", Some(2024), Some(30)).await;

    // Year DESC default.
    let (status, json) = http(&app, Method::GET, "/issues?sort=year", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    let titles: Vec<String> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap_or("").to_owned())
        .collect();
    assert_eq!(
        titles,
        vec!["New".to_owned(), "Mid".to_owned(), "Old".to_owned()]
    );

    // page_count DESC: 50 > 30 > 20.
    let (status, json) = http(&app, Method::GET, "/issues?sort=page_count", Some(&auth)).await;
    assert_eq!(status, StatusCode::OK);
    let titles: Vec<String> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap_or("").to_owned())
        .collect();
    assert_eq!(
        titles,
        vec!["Mid".to_owned(), "New".to_owned(), "Old".to_owned()]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issues_cross_library_user_rating_filter_excludes_unrated() {
    let app = TestApp::spawn().await;
    let auth = register(&app, "admin@example.com").await;
    promote_to_admin(&app, auth.user_id).await;
    let lib = seed_library_with_publishers(&app, "lib", &[]).await;
    let s = seed_one_series(&app, lib, "S1", "continuing", Some(2020), None, &[]).await;
    let iid_a = seed_issue_in_year(&app, lib, s, "Rated 5", Some(2020), Some(20)).await;
    let iid_b = seed_issue_in_year(&app, lib, s, "Rated 3", Some(2020), Some(20)).await;
    let _ = seed_issue_in_year(&app, lib, s, "Unrated", Some(2020), Some(20)).await;
    set_user_issue_rating(&app, auth.user_id, &iid_a, 5.0).await;
    set_user_issue_rating(&app, auth.user_id, &iid_b, 3.0).await;

    let (status, json) = http(
        &app,
        Method::GET,
        "/issues?user_rating_min=4&user_rating_max=5",
        Some(&auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"].as_str(), Some("Rated 5"));
}

/// Insert an issue under `series_id` with optional writer and page count.
/// Used by the cross-library `/issues` tests above; minimal helper, only
/// fills the fields the test needs.
async fn seed_issue_in_series(
    app: &TestApp,
    lib_id: Uuid,
    series_id: Uuid,
    title: &str,
    writer: Option<&str>,
    page_count: Option<i32>,
) -> String {
    seed_issue_full(app, lib_id, series_id, title, writer, None, page_count).await
}

async fn seed_issue_in_year(
    app: &TestApp,
    lib_id: Uuid,
    series_id: Uuid,
    title: &str,
    year: Option<i32>,
    page_count: Option<i32>,
) -> String {
    seed_issue_full(app, lib_id, series_id, title, None, year, page_count).await
}

async fn seed_issue_full(
    app: &TestApp,
    lib_id: Uuid,
    series_id: Uuid,
    title: &str,
    writer: Option<&str>,
    year: Option<i32>,
    page_count: Option<i32>,
) -> String {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    // BLAKE3-shaped id: 64 hex chars. We just need uniqueness.
    let id = format!("{:0>62}{:02x}", Uuid::now_v7().simple(), rand_byte());
    IssueAM {
        id: Set(id.clone()),
        library_id: Set(lib_id),
        series_id: Set(series_id),
        // Use the full id for the slug; the first chars are the
        // timestamp prefix and collide when rows are created in the
        // same millisecond.
        slug: Set(format!("issue-{id}")),
        file_path: Set(format!("/tmp/{title}-{id}.cbz")),
        file_size: Set(1),
        file_mtime: Set(now),
        state: Set("active".into()),
        content_hash: Set(id.clone()),
        title: Set(Some(title.into())),
        sort_number: Set(Some(1.0)),
        number_raw: Set(Some("1".into())),
        volume: Set(None),
        year: Set(year),
        month: Set(None),
        day: Set(None),
        summary: Set(None),
        notes: Set(None),
        language_code: Set(None),
        format: Set(None),
        black_and_white: Set(None),
        manga: Set(None),
        age_rating: Set(None),
        page_count: Set(page_count),
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
        writer: Set(writer.map(str::to_owned)),
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
    id
}

fn rand_byte() -> u8 {
    // Tests run sequentially in this file, so a static counter is enough
    // — we just need the issue ids to differ within one process.
    use std::sync::atomic::{AtomicU8, Ordering};
    static N: AtomicU8 = AtomicU8::new(1);
    N.fetch_add(1, Ordering::SeqCst)
}

async fn set_user_issue_rating(app: &TestApp, user_id: Uuid, issue_id: &str, rating: f64) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    entity::user_rating::ActiveModel {
        user_id: Set(user_id),
        target_type: Set("issue".into()),
        target_id: Set(issue_id.to_owned()),
        rating: Set(rating),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
}
