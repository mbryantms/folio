//! Provider-independent series coverage (`metadata::coverage`): every
//! provider's candidate series are listed with cover dates, local issues
//! are assigned by number + date, and a greedy set cover proposes the main
//! series + ranges per provider. All provider bodies are synthetic and
//! served by wiremock; no real provider is called.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, SeriesSeed, seed_library};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use server::metadata::identifier::{Identifier, Source};
use server::metadata::writers::{SetBy, set_external_id};
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

// ───────── synthetic provider data ─────────

/// `(number, "YYYY-MM-DD")` rows.
type Issues = Vec<(String, String)>;

fn monthly(numbers: impl IntoIterator<Item = u32>, start_year: i32, start_month: u32) -> Issues {
    numbers
        .into_iter()
        .enumerate()
        .map(|(i, n)| {
            let m0 = start_month - 1 + i as u32;
            let y = start_year + (m0 / 12) as i32;
            let m = m0 % 12 + 1;
            (n.to_string(), format!("{y}-{m:02}-01"))
        })
        .collect()
}

/// Daredevil (1964): #1–5 in 1964, legacy #500–512 from Oct 2009.
fn dd_1964() -> Issues {
    let mut v = monthly(1..=5, 1964, 4);
    v.extend(monthly(500..=512, 2009, 10));
    v
}

/// Daredevil (1998): #1–8 from Nov 1998.
fn dd_1998() -> Issues {
    monthly(1..=8, 1998, 11)
}

fn cv_env(results: Value, total: usize) -> Value {
    json!({
        "status_code": 1, "error": "OK",
        "number_of_total_results": total,
        "results": results,
    })
}

fn cv_issue_rows(vol: u32, name: &str, issues: &[(String, String)], id_base: u32) -> Value {
    Value::Array(
        issues
            .iter()
            .enumerate()
            .map(|(i, (n, d))| {
                json!({
                    "id": id_base + i as u32,
                    "issue_number": n,
                    "cover_date": d,
                    "volume": {"id": vol, "name": name},
                })
            })
            .collect(),
    )
}

async fn mount_cv_volume(cv: &MockServer, vol: u32, issues: &Issues, id_base: u32, expect: u64) {
    Mock::given(method("GET"))
        .and(path("/issues/"))
        .and(query_param("filter", format!("volume:{vol}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(cv_env(
            cv_issue_rows(vol, "Daredevil", issues, id_base),
            issues.len(),
        )))
        .expect(expect)
        .mount(cv)
        .await;
}

async fn mount_cv_daredevil(cv: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/volumes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(cv_env(
            json!([
                {"id": 2190, "name": "Daredevil", "start_year": "1964",
                 "publisher": {"id": 31, "name": "Marvel"}, "count_of_issues": 18},
                {"id": 6458, "name": "Daredevil", "start_year": "1998",
                 "publisher": {"id": 31, "name": "Marvel"}, "count_of_issues": 8},
            ]),
            2,
        )))
        .mount(cv)
        .await;
    // Each volume is listed once across every analysis: the issue list is
    // cached for 24 h.
    mount_cv_volume(cv, 2190, &dd_1964(), 10_000, 1).await;
    mount_cv_volume(cv, 6458, &dd_1998(), 20_000, 1).await;
}

fn paged(results: Value) -> Value {
    json!({
        "count": results.as_array().map(|a| a.len()).unwrap_or(0),
        "next": null, "previous": null, "results": results,
    })
}

async fn mount_metron_daredevil(metron: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/series/"))
        .and(query_param("name", "Daredevil"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paged(json!([
            {"id": 100, "series": "Daredevil (1964)", "year_began": 1964, "issue_count": 18},
            {"id": 200, "series": "Daredevil (1998)", "year_began": 1998, "issue_count": 8},
        ]))))
        .mount(metron)
        .await;
    for (sid, issues, base) in [(100u32, dd_1964(), 30_000u32), (200, dd_1998(), 40_000)] {
        let rows: Vec<Value> = issues
            .iter()
            .enumerate()
            .map(|(i, (n, d))| json!({"id": base + i as u32, "number": n, "cover_date": d}))
            .collect();
        Mock::given(method("GET"))
            .and(path("/api/issue/"))
            .and(query_param("series_id", sid.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(paged(Value::Array(rows))))
            .expect(1)
            .mount(metron)
            .await;
    }
}

fn gcd_series(id: u32, year: i32, issues: &Issues, base: u32) -> Value {
    json!({
        "api_url": format!("https://www.comics.org/api/series/{id}/?format=json"),
        "name": "Daredevil",
        "year_began": year,
        "publishing_format": "ongoing series",
        "active_issues": issues.iter().enumerate()
            .map(|(i, _)| format!("https://www.comics.org/api/issue/{}/?format=json", base + i as u32))
            .collect::<Vec<_>>(),
        "issue_descriptors": issues.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
    })
}

fn gcd_overview(issues: &Issues, base: u32) -> Value {
    paged(Value::Array(
        issues
            .iter()
            .enumerate()
            .map(|(i, (n, d))| {
                json!({
                    "issue_id": base + i as u32,
                    "descriptor": n,
                    "number": n,
                    "key_date": d,
                    "on_sale_date": d,
                })
            })
            .collect(),
    ))
}

async fn mount_gcd_daredevil(gcd: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/series/name/Daredevil/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(paged(json!([
            gcd_series(3000, 1964, &dd_1964(), 50_000),
            gcd_series(4000, 1998, &dd_1998(), 60_000),
        ]))))
        .mount(gcd)
        .await;
    for (sid, issues, base) in [(3000u32, dd_1964(), 50_000u32), (4000, dd_1998(), 60_000)] {
        Mock::given(method("GET"))
            .and(path(format!("/api/series/{sid}/overview/")))
            .respond_with(ResponseTemplate::new(200).set_body_json(gcd_overview(&issues, base)))
            .expect(1)
            .mount(gcd)
            .await;
    }
}

// ───────── local library ─────────

/// "Daredevil" (1998, Marvel) holding 1998 #1–5 and legacy #500–502,
/// with ComicInfo cover dates. `extra` adds more `(number, year, month)`.
async fn seed_daredevil(
    app: &TestApp,
    extra: &[(f64, i32, i32)],
) -> (Uuid, String, tempfile::TempDir) {
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let mut seed = SeriesSeed::new(lib, "Daredevil");
    seed.year = Some(1998);
    seed.publisher = Some("Marvel".into());
    let series_id = seed.insert(&db).await;
    let mut issues: Vec<(f64, i32, i32)> = vec![
        (1.0, 1998, 11),
        (2.0, 1998, 12),
        (3.0, 1999, 1),
        (4.0, 1999, 2),
        (5.0, 1999, 3),
        (500.0, 2009, 10),
        (501.0, 2009, 11),
        (502.0, 2009, 12),
    ];
    issues.extend_from_slice(extra);
    seed_issues(app, lib, series_id, tmp.path(), &issues).await;
    (series_id, slug_of(app, series_id).await, tmp)
}

async fn seed_issues(
    app: &TestApp,
    lib: Uuid,
    series_id: Uuid,
    dir: &std::path::Path,
    issues: &[(f64, i32, i32)],
) {
    let db = app.state().db.clone();
    for (n, y, m) in issues {
        let p = dir.join(format!("{series_id}-{n}.cbz"));
        let id = IssueSeed::new(
            lib,
            series_id,
            &p,
            format!("{series_id} {n}").as_bytes(),
            *n,
        )
        .insert(&db)
        .await;
        db.execute_unprepared(&format!(
            "UPDATE issues SET year = {y}, month = {m} WHERE id = '{id}'"
        ))
        .await
        .unwrap();
    }
}

async fn slug_of(app: &TestApp, series_id: Uuid) -> String {
    entity::series::Entity::find_by_id(series_id)
        .one(&app.state().db)
        .await
        .unwrap()
        .unwrap()
        .slug
}

// ───────── HTTP helpers ─────────

async fn register_admin(app: &TestApp) -> String {
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"admin@example.com","password":"correctly-horse-battery"}"#,
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

async fn call(
    app: &TestApp,
    cookie: &str,
    m: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let csrf = cookie
        .split("; ")
        .find_map(|c| c.strip_prefix("__Host-comic_csrf="))
        .unwrap()
        .to_owned();
    let mut b = Request::builder()
        .method(m)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .header("X-CSRF-Token", csrf);
    let body = match body {
        Some(v) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app
        .router
        .clone()
        .oneshot(b.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Queue an analysis, run the job inline, and return the finished grid.
async fn analyze(app: &TestApp, cookie: &str, slug: &str, auto_accept: bool) -> Value {
    let (st, job) = call(
        app,
        cookie,
        Method::POST,
        &format!("/api/series/{slug}/provider-coverage/analyze"),
        Some(json!({"auto_accept": auto_accept})),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{job}");
    assert_eq!(job["queued"], true, "{job}");
    let job_id = Uuid::parse_str(job["job_id"].as_str().unwrap()).unwrap();

    // While queued the grid is empty.
    let (st, pending) = call(
        app,
        cookie,
        Method::GET,
        &format!("/api/series/{slug}/provider-coverage/analysis"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(pending["state"], "queued");
    assert_eq!(pending["providers"], json!([]));

    server::jobs::provider_coverage::process(&app.state(), job_id)
        .await
        .unwrap();
    analysis(app, cookie, slug).await
}

async fn analysis(app: &TestApp, cookie: &str, slug: &str) -> Value {
    let (st, body) = call(
        app,
        cookie,
        Method::GET,
        &format!("/api/series/{slug}/provider-coverage/analysis"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["state"], "done", "{body}");
    body
}

async fn accept(app: &TestApp, cookie: &str, slug: &str, body: Value) -> Value {
    let (st, out) = call(
        app,
        cookie,
        Method::POST,
        &format!("/api/series/{slug}/provider-coverage/accept"),
        Some(body),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    out
}

fn provider<'a>(body: &'a Value, src: &str) -> &'a Value {
    body["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["source"] == src)
        .unwrap_or_else(|| panic!("no {src} in {body}"))
}

fn cell<'a>(p: &'a Value, number: &str) -> &'a Value {
    p["cells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["number"] == number)
        .unwrap()
}

async fn ranges(app: &TestApp, series_id: Uuid) -> Vec<entity::series_provider_range::Model> {
    let mut r = entity::series_provider_range::Entity::find()
        .filter(entity::series_provider_range::Column::SeriesId.eq(series_id))
        .all(&app.state().db)
        .await
        .unwrap();
    r.sort_by(|a, b| (&a.source, &a.range_low).cmp(&(&b.source, &b.range_low)));
    r
}

async fn series_ext(
    app: &TestApp,
    series_id: Uuid,
    src: &str,
) -> Option<entity::external_id::Model> {
    entity::external_id::Entity::find()
        .filter(entity::external_id::Column::EntityType.eq("series"))
        .filter(entity::external_id::Column::EntityId.eq(series_id.to_string()))
        .filter(entity::external_id::Column::Source.eq(src))
        .one(&app.state().db)
        .await
        .unwrap()
}

async fn audit_count(app: &TestApp, action: &str) -> usize {
    entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq(action))
        .all(&app.state().db)
        .await
        .unwrap()
        .len()
}

async fn daredevil_app() -> (TestApp, MockServer, MockServer, MockServer) {
    let cv = MockServer::start().await;
    let metron = MockServer::start().await;
    let gcd = MockServer::start().await;
    mount_cv_daredevil(&cv).await;
    mount_metron_daredevil(&metron).await;
    mount_gcd_daredevil(&gcd).await;
    let app = TestApp::spawn_with_all_providers(cv.uri(), metron.uri(), gcd.uri()).await;
    (app, cv, metron, gcd)
}

// ───────── tests ─────────

/// The canonical case: a "Daredevil" folder holding the 1998 volume's
/// #1–5 plus the legacy #500–502. Every provider lists two series; the
/// 1964 volume also lists #1–5, but its 1964 cover dates disagree with
/// the local 1998 dates, so #1–5 go to the 1998 volume (main) and only
/// #500–502 become a range onto the 1964 volume. Without the date check
/// the 1964 volume would cover all eight issues and win.
#[tokio::test]
async fn mixed_folder_assigns_by_number_and_cover_date() {
    let (app, cv, _metron, _gcd) = daredevil_app().await;
    let (series_id, slug, _tmp) = seed_daredevil(&app, &[]).await;
    let cookie = register_admin(&app).await;

    let body = analyze(&app, &cookie, &slug, false).await;
    assert_eq!(body["local_issues"].as_array().unwrap().len(), 8);
    for (src, main, alt) in [
        ("comicvine", "6458", "2190"),
        ("metron", "200", "100"),
        ("gcd", "4000", "3000"),
    ] {
        let p = provider(&body, src);
        assert_eq!(p["status"], "analyzed", "{src}: {p}");
        assert_eq!(p["main_series_id"], main, "{src}: {p}");
        assert_eq!(p["confidence"], "high", "{src}: {p}");
        assert_eq!(p["uncovered"], json!([]), "{src}");
        let ranges = p["proposed_ranges"].as_array().unwrap();
        assert_eq!(ranges.len(), 1, "{src}: {p}");
        assert_eq!(ranges[0]["provider_series_id"], alt);
        assert_eq!(
            (ranges[0]["low"].as_str(), ranges[0]["high"].as_str()),
            (Some("500"), Some("502"))
        );
        assert_eq!(ranges[0]["status"], "new");
        assert_eq!(ranges[0]["declared_year"], 1964, "{src}: {p}");
        // #1 exists in both volumes; the date picks 1998.
        assert_eq!(cell(p, "1")["provider_series_id"], main);
        assert_eq!(cell(p, "1")["date_match"], "confirmed");
        assert_eq!(cell(p, "501")["provider_series_id"], alt);
        assert!(p["requests"].as_u64().unwrap() <= p["request_budget"].as_u64().unwrap());
        assert!(p["auto_acceptable"].as_bool().unwrap());
        // Names are known for every candidate (GCD included).
        for c in p["candidates"].as_array().unwrap() {
            assert_eq!(c["name"], "Daredevil", "{src}: {c}");
        }
    }
    // The provider issue id rides along for a later direct lookup.
    let c = provider(&body, "comicvine");
    assert_eq!(cell(c, "1")["provider_issue_id"], "20000");
    assert_eq!(cell(c, "500")["provider_issue_id"], "10005");
    // ComicVine spent exactly what it reports: 1 search + 2 listings.
    assert_eq!(c["requests"], 3);
    assert_eq!(cv.received_requests().await.unwrap().len(), 3);
    // Nothing is written by an analysis.
    assert!(ranges(&app, series_id).await.is_empty());
    assert!(series_ext(&app, series_id, "comicvine").await.is_none());
    assert_eq!(
        audit_count(&app, "admin.series.provider_coverage_analyze").await,
        1
    );

    // Accept every provider: main ids as user-confirmed, one range each.
    for src in ["comicvine", "metron", "gcd"] {
        let out = accept(&app, &cookie, &slug, json!({"source": src})).await;
        assert_eq!(out["main_written"], true, "{out}");
        assert_eq!(out["ranges_created"].as_array().unwrap().len(), 1, "{out}");
    }
    assert_eq!(
        audit_count(&app, "admin.series.provider_coverage_accept").await,
        3
    );
    let ext = series_ext(&app, series_id, "gcd").await.unwrap();
    assert_eq!(
        (ext.external_id.as_str(), ext.set_by.as_str()),
        ("4000", "user")
    );
    let rows = ranges(&app, series_id).await;
    let got: Vec<(String, String, Option<String>, Option<String>)> = rows
        .iter()
        .map(|r| {
            (
                r.source.clone(),
                r.provider_series_id.clone(),
                r.range_low.clone(),
                r.range_high.clone(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (
                "comicvine".into(),
                "2190".into(),
                Some("500".into()),
                Some("502".into())
            ),
            (
                "gcd".into(),
                "3000".into(),
                Some("500".into()),
                Some("502".into())
            ),
            (
                "metron".into(),
                "100".into(),
                Some("500".into()),
                Some("502".into())
            ),
        ]
    );

    // range_map::fold_targets consumers see it: the coverage card splits
    // each provider into the main run + the 500–502 range, with names.
    let (st, card) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/series/{slug}/provider-coverage"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let gcd_card = card["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["source"] == "gcd")
        .unwrap();
    let segs = gcd_card["segments"].as_array().unwrap();
    assert_eq!(segs.len(), 2, "{gcd_card}");
    assert_eq!(
        segs[0]["provider_series_name"], "Daredevil",
        "default segment named via the issue-list cache"
    );
    assert_eq!(segs[1]["via_range"], true);

    // The re-read grid reflects the written rows.
    let after = analysis(&app, &cookie, &slug).await;
    for src in ["comicvine", "metron", "gcd"] {
        let p = provider(&after, src);
        assert_eq!(p["proposed_ranges"][0]["status"], "already_mapped", "{p}");
        assert_eq!(p["has_changes"], false, "{p}");
    }
}

/// Re-running is idempotent: the same proposal, listings served from the
/// 24 h cache (each volume's listing mock expects exactly one call), and a
/// second accept writes nothing new.
#[tokio::test]
async fn rerun_is_idempotent_and_uses_the_issue_list_cache() {
    let (app, cv, _metron, _gcd) = daredevil_app().await;
    let (series_id, slug, _tmp) = seed_daredevil(&app, &[]).await;
    let cookie = register_admin(&app).await;

    let first = analyze(&app, &cookie, &slug, true).await;
    // High confidence + no conflicts ⇒ accepted automatically, as
    // provider-set rows.
    assert_eq!(
        first["auto_accepted"].as_array().unwrap().len(),
        3,
        "{first}"
    );
    assert_eq!(
        series_ext(&app, series_id, "comicvine")
            .await
            .unwrap()
            .set_by,
        "comicvine"
    );
    assert_eq!(ranges(&app, series_id).await.len(), 3);
    assert_eq!(
        audit_count(&app, "admin.series.provider_coverage_accept").await,
        3
    );

    let second = analyze(&app, &cookie, &slug, true).await;
    assert_eq!(second["auto_accepted"], json!([]), "nothing left to accept");
    for src in ["comicvine", "metron", "gcd"] {
        let (a, b) = (provider(&first, src), provider(&second, src));
        assert_eq!(a["main_series_id"], b["main_series_id"]);
        assert_eq!(a["cells"], b["cells"], "{src}");
        assert_eq!(b["has_changes"], false);
    }
    // Second ComicVine run: one search, both listings from cache.
    assert_eq!(provider(&second, "comicvine")["requests"], 1);
    assert_eq!(cv.received_requests().await.unwrap().len(), 4);

    let out = accept(&app, &cookie, &slug, json!({"source": "metron"})).await;
    assert_eq!(out["ranges_created"], json!([]));
    assert_eq!(out["ranges_skipped"][0]["status"], "already_mapped");
    assert_eq!(ranges(&app, series_id).await.len(), 3);
}

/// User-set data is never overwritten: a user ComicVine link to the 1964
/// volume becomes the proposal's main (the 1998 issues become the range),
/// and a user Metron range blocks the overlapping proposed range.
#[tokio::test]
async fn user_set_ids_and_ranges_are_untouched() {
    let (app, _cv, _metron, _gcd) = daredevil_app().await;
    let (series_id, slug, _tmp) = seed_daredevil(&app, &[]).await;
    let db = app.state().db.clone();
    set_external_id(
        &db,
        "series",
        &series_id.to_string(),
        &Identifier::with_canonical_url(Source::ComicVine, "2190", "series"),
        SetBy::User,
    )
    .await
    .unwrap();
    db.execute_unprepared(&format!(
        "INSERT INTO series_provider_range (id, series_id, source, provider_series_id, range_low, range_high, set_by, first_set_at, last_synced_at) \
         VALUES ('{}', '{series_id}', 'metron', '999', '500', '502', 'user', now(), now())",
        Uuid::new_v4()
    ))
    .await
    .unwrap();
    let cookie = register_admin(&app).await;

    let body = analyze(&app, &cookie, &slug, true).await;
    let c = provider(&body, "comicvine");
    assert_eq!(
        c["main_series_id"], "2190",
        "built around the user's link: {c}"
    );
    assert_eq!(c["proposed_ranges"][0]["provider_series_id"], "6458");
    assert_eq!(c["proposed_ranges"][0]["low"], "1");
    assert_eq!(c["proposed_ranges"][0]["high"], "5");
    let m = provider(&body, "metron");
    assert_eq!(m["proposed_ranges"][0]["status"], "conflict", "{m}");
    assert!(!m["conflicts"].as_array().unwrap().is_empty());
    assert_eq!(m["auto_acceptable"], false);
    // Auto-accept skipped Metron (conflict) but took the clean providers.
    let auto: Vec<&str> = body["auto_accepted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["source"].as_str().unwrap())
        .collect();
    assert!(!auto.contains(&"metron"), "{auto:?}");

    // Choosing another ComicVine main can't replace the user's link.
    let out = accept(
        &app,
        &cookie,
        &slug,
        json!({"source": "comicvine", "main_series_id": "6458"}),
    )
    .await;
    assert_eq!(out["main_written"], false, "{out}");
    let ext = series_ext(&app, series_id, "comicvine").await.unwrap();
    assert_eq!(
        (ext.external_id.as_str(), ext.set_by.as_str()),
        ("2190", "user")
    );

    // Accepting Metron writes its main id but never the conflicting range.
    let out = accept(&app, &cookie, &slug, json!({"source": "metron"})).await;
    assert_eq!(out["main_written"], true);
    assert_eq!(out["ranges_created"], json!([]));
    assert_eq!(out["ranges_skipped"][0]["status"], "conflict");
    let user_rows: Vec<_> = ranges(&app, series_id)
        .await
        .into_iter()
        .filter(|r| r.source == "metron")
        .collect();
    assert_eq!(user_rows.len(), 1);
    assert_eq!(
        (
            user_rows[0].provider_series_id.as_str(),
            user_rows[0].set_by.as_str()
        ),
        ("999", "user")
    );

    // A non-candidate main is rejected.
    let (st, _) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/series/{slug}/provider-coverage/accept"),
        Some(json!({"source": "gcd", "main_series_id": "nope"})),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
}

/// Stale automated ranges are deleted when their provider's coverage is
/// accepted: GCD #500–502 → 7777 (the proposal files those issues in the
/// 1964 volume) and Metron #1–5 → 200 (points at the main series). The
/// deleted GCD row no longer blocks the proposed range, so the same accept
/// writes it. A user range is never deleted, even when it points at the
/// main series. Each removal lands in the accept's audit row.
#[tokio::test]
async fn stale_automated_ranges_are_deleted_on_accept() {
    let (app, _cv, _metron, _gcd) = daredevil_app().await;
    let (series_id, slug, _tmp) = seed_daredevil(&app, &[]).await;
    let db = app.state().db.clone();
    for (src, sid, lo, hi, by) in [
        ("gcd", "7777", "500", "502", "cross_reference"),
        ("metron", "200", "1", "5", "cross_reference"),
        ("comicvine", "6458", "1", "5", "user"),
    ] {
        db.execute_unprepared(&format!(
            "INSERT INTO series_provider_range (id, series_id, source, provider_series_id, provider_series_name, declared_year, range_low, range_high, set_by, first_set_at, last_synced_at) \
             VALUES ('{}', '{series_id}', '{src}', '{sid}', 'Daredevil', 1964, '{lo}', '{hi}', '{by}', now(), now())",
            Uuid::new_v4()
        ))
        .await
        .unwrap();
    }
    let cookie = register_admin(&app).await;
    let body = analyze(&app, &cookie, &slug, false).await;

    let g = provider(&body, "gcd");
    assert_eq!(g["stale_ranges"][0]["provider_series_id"], "7777", "{g}");
    assert_eq!(g["proposed_ranges"][0]["status"], "conflict");
    let m = provider(&body, "metron");
    assert_eq!(m["stale_ranges"][0]["provider_series_id"], "200", "{m}");
    assert_eq!(m["stale_ranges"][0]["reason"], "points at the main series");
    let c = provider(&body, "comicvine");
    assert_eq!(
        c["stale_ranges"],
        json!([]),
        "a user row is never stale: {c}"
    );
    assert!(
        c["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x.as_str().unwrap().contains("your mapping #1–5")),
        "{c}"
    );

    let out = accept(&app, &cookie, &slug, json!({"source": "gcd"})).await;
    assert_eq!(
        out["stale_ranges_removed"][0]["provider_series_id"], "7777",
        "{out}"
    );
    assert_eq!(out["stale_ranges_removed"][0]["declared_year"], 1964);
    assert_eq!(out["stale_ranges"], json!([]));
    assert_eq!(
        out["ranges_created"][0]["provider_series_id"], "3000",
        "the range the stale row blocked is written: {out}"
    );

    let out = accept(&app, &cookie, &slug, json!({"source": "metron"})).await;
    assert_eq!(
        out["stale_ranges_removed"][0]["provider_series_id"], "200",
        "{out}"
    );
    assert_eq!(out["ranges_created"][0]["provider_series_id"], "100");

    let out = accept(&app, &cookie, &slug, json!({"source": "comicvine"})).await;
    assert_eq!(out["main_written"], true, "{out}");
    assert_eq!(out["stale_ranges_removed"], json!([]), "{out}");

    let rows: Vec<(String, String, String)> = ranges(&app, series_id)
        .await
        .into_iter()
        .map(|r| (r.source, r.provider_series_id, r.set_by))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("comicvine".into(), "6458".into(), "user".into()),
            ("comicvine".into(), "2190".into(), "cross_reference".into()),
            ("gcd".into(), "3000".into(), "cross_reference".into()),
            ("metron".into(), "100".into(), "cross_reference".into()),
        ],
        "user range kept, stale automated rows gone"
    );

    let audits = entity::audit_log::Entity::find()
        .filter(entity::audit_log::Column::Action.eq("admin.series.provider_coverage_accept"))
        .all(&app.state().db)
        .await
        .unwrap();
    let gcd_audit = audits
        .iter()
        .find(|a| a.payload["source"] == "gcd")
        .expect("gcd accept audited");
    assert_eq!(
        gcd_audit.payload["stale_ranges_removed"][0]["provider_series_id"], "7777",
        "{}",
        gcd_audit.payload
    );
    assert_eq!(
        gcd_audit.payload["stale_ranges_removed"][0]["set_by"],
        "cross_reference"
    );
    assert_eq!(gcd_audit.payload["stale_ranges_removed"][0]["low"], "500");
}

/// Two series in equal proportions (no "main" by count): the cover still
/// assigns everything — one main, one range.
#[tokio::test]
async fn even_split_assigns_every_issue() {
    let cv = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/volumes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(cv_env(
            json!([
                {"id": 11, "name": "Nova", "start_year": "2001", "publisher": {"id": 31, "name": "Marvel"}},
                {"id": 22, "name": "Nova", "start_year": "2003", "publisher": {"id": 31, "name": "Marvel"}},
            ]),
            2,
        )))
        .mount(&cv)
        .await;
    mount_cv_volume(&cv, 11, &monthly(1..=4, 2001, 1), 1100, 1).await;
    mount_cv_volume(&cv, 22, &monthly(5..=8, 2003, 1), 2200, 1).await;
    let app = TestApp::spawn_with_comicvine_at("cv-key", cv.uri()).await;
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let mut seed = SeriesSeed::new(lib, "Nova");
    seed.year = Some(2001);
    seed.publisher = Some("Marvel".into());
    let series_id = seed.insert(&db).await;
    let mut issues: Vec<(f64, i32, i32)> = (1..=4).map(|n| (n as f64, 2001, n)).collect();
    issues.extend((5..=8).map(|n| (n as f64, 2003, n - 4)));
    seed_issues(&app, lib, series_id, tmp.path(), &issues).await;
    let slug = slug_of(&app, series_id).await;
    let cookie = register_admin(&app).await;

    let body = analyze(&app, &cookie, &slug, false).await;
    let c = provider(&body, "comicvine");
    assert_eq!(c["uncovered"], json!([]), "{c}");
    let main = c["main_series_id"].as_str().unwrap().to_owned();
    let ranges = c["proposed_ranges"].as_array().unwrap();
    assert_eq!(ranges.len(), 1, "{c}");
    assert_ne!(ranges[0]["provider_series_id"], main);
    assert_eq!(ranges[0]["issue_count"], 4);
    assert!(
        c["cells"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| !x["provider_series_id"].is_null())
    );
    // Equal size; the strict name + start-year match (2001) is the main.
    assert_eq!(main, "11");
}

/// ComicVine's volume listing pages 100 at a time; a 250-issue volume
/// costs exactly three requests and every page is read.
#[tokio::test]
async fn comicvine_listing_paginates_past_100() {
    let cv = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/volumes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(cv_env(
            json!([{"id": 777, "name": "X-Men", "start_year": "1991", "publisher": {"id": 31, "name": "Marvel"}}]),
            1,
        )))
        .mount(&cv)
        .await;
    let all = monthly(1..=250, 1991, 10);
    for offset in [0usize, 100, 200] {
        let page: Issues = all.iter().skip(offset).take(100).cloned().collect();
        Mock::given(method("GET"))
            .and(path("/issues/"))
            .and(query_param("filter", "volume:777"))
            .and(query_param("offset", offset.to_string()))
            .and(query_param("limit", "100"))
            .respond_with(ResponseTemplate::new(200).set_body_json(cv_env(
                cv_issue_rows(777, "X-Men", &page, 70_000 + offset as u32),
                250,
            )))
            .expect(1)
            .mount(&cv)
            .await;
    }
    let app = TestApp::spawn_with_comicvine_at("cv-key", cv.uri()).await;

    // The public cache-then-fetch path.
    let list = server::metadata::coverage::provider_issues(&app.state(), Source::ComicVine, "777")
        .await
        .unwrap();
    assert_eq!(list.issues.len(), 250);
    assert!(list.complete);
    assert_eq!(list.series_name.as_deref(), Some("X-Men"));
    let i250 = list.issues.iter().find(|i| i.number == "250").unwrap();
    assert_eq!(i250.external_id.as_deref(), Some("70249"));
    assert!(i250.cover_date.is_some());
    // Second call: cache hit, no request (the page mocks expect one each).
    let again = server::metadata::coverage::provider_issues(&app.state(), Source::ComicVine, "777")
        .await
        .unwrap();
    assert_eq!(again.issues.len(), 250);

    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let mut seed = SeriesSeed::new(lib, "X-Men");
    seed.year = Some(1991);
    let series_id = seed.insert(&db).await;
    seed_issues(
        &app,
        lib,
        series_id,
        tmp.path(),
        &[(1.0, 1991, 10), (150.0, 2004, 3), (250.0, 2012, 7)],
    )
    .await;
    let slug = slug_of(&app, series_id).await;
    let cookie = register_admin(&app).await;
    let body = analyze(&app, &cookie, &slug, false).await;
    let c = provider(&body, "comicvine");
    assert_eq!(c["main_series_id"], "777");
    assert_eq!(cell(c, "250")["provider_issue_id"], "70249");
    assert_eq!(c["uncovered"], json!([]));
}

/// Local issues no provider series has are reported, the gap issue search
/// runs at most `MAX_GAP_SEARCHES` times, and the provider never exceeds
/// its request budget.
#[tokio::test]
async fn uncovered_issues_are_reported_within_budget() {
    let cv = MockServer::start().await;
    mount_cv_daredevil(&cv).await;
    // Gap issue searches (broad, by name) find nothing.
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(cv_env(json!({"issue": []}), 0)))
        .expect(2)
        .mount(&cv)
        .await;
    let app = TestApp::spawn_with_comicvine_at("cv-key", cv.uri()).await;
    // #4.5, #499 and #900 exist in no volume. Each sits between covered
    // issues, so they are three separate runs — but the gap search budget
    // is two.
    let (_series_id, slug, _tmp) =
        seed_daredevil(&app, &[(4.5, 1999, 2), (499.0, 2009, 9), (900.0, 2015, 1)]).await;
    let cookie = register_admin(&app).await;
    let body = analyze(&app, &cookie, &slug, false).await;
    let c = provider(&body, "comicvine");
    assert_eq!(c["uncovered"], json!(["4.5", "499", "900"]), "{c}");
    assert!(cell(c, "900")["provider_series_id"].is_null());
    assert_eq!(c["main_series_id"], "6458");
    // 1 series search + 2 listings + 2 gap searches.
    assert_eq!(c["requests"], 5);
    assert!(c["requests"].as_u64().unwrap() <= c["request_budget"].as_u64().unwrap());
    assert_eq!(cv.received_requests().await.unwrap().len(), 5);
    // Unconfigured providers are reported as such.
    assert_eq!(provider(&body, "metron")["status"], "not_configured");
    assert_eq!(provider(&body, "gcd")["status"], "not_configured");
}

/// The analysis endpoint 404s before the first run, and accept refuses
/// until a run has finished.
#[tokio::test]
async fn analysis_lifecycle_errors() {
    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let series_id = SeriesSeed::new(lib, "Empty").insert(&db).await;
    let slug = slug_of(&app, series_id).await;
    let cookie = register_admin(&app).await;
    let (st, _) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/series/{slug}/provider-coverage/analysis"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, job) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/series/{slug}/provider-coverage/analyze"),
        Some(json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED);
    // A second request while queued returns the same job.
    let (_, again) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/series/{slug}/provider-coverage/analyze"),
        Some(json!({})),
    )
    .await;
    assert_eq!(again["job_id"], job["job_id"]);
    assert_eq!(again["queued"], false);
    let (st, _) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/series/{slug}/provider-coverage/accept"),
        Some(json!({"source": "comicvine"})),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
}
