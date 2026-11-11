//! Age-rating filters, filter options (facets) and saved views fold every
//! stored spelling of a rating onto its ladder rung
//! (`library::age_rating::filter_match`, `api::filter_options::age_ratings`).
//!
//! Fixture: one library. Series ratings `Mature 17+`, `Mature` (alias),
//! ` mature 17+ ` (case + spacing), `TEEN`, `Unknown`, `Rating Pending`,
//! `Sixteen` (off the ladder) and NULL. The NULL series holds five issues
//! rated `Adult` (alias), `Adults Only 18+`, ` adults only 18+ `,
//! `Teen Plus` (alias of `MA15+`) and `Unknown`.
//!
//! Unranked text (`Unknown`, `Rating Pending`, off-ladder words) stays its
//! own option and matches as written: that is what every filter and saved
//! view did before, so a view saved on `Unknown` keeps its exact meaning.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{SeriesSeed, seed_issue, seed_library};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement, Value};
use serde_json::json;
use std::collections::BTreeSet;
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
            .find(|c| c.starts_with(prefix))
            .and_then(|c| c.split(';').next())
            .map(|kv| kv.trim_start_matches(prefix).to_owned())
            .expect(prefix)
    };
    Authed {
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
    }
}

async fn http(
    app: &TestApp,
    method: Method,
    uri: &str,
    auth: &Authed,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder()
        .method(method.clone())
        .uri(uri)
        .header(
            header::COOKIE,
            format!(
                "__Host-comic_session={}; __Host-comic_csrf={}",
                auth.session, auth.csrf
            ),
        )
        .header("X-CSRF-Token", &auth.csrf);
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
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

async fn get(app: &TestApp, uri: &str, auth: &Authed) -> serde_json::Value {
    let (st, body) = http(app, Method::GET, uri, auth, None).await;
    assert_eq!(st, StatusCode::OK, "{uri}: {body}");
    body
}

fn strings(body: &serde_json::Value, key: Option<&str>) -> BTreeSet<String> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("no items: {body}"))
        .iter()
        .map(|v| {
            match key {
                Some(k) => &v[k],
                None => v,
            }
            .as_str()
            .unwrap()
            .to_owned()
        })
        .collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

async fn set_rating(db: &DatabaseConnection, table: &str, id: Value, rating: Option<&str>) {
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        format!("UPDATE {table} SET age_rating = $2 WHERE id = $1"),
        [id, Value::from(rating.map(str::to_owned))],
    ))
    .await
    .unwrap();
}

struct World {
    app: TestApp,
    admin: Authed,
    lib: Uuid,
    /// Series name → id.
    series: Vec<(&'static str, Uuid)>,
    /// Issue rating → id.
    issues: Vec<(&'static str, String)>,
}

impl World {
    fn series_ids(&self, names: &[&str]) -> BTreeSet<String> {
        self.series
            .iter()
            .filter(|(n, _)| names.contains(n))
            .map(|(_, id)| id.to_string())
            .collect()
    }

    fn issue_ids(&self, ratings: &[&str]) -> BTreeSet<String> {
        self.issues
            .iter()
            .filter(|(r, _)| ratings.contains(r))
            .map(|(_, id)| id.clone())
            .collect()
    }
}

const SERIES: [(&str, Option<&str>); 8] = [
    ("canon", Some("Mature 17+")),
    ("alias", Some("Mature")),
    ("spaced", Some(" mature 17+ ")),
    ("teen", Some("TEEN")),
    ("unknown", Some("Unknown")),
    ("pending", Some("Rating Pending")),
    ("off", Some("Sixteen")),
    ("unrated", None),
];

const ISSUES: [&str; 5] = [
    "Adult",
    "Adults Only 18+",
    " adults only 18+ ",
    "Teen Plus",
    "Unknown",
];

async fn world() -> World {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@spell.test").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let root = app._data_dir.path().join("spell-lib");
    std::fs::create_dir_all(&root).unwrap();
    let lib = seed_library(&db, &root).await;
    let mut series = Vec::new();
    for (name, rating) in SERIES {
        let id = SeriesSeed::new(lib, name).insert(&db).await;
        set_rating(&db, "series", Value::from(id), rating).await;
        series.push((name, id));
    }
    let holder = series.iter().find(|(n, _)| *n == "unrated").unwrap().1;
    let mut issues = Vec::new();
    for (n, rating) in ISSUES.into_iter().enumerate() {
        let path = root.join(format!("issue-{n}.cbz"));
        let payload = format!("\u{89}PNG payload {n}");
        let id = seed_issue(&db, lib, holder, &path, payload.as_bytes(), n as f64 + 1.0).await;
        set_rating(&db, "issues", Value::from(id.clone()), Some(rating)).await;
        issues.push((rating, id));
    }
    World {
        app,
        admin,
        lib,
        series,
        issues,
    }
}

// ───── facets ─────

#[tokio::test]
async fn series_facet_lists_one_option_per_rung_and_unranked_text_as_written() {
    let w = world().await;
    let body = get(
        &w.app,
        &format!("/api/filter-options/age_ratings?library={}", w.lib),
        &w.admin,
    )
    .await;
    let got: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        got.iter().cloned().collect::<BTreeSet<_>>(),
        set(&["Mature 17+", "Teen", "Unknown", "Rating Pending", "Sixteen"]),
    );
    // One entry each: the three Mature spellings fold into one option.
    assert_eq!(got.len(), 5, "{got:?}");

    // A prefix finds the rung by its label or by a stored spelling.
    let body = get(
        &w.app,
        &format!("/api/filter-options/age_ratings?library={}&q=mat", w.lib),
        &w.admin,
    )
    .await;
    assert_eq!(strings(&body, None), set(&["Mature 17+"]));
}

#[tokio::test]
async fn issue_facet_lists_the_issues_own_ratings_folded() {
    let w = world().await;
    let body = get(
        &w.app,
        &format!(
            "/api/filter-options/age_ratings?library={}&entity=issues",
            w.lib
        ),
        &w.admin,
    )
    .await;
    assert_eq!(
        strings(&body, None),
        set(&["Adults Only 18+", "MA15+", "Unknown"])
    );
    assert_eq!(body["items"].as_array().unwrap().len(), 3, "{body}");
    // `teen` is a prefix of no label here, only of the stored alias
    // `Teen Plus`, which brings its rung.
    let body = get(
        &w.app,
        &format!(
            "/api/filter-options/age_ratings?library={}&entity=issues&q=teen",
            w.lib
        ),
        &w.admin,
    )
    .await;
    assert_eq!(strings(&body, None), set(&["MA15+"]));
}

#[tokio::test]
async fn facet_pages_walk_with_the_cursor() {
    let w = world().await;
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut uri = format!("/api/filter-options/age_ratings?library={}&limit=2", w.lib);
        if let Some(c) = &cursor {
            uri.push_str(&format!("&cursor={c}"));
        }
        let body = get(&w.app, &uri, &w.admin).await;
        for v in body["items"].as_array().unwrap() {
            seen.push(v.as_str().unwrap().to_owned());
        }
        match body["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    assert_eq!(seen.len(), 5, "{seen:?}");
    assert_eq!(
        seen.iter().cloned().collect::<BTreeSet<_>>().len(),
        5,
        "{seen:?}"
    );
}

// ───── list filters ─────

#[tokio::test]
async fn series_filter_by_rung_or_alias_matches_every_spelling() {
    let w = world().await;
    let mature = w.series_ids(&["canon", "alias", "spaced"]);
    for value in ["Mature%2017%2B", "Mature", "MATURE", "mature%20"] {
        let body = get(
            &w.app,
            &format!("/api/series?library={}&age_rating={value}", w.lib),
            &w.admin,
        )
        .await;
        assert_eq!(strings(&body, Some("id")), mature, "{value}");
    }
    let body = get(
        &w.app,
        &format!("/api/series?library={}&age_rating=Teen,Unknown", w.lib),
        &w.admin,
    )
    .await;
    assert_eq!(
        strings(&body, Some("id")),
        w.series_ids(&["teen", "unknown"])
    );
    // Unranked text matches as written, as before.
    let body = get(
        &w.app,
        &format!("/api/series?library={}&age_rating=unknown", w.lib),
        &w.admin,
    )
    .await;
    assert!(strings(&body, Some("id")).is_empty(), "{body}");
}

#[tokio::test]
async fn issue_filter_by_rung_or_alias_matches_every_spelling() {
    let w = world().await;
    let adult = w.issue_ids(&["Adult", "Adults Only 18+", " adults only 18+ "]);
    for value in ["Adult", "Adults%20Only%2018%2B", "adults%20only%2018%2B"] {
        let body = get(
            &w.app,
            &format!("/api/issues?library={}&age_rating={value}", w.lib),
            &w.admin,
        )
        .await;
        assert_eq!(strings(&body, Some("id")), adult, "{value}");
    }
    let body = get(
        &w.app,
        &format!("/api/issues?library={}&age_rating=MA15%2B", w.lib),
        &w.admin,
    )
    .await;
    assert_eq!(strings(&body, Some("id")), w.issue_ids(&["Teen Plus"]));
}

// ───── saved views ─────

async fn view_results(
    w: &World,
    kind: &str,
    op: &str,
    value: serde_json::Value,
) -> BTreeSet<String> {
    let (st, view) = http(
        &w.app,
        Method::POST,
        "/api/me/saved-views",
        &w.admin,
        Some(json!({
            "kind": kind,
            "name": format!("rating {op} {value}"),
            "filter": {
                "match_mode": "all",
                "conditions": [
                    { "group_id": 0, "field": "age_rating", "op": op, "value": value }
                ]
            },
            "sort_field": "name",
            "sort_order": "asc",
            "result_limit": 50,
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{view}");
    // Stored as written: the compiler, not the store, folds the spelling.
    assert_eq!(view["conditions"][0]["value"], value, "{view}");
    let id = view["id"].as_str().unwrap();
    let path = if kind == "filter_issues" {
        "issue-results"
    } else {
        "results"
    };
    let body = get(
        &w.app,
        &format!("/api/me/saved-views/{id}/{path}"),
        &w.admin,
    )
    .await;
    strings(&body, Some("id"))
}

#[tokio::test]
async fn a_saved_view_on_an_alias_matches_every_spelling_of_its_rung() {
    let w = world().await;
    let mature = w.series_ids(&["canon", "alias", "spaced"]);
    assert_eq!(
        view_results(&w, "filter_series", "is", json!("Mature")).await,
        mature
    );
    assert_eq!(
        view_results(&w, "filter_series", "in", json!([" mature 17+"])).await,
        mature
    );
    // The negation keeps excluding the unrated (NULL) series, as `<>` did.
    assert_eq!(
        view_results(&w, "filter_series", "is_not", json!("Mature")).await,
        w.series_ids(&["teen", "unknown", "pending", "off"])
    );
    assert_eq!(
        view_results(
            &w,
            "filter_series",
            "not_in",
            json!(["Mature 17+", "Unknown"])
        )
        .await,
        w.series_ids(&["teen", "pending", "off"])
    );
    assert_eq!(
        view_results(&w, "filter_issues", "in", json!(["Explicit", "Adult"])).await,
        w.issue_ids(&["Adult", "Adults Only 18+", " adults only 18+ "])
    );
}

#[tokio::test]
async fn a_saved_view_on_unranked_text_keeps_matching_it_as_written() {
    let w = world().await;
    assert_eq!(
        view_results(&w, "filter_series", "is", json!("Unknown")).await,
        w.series_ids(&["unknown"])
    );
    assert_eq!(
        view_results(
            &w,
            "filter_series",
            "in",
            json!(["Rating Pending", "Sixteen"])
        )
        .await,
        w.series_ids(&["pending", "off"])
    );
    assert!(
        view_results(&w, "filter_series", "is", json!("unknown"))
            .await
            .is_empty()
    );
    assert_eq!(
        view_results(&w, "filter_issues", "is", json!("Unknown")).await,
        w.issue_ids(&["Unknown"])
    );
}
