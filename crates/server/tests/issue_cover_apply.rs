//! `writers::apply_cover` / `writers::set_issue_variants` against the
//! cover-slot uniqueness rule (`m20270218_000001_issue_cover_active_unique`).
//!
//! The M0 schema had a non-partial `UNIQUE (issue_id, kind, ordinal)`, so
//! an inactive row still occupied the slot: replacing an active primary
//! (deactivate + insert) always hit the constraint, and so did applying a
//! provider cover to any scanned issue, because the post-scan phash worker
//! leaves an inactive `archive_extracted` `primary/0` row behind. These
//! tests pin the fixed behaviour: exactly one active primary pointing at
//! the new file, atomic replace (a failed insert leaves the previous
//! cover active and no orphan file on disk).

mod common;

use common::TestApp;
use common::seed::{IssueSeed, SeriesSeed, seed_library};
use entity::issue_cover;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, Database, DatabaseConnection, EntityTrait,
    QueryFilter, Set,
};
use server::metadata::provider::VariantCoverCandidate;
use server::metadata::writers::{self, CoverOverwritePolicy, CoverWrite, SetBy};
use server::metadata::{Identifier, Source};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Not a decodable image — `apply_cover` soft-skips the phash step, which
/// is irrelevant here. The bytes only need to land on disk.
const COVER_BYTES: &[u8] = b"\x89PNG\r\n\x1a\nnot-a-real-png";

struct Fixture {
    _app: TestApp,
    _root: tempfile::TempDir,
    db: DatabaseConnection,
    data_path: PathBuf,
    issue_id: String,
}

async fn fixture() -> Fixture {
    let app = TestApp::spawn().await;
    let db = Database::connect(&app.db_url).await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let lib = seed_library(&db, root.path()).await;
    let series_id = SeriesSeed::new(lib, "Invincible").insert(&db).await;
    let file = root.path().join("invincible-001.cbz");
    let issue_id = IssueSeed::new(lib, series_id, &file, b"issue-payload", 1.0)
        .insert(&db)
        .await;
    let data_path = app.state().cfg().data_path.clone();
    Fixture {
        _app: app,
        _root: root,
        db,
        data_path,
        issue_id,
    }
}

fn provider_write<'a>(issue_id: &'a str, ident: &'a Identifier, url: &'a str) -> CoverWrite<'a> {
    CoverWrite {
        issue_id,
        kind: "primary",
        ordinal: 0,
        identifier: Some(ident),
        source_url: Some(url),
        variant_label: None,
        variant_artist_person_id: None,
        bytes: COVER_BYTES,
        width: None,
        height: None,
    }
}

fn cv(id: &str) -> Identifier {
    Identifier {
        source: Source::ComicVine,
        id: id.into(),
        url: None,
    }
}

async fn primary_rows(db: &DatabaseConnection, issue_id: &str) -> Vec<issue_cover::Model> {
    issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(issue_id))
        .filter(issue_cover::Column::Kind.eq("primary"))
        .filter(issue_cover::Column::Ordinal.eq(0))
        .all(db)
        .await
        .unwrap()
}

fn cover_files(data_path: &Path, issue_id: &str) -> Vec<String> {
    let dir = data_path.join(format!("thumbs/issues/{issue_id}/covers"));
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = rd
        .map(|e| {
            format!(
                "thumbs/issues/{issue_id}/covers/{}",
                e.unwrap().file_name().to_string_lossy()
            )
        })
        .collect();
    names.sort();
    names
}

/// Make the next `issue_cover` insert whose `source_url` contains `boom`
/// fail, so the replace path's failure branch runs against a real DB.
async fn install_insert_failure_trigger(db: &DatabaseConnection) {
    db.execute_unprepared(
        "CREATE FUNCTION issue_cover_boom() RETURNS trigger AS $$ \
         BEGIN \
           IF NEW.source_url LIKE '%boom%' THEN \
             RAISE EXCEPTION 'injected issue_cover insert failure'; \
           END IF; \
           RETURN NEW; \
         END $$ LANGUAGE plpgsql",
    )
    .await
    .unwrap();
    db.execute_unprepared(
        "CREATE TRIGGER issue_cover_boom BEFORE INSERT ON issue_cover \
         FOR EACH ROW EXECUTE FUNCTION issue_cover_boom()",
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn always_policy_replaces_an_existing_active_primary() {
    let f = fixture().await;
    let ident = cv("1001");
    let first = writers::apply_cover(
        &f.db,
        &f.data_path,
        provider_write(&f.issue_id, &ident, "https://cv.example/a.jpg"),
        CoverOverwritePolicy::Always,
    )
    .await
    .expect("first apply")
    .expect("first apply writes a row");

    let second = writers::apply_cover(
        &f.db,
        &f.data_path,
        provider_write(&f.issue_id, &ident, "https://cv.example/b.jpg"),
        CoverOverwritePolicy::Always,
    )
    .await
    .expect("replacing an active primary must succeed")
    .expect("Always policy writes a row");
    assert_ne!(first, second);

    let rows = primary_rows(&f.db, &f.issue_id).await;
    let active: Vec<_> = rows.iter().filter(|r| r.is_active).collect();
    assert_eq!(active.len(), 1, "exactly one active primary: {rows:?}");
    assert_eq!(active[0].id, second);
    assert_eq!(
        active[0].source_url.as_deref(),
        Some("https://cv.example/b.jpg")
    );
    assert!(f.data_path.join(&active[0].local_path).is_file());
    let prev = rows
        .iter()
        .find(|r| r.id == first)
        .expect("previous row kept");
    assert!(
        !prev.is_active,
        "previous primary is deactivated, not deleted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn always_policy_applies_over_an_inactive_archive_extracted_row() {
    let f = fixture().await;
    // What the post-scan phash worker leaves on every scanned issue.
    let archive_row = server::metadata::phash::upsert_archive_cover_hashes_from_parts(
        &f.db,
        &f.issue_id,
        &format!("thumbs/issues/{}/cover.webp", f.issue_id),
        (1, 2, 3),
        600,
        900,
    )
    .await
    .unwrap();

    let ident = cv("2002");
    let applied = writers::apply_cover(
        &f.db,
        &f.data_path,
        provider_write(&f.issue_id, &ident, "https://cv.example/c.jpg"),
        CoverOverwritePolicy::Always,
    )
    .await
    .expect("an inactive row must not block the primary slot")
    .expect("Always policy writes a row");

    let rows = primary_rows(&f.db, &f.issue_id).await;
    assert_eq!(rows.len(), 2, "archive hash row + provider row: {rows:?}");
    let active: Vec<_> = rows.iter().filter(|r| r.is_active).collect();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, applied);
    assert_eq!(active[0].source_provider.as_deref(), Some("comicvine"));
    assert!(f.data_path.join(&active[0].local_path).is_file());
    let archive = rows.iter().find(|r| r.id == archive_row).unwrap();
    assert!(!archive.is_active);
    assert_eq!(archive.phash, Some(1), "matcher side-channel untouched");

    // A later rescan's hash upsert still finds the archive row next to
    // the provider row and updates it in place.
    let again = server::metadata::phash::upsert_archive_cover_hashes_from_parts(
        &f.db,
        &f.issue_id,
        &format!("thumbs/issues/{}/cover.webp", f.issue_id),
        (4, 5, 6),
        600,
        900,
    )
    .await
    .unwrap();
    assert_eq!(again, archive_row);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn when_missing_policy_fills_an_issue_with_only_inactive_rows() {
    let f = fixture().await;
    server::metadata::phash::upsert_archive_cover_hashes_from_parts(
        &f.db,
        &f.issue_id,
        "",
        (1, 2, 3),
        600,
        900,
    )
    .await
    .unwrap();
    let ident = cv("3003");
    let applied = writers::apply_cover(
        &f.db,
        &f.data_path,
        provider_write(&f.issue_id, &ident, "https://cv.example/d.jpg"),
        CoverOverwritePolicy::WhenMissing,
    )
    .await
    .expect("apply succeeds");
    assert!(applied.is_some(), "no active primary → WhenMissing writes");

    // A second WhenMissing apply is now policy-denied (active row exists).
    let denied = writers::apply_cover(
        &f.db,
        &f.data_path,
        provider_write(&f.issue_id, &ident, "https://cv.example/e.jpg"),
        CoverOverwritePolicy::WhenMissing,
    )
    .await
    .unwrap();
    assert!(denied.is_none());
    assert_eq!(cover_files(&f.data_path, &f.issue_id).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_replace_keeps_previous_cover_active_and_removes_new_file() {
    let f = fixture().await;
    let ident = cv("4004");
    let first = writers::apply_cover(
        &f.db,
        &f.data_path,
        provider_write(&f.issue_id, &ident, "https://cv.example/ok.jpg"),
        CoverOverwritePolicy::Always,
    )
    .await
    .unwrap()
    .unwrap();
    let files_before = cover_files(&f.data_path, &f.issue_id);
    assert_eq!(files_before.len(), 1);

    install_insert_failure_trigger(&f.db).await;
    let err = writers::apply_cover(
        &f.db,
        &f.data_path,
        provider_write(&f.issue_id, &ident, "https://cv.example/boom.jpg"),
        CoverOverwritePolicy::Always,
    )
    .await
    .expect_err("injected insert failure surfaces as an error");
    assert!(err.to_string().contains("issue_cover insert"), "{err}");

    // Deactivate rolled back with the failed insert.
    let rows = primary_rows(&f.db, &f.issue_id).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].id, first);
    assert!(
        rows[0].is_active,
        "previous primary is still the active one"
    );
    // The file written for the failed insert was cleaned up.
    assert_eq!(cover_files(&f.data_path, &f.issue_id), files_before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_slot_is_unique_but_inactive_rows_may_repeat() {
    let f = fixture().await;
    let row = |active: bool| issue_cover::ActiveModel {
        id: Set(Uuid::now_v7()),
        issue_id: Set(f.issue_id.clone()),
        kind: Set("primary".into()),
        ordinal: Set(0),
        source_provider: Set(Some("comicvine".into())),
        source_external_id: Set(None),
        source_url: Set(None),
        variant_label: Set(None),
        variant_artist_person_id: Set(None),
        local_path: Set(String::new()),
        width: Set(None),
        height: Set(None),
        phash: Set(None),
        dhash: Set(None),
        ahash: Set(None),
        fetched_at: Set(chrono::Utc::now().fixed_offset()),
        is_active: Set(active),
    };
    row(false).insert(&f.db).await.unwrap();
    row(false).insert(&f.db).await.unwrap();
    row(true).insert(&f.db).await.unwrap();
    assert!(
        row(true).insert(&f.db).await.is_err(),
        "a second active row in the same slot must violate the partial index"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_variant_replace_keeps_the_previous_set() {
    let f = fixture().await;
    // Unresolvable URLs: the download soft-fails, so each variant lands as
    // a metadata-only (hotlink) row without any network.
    let variant = |url: &str| VariantCoverCandidate {
        label: Some("Variant".into()),
        artist_name: None,
        identifiers: Vec::new(),
        image_url: Some(url.into()),
    };
    let first = [
        variant("https://variant.invalid/one.jpg"),
        variant("https://variant.invalid/two.jpg"),
    ];
    let n = writers::set_issue_variants(
        &f.db,
        &f.data_path,
        &f.issue_id,
        &first,
        SetBy::Provider(Source::ComicVine),
    )
    .await
    .unwrap();
    assert_eq!(n, 2);

    install_insert_failure_trigger(&f.db).await;
    let second = [
        variant("https://variant.invalid/three.jpg"),
        variant("https://variant.invalid/boom.jpg"),
    ];
    writers::set_issue_variants(
        &f.db,
        &f.data_path,
        &f.issue_id,
        &second,
        SetBy::Provider(Source::ComicVine),
    )
    .await
    .expect_err("injected insert failure surfaces as an error");

    let mut urls: Vec<String> = issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(&f.issue_id))
        .filter(issue_cover::Column::Kind.eq("variant"))
        .all(&f.db)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|r| r.source_url)
        .collect();
    urls.sort();
    assert_eq!(
        urls,
        vec![
            "https://variant.invalid/one.jpg".to_owned(),
            "https://variant.invalid/two.jpg".to_owned(),
        ],
        "the failed replace rolled back — the previous variant set survives"
    );
}

// ───────── SE-6 / WP-6.3: provider cover bytes are magic-sniffed ─────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_image_cover_bytes_are_refused_before_anything_is_persisted() {
    let f = fixture().await;
    let ident = cv("6001");
    for junk in [
        &b"<!doctype html><html><body>404</body></html>"[..],
        &b"{\"error\":\"not found\"}"[..],
        &b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"[..],
        &b""[..],
    ] {
        let mut write = provider_write(&f.issue_id, &ident, "https://cv.example/cover.jpg");
        write.bytes = junk;
        let err = writers::apply_cover(&f.db, &f.data_path, write, CoverOverwritePolicy::Always)
            .await
            .expect_err("non-image bytes must be refused");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
    assert!(primary_rows(&f.db, &f.issue_id).await.is_empty());
    assert!(cover_files(&f.data_path, &f.issue_id).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stored_cover_extension_comes_from_the_bytes_not_the_url() {
    let f = fixture().await;
    let ident = cv("6002");
    // PNG bytes behind a `.jpg` URL: the file (and therefore the served
    // MIME) must say png.
    let id = writers::apply_cover(
        &f.db,
        &f.data_path,
        provider_write(&f.issue_id, &ident, "https://cv.example/actually-a-png.jpg"),
        CoverOverwritePolicy::Always,
    )
    .await
    .unwrap()
    .unwrap();
    let row = primary_rows(&f.db, &f.issue_id)
        .await
        .into_iter()
        .find(|r| r.id == id)
        .unwrap();
    assert!(
        row.local_path.ends_with(".png"),
        "local_path {:?} should carry the sniffed extension",
        row.local_path
    );
}

#[tokio::test]
async fn cover_fetch_refuses_plain_http_before_any_network() {
    // ComicVine + Metron cover URLs are https; a plain-http URL is refused at
    // pre-flight (no DNS, no connect), so this needs no network.
    let err = writers::fetch_cover_bytes("http://comicvine.gamespot.com/a/uploads/x.jpg")
        .await
        .expect_err("http cover URL must be refused");
    assert!(
        matches!(
            err,
            writers::CoverFetchError::Fetch(server::util::ssrf::FetchBytesError::Ssrf(
                server::util::ssrf::SsrfError::SchemeNotHttps
            ))
        ),
        "{err:?}"
    );
}

#[test]
fn sniff_cover_accepts_images_only() {
    assert!(writers::sniff_cover(COVER_BYTES).is_some());
    assert!(writers::sniff_cover(&[0xFF, 0xD8, 0xFF, 0xE0]).is_some());
    assert!(writers::sniff_cover(b"<html>").is_none());
    assert!(writers::sniff_cover(b"").is_none());
}

async fn admin_cookie(app: &TestApp) -> String {
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode, header};
    use tower::ServiceExt;
    let resp = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/auth/local/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"cover-admin@example.com","password":"correctly-horse-battery"}"#,
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
        .map(|c| c.split(';').next().unwrap_or("").to_owned())
        .collect::<Vec<_>>()
        .join("; ")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cover_endpoint_serves_sniffed_mime_and_refuses_non_image_files() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    let f = fixture().await;
    let cookie = admin_cookie(&f._app).await;
    let ident = cv("6003");
    let id = writers::apply_cover(
        &f.db,
        &f.data_path,
        provider_write(&f.issue_id, &ident, "https://cv.example/served.jpg"),
        CoverOverwritePolicy::Always,
    )
    .await
    .unwrap()
    .unwrap();
    let get = || async {
        f._app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/issues/{}/covers/{id}", f.issue_id))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    };
    let resp = get().await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(resp.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");

    // A legacy row whose file isn't an image (stored before the write-side
    // sniff existed) is refused rather than served as image/*.
    let row = primary_rows(&f.db, &f.issue_id)
        .await
        .into_iter()
        .find(|r| r.id == id)
        .unwrap();
    std::fs::write(
        f.data_path.join(&row.local_path),
        b"<html>not a cover</html>",
    )
    .unwrap();
    assert_eq!(get().await.status(), StatusCode::NOT_FOUND);
}
