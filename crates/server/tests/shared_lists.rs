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

    // Not shared yet → Bob can't see it.
    let (status, _) = http(&app, Method::GET, &list_url, Some(&bob), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

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
