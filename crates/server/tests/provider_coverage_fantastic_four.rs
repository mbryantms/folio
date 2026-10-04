//! Provider coverage for the hardest real series in the owner's library:
//! a "Fantastic Four" folder (labelled 2001) holding the 1998 volume —
//! #½, #1–70, the legacy-renumbered #500–588 and the 2012 #600–611 with
//! #605.1 — 173 issues cover-dated 1998-01 to 2012-12.
//!
//! How the providers file it:
//! - **ComicVine** lumps everything into volume 6211 (`"½"`, `"605.1"`).
//! - **Metron** splits #600–611 off: 1711 (1998) + 1713 (2012).
//! - **GCD** splits #600–611 off into 62349 (2012) and writes 11218's
//!   #42–70 / #500–508 with dual legacy numbering — `"42 (471)"`,
//!   `"500 (71)"` (plus a separate `"500 [Director's Cut]"`). Before the
//!   parse fix 11218 looked like it lacked #42–70, and the detector mapped
//!   them onto the 1961 volume (1482), whose own #42–70 are from 1965–67.
//!
//! Fixtures (`tests/fixtures/fantastic_four/`) are **recorded real
//! responses** (Oct 2026) unless noted:
//! - `cv_issues_6211_p{1,2}`, `cv_volumes_search_ff` (descriptions /
//!   images stripped), `metron_issues_1711_p{1,2}`, `metron_issues_1713_p1`,
//!   `metron_series_search_ff`, `gcd_overview_11218_p{1,2}`,
//!   `gcd_overview_62349_p1`, `gcd_issue_search_ff_600_2012`;
//! - `gcd_series_11218` / `gcd_series_62349`: series payloads rebuilt from
//!   the recorded issue index (ids + descriptors, verbatim) and summary;
//! - `local_issues.psv`: the owner's 173 rows (`number|year|month|title`);
//! - GCD 1482 reuses `tests/fixtures/gcd/` (search row, detail, overview
//!   page 1).
//!
//! Not recorded (kept within the approved request budget): GCD 11218
//! overview pages 3–4 (#509–588), so GCD matches those by number only;
//! other ComicVine / Metron search hits' issue lists (404 here, so those
//! candidates are skipped); GCD's series-name search (served as
//! [1482, 11218], i.e. 62349 is *not* on the first page, which the real
//! name-sorted search makes likely) and the `#0.5` issue search (served
//! empty — GCD has no #½ in either volume).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use common::TestApp;
use common::seed::{IssueSeed, SeriesSeed, seed_library};
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};
use serde_json::{Value, json};
use server::metadata::identifier::{Identifier, Source};
use server::metadata::writers::{SetBy, set_external_id};
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, path_regex, query_param, query_param_is_missing},
};

const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fantastic_four");
const GCD_FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/gcd");

fn fixture(dir: &str, name: &str) -> Value {
    let raw = std::fs::read_to_string(format!("{dir}/{name}"))
        .unwrap_or_else(|e| panic!("{dir}/{name}: {e}"));
    serde_json::from_str(&raw).unwrap()
}

fn ok(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

fn empty_page() -> Value {
    json!({"count": 0, "next": null, "previous": null, "results": []})
}

// ───────── providers ─────────

async fn mount_cv(cv: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/volumes"))
        .respond_with(ok(fixture(FIX, "cv_volumes_search_ff.json")))
        .mount(cv)
        .await;
    for (offset, file) in [
        ("0", "cv_issues_6211_p1.json"),
        ("100", "cv_issues_6211_p2.json"),
    ] {
        Mock::given(method("GET"))
            .and(path("/issues/"))
            .and(query_param("filter", "volume:6211"))
            .and(query_param("offset", offset))
            .respond_with(ok(fixture(FIX, file)))
            .mount(cv)
            .await;
    }
}

async fn mount_metron(metron: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/series/"))
        .and(query_param("name", "Fantastic Four"))
        .respond_with(ok(fixture(FIX, "metron_series_search_ff.json")))
        .mount(metron)
        .await;
    for (sid, page, file) in [
        ("1711", "1", "metron_issues_1711_p1.json"),
        ("1711", "2", "metron_issues_1711_p2.json"),
        ("1713", "1", "metron_issues_1713_p1.json"),
    ] {
        Mock::given(method("GET"))
            .and(path("/api/issue/"))
            .and(query_param("series_id", sid))
            .and(query_param("page", page))
            .respond_with(ok(fixture(FIX, file)))
            .mount(metron)
            .await;
    }
}

async fn mount_gcd(gcd: &MockServer) {
    let s1482 = fixture(GCD_FIX, "series_search_ff_1961.json")["results"][0].clone();
    let s11218 = fixture(FIX, "gcd_series_11218.json");
    Mock::given(method("GET"))
        .and(path("/api/series/name/Fantastic%20Four/"))
        .respond_with(ok(json!({
            "count": 2, "next": null, "previous": null,
            "results": [s1482, s11218],
        })))
        .mount(gcd)
        .await;
    for (sid, body) in [
        ("1482", fixture(GCD_FIX, "series_1482.json")),
        ("11218", s11218.clone()),
        ("62349", fixture(FIX, "gcd_series_62349.json")),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/api/series/{sid}/")))
            .respond_with(ok(body))
            .mount(gcd)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/publisher/78/"))
        .respond_with(ok(fixture(GCD_FIX, "publisher_78.json")))
        .mount(gcd)
        .await;
    for (sid, file, dir) in [
        ("1482", "series_1482_overview_p1.json", GCD_FIX),
        ("11218", "gcd_overview_11218_p1.json", FIX),
        ("62349", "gcd_overview_62349_p1.json", FIX),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/api/series/{sid}/overview/")))
            .and(query_param_is_missing("page"))
            .respond_with(ok(fixture(dir, file)))
            .mount(gcd)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/series/11218/overview/"))
        .and(query_param("page", "2"))
        .respond_with(ok(fixture(FIX, "gcd_overview_11218_p2.json")))
        .mount(gcd)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/series/name/Fantastic%20Four/issue/600/year/2012/",
        ))
        .respond_with(ok(fixture(FIX, "gcd_issue_search_ff_600_2012.json")))
        .mount(gcd)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(
            r"^/api/series/name/Fantastic%20Four/issue/0\.5/",
        ))
        .respond_with(ok(empty_page()))
        .mount(gcd)
        .await;
}

async fn ff_app() -> (TestApp, MockServer, MockServer, MockServer) {
    let cv = MockServer::start().await;
    let metron = MockServer::start().await;
    let gcd = MockServer::start().await;
    mount_cv(&cv).await;
    mount_metron(&metron).await;
    mount_gcd(&gcd).await;
    let app = TestApp::spawn_with_all_providers(cv.uri(), metron.uri(), gcd.uri()).await;
    (app, cv, metron, gcd)
}

// ───────── local library ─────────

/// `(number_raw, year, month)` for the owner's 173 issues.
fn local_rows() -> Vec<(String, i32, i32)> {
    std::fs::read_to_string(format!("{FIX}/local_issues.psv"))
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('|').collect();
            (
                f[0].to_owned(),
                f[1].parse().unwrap(),
                f[2].parse().unwrap(),
            )
        })
        .collect()
}

async fn seed_ff(app: &TestApp) -> (Uuid, String, tempfile::TempDir) {
    let db = app.state().db.clone();
    let tmp = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, tmp.path()).await;
    let mut seed = SeriesSeed::new(lib, "Fantastic Four");
    seed.year = Some(2001);
    seed.publisher = Some("Marvel".into());
    let series_id = seed.insert(&db).await;
    let rows = local_rows();
    assert_eq!(rows.len(), 173);
    for (raw, y, m) in &rows {
        let sort: f64 = raw.parse().unwrap();
        let p = tmp.path().join(format!("ff-{raw}.cbz"));
        let id = IssueSeed::new(lib, series_id, &p, format!("ff {raw}").as_bytes(), sort)
            .insert(&db)
            .await;
        db.execute_unprepared(&format!(
            "UPDATE issues SET number_raw = '{raw}', year = {y}, month = {m} WHERE id = '{id}'"
        ))
        .await
        .unwrap();
    }
    let slug = entity::series::Entity::find_by_id(series_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .slug;
    (series_id, slug, tmp)
}

/// The owner's dev-DB state before this fix: CV 6211 (applied), Metron
/// 1711 + GCD 11218 (Metron's cross-reference), the Metron #600–611 →
/// 1713 range, and #983's wrong GCD #42–70 → 1482 (1961) range.
async fn seed_owner_links(app: &TestApp, series_id: Uuid) {
    let db = &app.state().db;
    for (src, id, by) in [
        (Source::ComicVine, "6211", Source::ComicVine),
        (Source::Metron, "1711", Source::Metron),
        (Source::Gcd, "11218", Source::Metron),
    ] {
        set_external_id(
            db,
            "series",
            &series_id.to_string(),
            &Identifier::with_canonical_url(src, id.to_owned(), "series"),
            SetBy::Provider(by),
        )
        .await
        .unwrap();
    }
    insert_range(app, series_id, "metron", "1713", "600", "611", 2012).await;
    insert_range(app, series_id, "gcd", "1482", "42", "70", 1961).await;
}

/// An automated (`cross_reference`) range row, as #983 writes them.
async fn insert_range(
    app: &TestApp,
    series_id: Uuid,
    src: &str,
    sid: &str,
    low: &str,
    high: &str,
    year: i32,
) {
    let now = chrono::Utc::now().fixed_offset();
    entity::series_provider_range::ActiveModel {
        id: Set(Uuid::new_v4()),
        series_id: Set(series_id),
        source: Set(src.into()),
        provider_series_id: Set(sid.into()),
        provider_series_url: Set(None),
        provider_series_name: Set(Some("Fantastic Four".into())),
        range_low: Set(Some(low.into())),
        range_high: Set(Some(high.into())),
        declared_year: Set(Some(year)),
        set_by: Set("cross_reference".into()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    }
    .insert(&app.state().db)
    .await
    .unwrap();
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

async fn analyze(app: &TestApp, cookie: &str, slug: &str) -> Value {
    let (st, job) = call(
        app,
        cookie,
        Method::POST,
        &format!("/api/series/{slug}/provider-coverage/analyze"),
        Some(json!({"auto_accept": false})),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{job}");
    let job_id = Uuid::parse_str(job["job_id"].as_str().unwrap()).unwrap();
    server::jobs::provider_coverage::process(&app.state(), job_id)
        .await
        .unwrap();
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

fn provider<'a>(body: &'a Value, src: &str) -> &'a Value {
    body["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["source"] == src)
        .unwrap_or_else(|| panic!("no {src} in {body}"))
}

/// `number → provider series id` for every grid cell.
fn assignment(p: &Value) -> Vec<(String, Option<String>)> {
    p["cells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["number"].as_str().unwrap().to_owned(),
                c["provider_series_id"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

/// Every local number assigned to `sid`.
fn assigned_to(p: &Value, sid: &str) -> Vec<String> {
    assignment(p)
        .into_iter()
        .filter(|(_, s)| s.as_deref() == Some(sid))
        .map(|(n, _)| n)
        .collect()
}

fn ranges_of(p: &Value) -> Vec<(String, String, String, String)> {
    p["proposed_ranges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["provider_series_id"].as_str().unwrap().to_owned(),
                r["low"].as_str().unwrap().to_owned(),
                r["high"].as_str().unwrap().to_owned(),
                r["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn numbers(range: impl IntoIterator<Item = u32>) -> Vec<String> {
    range.into_iter().map(|n| n.to_string()).collect()
}

/// #600–611 with #605.1 after #605, as the grid orders them.
fn run_600s() -> Vec<String> {
    let mut v = numbers(600..=605);
    v.push("605.1".into());
    v.extend(numbers(606..=611));
    v
}

fn the_1998_volume() -> Vec<String> {
    let mut v = numbers(1..=70);
    v.extend(numbers(500..=588));
    v
}

// ───────── tests ─────────

/// The owner's state: every provider linked, the right Metron range and
/// the wrong GCD #42–70 → 1961 range already written.
#[tokio::test]
async fn fantastic_four_owner_state_covers_every_provider() {
    let (app, cv, _metron, _gcd) = ff_app().await;
    let (series_id, slug, _tmp) = seed_ff(&app).await;
    seed_owner_links(&app, series_id).await;
    let cookie = register_admin(&app).await;

    let body = analyze(&app, &cookie, &slug).await;
    let local = body["local_issues"].as_array().unwrap();
    assert_eq!(local.len(), 173);
    assert_eq!(local[0]["number"], "0.5");

    // ComicVine: one volume covers all 173, #½ included.
    let c = provider(&body, "comicvine");
    assert_eq!(c["status"], "analyzed", "{c}");
    assert_eq!(c["main_series_id"], "6211");
    assert_eq!(c["uncovered"], json!([]), "{c}");
    assert_eq!(c["proposed_ranges"], json!([]));
    assert_eq!(assigned_to(c, "6211").len(), 173);
    assert!(
        c["cells"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["date_match"] == "confirmed"),
        "{c}"
    );
    assert_eq!(c["confidence"], "high", "{c}");
    // 1 search + 2 listing pages; the other volumes' listings 404 here.
    assert_eq!(c["requests"], 3, "{c}");
    let _ = cv;

    // Metron: 1711 + the (existing) #600–611 → 1713 range.
    let m = provider(&body, "metron");
    assert_eq!(m["status"], "analyzed", "{m}");
    assert_eq!(m["main_series_id"], "1711");
    assert_eq!(m["uncovered"], json!([]), "{m}");
    assert_eq!(
        ranges_of(m),
        vec![(
            "1713".into(),
            "600".into(),
            "611".into(),
            "already_mapped".into()
        )]
    );
    assert_eq!(assigned_to(m, "1713"), run_600s());
    let mut main = vec!["0.5".to_owned()];
    main.extend(the_1998_volume());
    assert_eq!(assigned_to(m, "1711"), main);
    assert_eq!(m["confidence"], "high", "{m}");
    assert_eq!(m["stale_ranges"], json!([]));

    // GCD: 11218 holds #1–588 (dual-numbered #42–70 / #500–508 included),
    // 62349 holds #600–611, #½ is in neither, and the 1961 volume gets
    // nothing — the old #42–70 → 1482 range is reported stale.
    let g = provider(&body, "gcd");
    assert_eq!(g["status"], "analyzed", "{g}");
    assert_eq!(g["main_series_id"], "11218");
    assert_eq!(assigned_to(g, "11218"), the_1998_volume());
    assert_eq!(assigned_to(g, "62349"), run_600s());
    assert!(assigned_to(g, "1482").is_empty(), "{g}");
    assert_eq!(g["uncovered"], json!(["0.5"]), "{g}");
    assert_eq!(
        ranges_of(g),
        vec![("62349".into(), "600".into(), "611".into(), "new".into())]
    );
    let stale = g["stale_ranges"].as_array().unwrap();
    assert_eq!(stale.len(), 1, "{g}");
    assert_eq!(stale[0]["provider_series_id"], "1482");
    assert_eq!(
        (
            stale[0]["range_low"].as_str(),
            stale[0]["range_high"].as_str()
        ),
        (Some("42"), Some("70"))
    );
    // GCD pages 3–4 of 11218 weren't recorded: #509–588 match by number.
    assert_eq!(g["confidence"], "medium", "{g}");
    assert!(g["requests"].as_u64().unwrap() <= g["request_budget"].as_u64().unwrap());

    // No provider assigns anything to a 1961 volume.
    for (src, old) in [("comicvine", "2045"), ("metron", "26"), ("gcd", "1482")] {
        assert!(assigned_to(provider(&body, src), old).is_empty(), "{src}");
    }

    // Range hygiene: accepting GCD removes the stale automated #42–70 →
    // 1482 row (the owner's dev DB) and writes #600–611 → 62349; Metron's
    // range is untouched.
    let (st, out) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/series/{slug}/provider-coverage/accept"),
        Some(json!({"source": "gcd"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    let removed = out["stale_ranges_removed"].as_array().unwrap();
    assert_eq!(removed.len(), 1, "{out}");
    assert_eq!(removed[0]["provider_series_id"], "1482");
    assert_eq!(removed[0]["declared_year"], 1961);
    assert_eq!(
        (
            removed[0]["range_low"].as_str(),
            removed[0]["range_high"].as_str()
        ),
        (Some("42"), Some("70"))
    );
    assert_eq!(
        out["ranges_created"][0]["provider_series_id"], "62349",
        "{out}"
    );
    let mut left: Vec<(String, String)> = entity::series_provider_range::Entity::find()
        .filter(entity::series_provider_range::Column::SeriesId.eq(series_id))
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.source, r.provider_series_id))
        .collect();
    left.sort();
    assert_eq!(
        left,
        vec![
            ("gcd".to_owned(), "62349".to_owned()),
            ("metron".to_owned(), "1713".to_owned())
        ]
    );
}

/// No links, no ranges: every provider finds its series by search alone
/// (GCD's #600–611 series through a gap issue search).
#[tokio::test]
async fn fantastic_four_cold_start() {
    let (app, _cv, _metron, _gcd) = ff_app().await;
    let (_series_id, slug, _tmp) = seed_ff(&app).await;
    let cookie = register_admin(&app).await;

    let body = analyze(&app, &cookie, &slug).await;

    let c = provider(&body, "comicvine");
    assert_eq!(c["main_series_id"], "6211", "{c}");
    assert_eq!(c["uncovered"], json!([]));
    assert_eq!(c["confidence"], "high", "{c}");
    assert!(c["auto_acceptable"].as_bool().unwrap(), "{c}");

    let m = provider(&body, "metron");
    assert_eq!(m["main_series_id"], "1711", "{m}");
    assert_eq!(m["uncovered"], json!([]));
    assert_eq!(
        ranges_of(m),
        vec![("1713".into(), "600".into(), "611".into(), "new".into())]
    );
    assert_eq!(m["confidence"], "high", "{m}");
    assert!(m["auto_acceptable"].as_bool().unwrap(), "{m}");

    let g = provider(&body, "gcd");
    assert_eq!(g["main_series_id"], "11218", "{g}");
    assert_eq!(g["uncovered"], json!(["0.5"]));
    assert_eq!(
        ranges_of(g),
        vec![("62349".into(), "600".into(), "611".into(), "new".into())]
    );
    assert!(assigned_to(g, "1482").is_empty(), "{g}");
    let found = g["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["provider_series_id"] == "62349")
        .unwrap();
    assert_eq!(found["origin"], "issue_search", "{found}");
}

/// #983's detector ("Detect from providers") with the parse fix: GCD
/// 11218 now lists #42–70, so its gaps are #½ (unresolved) and #600–611
/// (→ 62349); never #42–70 → 1961. Metron's #½ matches 1711's `"½"`, and
/// its existing #600–611 → 1713 range is recognised without a search.
#[tokio::test]
async fn fantastic_four_detect_maps_only_the_2012_run() {
    let (app, _cv, _metron, _gcd) = ff_app().await;
    let (series_id, slug, _tmp) = seed_ff(&app).await;
    let db = &app.state().db;
    for (src, id, by) in [
        (Source::ComicVine, "6211", Source::ComicVine),
        (Source::Metron, "1711", Source::Metron),
        (Source::Gcd, "11218", Source::Metron),
    ] {
        set_external_id(
            db,
            "series",
            &series_id.to_string(),
            &Identifier::with_canonical_url(src, id.to_owned(), "series"),
            SetBy::Provider(by),
        )
        .await
        .unwrap();
    }
    insert_range(&app, series_id, "metron", "1713", "600", "611", 2012).await;
    let cookie = register_admin(&app).await;
    let (st, body) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/series/{slug}/provider-ranges/detect"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let result = |src: &str| {
        body["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["source"] == src)
            .unwrap_or_else(|| panic!("no {src} in {body}"))
            .clone()
    };
    let g = result("gcd");
    assert_eq!(g["provider_series_id"], "11218", "{g}");
    // #1–70 + #500–588: the dual-numbered and "500 (71)" / "500"
    // entries fold onto 159 distinct numbers.
    assert_eq!(g["covered_count"], 159, "{g}");
    // GCD has no #½ in either volume: reported, never mapped.
    assert_eq!(g["gaps"], json!(["0.5..0.5", "600..611"]), "{g}");
    assert_eq!(g["gap_details"][0]["status"], "unresolved", "{g}");
    assert_eq!(g["gap_details"][1]["status"], "mapped", "{g}");
    assert_eq!(g["created"].as_array().unwrap().len(), 1, "{g}");
    assert_eq!(g["created"][0]["provider_series_id"], "62349", "{g}");
    let m = result("metron");
    assert_eq!(m["provider_series_id"], "1711", "{m}");
    assert_eq!(m["matched_local"], 160, "{m}");
    assert_eq!(m["gaps"], json!(["600..611"]), "{m}");
    assert_eq!(m["gap_details"][0]["status"], "already_mapped", "{m}");
    assert_eq!(m["created"], json!([]), "{m}");

    let rows = entity::series_provider_range::Entity::find()
        .filter(entity::series_provider_range::Column::SeriesId.eq(series_id))
        .all(db)
        .await
        .unwrap();
    let mut got: Vec<(String, String, Option<String>, Option<String>)> = rows
        .into_iter()
        .map(|r| (r.source, r.provider_series_id, r.range_low, r.range_high))
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            (
                "gcd".into(),
                "62349".into(),
                Some("600".into()),
                Some("611".into())
            ),
            (
                "metron".into(),
                "1713".into(),
                Some("600".into()),
                Some("611".into())
            ),
        ]
    );
}
