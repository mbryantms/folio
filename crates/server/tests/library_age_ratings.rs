//! `GET /admin/libraries/age-ratings` (audit U-09): per-library series
//! counts at each age-rating rung, for the age-cap control.
//!
//! Covers: counts per rung and unrated, aliases and odd spellings counted
//! on their rung, removed series left out, several libraries in one
//! response (an empty one with zeros), non-admins get 403, and — so the
//! summary cannot drift from the real rule — the number hidden for every
//! cap, with and without `hide_unrated`, equals what `GET /api/series`
//! leaves out for a reader holding that grant. Also pins the web copy of
//! the ladder (`web/lib/age-rating.ts`) to the server's.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{LibrarySeed, SeriesSeed};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use server::library::age_rating::LADDER;
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
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
            .find_map(|c| c.split(';').next()?.strip_prefix(prefix).map(str::to_owned))
            .expect("cookie")
    };
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn call(
    app: &TestApp,
    user: &Authed,
    method: Method,
    path: &str,
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
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = app
        .router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

async fn exec(db: &sea_orm::DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}

/// A series in `lib` with its stored rating set directly (as a pinned or
/// derived value would be).
async fn rated(
    db: &sea_orm::DatabaseConnection,
    lib: Uuid,
    name: &str,
    rating: Option<&str>,
) -> Uuid {
    let id = SeriesSeed::new(lib, name).insert(db).await;
    exec(
        db,
        "UPDATE series SET age_rating = $1 WHERE id = $2",
        vec![rating.map(str::to_owned).into(), id.into()],
    )
    .await;
    id
}

fn row_for(rows: &serde_json::Value, id: Uuid) -> &serde_json::Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r["library_id"] == id.to_string())
        .unwrap_or_else(|| panic!("no row for {id}: {rows}"))
}

/// `rating -> series_count` in ladder order, as the response lists it.
fn rungs(row: &serde_json::Value) -> Vec<(String, u64)> {
    row["rungs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["rating"].as_str().unwrap().to_owned(),
                r["series_count"].as_u64().unwrap(),
            )
        })
        .collect()
}

/// What the web computes: series a grant with `cap` / `hide_unrated`
/// hides, from the histogram alone.
fn hidden_by(row: &serde_json::Value, cap: Option<&str>, hide_unrated: bool) -> u64 {
    let above: u64 = match cap {
        Some(cap) => {
            let at = LADDER.iter().position(|r| *r == cap).unwrap();
            rungs(row)[at + 1..].iter().map(|(_, n)| n).sum()
        }
        None => 0,
    };
    let unrated = row["unrated_count"].as_u64().unwrap();
    above + if hide_unrated { unrated } else { 0 }
}

#[tokio::test]
async fn age_rating_histogram_matches_what_a_capped_reader_sees() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "ages-admin@example.com").await;
    let reader = register(&app, "ages-reader@example.com").await;
    let db = app.state().db.clone();
    let reader_id: Uuid = {
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        entity::user::Entity::find()
            .filter(entity::user::Column::Email.eq("ages-reader@example.com"))
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .id
    };

    // ── Library A: every kind of rating ──────────────────────────────
    let dir_a = tempfile::tempdir().unwrap();
    let lib_a = LibrarySeed::new(dir_a.path()).insert(&db).await;
    rated(&db, lib_a, "Small Steps", Some("Everyone")).await;
    rated(&db, lib_a, "Teen One", Some("Teen")).await;
    // Case and surrounding whitespace are normalised like the cap does.
    rated(&db, lib_a, "Teen Two", Some("  teen\t")).await;
    // Metron aliases count on their rungs.
    rated(&db, lib_a, "Plus Run", Some("Teen Plus")).await; // MA15+
    rated(&db, lib_a, "Mature Run", Some("Mature")).await; // Mature 17+
    rated(&db, lib_a, "Mature Canon", Some("Mature 17+")).await;
    rated(&db, lib_a, "Explicit Run", Some("Explicit")).await; // X18+
    // Unrated: none, the "no rating" words, an off-ladder word.
    rated(&db, lib_a, "Bare", None).await;
    rated(&db, lib_a, "Unknown Run", Some("Unknown")).await;
    rated(&db, lib_a, "Pending Run", Some("Rating Pending")).await;
    rated(&db, lib_a, "Odd Run", Some("Sixteen and up")).await;
    // A removed series is in no figure.
    let gone = rated(&db, lib_a, "Gone Run", Some("X18+")).await;
    exec(
        &db,
        "UPDATE series SET removed_at = now() WHERE id = $1",
        vec![gone.into()],
    )
    .await;

    // ── Library B: two series; library C: none ───────────────────────
    let dir_b = tempfile::tempdir().unwrap();
    let lib_b = LibrarySeed::new(dir_b.path()).insert(&db).await;
    rated(&db, lib_b, "Kid Run", Some("Early Childhood")).await;
    rated(&db, lib_b, "Grown Run", Some("Adults Only 18+")).await;
    let dir_c = tempfile::tempdir().unwrap();
    let lib_c = LibrarySeed::new(dir_c.path()).insert(&db).await;

    // ── Access ───────────────────────────────────────────────────────
    let (st, _) = call(
        &app,
        &reader,
        Method::GET,
        "/api/admin/libraries/age-ratings",
        None,
    )
    .await;
    // The second account is a plain user.
    assert_eq!(st, StatusCode::FORBIDDEN);

    let (st, rows) = call(
        &app,
        &admin,
        Method::GET,
        "/api/admin/libraries/age-ratings",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{rows}");
    assert_eq!(rows.as_array().unwrap().len(), 3);

    // ── Library A figures ────────────────────────────────────────────
    let a = row_for(&rows, lib_a);
    let want_a: Vec<(String, u64)> = LADDER
        .iter()
        .map(|r| {
            let n = match *r {
                "Everyone" => 1,
                "Teen" => 2,
                "MA15+" => 1,
                "Mature 17+" => 2,
                "X18+" => 1,
                _ => 0,
            };
            ((*r).to_owned(), n)
        })
        .collect();
    assert_eq!(rungs(a), want_a);
    assert_eq!(a["unrated_count"], 4);
    assert_eq!(a["series_count"], 11);

    let b = row_for(&rows, lib_b);
    assert_eq!(rungs(b)[0], ("Early Childhood".to_owned(), 1));
    assert_eq!(rungs(b)[11], ("Adults Only 18+".to_owned(), 1));
    assert_eq!(b["unrated_count"], 0);
    assert_eq!(b["series_count"], 2);

    let c = row_for(&rows, lib_c);
    assert!(rungs(c).iter().all(|(_, n)| *n == 0));
    assert_eq!(rungs(c).len(), LADDER.len());
    assert_eq!(c["unrated_count"], 0);
    assert_eq!(c["series_count"], 0);

    // ── The summary agrees with the real rule ────────────────────────
    // For every cap (and none), with and without hide_unrated, a reader
    // holding that grant on library A is shown exactly the series the
    // histogram says are not hidden.
    let caps: Vec<Option<&str>> = std::iter::once(None)
        .chain(LADDER.iter().copied().map(Some))
        .collect();
    let user_path = format!("/api/admin/users/{reader_id}/library-access");
    let list_path = format!("/api/series?library={lib_a}&limit=100");
    for cap in caps {
        for hide in [false, true] {
            let mut caps_body = serde_json::Map::new();
            if let Some(cap) = cap {
                caps_body.insert(lib_a.to_string(), cap.into());
            }
            let mut hide_body = serde_json::Map::new();
            if hide {
                hide_body.insert(lib_a.to_string(), true.into());
            }
            let (st, body) = call(
                &app,
                &admin,
                Method::POST,
                &user_path,
                Some(serde_json::json!({
                    "library_ids": [lib_a.to_string()],
                    "age_rating_caps": caps_body,
                    "hide_unrated": hide_body,
                })),
            )
            .await;
            assert_eq!(st, StatusCode::OK, "{body}");

            let (st, list) = call(&app, &reader, Method::GET, &list_path, None).await;
            assert_eq!(st, StatusCode::OK, "{list}");
            let shown = list["items"].as_array().unwrap().len() as u64;
            assert_eq!(list["total"].as_u64(), Some(shown));
            assert_eq!(
                shown,
                11 - hidden_by(a, cap, hide),
                "cap {cap:?}, hide_unrated {hide}: {list}"
            );
        }
    }
}

/// `web/lib/age-rating.ts` drives the admin picker and the hidden-count
/// arithmetic; its ladder must be the server's, in the same order.
#[test]
fn web_ladder_matches_the_server_ladder() {
    let src = include_str!("../../../web/lib/age-rating.ts");
    let start = src
        .find("AGE_RATING_LADDER = [")
        .expect("AGE_RATING_LADDER in web/lib/age-rating.ts");
    let body = &src[start..];
    let body = &body[..body.find(']').expect("end of the ladder")];
    let web: Vec<&str> = body.split('"').skip(1).step_by(2).collect();
    assert_eq!(web, LADDER.to_vec());
}
