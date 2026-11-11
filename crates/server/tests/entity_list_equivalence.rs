//! Equivalence guard for the entity browse indexes (audit finding P-01,
//! roadmap C7): `GET /characters`, `/teams`, `/locations`, `/arcs`,
//! `/publishers`, `/creators`, `/people` and the OPDS v1 / v2 entity
//! navigation feeds.
//!
//! The list queries were rewritten so their cost follows the page instead
//! of the library. These tests pin the complete output — rows, order,
//! per-row counts, `total`, `next_cursor` — of every parameter shape the
//! handlers and the web callers use, for the three caller kinds whose
//! visibility differs:
//!
//! - the admin (sees everything that is not removed),
//! - a reader granted library A only, without a cap,
//! - a reader granted library A with a `Teen` cap and library C uncapped
//!   (the capped path: no `series_*` rollups, issue rating AND series
//!   rating).
//!
//! The fixture has entities reachable only through a hidden library, an
//! over-cap issue, an over-cap *series* (Teen issue in a Mature series), a
//! removed issue, a removed series, a non-active issue and a series-level
//! rollup row; FK-linked and name-only (`fk IS NULL`) junction rows; names
//! that tie on `lower(name)` (slug breaks the tie), differ only by case or
//! diacritics, sort under `#`; and enough rows that small limits put page
//! boundaries inside runs of equal keys. The snapshots were recorded
//! against the pre-rewrite queries.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use common::TestApp;
use common::seed::{IssueSeed, SeriesSeed, seed_library};
use entity::{library_user_access, user::Entity as UserEntity};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, Database, DatabaseConnection, EntityTrait,
    QueryFilter, Set, Statement, Unchanged,
};
use serde_json::{Value as Json, json};
use tower::ServiceExt;
use uuid::Uuid;

// ───────── HTTP helpers ─────────

struct Authed {
    label: &'static str,
    session: String,
    csrf: String,
    user_id: Uuid,
}

async fn register(app: &TestApp, email: &str, label: &'static str) -> Authed {
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
            .map(|kv| kv.split_once('=').map(|(_, v)| v.to_owned()).unwrap())
            .expect("cookie")
    };
    let db = Database::connect(&app.db_url).await.unwrap();
    let user_row = UserEntity::find()
        .filter(entity::user::Column::Email.eq(email))
        .one(&db)
        .await
        .unwrap()
        .expect("user row");
    Authed {
        label,
        session: extract("__Host-comic_session="),
        csrf: extract("__Host-comic_csrf="),
        user_id: user_row.id,
    }
}

/// GET with the caller's cookies. The list routes sit behind the per-user
/// `SEARCH` bucket (burst 60, 10/s); a 429 waits and retries so the walk
/// below never records a rate-limit response.
async fn get(app: &TestApp, uri: &str, user: &Authed) -> (StatusCode, Vec<u8>) {
    for _ in 0..200 {
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(
                header::COOKIE,
                format!(
                    "__Host-comic_session={}; __Host-comic_csrf={}",
                    user.session, user.csrf
                ),
            )
            .body(Body::empty())
            .unwrap();
        let resp = app.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        if status == StatusCode::TOO_MANY_REQUESTS {
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            continue;
        }
        return (status, bytes);
    }
    panic!("rate limited for too long: {uri}");
}

async fn get_json(app: &TestApp, uri: &str, user: &Authed) -> (StatusCode, Json) {
    let (status, bytes) = get(app, uri, user).await;
    let v = serde_json::from_slice(&bytes).unwrap_or(Json::Null);
    (status, v)
}

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// Walk a cursor-paginated list from the first page to the end; records
/// every page body. `base` already carries every query parameter except
/// `cursor`.
async fn walk(app: &TestApp, base: &str, user: &Authed) -> Json {
    let sep = if base.contains('?') { '&' } else { '?' };
    let mut pages = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..400 {
        let uri = match &cursor {
            Some(c) => format!("{base}{sep}cursor={}", enc(c)),
            None => base.to_owned(),
        };
        let (status, body) = get_json(app, &uri, user).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        cursor = body["next_cursor"].as_str().map(str::to_owned);
        pages.push(body);
        if cursor.is_none() {
            return Json::Array(pages);
        }
    }
    panic!("{base}: more than 400 pages");
}

// ───────── SQL helpers ─────────

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        sql,
        values,
    ))
    .await
    .unwrap();
}

/// Deterministic entity ids so the snapshots (which carry `id`) are stable.
fn eid(table: &str, n: u128) -> Uuid {
    let base: u128 = match table {
        "character" => 0x1000,
        "team" => 0x2000,
        "location" => 0x3000,
        "story_arc" => 0x4000,
        "publisher" => 0x5000,
        _ => 0x9000,
    };
    Uuid::from_u128((0x0194_0000_0000_7000_8000_0000_0000_0000u128) | (base << 16) | n)
}

async fn add_entity(
    db: &DatabaseConnection,
    table: &str,
    n: u128,
    slug: &str,
    name: &str,
    normalized: &str,
) -> Uuid {
    let id = eid(table, n);
    exec(
        db,
        &format!("INSERT INTO {table} (id, slug, name, normalized_name) VALUES ($1, $2, $3, $4)"),
        vec![id.into(), slug.into(), name.into(), normalized.into()],
    )
    .await;
    id
}

// ───────── fixture ─────────

struct Fixture {
    _tmp: tempfile::TempDir,
    db: DatabaseConnection,
    admin: Authed,
    partial: Authed,
    capped: Authed,
    s_alpha: Uuid,
    s_adult: Uuid,
    s_removed: Uuid,
    s_hidden: Uuid,
    s_cee: Uuid,
    s_empty: Uuid,
    /// Library A, series Alpha: three plain visible issues.
    a: Vec<String>,
    /// Alpha issue rated `Mature 17+` (over the reader's `Teen` cap).
    a_mature: String,
    /// Alpha issue soft-removed.
    a_removed: String,
    /// Alpha issue in a non-active state.
    a_malformed: String,
    /// Series Adult (rated `Mature 17+`): issue rated `Teen`, issue unrated.
    ad: Vec<String>,
    /// Removed series' issue.
    r1: String,
    /// Library B (granted to no reader).
    h: Vec<String>,
    /// Library C (granted uncapped to the capped reader only).
    c: Vec<String>,
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

async fn demote(db: &DatabaseConnection, user: Uuid) {
    entity::user::ActiveModel {
        id: Unchanged(user),
        role: Set(entity::user::UserRole::User),
        ..Default::default()
    }
    .update(db)
    .await
    .unwrap();
}

async fn issue(
    db: &DatabaseConnection,
    root: &std::path::Path,
    lib: Uuid,
    series: Uuid,
    tag: &str,
    n: f64,
) -> String {
    let p = root.join(format!("{tag}.cbz"));
    IssueSeed::new(lib, series, &p, tag.as_bytes(), n)
        .insert(db)
        .await
}

async fn fixture(app: &TestApp) -> Fixture {
    let admin = register(app, "admin@example.com", "admin").await;
    let partial = register(app, "partial@example.com", "partial").await;
    let capped = register(app, "capped@example.com", "capped").await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let mut roots = Vec::new();
    for d in ["a", "b", "c"] {
        let r = tmp.path().join(d);
        std::fs::create_dir_all(&r).unwrap();
        roots.push(r);
    }
    let lib_a = seed_library(&db, &roots[0]).await;
    let lib_b = seed_library(&db, &roots[1]).await;
    let lib_c = seed_library(&db, &roots[2]).await;
    demote(&db, partial.user_id).await;
    demote(&db, capped.user_id).await;
    grant(&db, lib_a, partial.user_id, None).await;
    grant(&db, lib_a, capped.user_id, Some("Teen")).await;
    grant(&db, lib_c, capped.user_id, None).await;

    let s_alpha = SeriesSeed::new(lib_a, "Alpha")
        .with_publisher("Acme Press")
        .insert(&db)
        .await;
    let s_adult = SeriesSeed::new(lib_a, "Adult Series")
        .with_publisher("Adult Press")
        .insert(&db)
        .await;
    let s_removed = SeriesSeed::new(lib_a, "Removed Series")
        .with_publisher("Ghost Press")
        .insert(&db)
        .await;
    let s_empty = SeriesSeed::new(lib_a, "Empty Series")
        .with_publisher("Lonely Press")
        .insert(&db)
        .await;
    let s_hidden = SeriesSeed::new(lib_b, "Hidden Tales")
        .with_publisher("Secret Press")
        .insert(&db)
        .await;
    let s_cee = SeriesSeed::new(lib_c, "Cee Series")
        .with_publisher("acme press ")
        .insert(&db)
        .await;
    exec(
        &db,
        "UPDATE series SET age_rating = 'Mature 17+' WHERE id = $1",
        vec![s_adult.into()],
    )
    .await;
    exec(
        &db,
        "UPDATE series SET removed_at = now() WHERE id = $1",
        vec![s_removed.into()],
    )
    .await;

    let (ra, rb, rc) = (&roots[0], &roots[1], &roots[2]);
    let mut a = Vec::new();
    for n in 1..=3 {
        a.push(issue(&db, ra, lib_a, s_alpha, &format!("alpha-{n}"), n as f64).await);
    }
    let a_mature = issue(&db, ra, lib_a, s_alpha, "alpha-mature", 4.0).await;
    exec(
        &db,
        "UPDATE issues SET age_rating = 'Mature 17+' WHERE id = $1",
        vec![a_mature.clone().into()],
    )
    .await;
    let a_removed = issue(&db, ra, lib_a, s_alpha, "alpha-removed", 5.0).await;
    exec(
        &db,
        "UPDATE issues SET removed_at = now() WHERE id = $1",
        vec![a_removed.clone().into()],
    )
    .await;
    let p = ra.join("alpha-malformed.cbz");
    let a_malformed = IssueSeed::new(lib_a, s_alpha, &p, b"alpha-malformed", 6.0)
        .with_state(entity::issue::IssueState::Malformed)
        .insert(&db)
        .await;
    let mut ad = Vec::new();
    for n in 1..=2 {
        ad.push(issue(&db, ra, lib_a, s_adult, &format!("adult-{n}"), n as f64).await);
    }
    exec(
        &db,
        "UPDATE issues SET age_rating = 'Teen' WHERE id = $1",
        vec![ad[0].clone().into()],
    )
    .await;
    let r1 = issue(&db, ra, lib_a, s_removed, "removed-1", 1.0).await;
    let mut h = Vec::new();
    for n in 1..=2 {
        h.push(issue(&db, rb, lib_b, s_hidden, &format!("hidden-{n}"), n as f64).await);
    }
    let mut c = Vec::new();
    for n in 1..=2 {
        c.push(issue(&db, rc, lib_c, s_cee, &format!("cee-{n}"), n as f64).await);
    }
    Fixture {
        _tmp: tmp,
        db,
        admin,
        partial,
        capped,
        s_alpha,
        s_adult,
        s_removed,
        s_hidden,
        s_cee,
        s_empty,
        a,
        a_mature,
        a_removed,
        a_malformed,
        ad,
        r1,
        h,
        c,
    }
}

impl Fixture {
    fn callers(&self) -> [&Authed; 3] {
        [&self.admin, &self.partial, &self.capped]
    }
}

/// Where one name-keyed entity (character / team / location) appears.
enum Hit<'a> {
    /// Issue junction row linked by FK, raw name as given.
    IssueFk(&'a str, &'a str),
    /// Issue junction row with `fk IS NULL` (name fallback), raw name.
    IssueName(&'a str, &'a str),
    /// Series rollup row linked by FK.
    SeriesFk(Uuid, &'a str),
    /// Series rollup row with `fk IS NULL`.
    SeriesName(Uuid, &'a str),
}

/// One name-keyed kind's tables.
struct NameKind {
    table: &'static str,
    issue_junction: &'static str,
    series_junction: &'static str,
    fk: &'static str,
    name: &'static str,
}

const CHARACTER: NameKind = NameKind {
    table: "character",
    issue_junction: "issue_characters",
    series_junction: "series_characters",
    fk: "character_id",
    name: "character",
};
const TEAM: NameKind = NameKind {
    table: "team",
    issue_junction: "issue_teams",
    series_junction: "series_teams",
    fk: "team_id",
    name: "team",
};
const LOCATION: NameKind = NameKind {
    table: "location",
    issue_junction: "issue_locations",
    series_junction: "series_locations",
    fk: "location_id",
    name: "location",
};

async fn add_hits(db: &DatabaseConnection, k: &NameKind, id: Uuid, hits: &[Hit<'_>]) {
    for hit in hits {
        let (sql, vals): (String, Vec<sea_orm::Value>) = match hit {
            Hit::IssueFk(issue, raw) => (
                format!(
                    "INSERT INTO {} (issue_id, {}, {}) VALUES ($1, $2, $3)",
                    k.issue_junction, k.name, k.fk
                ),
                vec![(*issue).into(), (*raw).into(), id.into()],
            ),
            Hit::IssueName(issue, raw) => (
                format!(
                    "INSERT INTO {} (issue_id, {}) VALUES ($1, $2)",
                    k.issue_junction, k.name
                ),
                vec![(*issue).into(), (*raw).into()],
            ),
            Hit::SeriesFk(series, raw) => (
                format!(
                    "INSERT INTO {} (series_id, {}, {}) VALUES ($1, $2, $3)",
                    k.series_junction, k.name, k.fk
                ),
                vec![(*series).into(), (*raw).into(), id.into()],
            ),
            Hit::SeriesName(series, raw) => (
                format!(
                    "INSERT INTO {} (series_id, {}) VALUES ($1, $2)",
                    k.series_junction, k.name
                ),
                vec![(*series).into(), (*raw).into()],
            ),
        };
        exec(db, &sql, vals).await;
    }
}

/// The shared name-keyed cast: every visibility edge, case / diacritic /
/// tie / `#` names. `fillers` extra FK-linked entities push the index past
/// one OPDS page (50) and make small-limit walks long.
async fn seed_name_kind(f: &Fixture, k: &NameKind, fillers: u128) {
    let db = &f.db;
    let (a, h, c, ad) = (&f.a, &f.h, &f.c, &f.ad);
    use Hit::*;

    let id = add_entity(db, k.table, 1, "batman", "Batman", "batman").await;
    add_hits(
        db,
        k,
        id,
        &[
            IssueFk(&a[0], "Batman"),
            IssueName(&a[1], "batman "),
            IssueName(&f.a_mature, "Batman"),
            IssueFk(&h[0], "Batman"),
            IssueName(&c[0], "BATMAN"),
            SeriesFk(f.s_alpha, "Batman"),
        ],
    )
    .await;
    let id = add_entity(db, k.table, 2, "joker", "Joker", "joker").await;
    add_hits(
        db,
        k,
        id,
        &[IssueName(&h[0], "Joker"), SeriesName(f.s_hidden, "Joker")],
    )
    .await;
    let id = add_entity(db, k.table, 3, "mature-guy", "Mature Guy", "mature guy").await;
    add_hits(db, k, id, &[IssueFk(&f.a_mature, "Mature Guy")]).await;
    let id = add_entity(
        db,
        k.table,
        4,
        "rated-series-guy",
        "Rated Series Guy",
        "rated series guy",
    )
    .await;
    add_hits(db, k, id, &[IssueName(&ad[0], "Rated Series Guy")]).await;
    let id = add_entity(
        db,
        k.table,
        5,
        "removed-issue-guy",
        "Removed Issue Guy",
        "removed issue guy",
    )
    .await;
    add_hits(db, k, id, &[IssueFk(&f.a_removed, "Removed Issue Guy")]).await;
    let id = add_entity(db, k.table, 6, "ghost", "Ghost", "ghost").await;
    add_hits(
        db,
        k,
        id,
        &[IssueFk(&f.r1, "Ghost"), SeriesFk(f.s_removed, "Ghost")],
    )
    .await;
    let id = add_entity(db, k.table, 7, "rollup-only", "Rollup Only", "rollup only").await;
    add_hits(db, k, id, &[SeriesName(f.s_alpha, "rollup only")]).await;
    let id = add_entity(
        db,
        k.table,
        8,
        "hidden-rollup",
        "Hidden Rollup",
        "hidden rollup",
    )
    .await;
    add_hits(db, k, id, &[SeriesFk(f.s_hidden, "Hidden Rollup")]).await;
    let id = add_entity(
        db,
        k.table,
        9,
        "malformed-guy",
        "Malformed Guy",
        "malformed guy",
    )
    .await;
    add_hits(db, k, id, &[IssueFk(&f.a_malformed, "Malformed Guy")]).await;
    let id = add_entity(db, k.table, 10, "cee-hero", "Cee Hero", "cee hero").await;
    add_hits(
        db,
        k,
        id,
        &[
            IssueFk(&c[0], "Cee Hero"),
            IssueName(&c[1], "Cee Hero"),
            SeriesFk(f.s_cee, "Cee Hero"),
        ],
    )
    .await;
    add_entity(db, k.table, 11, "orphan", "Orphan", "orphan").await;
    // Three entities whose `lower(name)` ties: the slug breaks the tie.
    let id = add_entity(db, k.table, 12, "robin-a", "Robin", "robin").await;
    add_hits(
        db,
        k,
        id,
        &[IssueFk(&a[0], "Robin"), SeriesFk(f.s_alpha, "Robin")],
    )
    .await;
    let id = add_entity(db, k.table, 13, "robin-b", "ROBIN", "robin (earth-2)").await;
    add_hits(db, k, id, &[IssueFk(&f.a_mature, "Robin (Earth-2)")]).await;
    let id = add_entity(db, k.table, 14, "robin-c", "robin", "robin (earth-3)").await;
    add_hits(
        db,
        k,
        id,
        &[
            IssueFk(&a[1], "Robin (Earth-3)"),
            IssueFk(&ad[0], "Robin (Earth-3)"),
        ],
    )
    .await;
    // Case / diacritic neighbours.
    let id = add_entity(db, k.table, 15, "elan-accent", "Élan", "élan").await;
    add_hits(db, k, id, &[IssueName(&a[2], "élan")]).await;
    let id = add_entity(db, k.table, 16, "elan", "Elan", "elan").await;
    add_hits(db, k, id, &[IssueFk(&a[2], "Elan")]).await;
    let id = add_entity(db, k.table, 17, "elan-lower", "elan", "elan (lower)").await;
    add_hits(
        db,
        k,
        id,
        &[
            IssueFk(&a[2], "elan (lower)"),
            IssueFk(&c[1], "elan (lower)"),
        ],
    )
    .await;
    // `#` bucket.
    let id = add_entity(db, k.table, 18, "42-agent", "42 Agent", "42 agent").await;
    add_hits(db, k, id, &[IssueFk(&a[0], "42 Agent")]).await;
    let id = add_entity(db, k.table, 19, "underscore", "_Underscore", "_underscore").await;
    add_hits(db, k, id, &[IssueName(&a[1], "_Underscore")]).await;
    let id = add_entity(db, k.table, 20, "zed", "Zed", "zed").await;
    add_hits(
        db,
        k,
        id,
        &[
            IssueFk(&c[1], "Zed"),
            IssueFk(&a[2], "Zed"),
            IssueFk(&ad[1], "Zed"),
            SeriesName(f.s_adult, "Zed"),
        ],
    )
    .await;
    // `%` / `_` in a name: the `q` filter is a literal substring.
    let id = add_entity(db, k.table, 21, "percent", "100% Pure", "100% pure").await;
    add_hits(db, k, id, &[IssueFk(&a[0], "100% Pure")]).await;
    // Fillers: every 4th hidden-library-only, every 5th over-cap-only, so
    // the offset pages and long walks differ per caller.
    for n in 1..=fillers {
        let name = format!("Filler {n:03}");
        let slug = format!("filler-{n:03}");
        let id = add_entity(db, k.table, 100 + n, &slug, &name, &name.to_lowercase()).await;
        let issue = if n % 4 == 0 {
            &h[1]
        } else if n % 5 == 0 {
            &f.a_mature
        } else {
            &a[(n % 3) as usize]
        };
        add_hits(db, k, id, &[IssueFk(issue, &name)]).await;
    }
}

const ENTITY_FILTERS: &[&str] = &[
    "",
    "starts_with=a",
    "starts_with=B",
    "starts_with=e",
    "starts_with=r",
    "starts_with=z",
    "starts_with=%23",
    "q=rob",
    "q=an",
    "q=ELAN",
    "q=%C3%A9",
    "q=%25",
    "q=_",
    "q=filler%2000",
    "starts_with=r&q=robin",
    "starts_with=f&q=1",
];

/// Snapshot of every filter × limit × caller walk of one entity index,
/// plus the error shapes.
async fn entity_index_snapshot(app: &TestApp, f: &Fixture, path: &str) -> Json {
    let mut out = serde_json::Map::new();
    for user in f.callers() {
        for filter in ENTITY_FILTERS {
            for limit in [2u32, 5, 100] {
                let mut base = format!("/api/{path}?limit={limit}");
                if !filter.is_empty() {
                    base.push('&');
                    base.push_str(filter);
                }
                let pages = walk(app, &base, user).await;
                out.insert(format!("{} {base}", user.label), pages);
            }
        }
        // Default limit (60) and an out-of-range limit (clamped to 100).
        for base in [
            format!("/api/{path}"),
            format!("/api/{path}?limit=0"),
            format!("/api/{path}?limit=500"),
        ] {
            let pages = walk(app, &base, user).await;
            out.insert(format!("{} {base}", user.label), pages);
        }
        // Error shapes are unchanged.
        for bad in [
            format!("/api/{path}?cursor=bogus"),
            format!("/api/{path}?starts_with=zz"),
        ] {
            let (status, body) = get_json(app, &bad, user).await;
            out.insert(
                format!("{} {bad}", user.label),
                json!({"status": status.as_u16(), "body": body}),
            );
        }
    }
    Json::Object(out)
}

/// `<updated>` timestamps are wall-clock; strip them from OPDS v1 XML.
fn strip_updated(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<updated>") {
        out.push_str(&rest[..i]);
        match rest[i..].find("</updated>") {
            Some(j) => rest = &rest[i + j + "</updated>".len()..],
            None => {
                rest = &rest[i..];
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn strip_v2_modified(v: &mut Json) {
    match v {
        Json::Object(m) => {
            m.remove("modified");
            for (_, x) in m.iter_mut() {
                strip_v2_modified(x);
            }
        }
        Json::Array(a) => a.iter_mut().for_each(strip_v2_modified),
        _ => {}
    }
}

/// OPDS v1 + v2 navigation feeds (offset paging over the same list query).
async fn opds_snapshot(app: &TestApp, f: &Fixture, path: &str) -> Json {
    let mut out = serde_json::Map::new();
    for user in f.callers() {
        for page in [1u32, 2, 3] {
            let uri = if page == 1 {
                format!("/opds/v1/{path}")
            } else {
                format!("/opds/v1/{path}?page={page}")
            };
            let (status, bytes) = get(app, &uri, user).await;
            let xml = strip_updated(&String::from_utf8(bytes).unwrap());
            out.insert(
                format!("{} {uri}", user.label),
                json!({"status": status.as_u16(), "body": xml}),
            );
            let uri = if page == 1 {
                format!("/opds/v2/{path}")
            } else {
                format!("/opds/v2/{path}?page={page}")
            };
            let (status, mut body) = get_json(app, &uri, user).await;
            strip_v2_modified(&mut body);
            out.insert(
                format!("{} {uri}", user.label),
                json!({"status": status.as_u16(), "body": body}),
            );
        }
    }
    Json::Object(out)
}

fn settings() -> insta::Settings {
    let mut s = insta::Settings::clone_current();
    s.set_sort_maps(true);
    s
}

#[tokio::test]
async fn characters_index_is_unchanged() {
    let app = TestApp::spawn().await;
    let f = fixture(&app).await;
    seed_name_kind(&f, &CHARACTER, 60).await;
    let api = entity_index_snapshot(&app, &f, "characters").await;
    let opds = opds_snapshot(&app, &f, "characters").await;
    settings().bind(|| {
        insta::assert_json_snapshot!("characters_api", api);
        insta::assert_json_snapshot!("characters_opds", opds);
    });
}

#[tokio::test]
async fn teams_index_is_unchanged() {
    let app = TestApp::spawn().await;
    let f = fixture(&app).await;
    seed_name_kind(&f, &TEAM, 6).await;
    let api = entity_index_snapshot(&app, &f, "teams").await;
    let opds = opds_snapshot(&app, &f, "teams").await;
    settings().bind(|| {
        insta::assert_json_snapshot!("teams_api", api);
        insta::assert_json_snapshot!("teams_opds", opds);
    });
}

#[tokio::test]
async fn locations_index_is_unchanged() {
    let app = TestApp::spawn().await;
    let f = fixture(&app).await;
    seed_name_kind(&f, &LOCATION, 6).await;
    let api = entity_index_snapshot(&app, &f, "locations").await;
    settings().bind(|| {
        insta::assert_json_snapshot!("locations_api", api);
    });
}

#[tokio::test]
async fn arcs_index_is_unchanged() {
    let app = TestApp::spawn().await;
    let f = fixture(&app).await;
    let db = &f.db;
    async fn ia(db: &DatabaseConnection, issue: &str, arc: Uuid, pos: Option<i32>) {
        exec(
            db,
            "INSERT INTO issue_arcs (issue_id, arc_id, position_in_arc) VALUES ($1, $2, $3)",
            vec![issue.into(), arc.into(), pos.into()],
        )
        .await;
    }
    async fn sa(db: &DatabaseConnection, series: Uuid, arc: Uuid) {
        exec(
            db,
            "INSERT INTO series_arcs (series_id, arc_id) VALUES ($1, $2)",
            vec![series.into(), arc.into()],
        )
        .await;
    }
    let (a, h, c, ad) = (&f.a, &f.h, &f.c, &f.ad);
    let id = add_entity(db, "story_arc", 1, "knightfall", "Knightfall", "knightfall").await;
    for (i, issue) in [&a[0], &a[1], &f.a_mature, &h[0], &c[0]].iter().enumerate() {
        ia(db, issue, id, Some(i as i32 + 1)).await;
    }
    sa(db, f.s_alpha, id).await;
    let id = add_entity(db, "story_arc", 2, "hidden-war", "Hidden War", "hidden war").await;
    ia(db, &h[1], id, None).await;
    sa(db, f.s_hidden, id).await;
    let id = add_entity(
        db,
        "story_arc",
        3,
        "mature-saga",
        "Mature Saga",
        "mature saga",
    )
    .await;
    ia(db, &f.a_mature, id, None).await;
    let id = add_entity(
        db,
        "story_arc",
        4,
        "rated-series-arc",
        "Rated Series Arc",
        "rated series arc",
    )
    .await;
    ia(db, &ad[0], id, None).await;
    // `series_arcs` follows series visibility even for the capped caller.
    let id = add_entity(
        db,
        "story_arc",
        5,
        "adult-series-arc",
        "Adult Series Arc",
        "adult series arc",
    )
    .await;
    sa(db, f.s_adult, id).await;
    let id = add_entity(
        db,
        "story_arc",
        6,
        "series-only-arc",
        "Series Only Arc",
        "series only arc",
    )
    .await;
    sa(db, f.s_alpha, id).await;
    let id = add_entity(
        db,
        "story_arc",
        7,
        "removed-arc",
        "Removed Arc",
        "removed arc",
    )
    .await;
    ia(db, &f.a_removed, id, None).await;
    ia(db, &f.r1, id, None).await;
    sa(db, f.s_removed, id).await;
    let id = add_entity(
        db,
        "story_arc",
        8,
        "malformed-arc",
        "Malformed Arc",
        "malformed arc",
    )
    .await;
    ia(db, &f.a_malformed, id, None).await;
    add_entity(db, "story_arc", 9, "orphan-arc", "Orphan Arc", "orphan arc").await;
    let id = add_entity(db, "story_arc", 10, "event-a", "Event", "event").await;
    ia(db, &a[2], id, None).await;
    let id = add_entity(db, "story_arc", 11, "event-b", "EVENT", "event (2)").await;
    ia(db, &f.a_mature, id, None).await;
    let id = add_entity(db, "story_arc", 12, "event-c", "event", "event (3)").await;
    ia(db, &c[1], id, None).await;
    ia(db, &a[0], id, None).await;
    let id = add_entity(db, "story_arc", 13, "eclipse-accent", "Éclipse", "éclipse").await;
    ia(db, &a[1], id, None).await;
    let id = add_entity(db, "story_arc", 14, "1-shot", "1-Shot", "1-shot").await;
    ia(db, &c[1], id, None).await;
    let id = add_entity(db, "story_arc", 15, "cee-arc", "Cee Arc", "cee arc").await;
    sa(db, f.s_cee, id).await;
    for n in 1..=6u128 {
        let name = format!("Arc Filler {n:03}");
        let id = add_entity(
            db,
            "story_arc",
            100 + n,
            &format!("arc-filler-{n:03}"),
            &name,
            &name.to_lowercase(),
        )
        .await;
        let issue = if n % 3 == 0 {
            &h[0]
        } else {
            &a[(n % 3) as usize]
        };
        ia(db, issue, id, None).await;
    }
    let _ = (f.s_empty, f.s_cee);
    let api = entity_index_snapshot(&app, &f, "arcs").await;
    let opds = opds_snapshot(&app, &f, "arcs").await;
    settings().bind(|| {
        insta::assert_json_snapshot!("arcs_api", api);
        insta::assert_json_snapshot!("arcs_opds", opds);
    });
}

#[tokio::test]
async fn publishers_index_is_unchanged() {
    let app = TestApp::spawn().await;
    let f = fixture(&app).await;
    let db = &f.db;
    // Alpha links Acme by FK; Cee names it (`acme press `) without one.
    let acme = add_entity(db, "publisher", 1, "acme-press", "Acme Press", "acme press").await;
    exec(
        db,
        "UPDATE series SET publisher_id = $1 WHERE id = $2",
        vec![acme.into(), f.s_alpha.into()],
    )
    .await;
    add_entity(
        db,
        "publisher",
        2,
        "adult-press",
        "Adult Press",
        "adult press",
    )
    .await;
    add_entity(
        db,
        "publisher",
        3,
        "ghost-press",
        "Ghost Press",
        "ghost press",
    )
    .await;
    add_entity(
        db,
        "publisher",
        4,
        "secret-press",
        "Secret Press",
        "secret press",
    )
    .await;
    add_entity(
        db,
        "publisher",
        5,
        "lonely-press",
        "Lonely Press",
        "lonely press",
    )
    .await;
    add_entity(
        db,
        "publisher",
        6,
        "orphan-press",
        "Orphan Press",
        "orphan press",
    )
    .await;
    // A series with no publisher text but an FK, and one whose text
    // names no entity.
    let extra = add_entity(
        db,
        "publisher",
        7,
        "fk-only-press",
        "FK Only Press",
        "fk only press",
    )
    .await;
    exec(
        db,
        "UPDATE series SET publisher = NULL, publisher_id = $1 WHERE id = $2",
        vec![extra.into(), f.s_empty.into()],
    )
    .await;
    exec(
        db,
        "UPDATE series SET publisher = 'Nobody Press' WHERE id = $1",
        vec![f.s_hidden.into()],
    )
    .await;
    let _ = (f.s_adult, f.s_removed, f.s_cee);
    let api = entity_index_snapshot(&app, &f, "publishers").await;
    let opds = opds_snapshot(&app, &f, "publishers").await;
    settings().bind(|| {
        insta::assert_json_snapshot!("publishers_api", api);
        insta::assert_json_snapshot!("publishers_opds", opds);
    });
}

// ───────── creators + people ─────────

async fn issue_credit(db: &DatabaseConnection, issue: &str, role: &str, person: &str) {
    exec(
        db,
        "INSERT INTO issue_credits (issue_id, role, person, person_id) \
         VALUES ($1, $2, $3, (SELECT id FROM person WHERE normalized_name = btrim(lower($3))))",
        vec![issue.into(), role.into(), person.into()],
    )
    .await;
}

async fn series_credit(db: &DatabaseConnection, series: Uuid, role: &str, person: &str) {
    exec(
        db,
        "INSERT INTO series_credits (series_id, role, person, person_id) \
         VALUES ($1, $2, $3, (SELECT id FROM person WHERE normalized_name = btrim(lower($3))))",
        vec![series.into(), role.into(), person.into()],
    )
    .await;
}

async fn person(db: &DatabaseConnection, slug: &str, name: &str) {
    exec(
        db,
        "INSERT INTO person (slug, name, normalized_name) VALUES ($1, $2, btrim(lower($2)))",
        vec![slug.into(), name.into()],
    )
    .await;
}

async fn seed_credits(f: &Fixture) {
    let db = &f.db;
    let (a, h, c, ad) = (&f.a, &f.h, &f.c, &f.ad);
    for (slug, name) in [
        ("stan-lee", "Stan Lee"),
        ("jack-kirby", "Jack Kirby"),
        ("mature-artist", "Mature Artist"),
        ("elodie", "Elodie"),
        ("elodie-accent", "Élodie"),
        ("ann", "Ann"),
        ("writer-042", "Writer 042"),
    ] {
        person(db, slug, name).await;
    }
    // Two raw spellings of one person: two index rows, one slug.
    issue_credit(db, &a[0], "writer", "Stan Lee").await;
    issue_credit(db, &a[1], "writer", "Stan Lee").await;
    series_credit(db, f.s_alpha, "editor", "Stan Lee").await;
    series_credit(db, f.s_alpha, "writer", "Stan Lee").await;
    issue_credit(db, &h[0], "writer", "stan lee").await;
    issue_credit(db, &c[0], "writer", "stan lee").await;
    // Visible + over-cap + over-cap-series credits, plus a rollup row.
    issue_credit(db, &a[0], "penciller", "Jack Kirby").await;
    issue_credit(db, &f.a_mature, "penciller", "Jack Kirby").await;
    issue_credit(db, &ad[0], "cover_artist", "Jack Kirby").await;
    series_credit(db, f.s_alpha, "penciller", "Jack Kirby").await;
    series_credit(db, f.s_adult, "cover_artist", "Jack Kirby").await;
    issue_credit(db, &h[0], "writer", "Hidden Writer").await;
    series_credit(db, f.s_hidden, "writer", "Hidden Writer").await;
    // Rollup row whose only issue credit is over the cap.
    issue_credit(db, &f.a_mature, "artist", "Mature Artist").await;
    series_credit(db, f.s_alpha, "artist", "Mature Artist").await;
    // Rollup-only (no issue credit at all).
    series_credit(db, f.s_alpha, "editor", "Rollup Editor").await;
    issue_credit(db, &ad[0], "writer", "Series Rated").await;
    series_credit(db, f.s_adult, "writer", "Series Rated").await;
    issue_credit(db, &f.a_removed, "writer", "Removed Person").await;
    series_credit(db, f.s_removed, "writer", "Removed Person").await;
    issue_credit(db, &f.r1, "writer", "Removed Person").await;
    issue_credit(db, &f.a_malformed, "writer", "Malformed Person").await;
    issue_credit(db, &a[2], "inker", "Élodie").await;
    issue_credit(db, &a[2], "colorist", "Elodie").await;
    issue_credit(db, &c[1], "colorist", "elodie").await;
    issue_credit(db, &a[0], "letterer", " Ann").await;
    issue_credit(db, &a[1], "letterer", "Ann").await;
    issue_credit(db, &a[1], "inker", "Ann").await;
    issue_credit(db, &c[0], "letterer", "ann").await;
    issue_credit(db, &a[0], "editor", "2000AD Staff").await;
    issue_credit(db, &a[1], "editor", "_Underscore Ed").await;
    issue_credit(db, &c[1], "writer", "Zed Zero").await;
    issue_credit(db, &a[2], "writer", "Zed Zero").await;
    issue_credit(db, &a[0], "writer", "Writer 042").await;
    issue_credit(db, &a[1], "writer", "Writer 0420").await;
    issue_credit(db, &h[1], "writer", "Writer 042x").await;
    issue_credit(db, &a[2], "writer", "100% Writer").await;
    // Credited on every issue of a series as the same role (dedupe).
    for issue in [&a[0], &a[1], &a[2], &f.a_mature] {
        issue_credit(db, issue, "colorist", "Same Role").await;
    }
    series_credit(db, f.s_alpha, "colorist", "Same Role").await;
    for n in 1..=24u32 {
        let name = format!("Creator {n:03}");
        let issue = match n % 4 {
            0 => &h[0],
            1 => &f.a_mature,
            2 => &c[0],
            _ => &a[(n % 3) as usize],
        };
        issue_credit(
            db,
            issue,
            if n % 2 == 0 { "writer" } else { "artist" },
            &name,
        )
        .await;
    }
}

#[tokio::test]
async fn creators_index_is_unchanged() {
    let app = TestApp::spawn().await;
    let f = fixture(&app).await;
    seed_credits(&f).await;
    let mut out = serde_json::Map::new();
    for user in f.callers() {
        for filter in [
            "",
            "starts_with=a",
            "starts_with=E",
            "starts_with=s",
            "starts_with=w",
            "starts_with=z",
            "starts_with=%23",
        ] {
            for limit in [1u32, 3, 100] {
                let mut base = format!("/api/creators?limit={limit}");
                if !filter.is_empty() {
                    base.push('&');
                    base.push_str(filter);
                }
                let pages = walk(&app, &base, user).await;
                out.insert(format!("{} {base}", user.label), pages);
            }
        }
        let base = "/api/creators".to_owned();
        let pages = walk(&app, &base, user).await;
        out.insert(format!("{} {base}", user.label), pages);
        for bad in ["/api/creators?cursor=bogus", "/api/creators?starts_with=zz"] {
            let (status, body) = get_json(&app, bad, user).await;
            out.insert(
                format!("{} {bad}", user.label),
                json!({"status": status.as_u16(), "body": body}),
            );
        }
    }
    let _ = (f.s_empty, f.s_cee);
    settings().bind(|| insta::assert_json_snapshot!("creators_api", Json::Object(out)));
}

#[tokio::test]
async fn people_search_is_unchanged() {
    let app = TestApp::spawn().await;
    let f = fixture(&app).await;
    seed_credits(&f).await;
    let mut out = serde_json::Map::new();
    for user in f.callers() {
        for q in [
            "stan",
            "lee",
            "STAN%20LEE",
            "ann",
            "elodie",
            "%C3%A9lodie",
            "writer%20042",
            "kirby",
            "creator",
            "creator%2001",
            "mature",
            "rollup",
            "removed",
            "hidden",
            "same",
            "%25",
            "_",
            "zzzz",
            "",
        ] {
            for limit in ["", "&limit=1", "&limit=3", "&limit=100", "&limit=0"] {
                let uri = format!("/api/people?q={q}{limit}");
                let (status, body) = get_json(&app, &uri, user).await;
                out.insert(
                    format!("{} {uri}", user.label),
                    json!({"status": status.as_u16(), "body": body}),
                );
            }
        }
    }
    let _ = (f.s_empty, f.s_cee);
    settings().bind(|| insta::assert_json_snapshot!("people_api", Json::Object(out)));
}
