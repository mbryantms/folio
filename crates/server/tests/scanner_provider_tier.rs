//! Roadmap WP-2.5 (decision D4): on rescan of a non-writeback library a
//! file-tier value never replaces a value the user or a provider set.
//!
//! Before this, the scanner honoured only `set_by='user'` pins: a
//! provider-applied summary or credit list was replaced by the ComicInfo
//! values on any content change or force scan, while the provenance row
//! (protected by the writer's guard) kept saying "comicvine".

mod common;

use common::TestApp;
use common::seed::LibrarySeed;
use entity::{field_provenance, issue::Entity as IssueEntity, issue_credit};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::library::scanner;
use std::io::Write;
use std::path::Path;

fn write_cbz(path: &Path, comic_info: &str, unique_marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    png.extend_from_slice(&unique_marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    zw.start_file("ComicInfo.xml", opts).unwrap();
    zw.write_all(comic_info.as_bytes()).unwrap();
    zw.finish().unwrap();
}

fn comicinfo(title: &str, summary: &str, writer: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<ComicInfo>
  <Title>{title}</Title>
  <Series>Secret Warriors</Series>
  <Number>10</Number>
  <Year>2010</Year>
  <Summary>{summary}</Summary>
  <Writer>{writer}</Writer>
  <Publisher>Marvel</Publisher>
</ComicInfo>"#
    )
}

/// Attribute `field` to ComicVine through the real writer (an upsert:
/// the first scan already recorded a file-tier row for it).
async fn pin_provider(app: &TestApp, issue_id: &str, field: server::metadata::MetadataField) {
    server::metadata::writers::write_field_provenance(
        &app.state().db,
        "issue",
        issue_id,
        field,
        server::metadata::writers::SetBy::Provider(server::metadata::identifier::Source::ComicVine),
        None,
    )
    .await
    .unwrap();
}

async fn credits(app: &TestApp, issue_id: &str) -> Vec<(String, String, i32)> {
    let mut rows: Vec<_> = issue_credit::Entity::find()
        .filter(issue_credit::Column::IssueId.eq(issue_id))
        .all(&app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.role, r.person, r.ordinal))
        .collect();
    rows.sort();
    rows
}

#[tokio::test]
async fn rescan_keeps_provider_set_columns_and_junctions() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Secret Warriors (2009)");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("Secret Warriors 010.cbz");
    write_cbz(
        &file,
        &comicinfo("File Title", "File summary", "File Writer"),
        1,
    );
    let lib_id = LibrarySeed::new(tmp.path()).insert(&app.state().db).await;
    let state = app.state();

    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(stats.files_added, 1, "{stats:?}");
    let row = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.summary.as_deref(), Some("File summary"));
    assert_eq!(row.writer.as_deref(), Some("File Writer"));
    let issue_id = row.id.clone();

    // Simulate a provider apply (DB-direct path): summary + credits owned
    // by ComicVine, with the junction row shape the writers produce.
    let mut am: entity::issue::ActiveModel = row.into();
    am.summary = Set(Some("Provider summary".into()));
    am.writer = Set(Some("Provider Writer".into()));
    am.update(&state.db).await.unwrap();
    issue_credit::Entity::delete_many()
        .filter(issue_credit::Column::IssueId.eq(&issue_id))
        .exec(&state.db)
        .await
        .unwrap();
    issue_credit::ActiveModel {
        issue_id: Set(issue_id.clone()),
        role: Set("writer".into()),
        person: Set("Provider Writer".into()),
        person_id: Set(None),
        ordinal: Set(3),
    }
    .insert(&state.db)
    .await
    .unwrap();
    pin_provider(
        &app,
        &issue_id,
        server::metadata::MetadataField::Description,
    )
    .await;
    pin_provider(&app, &issue_id, server::metadata::MetadataField::Credits).await;

    // The file changes on disk (a retag) with different values for the
    // protected fields AND for an unprotected one (title).
    write_cbz(
        &file,
        &comicinfo("New File Title", "Newer file summary", "Newer File Writer"),
        2,
    );
    let stats = scanner::scan_library(&state, lib_id).await.unwrap();
    assert_eq!(stats.files_updated, 1, "{stats:?}");

    let after = IssueEntity::find_by_id(issue_id.clone())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.title.as_deref(),
        Some("New File Title"),
        "unprotected column follows the file"
    );
    assert_eq!(
        after.summary.as_deref(),
        Some("Provider summary"),
        "provider-set summary survives the rescan"
    );
    assert_eq!(
        after.writer.as_deref(),
        Some("Provider Writer"),
        "provider-set credit CSV survives"
    );
    assert_eq!(
        credits(&app, &issue_id).await,
        vec![("writer".to_owned(), "Provider Writer".to_owned(), 3)],
        "provider-written junction rows (with ordinal) are not rebuilt from the CSV"
    );
    let prov = field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityId.eq(&issue_id))
        .filter(field_provenance::Column::Field.eq("description"))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        prov.set_by, "comicvine",
        "provenance still names the provider"
    );

    // A force scan with unchanged bytes must not undo it either.
    scanner::scan_library_with(&state, lib_id, true)
        .await
        .unwrap();
    let again = IssueEntity::find_by_id(issue_id.clone())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.summary.as_deref(), Some("Provider summary"));
    assert_eq!(credits(&app, &issue_id).await.len(), 1);
}

/// File-tier provenance never blocks a rescan: a value the scanner set
/// itself is refreshed from the file as before.
#[tokio::test]
async fn rescan_still_refreshes_file_tier_columns() {
    let app = TestApp::spawn().await;
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("Secret Warriors (2009)");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("Secret Warriors 010.cbz");
    write_cbz(&file, &comicinfo("T1", "S1", "W1"), 1);
    let lib_id = LibrarySeed::new(tmp.path()).insert(&app.state().db).await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.unwrap();
    let row = IssueEntity::find()
        .filter(entity::issue::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    let prov = field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityId.eq(&row.id))
        .filter(field_provenance::Column::Field.eq("description"))
        .one(&state.db)
        .await
        .unwrap()
        .expect("scanner wrote file-tier provenance for the summary");
    assert_eq!(prov.set_by, "comicinfo");

    write_cbz(&file, &comicinfo("T2", "S2", "W2"), 2);
    scanner::scan_library(&state, lib_id).await.unwrap();
    let after = IssueEntity::find_by_id(row.id.clone())
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.summary.as_deref(), Some("S2"));
    assert_eq!(after.writer.as_deref(), Some("W2"));
    assert_eq!(
        credits(&app, &row.id).await,
        vec![("writer".to_owned(), "W2".to_owned(), 0)]
    );
}
