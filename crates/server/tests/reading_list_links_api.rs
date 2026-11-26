//! Linked reading lists, PR 2a: the links API.
//!
//! - create / move / delete events and the next part, with the view's
//!   per-event progress, chapters and `linked_from`;
//! - the rules: one level of events, no part loops, no self-links, shared
//!   lists link shared lists only, writes need the list's manager;
//! - per-reader skip, mark-read-through (dry run first), and the entries
//!   endpoint's `from_position` / `before` seek.
//!
//! Library / issue seeding is the `cbl_window_paginated` helper.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{
    audit_log, cbl_entry, cbl_list,
    issue::ActiveModel as IssueAM,
    library,
    series::{ActiveModel as SeriesAM, normalize_name},
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Database, EntityTrait, PaginatorTrait, QueryFilter, Set,
};
use tower::ServiceExt;
use uuid::Uuid;

async fn body_json(b: Body) -> serde_json::Value {
    let bytes = to_bytes(b, usize::MAX).await.unwrap();
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

async fn send(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: &Authed,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf);
    let req = match body {
        Some(b) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, json)
}

async fn get_json(app: &TestApp, uri: &str, auth: &Authed) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf)
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

/// Insert a library + one series + `count` issues + a CBL list owned by
/// `user` with each issue placed at position 0..count. Returns the
/// list id and the issue ids in position order so tests can mark
/// arbitrary entries as finished.
async fn seed_cbl_with_matched_entries(
    app: &TestApp,
    user: Uuid,
    count: usize,
) -> (Uuid, Vec<String>) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let lib_id = Uuid::now_v7();
    library::ActiveModel {
        id: Set(lib_id),
        name: Set("cbl-window lib".into()),
        root_path: Set(format!("/tmp/cbl-window-{lib_id}")),
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
        thumbnail_format: Set("webp".into()),
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

    let series_id = Uuid::now_v7();
    SeriesAM {
        id: Set(series_id),
        library_id: Set(lib_id),
        name: Set("Window Test".into()),
        normalized_name: Set(normalize_name("Window Test")),
        year: Set(Some(2020)),
        volume: Set(Some(1)),
        publisher: Set(None),
        imprint: Set(None),
        status: Set("ended".into()),
        total_issues: Set(None),
        age_rating: Set(None),
        declared_age_rating: Set(None),
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
        slug: Set("window-test-2020".into()),
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
    .insert(&db)
    .await
    .unwrap();

    let mut issue_ids: Vec<String> = Vec::with_capacity(count);
    for n in 0..count {
        let issue_id = format!("{:0>62}{:02x}", series_id.simple(), n as u8);
        IssueAM {
            id: Set(issue_id.clone()),
            library_id: Set(lib_id),
            series_id: Set(series_id),
            slug: Set(format!("window-test-{n}")),
            file_path: Set(format!("/tmp/cbl-window/Window #{n}.cbz")),
            file_size: Set(1),
            file_mtime: Set(now),
            state: Set(entity::issue::IssueState::Active),
            content_hash: Set(issue_id.clone()),
            title: Set(None),
            sort_number: Set(Some(n as f64)),
            number_raw: Set(Some(n.to_string())),
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
            deck: Set(None),
            store_date: Set(None),
            foc_date: Set(None),
            price: Set(None),
            sku: Set(None),
            staff_rating: Set(None),
            aliases: Set(serde_json::json!([])),
            last_metadata_sync_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            removed_at: Set(None),
            removal_confirmed_at: Set(None),
            superseded_by: Set(None),
            special_type: Set(None),
            hash_algorithm: Set(1),
            metroninfo_present: Set(None),
            thumbnails_generated_at: Set(None),
            thumbnail_version: Set(0),
            thumbnails_error: Set(None),
            additional_links: Set(serde_json::json!([])),
            comicinfo_count: Set(None),
            last_rewrite_at: Set(None),
            last_rewrite_kind: Set(None),
            last_sidecar_rewrite_at: Set(None),
            metron_info_raw: Set(None),
            cover_page_index: Set(0),
            cover_tone: Set(None),
            metadata_review_accepted_at: Set(None),
            metadata_review_accepted_by: Set(None),
        }
        .insert(&db)
        .await
        .unwrap();
        issue_ids.push(issue_id);
    }

    let list_id = Uuid::now_v7();
    cbl_list::ActiveModel {
        id: Set(list_id),
        owner_user_id: Set(Some(user)),
        source_kind: Set("upload".into()),
        source_url: Set(None),
        catalog_source_id: Set(None),
        catalog_path: Set(None),
        github_blob_sha: Set(None),
        source_etag: Set(None),
        source_last_modified: Set(None),
        raw_sha256: Set(vec![0u8; 32]),
        raw_xml: Set(String::new()),
        parsed_name: Set("Window Test".into()),
        parsed_matchers_present: Set(false),
        num_issues_declared: Set(Some(count as i32)),
        description: Set(None),
        imported_at: Set(now),
        last_refreshed_at: Set(None),
        last_match_run_at: Set(Some(now)),
        refresh_schedule: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        preserve_canonical_order: Set(false),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();

    for (i, issue_id) in issue_ids.iter().enumerate() {
        cbl_entry::ActiveModel {
            id: Set(Uuid::now_v7()),
            cbl_list_id: Set(list_id),
            position: Set(i as i32),
            series_name: Set("Window Test".into()),
            issue_number: Set(i.to_string()),
            volume: Set(None),
            year: Set(None),
            cv_series_id: Set(None),
            cv_issue_id: Set(None),
            metron_series_id: Set(None),
            metron_issue_id: Set(None),
            matched_issue_id: Set(Some(issue_id.clone())),
            match_status: Set("matched".into()),
            match_method: Set(Some("test".into())),
            match_confidence: Set(Some(1.0)),
            ambiguous_candidates: Set(None),
            user_resolved_at: Set(None),
            matched_at: Set(Some(now)),
        }
        .insert(&db)
        .await
        .unwrap();
    }

    (list_id, issue_ids)
}

/// A list owned by `owner` (`None` = shared) whose entries are matched to
/// `issues`, in order.
async fn make_list(app: &TestApp, owner: Option<Uuid>, name: &str, issues: &[String]) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let id = Uuid::now_v7();
    cbl_list::ActiveModel {
        id: Set(id),
        owner_user_id: Set(owner),
        source_kind: Set("upload".into()),
        raw_sha256: Set(vec![0u8; 32]),
        raw_xml: Set(String::new()),
        parsed_name: Set(name.into()),
        parsed_matchers_present: Set(false),
        imported_at: Set(now),
        created_at: Set(now),
        updated_at: Set(now),
        preserve_canonical_order: Set(false),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    for (i, issue_id) in issues.iter().enumerate() {
        cbl_entry::ActiveModel {
            id: Set(Uuid::now_v7()),
            cbl_list_id: Set(id),
            position: Set(i as i32),
            series_name: Set(format!("{name} series")),
            issue_number: Set(i.to_string()),
            volume: Set(None),
            year: Set(None),
            cv_series_id: Set(None),
            cv_issue_id: Set(None),
            metron_series_id: Set(None),
            metron_issue_id: Set(None),
            matched_issue_id: Set(Some(issue_id.clone())),
            match_status: Set("matched".into()),
            match_method: Set(Some("test".into())),
            match_confidence: Set(Some(1.0)),
            ambiguous_candidates: Set(None),
            user_resolved_at: Set(None),
            matched_at: Set(Some(now)),
        }
        .insert(&db)
        .await
        .unwrap();
    }
    id
}

/// The entry id at `position` of `list`.
async fn entry_at(app: &TestApp, list: Uuid, position: i32) -> String {
    let db = Database::connect(&app.db_url).await.unwrap();
    cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(list))
        .filter(cbl_entry::Column::Position.eq(position))
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .id
        .to_string()
}

fn links_uri(list: Uuid) -> String {
    format!("/api/me/cbl-lists/{list}/links")
}

#[tokio::test]
async fn event_and_next_part_round_trip_with_progress_and_chapters() {
    let app = TestApp::spawn().await;
    // The first account is the admin, who sees every library; the seeded
    // library grants nobody else.
    let user = register(&app, "links-admin@example.com").await;
    // 16 issues: the master is the seeded list over all of them; the event
    // reads 10..14 and the next part 14..16 through their own lists.
    let (seeded, issues) = seed_cbl_with_matched_entries(&app, user.user_id, 16).await;
    let master = make_list(&app, Some(user.user_id), "Master", &issues[..10]).await;
    let event = make_list(&app, Some(user.user_id), "House of M", &issues[10..14]).await;
    let next = make_list(&app, Some(user.user_id), "Part 8", &issues[14..]).await;
    let _ = seeded;

    let after = entry_at(&app, master, 4).await;
    let (status, body) = send(
        &app,
        Method::POST,
        &links_uri(master),
        &user,
        Some(serde_json::json!({"child_list_id": event, "kind": "event", "after_entry_id": after})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:#?}");
    let ev = &body["events"][0];
    assert_eq!(ev["child_name"], "House of M");
    assert_eq!(ev["anchor"]["position"], 4);
    assert_eq!(ev["anchor"]["status"], "placed");
    assert_eq!(
        (
            ev["total"].as_i64(),
            ev["in_library"].as_i64(),
            ev["read"].as_i64()
        ),
        (Some(4), Some(4), Some(0))
    );
    assert_eq!(ev["first_unread"]["issue_id"], issues[10].as_str());
    assert!(ev["first_unread"]["series_slug"].is_string());
    assert_eq!(ev["skipped"], false);
    assert!(
        ev["child_view_id"].is_string(),
        "linking an event gives the linker a view of it"
    );
    let chapters = body["chapters"].as_array().unwrap();
    assert_eq!(chapters.len(), 2, "{chapters:#?}");
    assert_eq!(
        (
            chapters[0]["start_position"].as_i64(),
            chapters[0]["end_position"].as_i64()
        ),
        (Some(0), Some(4))
    );
    assert_eq!(chapters[0]["label"], "Up to House of M");
    assert_eq!(chapters[0]["event_link_ids"][0], ev["id"]);
    assert_eq!(
        (
            chapters[1]["start_position"].as_i64(),
            chapters[1]["end_position"].as_i64()
        ),
        (Some(5), Some(9))
    );
    assert_eq!(chapters[1]["label"], "After House of M");
    let link_id = ev["id"].as_str().unwrap().to_owned();

    // Duplicate event → 409; event without an anchor → 422 bound to the field.
    let (status, _) = send(
        &app,
        Method::POST,
        &links_uri(master),
        &user,
        Some(serde_json::json!({"child_list_id": event, "kind": "event", "after_entry_id": after})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, body) = send(
        &app,
        Method::POST,
        &links_uri(master),
        &user,
        Some(serde_json::json!({"child_list_id": next, "kind": "event"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["details"][0]["field"], "after_entry_id");

    // Next part; a second one is refused.
    let (status, body) = send(
        &app,
        Method::POST,
        &links_uri(master),
        &user,
        Some(serde_json::json!({"child_list_id": next, "kind": "continues"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:#?}");
    assert_eq!(body["continues"]["child_name"], "Part 8");
    assert!(body["continues"]["anchor"].is_null());
    let (status, _) = send(
        &app,
        Method::POST,
        &links_uri(master),
        &user,
        Some(serde_json::json!({"child_list_id": event, "kind": "continues"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Mark the event read through its second issue: count first, then do it.
    let through = format!("/api/me/cbl-lists/{event}/progress/through");
    let (status, body) = send(
        &app,
        Method::POST,
        &through,
        &user,
        Some(serde_json::json!({"position": 1, "dry_run": true})),
    )
    .await;
    assert_eq!((status, body["marked"].as_u64()), (StatusCode::OK, Some(2)));
    let (_, body) = get_json(&app, &links_uri(master), &user).await;
    assert_eq!(body["events"][0]["read"], 0, "a dry run writes nothing");
    let (status, body) = send(
        &app,
        Method::POST,
        &through,
        &user,
        Some(serde_json::json!({"position": 1})),
    )
    .await;
    assert_eq!((status, body["marked"].as_u64()), (StatusCode::OK, Some(2)));
    let (status, body) = send(
        &app,
        Method::POST,
        &through,
        &user,
        Some(serde_json::json!({"position": 1, "dry_run": true})),
    )
    .await;
    assert_eq!(
        (status, body["marked"].as_u64()),
        (StatusCode::OK, Some(0)),
        "already read"
    );
    let (_, body) = get_json(&app, &links_uri(master), &user).await;
    assert_eq!(body["events"][0]["read"], 2);
    assert_eq!(
        body["events"][0]["first_unread"]["issue_id"],
        issues[12].as_str()
    );

    // The event list knows it is linked from one list.
    let (_, body) = get_json(&app, &links_uri(event), &user).await;
    assert_eq!(body["linked_from"], 1);
    assert_eq!(body["chapters"].as_array().unwrap().len(), 1);
    assert_eq!(body["chapters"][0]["label"], "House of M");

    // Skip, then un-skip.
    let skip = format!("{}/{link_id}/skip", links_uri(master));
    assert_eq!(
        send(&app, Method::PUT, &skip, &user, None).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send(&app, Method::PUT, &skip, &user, None).await.0,
        StatusCode::NO_CONTENT,
        "idempotent"
    );
    let (_, body) = get_json(&app, &links_uri(master), &user).await;
    assert_eq!(body["events"][0]["skipped"], true);
    assert_eq!(
        send(&app, Method::DELETE, &skip, &user, None).await.0,
        StatusCode::NO_CONTENT
    );
    let (_, body) = get_json(&app, &links_uri(master), &user).await;
    assert_eq!(body["events"][0]["skipped"], false);

    // Move it after position 7.
    let to = entry_at(&app, master, 7).await;
    let (status, body) = send(
        &app,
        Method::PATCH,
        &format!("{}/{link_id}", links_uri(master)),
        &user,
        Some(serde_json::json!({"after_entry_id": to})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    assert_eq!(body["events"][0]["anchor"]["position"], 7);
    assert_eq!(body["chapters"][0]["end_position"], 7);

    // Delete it.
    let (status, _) = send(
        &app,
        Method::DELETE,
        &format!("{}/{link_id}", links_uri(master)),
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = get_json(&app, &links_uri(master), &user).await;
    assert!(body["events"].as_array().unwrap().is_empty());
    assert_eq!(body["chapters"][0]["label"], "Master");
}

#[tokio::test]
async fn link_rules_refuse_nesting_loops_and_self_links() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "rules-admin@example.com").await;
    let user = register(&app, "rules-user@example.com").await;
    let (_, issues) = seed_cbl_with_matched_entries(&app, user.user_id, 6).await;
    let a = make_list(&app, Some(user.user_id), "A", &issues[..2]).await;
    let b = make_list(&app, Some(user.user_id), "B", &issues[2..4]).await;
    let c = make_list(&app, Some(user.user_id), "C", &issues[4..]).await;
    let post = |list: Uuid, body: serde_json::Value| {
        let app = &app;
        let user = &user;
        async move { send(app, Method::POST, &links_uri(list), user, Some(body)).await }
    };

    let (status, body) = post(
        a,
        serde_json::json!({"child_list_id": a, "kind": "continues"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "self-link: {body:#?}"
    );

    // A → B as an event: B can't take events, and A can't become an event.
    let after = entry_at(&app, a, 0).await;
    let (status, _) = post(
        a,
        serde_json::json!({"child_list_id": b, "kind": "event", "after_entry_id": after}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let b_entry = entry_at(&app, b, 0).await;
    let (status, body) = post(
        b,
        serde_json::json!({"child_list_id": c, "kind": "event", "after_entry_id": b_entry}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "event list taking events: {body:#?}"
    );
    let c_entry = entry_at(&app, c, 0).await;
    let (status, body) = post(
        c,
        serde_json::json!({"child_list_id": a, "kind": "event", "after_entry_id": c_entry}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "list with events as an event: {body:#?}"
    );

    // Parts: A → C, then C → A would loop.
    let (status, _) = post(
        a,
        serde_json::json!({"child_list_id": c, "kind": "continues"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = post(
        c,
        serde_json::json!({"child_list_id": a, "kind": "continues"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "loop: {body:#?}");

    // An entry of another list is not an anchor here.
    let (status, body) = post(
        a,
        serde_json::json!({"child_list_id": c, "kind": "event", "after_entry_id": c_entry}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#?}");
}

#[tokio::test]
async fn shared_lists_link_shared_lists_and_admin_writes_are_audited() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "share-admin@example.com").await;
    let user = register(&app, "share-user@example.com").await;
    let (_, issues) = seed_cbl_with_matched_entries(&app, user.user_id, 6).await;
    let shared_master = make_list(&app, None, "Shared master", &issues[..2]).await;
    let shared_event = make_list(&app, None, "Shared event", &issues[2..4]).await;
    let admins_private = make_list(&app, Some(admin.user_id), "Admin private", &issues[4..]).await;
    let users_private = make_list(&app, Some(user.user_id), "User private", &issues[4..]).await;
    let after = entry_at(&app, shared_master, 1).await;

    let (status, body) = send(
        &app,
        Method::POST,
        &links_uri(shared_master),
        &admin,
        Some(serde_json::json!({"child_list_id": admins_private, "kind": "event", "after_entry_id": after})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body:#?}");
    assert_eq!(body["error"]["details"][0]["field"], "child_list_id");

    let (status, _) = send(
        &app,
        Method::POST,
        &links_uri(shared_master),
        &user,
        Some(serde_json::json!({"child_list_id": shared_event, "kind": "event", "after_entry_id": after})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "readers don't manage shared lists"
    );

    let (status, body) = send(
        &app,
        Method::POST,
        &links_uri(shared_master),
        &admin,
        Some(serde_json::json!({"child_list_id": shared_event, "kind": "event", "after_entry_id": after})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:#?}");
    let link_id = body["events"][0]["id"].as_str().unwrap().to_owned();
    let db = Database::connect(&app.db_url).await.unwrap();
    let audited = audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("admin.cbl_list.link.create"))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(audited.len(), 1);

    // A reader sees the event and can skip it for themselves.
    let (status, body) = get_json(&app, &links_uri(shared_master), &user).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["events"][0]["child_name"], "Shared event");
    let skip = format!("{}/{link_id}/skip", links_uri(shared_master));
    assert_eq!(
        send(&app, Method::PUT, &skip, &user, None).await.0,
        StatusCode::NO_CONTENT
    );
    let (_, body) = get_json(&app, &links_uri(shared_master), &admin).await;
    assert_eq!(body["events"][0]["skipped"], false, "skip is per reader");

    // The reader has no view of the shared event: the band opens it by
    // creating theirs once, then reuses it.
    let (_, body) = get_json(&app, &links_uri(shared_master), &user).await;
    let admin_view = body["events"][0]["child_view_id"].clone();
    assert!(
        admin_view.is_null(),
        "the admin's personal view isn't theirs"
    );
    let view_uri = format!("/api/me/cbl-lists/{shared_event}/view");
    let (status, first) = send(&app, Method::POST, &view_uri, &user, None).await;
    assert_eq!(status, StatusCode::CREATED, "{first:#?}");
    assert_eq!(first["created"], true);
    let (status, again) = send(&app, Method::POST, &view_uri, &user, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["view_id"], first["view_id"]);
    let (_, body) = get_json(&app, &links_uri(shared_master), &user).await;
    assert_eq!(body["events"][0]["child_view_id"], first["view_id"]);

    // Someone else's private list answers like an unknown one.
    let (status, _) = get_json(&app, &links_uri(users_private), &admin).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &app,
        Method::POST,
        &format!("/api/me/cbl-lists/{users_private}/view"),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &app,
        Method::DELETE,
        &format!("{}/{link_id}", links_uri(shared_master)),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let deleted = audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("admin.cbl_list.link.delete"))
        .all(&db)
        .await
        .unwrap();
    assert_eq!(deleted.len(), 1);
}

fn breaks_uri(list: Uuid) -> String {
    format!("/api/me/cbl-lists/{list}/breaks")
}

/// (start, end, label, break_id set) of each chapter.
fn chapter_shape(body: &serde_json::Value) -> Vec<(i64, i64, String, bool)> {
    body["chapters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["start_position"].as_i64().unwrap(),
                c["end_position"].as_i64().unwrap(),
                c["label"].as_str().unwrap().to_owned(),
                c["break_id"].is_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn chapter_breaks_split_chapters_with_and_without_events() {
    let app = TestApp::spawn().await;
    let user = register(&app, "breaks-admin@example.com").await;
    let other = register(&app, "breaks-other@example.com").await;
    let (_, issues) = seed_cbl_with_matched_entries(&app, user.user_id, 14).await;
    let master = make_list(&app, Some(user.user_id), "Master", &issues[..10]).await;
    let event = make_list(&app, Some(user.user_id), "House of M", &issues[10..]).await;

    // An unnamed break at 3 is named after its first issue.
    let at3 = entry_at(&app, master, 3).await;
    let (status, body) = send(
        &app,
        Method::POST,
        &breaks_uri(master),
        &user,
        Some(serde_json::json!({ "entry_id": at3 })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:#?}");
    assert_eq!(
        chapter_shape(&body),
        vec![
            (0, 2, "Master".into(), false),
            (3, 9, "From Master series #3".into(), true),
        ]
    );
    let break3 = body["breaks"][0]["id"].as_str().unwrap().to_owned();

    // An event after 5 ends the second chapter; a named break at 8.
    let at5 = entry_at(&app, master, 5).await;
    let (status, _) = send(
        &app,
        Method::POST,
        &links_uri(master),
        &user,
        Some(serde_json::json!({"child_list_id": event, "kind": "event", "after_entry_id": at5})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let at8 = entry_at(&app, master, 8).await;
    let (status, body) = send(
        &app,
        Method::POST,
        &breaks_uri(master),
        &user,
        Some(serde_json::json!({ "entry_id": at8, "label": "  Aftermath  " })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:#?}");
    assert_eq!(
        chapter_shape(&body),
        vec![
            (0, 2, "Master".into(), false),
            (3, 5, "Up to House of M".into(), true),
            (6, 7, "After House of M".into(), false),
            (8, 9, "Aftermath".into(), true),
        ]
    );
    assert_eq!(
        body["chapters"][1]["event_link_ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(body["chapters"][1]["break_id"], break3.as_str());
    let break8 = body["breaks"][1]["id"].as_str().unwrap().to_owned();

    // Rules: the first issue, a second break at one issue, an over-long name.
    let at0 = entry_at(&app, master, 0).await;
    let (status, body) = send(
        &app,
        Method::POST,
        &breaks_uri(master),
        &user,
        Some(serde_json::json!({ "entry_id": at0 })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["details"][0]["field"], "entry_id");
    let (status, _) = send(
        &app,
        Method::POST,
        &breaks_uri(master),
        &user,
        Some(serde_json::json!({ "entry_id": at3 })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, body) = send(
        &app,
        Method::POST,
        &breaks_uri(master),
        &user,
        Some(serde_json::json!({ "entry_id": at5, "label": "x".repeat(121) })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["details"][0]["field"], "label");

    // Someone else's personal list answers like an unknown one.
    let (status, _) = send(
        &app,
        Method::POST,
        &breaks_uri(master),
        &other,
        Some(serde_json::json!({ "entry_id": at5 })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Rename, unname (blank), remove.
    let (status, body) = send(
        &app,
        Method::PATCH,
        &format!("{}/{break3}", breaks_uri(master)),
        &user,
        Some(serde_json::json!({ "label": "Decimation" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    assert_eq!(body["chapters"][1]["label"], "Decimation");
    let (_, body) = send(
        &app,
        Method::PATCH,
        &format!("{}/{break8}", breaks_uri(master)),
        &user,
        Some(serde_json::json!({ "label": " " })),
    )
    .await;
    assert!(body["breaks"][1]["label"].is_null());
    assert_eq!(
        body["chapters"][3]["label"], "From Master series #8",
        "not right after the event, so named after its first issue"
    );
    let (status, _) = send(
        &app,
        Method::DELETE,
        &format!("{}/{break3}", breaks_uri(master)),
        &user,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = get_json(&app, &links_uri(master), &user).await;
    assert_eq!(
        chapter_shape(&body),
        vec![
            (0, 5, "Up to House of M".into(), false),
            (6, 7, "After House of M".into(), false),
            (8, 9, "From Master series #8".into(), true),
        ]
    );
}

#[tokio::test]
async fn admin_breaks_on_a_shared_list_are_audited() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "breaks-share-admin@example.com").await;
    let reader = register(&app, "breaks-share-reader@example.com").await;
    let (_, issues) = seed_cbl_with_matched_entries(&app, admin.user_id, 4).await;
    let shared = make_list(&app, None, "Shared", &issues).await;
    let at2 = entry_at(&app, shared, 2).await;

    let (status, _) = send(
        &app,
        Method::POST,
        &breaks_uri(shared),
        &reader,
        Some(serde_json::json!({ "entry_id": at2 })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "readers don't manage shared lists"
    );
    let (status, body) = send(
        &app,
        Method::POST,
        &breaks_uri(shared),
        &admin,
        Some(serde_json::json!({ "entry_id": at2, "label": "Part two" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:#?}");
    let (_, body) = get_json(&app, &links_uri(shared), &reader).await;
    assert_eq!(
        body["chapters"][1]["label"], "Part two",
        "readers see breaks"
    );

    let db = Database::connect(&app.db_url).await.unwrap();
    let audited = audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("admin.cbl_list.break.create"))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(audited, 1);
}

#[tokio::test]
async fn entries_seek_by_position_and_walk_back() {
    let app = TestApp::spawn().await;
    let _admin = register(&app, "seek-admin@example.com").await;
    let user = register(&app, "seek-user@example.com").await;
    let (list, _) = seed_cbl_with_matched_entries(&app, user.user_id, 10).await;
    let base = format!("/api/me/cbl-lists/{list}/entries?limit=3");
    let positions = |body: &serde_json::Value| -> Vec<i64> {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["position"].as_i64().unwrap())
            .collect()
    };

    let (status, body) = get_json(&app, &format!("{base}&from_position=5"), &user).await;
    assert_eq!(status, StatusCode::OK, "{body:#?}");
    assert_eq!(positions(&body), vec![5, 6, 7]);
    assert_eq!(body["total"], 10);
    let prev = body["prev_cursor"]
        .as_str()
        .expect("rows precede position 5")
        .to_owned();
    assert!(body["next_cursor"].is_string());

    let (_, body) = get_json(&app, &format!("{base}&before={prev}"), &user).await;
    assert_eq!(positions(&body), vec![2, 3, 4]);
    let prev = body["prev_cursor"]
        .as_str()
        .expect("rows precede position 2")
        .to_owned();
    let (_, body) = get_json(&app, &format!("{base}&before={prev}"), &user).await;
    assert_eq!(positions(&body), vec![0, 1]);
    assert!(
        body.get("prev_cursor").is_none_or(|v| v.is_null()),
        "top reached"
    );

    let (_, body) = get_json(&app, &format!("{base}&from_position=0"), &user).await;
    assert_eq!(positions(&body), vec![0, 1, 2]);
    assert!(body.get("prev_cursor").is_none_or(|v| v.is_null()));

    let (status, _) = get_json(
        &app,
        &format!("{base}&from_position=2&before={prev}"),
        &user,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

// ───── PR 3b: find events and parts from the catalog ─────

const PART1: &str = "Marvel/Master Reading Order/CBRO/Marvel Master Part #1.cbl";
const PART2: &str = "Marvel/Master Reading Order/CBRO/Marvel Master Part #2.cbl";
const PART3: &str = "Marvel/Master Reading Order/CBRO/Marvel Master Part #3.cbl";
const EVENT: &str = "Marvel/Events/CBRO/2004-2006 - Part 7/House of M.cbl";

/// A catalog source whose cached index is fresh (so nothing asks GitHub)
/// and lists `paths`. None has a with-events twin, so a preview never
/// downloads anything.
async fn seed_catalog(app: &TestApp, paths: &[&str]) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let id = Uuid::now_v7();
    let entries: Vec<_> = paths
        .iter()
        .map(|p| {
            serde_json::json!({
                "path": p, "name": p, "publisher": "Marvel",
                "sha": "0123456789abcdef0123456789abcdef01234567", "size": 100,
            })
        })
        .collect();
    entity::catalog_source::ActiveModel {
        id: Set(id),
        display_name: Set("Test catalog".into()),
        github_owner: Set(format!("folio-test-owner-{id}")),
        github_repo: Set("no-such-repository".into()),
        github_branch: Set("main".into()),
        enabled: Set(true),
        last_indexed_at: Set(Some(now)),
        index_etag: Set(None),
        index_json: Set(Some(serde_json::json!({ "entries": entries }))),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    id
}

/// Make `list` look imported from `source` at `path`.
async fn from_catalog(app: &TestApp, list: Uuid, source: Uuid, path: &str) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let row = cbl_list::Entity::find_by_id(list)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: cbl_list::ActiveModel = row.into();
    am.source_kind = Set("catalog".into());
    am.catalog_source_id = Set(Some(source));
    am.catalog_path = Set(Some(path.into()));
    am.update(&db).await.unwrap();
}

fn derive_uri(list: Uuid) -> String {
    format!("/api/me/cbl-lists/{list}/links/derive")
}

#[tokio::test]
async fn derive_chains_parts_and_links_imported_events() {
    let app = TestApp::spawn().await;
    let user = register(&app, "derive-user@example.com").await;
    let other = register(&app, "derive-other@example.com").await;
    let (_, issues) = seed_cbl_with_matched_entries(&app, user.user_id, 8).await;
    let source = seed_catalog(&app, &[PART1, PART2, PART3, EVENT]).await;
    let part1 = make_list(&app, Some(user.user_id), "Part 1", &issues[..3]).await;
    let part2 = make_list(&app, Some(user.user_id), "Part 2", &issues[3..5]).await;
    let event = make_list(&app, Some(user.user_id), "House of M", &issues[5..]).await;
    // Someone else's copy of part 3 is never proposed.
    let others = make_list(&app, Some(other.user_id), "Part 3", &issues[..1]).await;
    from_catalog(&app, part2, source, PART2).await;
    from_catalog(&app, part1, source, PART1).await;
    from_catalog(&app, event, source, EVENT).await;
    from_catalog(&app, others, source, PART3).await;

    // Part 2's neighbours: part 1 (previous), and no next of the user's.
    let (status, preview) = send(&app, Method::POST, &derive_uri(part2), &user, None).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["twin_found"], false);
    assert_eq!(preview["events"].as_array().unwrap().len(), 0);
    let parts = preview["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 1, "{preview}");
    assert_eq!(parts[0]["direction"], "previous");
    assert_eq!(parts[0]["list_id"], part1.to_string());
    assert_eq!(parts[0]["already_linked"], false);

    // Someone else can't preview the user's list.
    let (status, _) = send(&app, Method::POST, &derive_uri(part2), &other, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Apply: chain part 1 → part 2, and place the event after part 2's
    // first issue (the event list is already the user's, so no import).
    let after = entry_at(&app, part2, 0).await;
    let body = serde_json::json!({
        "events": [
            {"catalog_path": EVENT, "after_entry_id": after},
            {"catalog_path": "Marvel/Events/CBRO/Nowhere.cbl", "after_entry_id": after},
        ],
        "parts": [{"direction": "previous", "list_id": part1}],
    });
    let (status, resp) = send(
        &app,
        Method::POST,
        &format!("{}/apply", derive_uri(part2)),
        &user,
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["imported"], 0);
    assert_eq!(resp["linked"], 2);
    let failed = resp["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 1, "{resp}");
    assert_eq!(failed[0]["message"], "not in the catalog");
    let events = resp["links"]["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["child_list_id"], event.to_string());

    let db = Database::connect(&app.db_url).await.unwrap();
    let links = entity::reading_list_link::Entity::find()
        .all(&db)
        .await
        .unwrap();
    use entity::reading_list_link::{LinkKind, LinkPlacement};
    let ev = links.iter().find(|l| l.kind == LinkKind::Event).unwrap();
    assert_eq!(ev.parent_list_id, part2);
    assert_eq!(ev.placement, LinkPlacement::AutoTwin);
    let chain = links
        .iter()
        .find(|l| l.kind == LinkKind::Continues)
        .unwrap();
    assert_eq!((chain.parent_list_id, chain.child_list_id), (part1, part2));
    assert_eq!(chain.placement, LinkPlacement::AutoChain);

    // Again: everything is already linked, nothing new.
    let (_, preview) = send(&app, Method::POST, &derive_uri(part2), &user, None).await;
    assert_eq!(preview["parts"][0]["already_linked"], true);
    let (status, resp) = send(
        &app,
        Method::POST,
        &format!("{}/apply", derive_uri(part2)),
        &user,
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["linked"], 0);
}

#[tokio::test]
async fn derive_needs_a_catalog_list() {
    let app = TestApp::spawn().await;
    let user = register(&app, "derive-upload@example.com").await;
    let (_, issues) = seed_cbl_with_matched_entries(&app, user.user_id, 4).await;
    let uploaded = make_list(&app, Some(user.user_id), "Uploaded", &issues[..2]).await;

    let (status, preview) = send(&app, Method::POST, &derive_uri(uploaded), &user, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview["twin_found"], false);
    assert!(preview["parts"].as_array().unwrap().is_empty());

    let after = entry_at(&app, uploaded, 0).await;
    let (status, resp) = send(
        &app,
        Method::POST,
        &format!("{}/apply", derive_uri(uploaded)),
        &user,
        Some(serde_json::json!({
            "events": [{"catalog_path": EVENT, "after_entry_id": after}],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{resp}");
}

// ───── PR 4: Up Next across linked lists ─────

async fn finish(app: &TestApp, user_id: Uuid, issue_id: &str, ago_secs: i64) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let at = (Utc::now() - chrono::Duration::seconds(ago_secs)).fixed_offset();
    entity::progress_record::ActiveModel {
        user_id: Set(user_id),
        issue_id: Set(issue_id.into()),
        last_page: Set(19),
        percent: Set(1.0),
        finished: Set(true),
        finished_at: Set(Some(at)),
        updated_at: Set(at),
        device: Set(None),
        is_backfill: Set(false),
        run: Set(0),
        page_hash: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
}

async fn view_of(app: &TestApp, list: Uuid, auth: &Authed) -> String {
    let (status, body) = send(
        app,
        Method::POST,
        &format!("/api/me/cbl-lists/{list}/view"),
        auth,
        None,
    )
    .await;
    assert!(status.is_success(), "{body}");
    body["view_id"].as_str().unwrap().to_owned()
}

async fn link(
    app: &TestApp,
    auth: &Authed,
    parent: Uuid,
    child: Uuid,
    after: Option<i32>,
) -> String {
    let after_entry_id = match after {
        Some(p) => Some(entry_at(app, parent, p).await),
        None => None,
    };
    let kind = if after.is_some() {
        "event"
    } else {
        "continues"
    };
    let (status, body) = send(
        app,
        Method::POST,
        &links_uri(parent),
        auth,
        Some(serde_json::json!({"child_list_id": child, "kind": kind, "after_entry_id": after_entry_id})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let all = body["events"]
        .as_array()
        .unwrap()
        .iter()
        .chain(body["continues"].as_object().map(|_| &body["continues"]));
    all.into_iter()
        .find(|l| l["child_list_id"] == child.to_string())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn next_up(app: &TestApp, auth: &Authed, issue: &str, query: &str) -> serde_json::Value {
    let (status, body) = get_json(app, &format!("/api/issues/{issue}/next-up?{query}"), auth).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn detour_names(body: &serde_json::Value) -> Vec<String> {
    body["detours"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|d| d["name"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn up_next_offers_events_returns_from_them_and_continues_in_the_next_part() {
    let app = TestApp::spawn().await;
    let user = register(&app, "upnext-user@example.com").await;
    let (_, issues) = seed_cbl_with_matched_entries(&app, user.user_id, 12).await;
    let master = make_list(&app, Some(user.user_id), "Master", &issues[0..4]).await;
    let event_a = make_list(&app, Some(user.user_id), "Event A", &issues[4..6]).await;
    let event_b = make_list(&app, Some(user.user_id), "Event B", &issues[6..8]).await;
    let part2 = make_list(&app, Some(user.user_id), "Part 2", &issues[8..10]).await;
    let other = make_list(&app, Some(user.user_id), "Other master", &issues[10..12]).await;
    let _a = link(&app, &user, master, event_a, Some(1)).await;
    let b = link(&app, &user, master, event_b, Some(1)).await;
    link(&app, &user, master, part2, None).await;
    let master_view = view_of(&app, master, &user).await;
    let a_view = view_of(&app, event_a, &user).await;
    let part2_view = view_of(&app, part2, &user).await;
    let cbl = |v: &str| format!("cbl={v}");

    // At the anchor: the main line is next, both events are offered in
    // their order, each starting at its first unread issue.
    let body = next_up(&app, &user, &issues[1], &cbl(&master_view)).await;
    assert_eq!(body["source"], "cbl");
    assert_eq!(body["target"]["id"], issues[2]);
    assert!(body.get("switch_to").is_none());
    assert_eq!(detour_names(&body), ["Event A", "Event B"]);
    assert_eq!(body["detours"][0]["first_unread"]["issue_id"], issues[4]);
    assert_eq!(body["detours"][0]["view_id"], a_view);
    // Elsewhere: none.
    let body = next_up(&app, &user, &issues[0], &cbl(&master_view)).await;
    assert!(detour_names(&body).is_empty());

    // A skipped event isn't offered; a read one isn't either.
    let skip = format!("{}/{b}/skip", links_uri(master));
    assert_eq!(
        send(&app, Method::PUT, &skip, &user, None).await.0,
        StatusCode::NO_CONTENT
    );
    let body = next_up(&app, &user, &issues[1], &cbl(&master_view)).await;
    assert_eq!(detour_names(&body), ["Event A"]);
    finish(&app, user.user_id, &issues[4], 60).await;
    finish(&app, user.user_id, &issues[5], 50).await;
    let body = next_up(&app, &user, &issues[1], &cbl(&master_view)).await;
    assert!(detour_names(&body).is_empty(), "{body}");
    assert_eq!(
        send(&app, Method::DELETE, &skip, &user, None).await.0,
        StatusCode::NO_CONTENT
    );

    // At an event's end: back to the master, at the first unread issue
    // after the anchor, offering the anchor's other unread event.
    let from = format!("{}&via={master_view}", cbl(&a_view));
    let body = next_up(&app, &user, &issues[5], &from).await;
    assert_eq!(body["source"], "cbl");
    assert_eq!(body["target"]["id"], issues[2]);
    assert_eq!(body["cbl_list_id"], master.to_string());
    assert_eq!(body["switch_to"]["reason"], "return");
    assert_eq!(body["switch_to"]["view_id"], master_view);
    assert_eq!(detour_names(&body), ["Event B"]);

    // Two parents: without `via`, the most recently read one wins.
    link(&app, &user, other, event_a, Some(0)).await;
    let other_view = view_of(&app, other, &user).await;
    finish(&app, user.user_id, &issues[10], 5).await;
    let body = next_up(&app, &user, &issues[5], &cbl(&a_view)).await;
    assert_eq!(body["switch_to"]["list_id"], other.to_string(), "{body}");
    assert_eq!(body["target"]["id"], issues[11]);
    // `via` picks the parent the reader came from.
    let body = next_up(&app, &user, &issues[5], &from).await;
    assert_eq!(body["switch_to"]["list_id"], master.to_string());
    // A `via` that isn't a parent is stale; the fallback still applies.
    let body = next_up(
        &app,
        &user,
        &issues[5],
        &format!("{}&via={part2_view}", cbl(&a_view)),
    )
    .await;
    assert_eq!(body["via_param_was_stale"], true);
    assert_eq!(body["switch_to"]["view_id"], other_view);

    // The event list page lists its parents, most recently read first.
    let (_, links) = get_json(&app, &links_uri(event_a), &user).await;
    assert_eq!(links["linked_from"], 2);
    let parents: Vec<_> = links["parents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(parents, ["Other master", "Master"]);
    assert_eq!(links["parents"][1]["anchor_position"], 1);
    assert_eq!(links["parents"][1]["view_id"], master_view);

    // At the master's end: into the next part.
    let body = next_up(&app, &user, &issues[3], &cbl(&master_view)).await;
    assert_eq!(body["target"]["id"], issues[8], "{body:#}");
    assert_eq!(body["switch_to"]["reason"], "continues");
    assert_eq!(body["switch_to"]["view_id"], part2_view);
    let _ = event_b;
}

// ───── PR 5: reading lists in collections, read as detours ─────

async fn new_collection(app: &TestApp, auth: &Authed, name: &str) -> String {
    let (status, v) = send(
        app,
        Method::POST,
        "/api/me/collections",
        auth,
        Some(serde_json::json!({ "name": name })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    v["id"].as_str().unwrap().to_owned()
}

async fn add_to_collection(
    app: &TestApp,
    auth: &Authed,
    cid: &str,
    kind: &str,
    ref_id: &str,
) -> (StatusCode, serde_json::Value) {
    send(
        app,
        Method::POST,
        &format!("/api/me/collections/{cid}/entries"),
        auth,
        Some(serde_json::json!({ "entry_kind": kind, "ref_id": ref_id })),
    )
    .await
}

#[tokio::test]
async fn a_reading_list_in_a_collection_is_a_detour() {
    let app = TestApp::spawn().await;
    let user = register(&app, "coll-list@example.com").await;
    let other = register(&app, "coll-other@example.com").await;
    let (_, issues) = seed_cbl_with_matched_entries(&app, user.user_id, 8).await;
    let event = make_list(&app, Some(user.user_id), "Event", &issues[4..6]).await;
    let others_list = make_list(&app, Some(other.user_id), "Not yours", &issues[6..8]).await;
    let cid = new_collection(&app, &user, "My run").await;
    for i in &issues[0..2] {
        let (s, b) = add_to_collection(&app, &user, &cid, "issue", i).await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
    }
    let (s, b) = add_to_collection(&app, &user, &cid, "list", &event.to_string()).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert_eq!(b["list"]["name"], "Event");
    assert_eq!(b["list"]["total"], 2);
    for i in &issues[2..4] {
        add_to_collection(&app, &user, &cid, "issue", i).await;
    }
    // Twice is a conflict; someone else's private list is not found.
    let (s, _) = add_to_collection(&app, &user, &cid, "list", &event.to_string()).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = add_to_collection(&app, &user, &cid, "list", &others_list.to_string()).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Listed in its place; the reading order skips it.
    let (_, page) = get_json(&app, &format!("/api/me/collections/{cid}/entries"), &user).await;
    let kinds: Vec<_> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["entry_kind"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(kinds, ["issue", "issue", "list", "issue", "issue"]);
    assert_eq!(page["total"], 5);
    let ctx = format!("collection={cid}");
    let body = next_up(&app, &user, &issues[1], &ctx).await;
    assert_eq!(body["source"], "collection");
    assert_eq!(body["target"]["id"], issues[2], "{body}");
    assert_eq!(body["collection_total"], 4, "list entries add no issues");
    // ...and offered as a detour after the issue before it, only there.
    assert_eq!(detour_names(&body), ["Event"]);
    assert_eq!(body["detours"][0]["first_unread"]["issue_id"], issues[4]);
    let body = next_up(&app, &user, &issues[0], &ctx).await;
    assert!(detour_names(&body).is_empty());

    // At the list's end, read with `via` = the collection: back to it,
    // after the list's place.
    let event_view = view_of(&app, event, &user).await;
    let body = next_up(
        &app,
        &user,
        &issues[5],
        &format!("cbl={event_view}&via={cid}"),
    )
    .await;
    assert_eq!(body["source"], "collection", "{body}");
    assert_eq!(body["target"]["id"], issues[2]);
    assert_eq!(body["switch_to"]["collection"], true);
    assert_eq!(body["switch_to"]["view_id"], cid);
    assert_eq!(body["switch_to"]["reason"], "return");

    // A list the caller can no longer see drops out of the listing.
    let db = Database::connect(&app.db_url).await.unwrap();
    let row = cbl_list::Entity::find_by_id(event)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: cbl_list::ActiveModel = row.into();
    am.owner_user_id = Set(Some(other.user_id));
    am.update(&db).await.unwrap();
    let (_, page) = get_json(&app, &format!("/api/me/collections/{cid}/entries"), &user).await;
    assert_eq!(page["total"], 4);
    let body = next_up(&app, &user, &issues[1], &ctx).await;
    assert!(detour_names(&body).is_empty());
}

#[tokio::test]
async fn collection_list_entries_obey_the_kind_checks() {
    let app = TestApp::spawn().await;
    let user = register(&app, "coll-chk@example.com").await;
    let (_, issues) = seed_cbl_with_matched_entries(&app, user.user_id, 2).await;
    let list = make_list(&app, Some(user.user_id), "L", &issues[..1]).await;
    let cid = new_collection(&app, &user, "C").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let bad = |sql: String| {
        let db = db.clone();
        async move { db.execute_unprepared(&sql).await.is_err() }
    };
    use sea_orm::ConnectionTrait;
    // A list entry needs the list and nothing else.
    assert!(
        bad(format!(
            "INSERT INTO collection_entries (id, saved_view_id, position, entry_kind) \
             VALUES (gen_random_uuid(), '{cid}', 0, 'list')"
        ))
        .await
    );
    assert!(
        bad(format!(
            "INSERT INTO collection_entries (id, saved_view_id, position, entry_kind, issue_id, cbl_list_id) \
             VALUES (gen_random_uuid(), '{cid}', 0, 'list', '{}', '{list}')",
            issues[0]
        ))
        .await
    );
    assert!(
        bad(format!(
            "INSERT INTO collection_entries (id, saved_view_id, position, entry_kind, issue_id, cbl_list_id) \
             VALUES (gen_random_uuid(), '{cid}', 0, 'issue', '{}', '{list}')",
            issues[0]
        ))
        .await
    );
    assert!(
        !bad(format!(
            "INSERT INTO collection_entries (id, saved_view_id, position, entry_kind, cbl_list_id) \
             VALUES (gen_random_uuid(), '{cid}', 0, 'list', '{list}')"
        ))
        .await
    );
    // Deleting the list deletes the entry.
    cbl_list::Entity::delete_by_id(list)
        .exec(&db)
        .await
        .unwrap();
    let left = entity::collection_entry::Entity::find()
        .filter(entity::collection_entry::Column::CblListId.eq(list))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

async fn get_text(app: &TestApp, uri: &str, auth: &Authed) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .body(Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

/// PR 6: OPDS clients see a list's events, its next part and the lists
/// it is part of as `related` links (v1) / a `groups` navigation block
/// (v2), and a reading list kept in a collection as a subsection in its
/// place.
#[tokio::test]
async fn opds_feeds_carry_linked_lists_and_collection_list_entries() {
    let app = TestApp::spawn().await;
    let user = register(&app, "opds-links@example.com").await;
    let (master, issues) = seed_cbl_with_matched_entries(&app, user.user_id, 8).await;
    let event = make_list(&app, Some(user.user_id), "Age of Ultron", &issues[4..6]).await;
    let next = make_list(&app, Some(user.user_id), "Part Two", &issues[6..8]).await;
    link(&app, &user, master, event, Some(2)).await;
    link(&app, &user, master, next, None).await;

    // v1: the master lists the event (after #3) and the next part.
    let (s, xml) = get_text(&app, &format!("/opds/v1/lists/{master}"), &user).await;
    assert_eq!(s, StatusCode::OK, "{xml}");
    let event_href = format!("rel=\"related\" href=\"/opds/v1/lists/{event}\"");
    assert!(xml.contains(&event_href), "{xml}");
    assert!(xml.contains("title=\"Event after #3"), "{xml}");
    assert!(xml.contains("Age of Ultron\""), "{xml}");
    let next_href = format!("rel=\"related\" href=\"/opds/v1/lists/{next}\"");
    assert!(xml.contains(&next_href), "{xml}");
    assert!(xml.contains("title=\"Continues in: Part Two\""), "{xml}");
    // ...and the event points back at the master.
    let (_, xml) = get_text(&app, &format!("/opds/v1/lists/{event}"), &user).await;
    let back = format!("rel=\"related\" href=\"/opds/v1/lists/{master}\"");
    assert!(xml.contains(&back), "{xml}");
    assert!(xml.contains("title=\"Part of: "), "{xml}");

    // v2: the same links, plus a navigation group.
    let (s, body) = get_json(&app, &format!("/opds/v2/lists/{master}"), &user).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let related: Vec<&serde_json::Value> = body["links"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["rel"] == "related")
        .collect();
    assert_eq!(related.len(), 2, "{body}");
    let ev = related
        .iter()
        .find(|l| l["properties"]["folio:link"] == "event")
        .unwrap();
    assert_eq!(ev["properties"]["folio:afterPosition"], 3);
    let nav = body["groups"][0]["navigation"].as_array().unwrap();
    assert!(
        nav.iter()
            .any(|n| n["href"] == format!("/opds/v2/lists/{event}")),
        "{body}"
    );

    // A collection holding the event shows it as a subsection in place.
    let cid = new_collection(&app, &user, "Shelf").await;
    add_to_collection(&app, &user, &cid, "issue", &issues[0]).await;
    let (s, b) = add_to_collection(&app, &user, &cid, "list", &event.to_string()).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let (s, xml) = get_text(&app, &format!("/opds/v1/collections/{cid}"), &user).await;
    assert_eq!(s, StatusCode::OK, "{xml}");
    let sub = format!("rel=\"subsection\" href=\"/opds/v1/lists/{event}\"");
    assert!(xml.contains(&sub), "{xml}");
    let (_, body) = get_json(&app, &format!("/opds/v2/collections/{cid}"), &user).await;
    let entry_kinds: Vec<_> = body["navigation"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|n| n["properties"]["folio:entryKind"].clone())
                .collect()
        })
        .unwrap_or_default();
    assert!(entry_kinds.contains(&serde_json::json!("list")), "{body}");
}

fn cbl_xml(name: &str, series: &str, count: usize) -> String {
    let books: String = (1..=count)
        .map(|n| format!(r#"<Book Series="{series}" Number="{n}" Volume="2012" Year="2013"/>"#))
        .collect();
    format!("<ReadingList><Name>{name}</Name><Books>{books}</Books></ReadingList>")
}

async fn set_xml(app: &TestApp, list: Uuid, xml: String) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let row = cbl_list::Entity::find_by_id(list)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: cbl_list::ActiveModel = row.into();
    am.raw_xml = Set(xml);
    am.update(&db).await.unwrap();
}

/// The CBL export is the stored file unless `inline_events=true`, which
/// writes each placed event's books in after its anchor issue.
#[tokio::test]
async fn cbl_export_can_inline_events() {
    let app = TestApp::spawn().await;
    let user = register(&app, "inline@example.com").await;
    let (master, issues) = seed_cbl_with_matched_entries(&app, user.user_id, 4).await;
    let event = make_list(&app, Some(user.user_id), "Event", &issues[0..2]).await;
    let master_xml = cbl_xml("Master", "Main", 4);
    set_xml(&app, master, master_xml.clone()).await;
    set_xml(&app, event, cbl_xml("Event", "Side", 2)).await;
    link(&app, &user, master, event, Some(1)).await;

    let (s, plain) = get_text(&app, &format!("/api/me/cbl-lists/{master}/export"), &user).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(plain, master_xml, "the stored file, unchanged");

    let (s, xml) = get_text(
        &app,
        &format!("/api/me/cbl-lists/{master}/export?inline_events=true"),
        &user,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{xml}");
    let parsed = parsers::cbl::parse(xml.as_bytes()).unwrap();
    let order: Vec<String> = parsed
        .books
        .iter()
        .map(|b| format!("{} {}", b.series, b.number))
        .collect();
    assert_eq!(
        order,
        ["Main 1", "Main 2", "Side 1", "Side 2", "Main 3", "Main 4"]
    );
    assert_eq!(parsed.name, "Master");
}

/// 2.0 PR 2: Up Next across links when the straight path isn't there.
/// - an event anchored after an issue Up Next steps over is still offered;
/// - an event's end with nothing left in the master follows the master's
///   next part, past a finished part, else reports "back to the master"
///   instead of falling into series order;
/// - an event opened on its own doesn't return into a master the reader
///   never read, and never into one whose anchor is lost.
#[tokio::test]
async fn up_next_handles_passed_anchors_event_ends_and_part_chains() {
    let app = TestApp::spawn().await;
    let user = register(&app, "upnext-edges@example.com").await;
    let (_, issues) = seed_cbl_with_matched_entries(&app, user.user_id, 14).await;
    let master = make_list(&app, Some(user.user_id), "Master", &issues[0..4]).await;
    let event = make_list(&app, Some(user.user_id), "Event", &issues[4..6]).await;
    let part2 = make_list(&app, Some(user.user_id), "Part 2", &issues[6..8]).await;
    let part3 = make_list(&app, Some(user.user_id), "Part 3", &issues[8..10]).await;
    let lone_event = make_list(&app, Some(user.user_id), "Lone event", &issues[10..12]).await;
    let lone_master = make_list(&app, Some(user.user_id), "Unread master", &issues[12..14]).await;
    link(&app, &user, master, event, Some(1)).await;
    link(&app, &user, master, part2, None).await;
    link(&app, &user, part2, part3, None).await;
    let lone_link = link(&app, &user, lone_master, lone_event, Some(0)).await;
    let master_view = view_of(&app, master, &user).await;
    let event_view = view_of(&app, event, &user).await;
    let part3_view = view_of(&app, part3, &user).await;
    let lone_event_view = view_of(&app, lone_event, &user).await;
    let lone_master_view = view_of(&app, lone_master, &user).await;
    let cbl = |v: &str| format!("cbl={v}");

    // The anchor issue (position 1) was already read: from position 0 Up
    // Next steps over it to position 2, and still offers the event.
    finish(&app, user.user_id, &issues[1], 300).await;
    let body = next_up(&app, &user, &issues[0], &cbl(&master_view)).await;
    assert_eq!(body["target"]["id"], issues[2], "{body}");
    assert_eq!(detour_names(&body), ["Event"]);
    // From position 2 it is behind the reader: not offered again.
    let body = next_up(&app, &user, &issues[2], &cbl(&master_view)).await;
    assert!(detour_names(&body).is_empty(), "{body}");

    // The master is finished after the event's place, and Part 2 is
    // finished too: the event's end continues in Part 3.
    for (i, ago) in [(2, 290), (3, 280), (6, 270), (7, 260)] {
        finish(&app, user.user_id, &issues[i], ago).await;
    }
    let from_master = format!("{}&via={master_view}", cbl(&event_view));
    let body = next_up(&app, &user, &issues[5], &from_master).await;
    assert_eq!(body["source"], "cbl", "{body}");
    assert_eq!(body["target"]["id"], issues[8]);
    assert_eq!(body["switch_to"]["reason"], "continues");
    assert_eq!(body["switch_to"]["list_id"], part3.to_string());
    assert_eq!(body["switch_to"]["view_id"], part3_view);
    // The master's own end also walks past the finished part.
    let body = next_up(&app, &user, &issues[3], &cbl(&master_view)).await;
    assert_eq!(body["target"]["id"], issues[8], "{body}");
    assert_eq!(body["switch_to"]["list_id"], part3.to_string());
    // The event's place is behind the reader by then: not offered.
    assert!(detour_names(&body).is_empty(), "{body}");

    // Nothing left anywhere: the order ends, pointing back at the master,
    // with no issue to open (not "next in series").
    finish(&app, user.user_id, &issues[8], 250).await;
    finish(&app, user.user_id, &issues[9], 240).await;
    let body = next_up(&app, &user, &issues[5], &from_master).await;
    assert_eq!(body["source"], "none", "{body}");
    assert!(body.get("target").is_none(), "{body}");
    assert_eq!(body["switch_to"]["reason"], "return");
    assert_eq!(body["switch_to"]["list_id"], master.to_string());
    assert_eq!(body["switch_to"]["view_id"], master_view);

    // An event opened on its own: no jump into a master never read.
    let body = next_up(&app, &user, &issues[11], &cbl(&lone_event_view)).await;
    assert!(body.get("switch_to").is_none(), "{body}");
    // Coming from that master (`via`), it returns there.
    let via = format!("{}&via={lone_master_view}", cbl(&lone_event_view));
    let body = next_up(&app, &user, &issues[11], &via).await;
    assert_eq!(body["switch_to"]["reason"], "return", "{body}");
    assert_eq!(body["target"]["id"], issues[13]);
    // A lost anchor's last position is not a place to return to.
    let db = Database::connect(&app.db_url).await.unwrap();
    let row = entity::reading_list_link::Entity::find_by_id(Uuid::parse_str(&lone_link).unwrap())
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::reading_list_link::ActiveModel = row.into();
    am.anchor_status = Set(entity::reading_list_link::AnchorStatus::Lost);
    am.update(&db).await.unwrap();
    let body = next_up(&app, &user, &issues[11], &via).await;
    assert!(body.get("switch_to").is_none(), "{body}");
}

/// 2.0 PR 2: a reading list in a collection is offered when Up Next
/// steps over the item it follows, and when it sits at the very top.
#[tokio::test]
async fn collection_lists_are_offered_across_skipped_items_and_at_the_top() {
    let app = TestApp::spawn().await;
    let user = register(&app, "coll-range@example.com").await;
    let (_, issues) = seed_cbl_with_matched_entries(&app, user.user_id, 8).await;
    let mid = make_list(&app, Some(user.user_id), "Mid list", &issues[4..6]).await;
    let top = make_list(&app, Some(user.user_id), "Top list", &issues[6..8]).await;

    // [issue0, issue1, Mid list, issue2]; issue1 is already read.
    let cid = new_collection(&app, &user, "Run").await;
    add_to_collection(&app, &user, &cid, "issue", &issues[0]).await;
    add_to_collection(&app, &user, &cid, "issue", &issues[1]).await;
    add_to_collection(&app, &user, &cid, "list", &mid.to_string()).await;
    add_to_collection(&app, &user, &cid, "issue", &issues[2]).await;
    finish(&app, user.user_id, &issues[1], 60).await;
    let body = next_up(&app, &user, &issues[0], &format!("collection={cid}")).await;
    assert_eq!(body["target"]["id"], issues[2], "{body}");
    assert_eq!(detour_names(&body), ["Mid list"]);

    // [Top list, issue3]: no issue comes before the list, so the first
    // issue offers it.
    let cid2 = new_collection(&app, &user, "Top first").await;
    add_to_collection(&app, &user, &cid2, "list", &top.to_string()).await;
    add_to_collection(&app, &user, &cid2, "issue", &issues[3]).await;
    add_to_collection(&app, &user, &cid2, "issue", &issues[0]).await;
    let body = next_up(&app, &user, &issues[3], &format!("collection={cid2}")).await;
    assert_eq!(detour_names(&body), ["Top list"], "{body}");
}
