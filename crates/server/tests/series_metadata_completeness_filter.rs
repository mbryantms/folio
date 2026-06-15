//! `GET /series?metadata_completeness=<tier>` — the "Needs metadata" worklist
//! grid filter (metadata-at-scale B4, part 2).
//!
//! The grid facet must filter server-side by the SAME per-series rollup the
//! card badge and the saved-view `metadata_completeness` predicate use, so the
//! worklist grid and a saved view agree to the row — including the operator
//! "mark complete" acknowledgement, which drops an accepted series out of the
//! `needs_metadata` tier without faking field presence.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::{TestApp, seed};
use sea_orm::{ActiveModelTrait, EntityTrait, IntoActiveModel, Set};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

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
                    r#"{"email":"worklist-admin@example.com","password":"correctly-horse-battery"}"#,
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
    let pick = |p: &str| {
        cookies
            .iter()
            .find(|c| c.starts_with(p))
            .map(|c| {
                c.split(';')
                    .next()
                    .unwrap()
                    .trim_start_matches(p)
                    .to_owned()
            })
            .expect(p)
    };
    Authed {
        session: pick("__Host-comic_session="),
        csrf: pick("__Host-comic_csrf="),
    }
}

/// Promote a bare issue to "metadata satisfied": cover date + summary +
/// page count (seeded at 20) + a creator credit + a provider external id.
async fn make_complete(db: &impl sea_orm::ConnectionTrait, issue_id: &str) {
    let now = Utc::now().fixed_offset();
    let mut am = entity::issue::Entity::find_by_id(issue_id.to_owned())
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    am.year = Set(Some(2020));
    am.summary = Set(Some("A fully fleshed-out issue.".into()));
    am.writer = Set(Some("Jane Doe".into()));
    am.update(db).await.unwrap();
    entity::external_id::ActiveModel {
        entity_type: Set("issue".into()),
        entity_id: Set(issue_id.to_owned()),
        source: Set("comicvine".into()),
        // Unique per issue — the (source, external_id, entity_type) key is
        // unique, and this helper runs for more than one issue.
        external_id: Set(format!("cv-{issue_id}")),
        external_url: Set(None),
        set_by: Set("comicvine".into()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

/// Stamp the operator "mark complete" acknowledgement onto a bare issue.
async fn make_accepted(db: &impl sea_orm::ConnectionTrait, issue_id: &str) {
    let now = Utc::now().fixed_offset();
    let mut am = entity::issue::Entity::find_by_id(issue_id.to_owned())
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    am.metadata_review_accepted_at = Set(Some(now));
    am.update(db).await.unwrap();
}

async fn slugs_for(app: &TestApp, auth: &Authed, lib: Uuid, tier: &str) -> Vec<String> {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/api/series?library={lib}&metadata_completeness={tier}&limit=100"
                ))
                .header(header::COOKIE, auth.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "tier={tier}");
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["slug"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn filter_splits_series_by_completeness_tier_and_excludes_accepted() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let st = app.state();
    let db = &st.db;
    let dir = tempfile::tempdir().unwrap();
    let lib = seed::seed_library(db, dir.path()).await;

    // needs: one bare issue.
    let needs = seed::seed_series(db, lib, "Needs Meta").await;
    seed::seed_issue(db, lib, needs, &dir.path().join("n.cbz"), b"n", 1.0).await;

    // complete: one fully-populated issue.
    let complete = seed::seed_series(db, lib, "All Good").await;
    let c_iss = seed::seed_issue(db, lib, complete, &dir.path().join("c.cbz"), b"c", 1.0).await;
    make_complete(db, &c_iss).await;

    // partial: one complete + one bare issue.
    let partial = seed::seed_series(db, lib, "Half Done").await;
    let p_iss = seed::seed_issue(db, lib, partial, &dir.path().join("p1.cbz"), b"p", 1.0).await;
    make_complete(db, &p_iss).await;
    seed::seed_issue(db, lib, partial, &dir.path().join("p2.cbz"), b"q", 2.0).await;

    // accepted: one bare issue, operator-acknowledged → counts as satisfied.
    let accepted = seed::seed_series(db, lib, "Acknowledged").await;
    let a_iss = seed::seed_issue(db, lib, accepted, &dir.path().join("a.cbz"), b"a", 1.0).await;
    make_accepted(db, &a_iss).await;

    let slug = |id: Uuid| async move {
        entity::series::Entity::find_by_id(id)
            .one(db)
            .await
            .unwrap()
            .unwrap()
            .slug
    };
    let needs_slug = slug(needs).await;
    let complete_slug = slug(complete).await;
    let partial_slug = slug(partial).await;
    let accepted_slug = slug(accepted).await;

    // needs_metadata → only the bare series; the accepted one is excluded.
    let got = slugs_for(&app, &auth, lib, "needs_metadata").await;
    assert_eq!(got, vec![needs_slug.clone()], "needs_metadata tier");

    // complete → fully-populated AND accepted (the acknowledgement satisfies).
    let mut got = slugs_for(&app, &auth, lib, "complete").await;
    got.sort();
    let mut want = vec![complete_slug.clone(), accepted_slug.clone()];
    want.sort();
    assert_eq!(got, want, "complete tier includes accepted");

    // partial → only the mixed series.
    let got = slugs_for(&app, &auth, lib, "partial").await;
    assert_eq!(got, vec![partial_slug.clone()], "partial tier");
}

#[tokio::test]
async fn invalid_completeness_value_is_422() {
    let app = TestApp::spawn().await;
    let auth = register_admin(&app).await;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/series?metadata_completeness=bogus")
                .header(header::COOKIE, auth.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
