//! Shared reading lists (admin share-with-everyone).
//!
//! Covers:
//!   - Admin share: an admin's own CBL view becomes a shared list every
//!     user can see; only admins may share, and only their own lists.
//!   - Shared lists are admin-managed: a regular user can read but not
//!     edit or delete them.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use entity::{cbl_entry, cbl_list, user::Entity as UserEntity};
use sea_orm::{ActiveModelTrait, ConnectionTrait, Database, EntityTrait, Set};
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

async fn demote_to_user(app: &TestApp, user_id: Uuid) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let user = UserEntity::find_by_id(user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let mut am: entity::user::ActiveModel = user.into();
    am.role = Set(entity::user::UserRole::User);
    am.update(&db).await.unwrap();
}

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: Option<&Authed>,
    body: Option<serde_json::Value>,
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
    let req = if let Some(b) = body {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

/// A shared CBL list whose entries can be unmatched (`None`), with
/// `match_status = 'missing'` — for exercising lists whose head entry
/// has no local match.
async fn seed_cbl_list_mixed(app: &TestApp, name: &str, entries: &[(i32, Option<&str>)]) -> Uuid {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    let list_id = Uuid::now_v7();
    cbl_list::ActiveModel {
        id: Set(list_id),
        owner_user_id: Set(None),
        source_kind: Set("upload".into()),
        source_url: Set(None),
        catalog_source_id: Set(None),
        catalog_path: Set(None),
        github_blob_sha: Set(None),
        source_etag: Set(None),
        source_last_modified: Set(None),
        raw_sha256: Set(vec![0u8; 32]),
        raw_xml: Set("<ReadingList />".into()),
        parsed_name: Set(name.into()),
        parsed_matchers_present: Set(false),
        num_issues_declared: Set(Some(entries.len() as i32)),
        description: Set(None),
        imported_at: Set(now),
        last_refreshed_at: Set(None),
        last_match_run_at: Set(None),
        refresh_schedule: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        preserve_canonical_order: Set(false),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();

    for (pos, issue_id) in entries {
        let matched = issue_id.is_some();
        cbl_entry::ActiveModel {
            id: Set(Uuid::now_v7()),
            cbl_list_id: Set(list_id),
            position: Set(*pos),
            series_name: Set("Series".into()),
            issue_number: Set(format!("{pos}")),
            volume: Set(None),
            year: Set(None),
            cv_series_id: Set(None),
            cv_issue_id: Set(None),
            metron_series_id: Set(None),
            metron_issue_id: Set(None),
            matched_issue_id: Set(issue_id.map(Into::into)),
            match_status: Set(if matched { "matched" } else { "missing" }.into()),
            match_method: Set(matched.then(|| "test".into())),
            match_confidence: Set(matched.then_some(1.0)),
            ambiguous_candidates: Set(None),
            user_resolved_at: Set(None),
            matched_at: Set(matched.then_some(now)),
        }
        .insert(&db)
        .await
        .unwrap();
    }
    list_id
}

/// An admin-owned CBL list + saved view over `entries` (the shape a normal
/// import produces). Returns `(list_id, view_id)`.
async fn seed_owned_cbl_view(
    app: &TestApp,
    owner: Uuid,
    name: &str,
    entries: &[(i32, Option<&str>)],
) -> (Uuid, Uuid) {
    let list_id = seed_cbl_list_mixed(app, name, entries).await;
    let db = Database::connect(&app.db_url).await.unwrap();
    db.execute_unprepared(&format!(
        "UPDATE cbl_lists SET owner_user_id = '{owner}' WHERE id = '{list_id}'"
    ))
    .await
    .unwrap();
    let view_id = Uuid::now_v7();
    db.execute_unprepared(&format!(
        "INSERT INTO saved_views (id, user_id, kind, name, custom_tags, cbl_list_id, auto_pin) \
         VALUES ('{view_id}', '{owner}', 'cbl', '{name}', ARRAY[]::text[], '{list_id}', FALSE)"
    ))
    .await
    .unwrap();
    (list_id, view_id)
}

// ───── Tests ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_shares_a_list_with_everyone() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let bob = register(&app, "bob@example.com").await;
    demote_to_user(&app, bob.user_id).await;
    let (list_id, view_id) = seed_owned_cbl_view(
        &app,
        admin.user_id,
        "Intro to Marvel",
        &[(1, None), (2, None)],
    )
    .await;
    let list_url = format!("/api/me/cbl-lists/{list_id}");
    let share_url = format!("/api/admin/saved-views/{view_id}/share");

    // Not shared yet → Bob can't see it: it answers like an unknown list.
    let (status, nf) = http(&app, Method::GET, &list_url, Some(&bob), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(nf["error"]["code"], "not_found", "{nf}");

    // Only an admin can share.
    let (status, _) = http(&app, Method::POST, &share_url, Some(&bob), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, json) = http(&app, Method::POST, &share_url, Some(&admin), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{json:#?}");

    // Shared: Bob can read it and sees it among his saved views...
    let (status, _) = http(&app, Method::GET, &list_url, Some(&bob), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, json) = http(&app, Method::GET, "/api/me/saved-views", Some(&bob), None).await;
    assert!(
        json["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == view_id.to_string()),
        "shared view visible to everyone: {json:#?}"
    );
    // ...but can't change or delete it.
    let (status, _) = http(
        &app,
        Method::PATCH,
        &list_url,
        Some(&bob),
        Some(serde_json::json!({ "refresh_schedule": null })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "regular users can't edit a shared list"
    );
    let (status, _) = http(&app, Method::DELETE, &list_url, Some(&bob), None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "regular users can't delete a shared list"
    );
    // The admin still can.
    let (status, _) = http(
        &app,
        Method::PATCH,
        &list_url,
        Some(&admin),
        Some(serde_json::json!({ "refresh_schedule": null })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn share_rejects_a_view_the_admin_does_not_own() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    let bob = register(&app, "bob@example.com").await;
    demote_to_user(&app, bob.user_id).await;
    let (_list, view_id) = seed_owned_cbl_view(&app, bob.user_id, "Bobs list", &[]).await;
    let (status, _) = http(
        &app,
        Method::POST,
        &format!("/api/admin/saved-views/{view_id}/share"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ───── library ACL on a shared list ─────

async fn grant_library(app: &TestApp, user_id: Uuid, library_id: Uuid, cap: Option<&str>) {
    let db = Database::connect(&app.db_url).await.unwrap();
    let now = Utc::now().fixed_offset();
    entity::library_user_access::ActiveModel {
        user_id: Set(user_id),
        library_id: Set(library_id),
        age_rating_max: Set(cap.map(str::to_owned)),
        hide_unrated: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
}

fn entry_at(body: &serde_json::Value, position: i64) -> &serde_json::Value {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["position"].as_i64() == Some(position))
        .unwrap_or_else(|| panic!("entry {position}: {body:#?}"))
}

fn match_facts(e: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "match_status": e["match_status"],
        "matched_issue_id": e["matched_issue_id"],
        "match_method": e["match_method"],
        "match_confidence": e["match_confidence"],
        "ambiguous_candidates": e["ambiguous_candidates"],
        "matched_at": e["matched_at"],
        "issue": e["issue"],
    })
}

fn stat_counts(stats: &serde_json::Value) -> [i64; 4] {
    [
        stats["total"].as_i64().unwrap(),
        stats["matched"].as_i64().unwrap(),
        stats["ambiguous"].as_i64().unwrap(),
        stats["missing"].as_i64().unwrap(),
    ]
}

/// A list shared by an admin (`owner_user_id IS NULL`) whose entries match
/// issues in a library some readers cannot see: every reader gets their own
/// view of it, the admin the full one, and a later grant reveals the stored
/// match.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_list_entries_respect_each_readers_library_access() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "acl-admin@example.com").await;
    let bob = register(&app, "acl-bob@example.com").await;
    let carol = register(&app, "acl-carol@example.com").await;
    demote_to_user(&app, bob.user_id).await;
    demote_to_user(&app, carol.user_id).await;

    let db = Database::connect(&app.db_url).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let lib_a_root = dir.path().join("lib_a");
    std::fs::create_dir_all(&lib_a_root).unwrap();
    let lib_a = common::seed::LibrarySeed::new(&lib_a_root)
        .insert(&db)
        .await;
    let lib_b_root = dir.path().join("lib_b");
    std::fs::create_dir_all(&lib_b_root).unwrap();
    let lib_b = common::seed::LibrarySeed::new(&lib_b_root)
        .insert(&db)
        .await;
    let series_a = common::seed::seed_series(&db, lib_a, "Open Series").await;
    let series_b = common::seed::seed_series(&db, lib_b, "Closed Series").await;
    let issue = |lib: Uuid, series: Uuid, n: f64, title: &'static str| {
        let db = db.clone();
        let path = dir.path().join(format!("{title}.cbz"));
        async move {
            common::seed::IssueSeed::new(lib, series, &path, title.as_bytes(), n)
                .with_title(title)
                .insert(&db)
                .await
        }
    };
    let open = issue(lib_a, series_a, 1.0, "Open Book").await;
    let mature = issue(lib_a, series_a, 2.0, "Late Show").await;
    let closed = issue(lib_b, series_b, 1.0, "Closed Book").await;
    db.execute_unprepared(&format!(
        "UPDATE issues SET age_rating = 'Mature 17+' WHERE id = '{mature}'"
    ))
    .await
    .unwrap();

    // 0 → A, 1 → B (hidden), 2 → missing, 3 → ambiguous [A, B], 4 → A Mature.
    let list_id = seed_cbl_list_mixed(
        &app,
        "Shared ACL",
        &[
            (0, Some(&open)),
            (1, Some(&closed)),
            (2, None),
            (4, Some(&mature)),
        ],
    )
    .await;
    cbl_entry::ActiveModel {
        id: Set(Uuid::now_v7()),
        cbl_list_id: Set(list_id),
        position: Set(3),
        series_name: Set("Series".into()),
        issue_number: Set("3".into()),
        volume: Set(None),
        year: Set(None),
        cv_series_id: Set(None),
        cv_issue_id: Set(None),
        metron_series_id: Set(None),
        metron_issue_id: Set(None),
        matched_issue_id: Set(None),
        match_status: Set("ambiguous".into()),
        match_method: Set(None),
        match_confidence: Set(None),
        ambiguous_candidates: Set(Some(serde_json::json!([
            { "issue_id": open, "series_name": "Open Series", "similarity": 0.9 },
            { "issue_id": closed, "series_name": "Closed Series", "similarity": 0.8 },
        ]))),
        user_resolved_at: Set(None),
        matched_at: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();
    grant_library(&app, bob.user_id, lib_a, None).await;
    grant_library(&app, carol.user_id, lib_a, Some("Teen")).await;

    let detail_url = format!("/api/me/cbl-lists/{list_id}");
    let entries_url = format!("/api/me/cbl-lists/{list_id}/entries");
    let window_url = format!("/api/me/cbl-lists/{list_id}/window");

    // Admin: the full view.
    let (_, detail) = http(&app, Method::GET, &detail_url, Some(&admin), None).await;
    assert_eq!(stat_counts(&detail["stats"]), [5, 3, 1, 1], "{detail:#?}");
    let (_, body) = http(&app, Method::GET, &entries_url, Some(&admin), None).await;
    assert_eq!(entry_at(&body, 1)["matched_issue_id"], closed.as_str());
    assert_eq!(
        entry_at(&body, 3)["ambiguous_candidates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // Bob (library A, uncapped).
    let (_, detail) = http(&app, Method::GET, &detail_url, Some(&bob), None).await;
    assert_eq!(stat_counts(&detail["stats"]), [5, 2, 0, 3], "{detail:#?}");
    let (_, body) = http(&app, Method::GET, &entries_url, Some(&bob), None).await;
    assert_eq!(body["total"].as_i64(), Some(5));
    assert_eq!(
        match_facts(entry_at(&body, 1)),
        match_facts(entry_at(&body, 2)),
        "{body:#?}"
    );
    assert_eq!(entry_at(&body, 1)["match_status"], "missing");
    // One visible candidate of two reads as missing, not ambiguous.
    assert_eq!(
        match_facts(entry_at(&body, 3)),
        match_facts(entry_at(&body, 2)),
        "{body:#?}"
    );
    let (_, found) = http(
        &app,
        Method::GET,
        &format!("{entries_url}?q=Closed"),
        Some(&bob),
        None,
    )
    .await;
    assert_eq!(found["total"].as_i64(), Some(0), "{found:#?}");
    let (_, matched) = http(
        &app,
        Method::GET,
        &format!("{entries_url}?status=matched"),
        Some(&bob),
        None,
    )
    .await;
    assert_eq!(matched["total"].as_i64(), Some(2), "{matched:#?}");
    let (_, window) = http(&app, Method::GET, &window_url, Some(&bob), None).await;
    assert_eq!(window["total_matched"].as_i64(), Some(2), "{window:#?}");

    // Carol (library A, capped at Teen): the Mature match is hidden too.
    let (_, detail) = http(&app, Method::GET, &detail_url, Some(&carol), None).await;
    assert_eq!(stat_counts(&detail["stats"]), [5, 1, 0, 4], "{detail:#?}");
    let (_, body) = http(&app, Method::GET, &entries_url, Some(&carol), None).await;
    assert_eq!(
        match_facts(entry_at(&body, 4)),
        match_facts(entry_at(&body, 2)),
        "{body:#?}"
    );
    let (_, found) = http(
        &app,
        Method::GET,
        &format!("{entries_url}?q=Late"),
        Some(&carol),
        None,
    )
    .await;
    assert_eq!(found["total"].as_i64(), Some(0), "{found:#?}");
    let (_, window) = http(&app, Method::GET, &window_url, Some(&carol), None).await;
    assert_eq!(window["total_matched"].as_i64(), Some(1), "{window:#?}");

    // Granting Bob library B reveals the stored match.
    grant_library(&app, bob.user_id, lib_b, None).await;
    let (_, detail) = http(&app, Method::GET, &detail_url, Some(&bob), None).await;
    assert_eq!(stat_counts(&detail["stats"]), [5, 3, 1, 1], "{detail:#?}");
    let (_, body) = http(&app, Method::GET, &entries_url, Some(&bob), None).await;
    assert_eq!(entry_at(&body, 1)["match_status"], "matched");
    assert_eq!(entry_at(&body, 1)["matched_issue_id"], closed.as_str());
    assert!(entry_at(&body, 1)["issue"].is_object());
}

// ───── Audit: admin writes to shared lists through `/me` routes ─────

async fn cbl_audit_rows(app: &TestApp) -> Vec<entity::audit_log::Model> {
    let db = Database::connect(&app.db_url).await.unwrap();
    let mut rows: Vec<_> = entity::audit_log::Entity::find()
        .all(&db)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.action.starts_with("admin.cbl_list."))
        .collect();
    // Ids are UUIDv7: id order is write order.
    rows.sort_by_key(|r| r.id);
    rows
}

/// An admin changing a shared list (no owner) through `/me/cbl-lists` is an
/// admin action on a list that is not theirs: each write leaves one audit
/// row naming the admin, the list and its (absent) owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_writes_to_a_shared_list_are_audited() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "shared-audit-admin@example.com").await;
    let list_id = seed_cbl_list_mixed(&app, "Shared Audit", &[(1, None)]).await;

    let (status, _) = http(
        &app,
        Method::PATCH,
        &format!("/api/me/cbl-lists/{list_id}"),
        Some(&admin),
        Some(serde_json::json!({ "description": "Read these first" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let rows = cbl_audit_rows(&app).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row.action, "admin.cbl_list.update");
    assert_eq!(row.actor_id, admin.user_id);
    assert_eq!(row.target_type.as_deref(), Some("cbl_list"));
    assert_eq!(row.target_id, Some(list_id.to_string()));
    assert!(row.payload["owner_user_id"].is_null(), "{}", row.payload);
    assert_eq!(row.payload["name"], "Shared Audit");
    assert_eq!(
        row.payload["changed"]["description"], "Read these first",
        "{}",
        row.payload
    );

    let db = Database::connect(&app.db_url).await.unwrap();
    let entry_id = cbl_entry::Entity::find()
        .all(&db)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.cbl_list_id == list_id)
        .unwrap()
        .id;
    let (status, _) = http(
        &app,
        Method::POST,
        &format!("/api/me/cbl-lists/{list_id}/entries/{entry_id}/clear-match"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = http(
        &app,
        Method::DELETE,
        &format!("/api/me/cbl-lists/{list_id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let actions: Vec<String> = cbl_audit_rows(&app)
        .await
        .into_iter()
        .map(|r| r.action)
        .collect();
    assert_eq!(
        actions,
        [
            "admin.cbl_list.update",
            "admin.cbl_list.entry.clear_match",
            "admin.cbl_list.delete",
        ]
    );
}

/// An owner editing their own list is not an admin action, whatever the
/// owner's role: no audit row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owners_editing_their_own_list_are_not_audited() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "own-list-admin@example.com").await;
    let reader = register(&app, "own-list-reader@example.com").await;
    demote_to_user(&app, reader.user_id).await;
    let (admin_list, _) = seed_owned_cbl_view(&app, admin.user_id, "Admin Own", &[(1, None)]).await;
    let (reader_list, _) =
        seed_owned_cbl_view(&app, reader.user_id, "Reader Own", &[(1, None)]).await;

    for (who, list) in [(&admin, admin_list), (&reader, reader_list)] {
        let (status, _) = http(
            &app,
            Method::PATCH,
            &format!("/api/me/cbl-lists/{list}"),
            Some(who),
            Some(serde_json::json!({ "description": "mine" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = http(
            &app,
            Method::DELETE,
            &format!("/api/me/cbl-lists/{list}"),
            Some(who),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    assert!(cbl_audit_rows(&app).await.is_empty());
}
