//! Integration tests for `GET /me/holdings.csv` (product audit 2026-11
//! §5 item 14).
//!
//! Covers both scopes, the visibility rule (library grant, age cap,
//! soft-deleted series / issues, non-active scan state), the admin-only
//! path column, RFC 4180 quoting round-tripped through a real CSV reader,
//! the `library` filter, a body that spans several keyset batches, and the
//! rate limit.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, Response, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed, seed_progress, seed_progress_finished};
use entity::library_user_access;
use sea_orm::{ActiveModelTrait, ConnectionTrait, Database, DatabaseConnection, Set};
use server::api::holdings_export::{HOLDINGS_BATCH, UTF8_BOM};
use tower::ServiceExt;
use uuid::Uuid;

struct Authed {
    session: String,
    csrf: String,
    user_id: Uuid,
}

impl Authed {
    fn cookies(&self) -> String {
        format!(
            "__Host-comic_session={}; __Host-comic_csrf={}",
            self.session, self.csrf
        )
    }
}

fn extract_cookie(resp: &Response<Body>, name: &str) -> String {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|s| {
            let prefix = format!("{name}=");
            s.split(';')
                .next()
                .and_then(|kv| kv.strip_prefix(&prefix))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| panic!("expected cookie {name}"))
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
    let session = extract_cookie(&resp, "__Host-comic_session");
    let csrf = extract_cookie(&resp, "__Host-comic_csrf");
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let user_id = Uuid::parse_str(json["user"]["id"].as_str().unwrap()).unwrap();
    Authed {
        session,
        csrf,
        user_id,
    }
}

async fn get(app: &TestApp, auth: &Authed, query: &str) -> Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/me/holdings.csv{query}"))
                .header(header::COOKIE, auth.cookies())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

/// Header + rows, parsed back with a real RFC 4180 reader after checking
/// and stripping the BOM.
struct Csv {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    raw: String,
}

impl Csv {
    fn col(&self, name: &str) -> usize {
        self.headers
            .iter()
            .position(|h| h == name)
            .unwrap_or_else(|| panic!("no column {name} in {:?}", self.headers))
    }

    /// The row whose `key_col` equals `key`.
    fn row(&self, key_col: &str, key: &str) -> &Vec<String> {
        let c = self.col(key_col);
        self.rows
            .iter()
            .find(|r| r[c] == key)
            .unwrap_or_else(|| panic!("no row with {key_col} = {key:?}"))
    }

    fn values(&self, name: &str) -> Vec<String> {
        let c = self.col(name);
        let mut v: Vec<String> = self.rows.iter().map(|r| r[c].clone()).collect();
        v.sort();
        v
    }
}

async fn csv_ok(app: &TestApp, auth: &Authed, query: &str, scope: &str) -> Csv {
    let resp = get(app, auth, query).await;
    assert_eq!(resp.status(), StatusCode::OK, "query {query}");
    let h = resp.headers();
    assert_eq!(h[header::CONTENT_TYPE], "text/csv; charset=utf-8");
    let cd = h[header::CONTENT_DISPOSITION].to_str().unwrap().to_owned();
    let date = Utc::now().format("%Y-%m-%d").to_string();
    assert_eq!(
        cd,
        format!("attachment; filename=\"folio-holdings-{scope}-{date}.csv\"")
    );
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert!(
        bytes.starts_with(UTF8_BOM),
        "body starts with the UTF-8 BOM"
    );
    let body = &bytes[UTF8_BOM.len()..];
    let mut rdr = csv::ReaderBuilder::new().from_reader(body);
    let headers = rdr
        .headers()
        .unwrap()
        .iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let rows = rdr
        .records()
        .map(|r| r.unwrap().iter().map(str::to_owned).collect())
        .collect();
    Csv {
        headers,
        rows,
        raw: String::from_utf8(body.to_vec()).unwrap(),
    }
}

async fn grant(db: &DatabaseConnection, lib: Uuid, user: Uuid, cap: Option<&str>) {
    let now = Utc::now().fixed_offset();
    library_user_access::ActiveModel {
        library_id: Set(lib),
        user_id: Set(user),
        age_rating_max: Set(cap.map(str::to_owned)),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

async fn exec(db: &DatabaseConnection, sql: &str) {
    db.execute_unprepared(sql).await.unwrap();
}

async fn ext_id(
    db: &DatabaseConnection,
    entity_type: &str,
    entity_id: &str,
    source: &str,
    v: &str,
) {
    exec(
        db,
        &format!(
            "INSERT INTO external_ids (entity_type, entity_id, source, external_id, set_by, \
             first_set_at, last_synced_at) VALUES ('{entity_type}', '{entity_id}', '{source}', \
             '{v}', 'user', now(), now())"
        ),
    )
    .await;
}

/// Insert `n` copies of the row `id = template` in `table`, each with the
/// `jsonb_build_object` key/value list `overrides` applied (`g` is the copy
/// index). Generated columns are skipped (they reject explicit values).
async fn clone_rows(db: &DatabaseConnection, table: &str, template: &str, overrides: &str, n: i64) {
    let cols: Vec<String> = db
        .query_all_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT column_name::text FROM information_schema.columns \
             WHERE table_schema = 'public' AND table_name = $1 AND is_generated = 'NEVER' \
             ORDER BY ordinal_position",
            [table.into()],
        ))
        .await
        .unwrap()
        .into_iter()
        .map(|r| format!("\"{}\"", r.try_get_by_index::<String>(0).unwrap()))
        .collect();
    let cols = cols.join(", ");
    exec(
        db,
        &format!(
            "INSERT INTO {table} ({cols}) SELECT {cols} FROM ( \
               SELECT (jsonb_populate_record(t, jsonb_build_object({overrides}))).* \
               FROM {table} t, generate_series(1, {n}) g WHERE t.id = {template}) x"
        ),
    )
    .await;
}

/// Two libraries; library A carries the interesting rows.
struct World {
    lib_a: Uuid,
    lib_b: Uuid,
    /// "Saga" in A: two active issues (one finished), one removed issue,
    /// one malformed issue; CV / Metron / GCD ids on the series and issue 1.
    saga: Uuid,
    saga_1: String,
    saga_2: String,
    /// "Tricky, \"Quoted\"\nName" in A — exercises quoting.
    tricky: Uuid,
    /// Soft-deleted series in A (with one live-looking issue).
    removed: Uuid,
    /// Mature-rated series in A, hidden by a Teen cap.
    mature: Uuid,
    /// Series in B.
    other: Uuid,
}

async fn seed_world(db: &DatabaseConnection, tmp: &std::path::Path, reader: Uuid) -> World {
    let root_a = tmp.join("a");
    let root_b = tmp.join("b");
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();
    let mut seed_a = LibrarySeed::new(&root_a);
    seed_a.name = Some("Library A".into());
    let lib_a = seed_a.insert(db).await;
    let mut seed_b = LibrarySeed::new(&root_b);
    seed_b.name = Some("Library B".into());
    let lib_b = seed_b.insert(db).await;

    let saga = SeriesSeed::new(lib_a, "Saga")
        .with_publisher("Image")
        .insert(db)
        .await;
    exec(
        db,
        &format!(
            "UPDATE series SET volume = 1, year = 2012, status = 'continuing', total_issues = 66, \
             folder_path = '/comics/Saga (2012)' WHERE id = '{saga}'"
        ),
    )
    .await;
    let saga_1 = IssueSeed::new(lib_a, saga, &root_a.join("saga-1.cbz"), b"saga-1", 1.0)
        .with_title("Chapter One")
        .with_page_count(44)
        .insert(db)
        .await;
    exec(
        db,
        &format!(
            "UPDATE issues SET year = 2012, month = 3, day = 14, format = 'Series' \
             WHERE id = '{saga_1}'"
        ),
    )
    .await;
    let saga_2 = IssueSeed::new(lib_a, saga, &root_a.join("saga-2.cbz"), b"saga-2", 2.0)
        .insert(db)
        .await;
    exec(
        db,
        &format!("UPDATE issues SET year = 2012, month = 4 WHERE id = '{saga_2}'"),
    )
    .await;
    let saga_gone = IssueSeed::new(lib_a, saga, &root_a.join("saga-3.cbz"), b"saga-3", 3.0)
        .insert(db)
        .await;
    exec(
        db,
        &format!("UPDATE issues SET removed_at = now() WHERE id = '{saga_gone}'"),
    )
    .await;
    IssueSeed::new(lib_a, saga, &root_a.join("saga-4.cbz"), b"saga-4", 4.0)
        .with_state(entity::issue::IssueState::Malformed)
        .insert(db)
        .await;
    seed_progress_finished(db, reader, &saga_1).await;
    seed_progress(db, reader, &saga_2, 5, 0.25, false).await;
    ext_id(db, "series", &saga.to_string(), "comicvine", "4050-49901").await;
    ext_id(db, "series", &saga.to_string(), "metron", "1234").await;
    ext_id(db, "series", &saga.to_string(), "gcd", "777").await;
    // A non-provider source never becomes a column.
    ext_id(db, "series", &saga.to_string(), "isbn", "9781607066019").await;
    ext_id(db, "issue", &saga_1, "comicvine", "4000-325000").await;
    ext_id(db, "issue", &saga_1, "metron", "5555").await;
    ext_id(db, "issue", &saga_1, "gcd", "8888").await;

    let tricky = SeriesSeed::new(lib_a, "Tricky, \"Quoted\"\nName")
        .with_publisher("=HYPERLINK(\"http://x\")")
        .insert(db)
        .await;
    IssueSeed::new(
        lib_a,
        tricky,
        &root_a.join("tricky-1.cbz"),
        b"tricky-1",
        1.0,
    )
    .with_title("Commas, \"quotes\"\r\nand lines")
    .insert(db)
    .await;

    let removed = SeriesSeed::new(lib_a, "Removed Series").insert(db).await;
    IssueSeed::new(
        lib_a,
        removed,
        &root_a.join("removed-1.cbz"),
        b"removed-1",
        1.0,
    )
    .insert(db)
    .await;
    exec(
        db,
        &format!("UPDATE series SET removed_at = now() WHERE id = '{removed}'"),
    )
    .await;

    let mature = SeriesSeed::new(lib_a, "Mature Series").insert(db).await;
    exec(
        db,
        &format!("UPDATE series SET age_rating = 'Mature 17+' WHERE id = '{mature}'"),
    )
    .await;
    IssueSeed::new(
        lib_a,
        mature,
        &root_a.join("mature-1.cbz"),
        b"mature-1",
        1.0,
    )
    .insert(db)
    .await;

    let other = SeriesSeed::new(lib_b, "Other Library Series")
        .insert(db)
        .await;
    IssueSeed::new(lib_b, other, &root_b.join("other-1.cbz"), b"other-1", 1.0)
        .insert(db)
        .await;

    World {
        lib_a,
        lib_b,
        saga,
        saga_1,
        saga_2,
        tricky,
        removed,
        mature,
        other,
    }
}

const SERIES_READER_COLUMNS: [&str; 12] = [
    "library",
    "publisher",
    "series",
    "volume",
    "start_year",
    "status",
    "issues_owned",
    "total_issues",
    "issues_read",
    "comicvine_id",
    "metron_id",
    "gcd_id",
];

const ISSUE_READER_COLUMNS: [&str; 14] = [
    "library",
    "publisher",
    "series",
    "volume",
    "number",
    "title",
    "cover_date",
    "format",
    "page_count",
    "file_size_bytes",
    "read_state",
    "comicvine_id",
    "metron_id",
    "gcd_id",
];

#[tokio::test]
async fn admin_series_scope_lists_every_live_series_with_paths() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::connect(&app.db_url).await.unwrap();
    let admin = register(&app, "admin@example.com").await;
    let w = seed_world(&db, tmp.path(), admin.user_id).await;

    let csv = csv_ok(&app, &admin, "?scope=series", "series").await;
    let mut expected: Vec<&str> = SERIES_READER_COLUMNS.to_vec();
    expected.push("folder_path");
    assert_eq!(csv.headers, expected);
    // Saga, Tricky, Mature (admins are uncapped), Other — never Removed.
    assert_eq!(csv.rows.len(), 4, "{:?}", csv.rows);
    assert!(!csv.raw.contains("Removed Series"));

    let saga = csv.row("series", "Saga");
    let c = |n: &str| csv.col(n);
    assert_eq!(saga[c("library")], "Library A");
    assert_eq!(saga[c("publisher")], "Image");
    assert_eq!(saga[c("volume")], "1");
    assert_eq!(saga[c("start_year")], "2012");
    assert_eq!(saga[c("status")], "continuing");
    // Removed and malformed issues are not "owned".
    assert_eq!(saga[c("issues_owned")], "2");
    assert_eq!(saga[c("total_issues")], "66");
    assert_eq!(saga[c("issues_read")], "1");
    assert_eq!(saga[c("comicvine_id")], "4050-49901");
    assert_eq!(saga[c("metron_id")], "1234");
    assert_eq!(saga[c("gcd_id")], "777");
    assert_eq!(saga[c("folder_path")], "/comics/Saga (2012)");
    assert!(!csv.raw.contains("9781607066019"), "isbn is not a column");

    let other = csv.row("series", "Other Library Series");
    assert_eq!(other[c("library")], "Library B");
    assert_eq!(other[c("issues_owned")], "1");
    assert_eq!(other[c("issues_read")], "0");
    assert_eq!(other[c("total_issues")], "");
    let _ = (w.saga, w.tricky, w.removed, w.mature, w.other, w.lib_b);
}

#[tokio::test]
async fn admin_issues_scope_has_read_state_dates_ids_and_paths() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::connect(&app.db_url).await.unwrap();
    let admin = register(&app, "admin@example.com").await;
    let w = seed_world(&db, tmp.path(), admin.user_id).await;

    let csv = csv_ok(&app, &admin, "?scope=issues", "issues").await;
    let mut expected: Vec<&str> = ISSUE_READER_COLUMNS.to_vec();
    expected.push("file_path");
    assert_eq!(csv.headers, expected);
    // saga 1 + 2, tricky 1, mature 1, other 1. Not the removed issue, the
    // malformed one, or the issue under the removed series.
    assert_eq!(csv.rows.len(), 5, "{:?}", csv.rows);
    assert!(!csv.raw.contains("saga-3.cbz"));
    assert!(!csv.raw.contains("saga-4.cbz"));
    assert!(!csv.raw.contains("removed-1.cbz"));

    let c = |n: &str| csv.col(n);
    let one = csv.row("title", "Chapter One");
    assert_eq!(one[c("library")], "Library A");
    assert_eq!(one[c("publisher")], "Image");
    assert_eq!(one[c("series")], "Saga");
    assert_eq!(one[c("volume")], "1", "falls back to the series volume");
    assert_eq!(one[c("number")], "1");
    assert_eq!(one[c("cover_date")], "2012-03-14");
    assert_eq!(one[c("format")], "Series");
    assert_eq!(one[c("page_count")], "44");
    assert_eq!(one[c("file_size_bytes")], b"saga-1".len().to_string());
    assert_eq!(one[c("read_state")], "read");
    assert_eq!(one[c("comicvine_id")], "4000-325000");
    assert_eq!(one[c("metron_id")], "5555");
    assert_eq!(one[c("gcd_id")], "8888");
    assert!(one[c("file_path")].ends_with("saga-1.cbz"));

    let two = csv.row(
        "file_path",
        &tmp.path().join("a/saga-2.cbz").to_string_lossy(),
    );
    assert_eq!(two[c("read_state")], "in_progress");
    assert_eq!(two[c("cover_date")], "2012-04");
    assert_eq!(two[c("comicvine_id")], "");

    let other = csv.row("series", "Other Library Series");
    assert_eq!(other[c("read_state")], "unread");
    assert_eq!(other[c("cover_date")], "");
    let _ = (w.saga_1, w.saga_2);
}

#[tokio::test]
async fn reader_sees_only_granted_uncapped_rows_and_no_paths() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::connect(&app.db_url).await.unwrap();
    let _admin = register(&app, "admin@example.com").await;
    let reader = register(&app, "reader@example.com").await;
    let w = seed_world(&db, tmp.path(), reader.user_id).await;
    // Library A only, capped at Teen: Mature Series is hidden, B is not granted.
    grant(&db, w.lib_a, reader.user_id, Some("Teen")).await;

    let series = csv_ok(&app, &reader, "?scope=series", "series").await;
    assert_eq!(series.headers, SERIES_READER_COLUMNS.to_vec());
    assert_eq!(
        series.values("series"),
        vec!["Saga".to_owned(), "Tricky, \"Quoted\"\nName".to_owned()]
    );
    assert!(
        !series.raw.contains("/comics/"),
        "no folder paths for readers"
    );
    let saga = series.row("series", "Saga");
    assert_eq!(saga[series.col("issues_owned")], "2");
    assert_eq!(saga[series.col("issues_read")], "1");

    let issues = csv_ok(&app, &reader, "?scope=issues", "issues").await;
    assert_eq!(issues.headers, ISSUE_READER_COLUMNS.to_vec());
    assert_eq!(issues.rows.len(), 3, "{:?}", issues.rows);
    assert!(!issues.raw.contains(".cbz"), "no file paths for readers");
    assert!(!issues.raw.contains("Other Library Series"));
    assert!(!issues.raw.contains("Mature Series"));

    // The other library is invisible, so filtering on it is a 404, the same
    // answer as a slug that does not exist.
    let resp = get(&app, &reader, &format!("?library={}", w.lib_b)).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = get(&app, &reader, "?library=no-such-library").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn library_filter_quoting_and_formula_guard() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::connect(&app.db_url).await.unwrap();
    let admin = register(&app, "admin@example.com").await;
    let w = seed_world(&db, tmp.path(), admin.user_id).await;

    let only_b = csv_ok(&app, &admin, &format!("?library={}", w.lib_b), "series").await;
    assert_eq!(
        only_b.values("series"),
        vec!["Other Library Series".to_owned()]
    );

    let only_a = csv_ok(
        &app,
        &admin,
        &format!("?scope=issues&library={}", w.lib_a),
        "issues",
    )
    .await;
    assert_eq!(only_a.rows.len(), 4);
    assert!(
        only_a
            .rows
            .iter()
            .all(|r| r[only_a.col("library")] == "Library A")
    );

    // The comma / quote / newline name survives a round trip through a real
    // CSV reader, and the raw bytes use RFC 4180 quoting.
    let tricky_title = "Commas, \"quotes\"\r\nand lines";
    let tricky = only_a.row("title", tricky_title);
    assert_eq!(tricky[only_a.col("series")], "Tricky, \"Quoted\"\nName");
    assert!(
        only_a.raw.contains("\"Tricky, \"\"Quoted\"\"\nName\""),
        "{}",
        only_a.raw
    );
    assert!(only_a.raw.contains("\r\n"), "CRLF record terminators");
    // A publisher that a spreadsheet would evaluate is neutralised.
    assert_eq!(tricky[only_a.col("publisher")], "'=HYPERLINK(\"http://x\")");

    let resp = get(&app, &admin, "?scope=everything").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "validation");
}

#[tokio::test]
async fn streams_across_several_keyset_batches() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::connect(&app.db_url).await.unwrap();
    let admin = register(&app, "admin@example.com").await;
    let root = tmp.path().join("bulk");
    std::fs::create_dir_all(&root).unwrap();
    let lib = LibrarySeed::new(&root).insert(&db).await;
    let series = SeriesSeed::new(lib, "Bulk Template").insert(&db).await;
    let template = IssueSeed::new(lib, series, &root.join("template.cbz"), b"template", 1.0)
        .insert(&db)
        .await;

    // Two full batches plus a partial one, and an exact multiple on the
    // other scope (the empty trailing batch must end the stream cleanly).
    let total_series = (HOLDINGS_BATCH * 2 + 7) as i64;
    let total_issues = (HOLDINGS_BATCH * 2) as i64;
    clone_rows(
        &db,
        "series",
        &format!("'{series}'"),
        "'id', gen_random_uuid(), 'name', 'Bulk ' || g, \
         'normalized_name', 'bulk ' || g, 'slug', 'bulk-' || g",
        total_series - 1,
    )
    .await;
    clone_rows(
        &db,
        "issues",
        &format!("'{template}'"),
        "'id', 'bulk' || lpad(g::text, 6, '0'), 'slug', 'bulk-issue-' || g, \
         'content_hash', 'bulk' || lpad(g::text, 6, '0'), \
         'file_path', '/bulk/' || g || '.cbz', 'sort_number', g + 1",
        total_issues - 1,
    )
    .await;

    let s = csv_ok(&app, &admin, "?scope=series", "series").await;
    assert_eq!(s.rows.len() as i64, total_series);
    let mut names = s.values("series");
    names.dedup();
    assert_eq!(names.len() as i64, total_series, "no row written twice");
    let template_row = s.row("series", "Bulk Template");
    assert_eq!(
        template_row[s.col("issues_owned")],
        total_issues.to_string()
    );

    let i = csv_ok(&app, &admin, "?scope=issues", "issues").await;
    assert_eq!(i.rows.len() as i64, total_issues);
    let mut paths = i.values("file_path");
    paths.dedup();
    assert_eq!(paths.len() as i64, total_issues, "no row written twice");
}

#[tokio::test]
async fn export_is_rate_limited() {
    let app = TestApp::spawn().await;
    let admin = register(&app, "admin@example.com").await;
    // Burst 6 (`rate_limit::HOLDINGS_EXPORT`); the seventh is refused.
    for _ in 0..6 {
        let resp = get(&app, &admin, "").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let _ = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    }
    let resp = get(&app, &admin, "").await;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}
