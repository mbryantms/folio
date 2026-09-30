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
        ext: "jpg",
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
