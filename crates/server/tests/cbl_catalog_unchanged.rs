//! A catalog list whose stored blob SHA equals the catalog index's SHA is
//! checked and refreshed without downloading the blob.

mod common;

use chrono::Utc;
use common::TestApp;
use entity::{catalog_source, cbl_entry, cbl_list};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};
use server::cbl::fetch::REQUEST_BUDGET;
use server::cbl::import::{self, RefreshTrigger};
use server::cbl::refresh;
use uuid::Uuid;

const PATH: &str = "Marvel/Unchanged List.cbl";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const XML: &str = r#"<?xml version="1.0"?>
<ReadingList><Name>Unchanged</Name><Books>
<Book Series="Nowhere" Number="1" Volume="2020" Year="2020" />
<Book Series="Nowhere" Number="2" Volume="2020" Year="2020" />
</Books></ReadingList>
"#;

/// The catalog hosts are constants, so a test cannot serve the blob. The
/// catalog source here names a repository that does not exist, with a
/// cached index that is fresh (no tree request) and lists the list's path
/// under the SHA the list already stores. Before the index SHA was
/// compared ahead of the download, check and refresh downloaded the blob
/// first and only then compared SHAs: against this repository that
/// download fails (`404`, or a transport error without network), so both
/// calls failed. Now neither makes a network request: both succeed and
/// report nothing changed upstream, the refresh re-matching the stored
/// entries.
#[tokio::test]
async fn unchanged_catalog_sha_skips_the_blob_download() {
    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    let now = Utc::now().fixed_offset();

    let source_id = Uuid::now_v7();
    catalog_source::ActiveModel {
        id: Set(source_id),
        display_name: Set("Test catalog".into()),
        github_owner: Set(format!("folio-test-owner-{source_id}")),
        github_repo: Set("no-such-repository".into()),
        github_branch: Set("main".into()),
        enabled: Set(true),
        last_indexed_at: Set(Some(now)),
        index_etag: Set(None),
        index_json: Set(Some(serde_json::json!({
            "entries": [{
                "path": PATH,
                "name": "Unchanged List",
                "publisher": "Marvel",
                "sha": SHA,
                // Over the blob cap: an unchanged blob is not size-checked
                // either (stored content is not re-checked).
                "size": 64 * 1024 * 1024,
            }]
        }))),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();

    let parsed = server::cbl::parser::parse(XML.as_bytes()).unwrap();
    let list_id = Uuid::now_v7();
    cbl_list::ActiveModel {
        id: Set(list_id),
        owner_user_id: Set(None),
        source_kind: Set("catalog".into()),
        source_url: Set(None),
        catalog_source_id: Set(Some(source_id)),
        catalog_path: Set(Some(PATH.into())),
        github_blob_sha: Set(Some(SHA.into())),
        source_etag: Set(None),
        source_last_modified: Set(None),
        raw_sha256: Set(import::sha256_of(XML.as_bytes())),
        raw_xml: Set(XML.into()),
        parsed_name: Set(parsed.name.clone()),
        parsed_matchers_present: Set(parsed.matchers_present),
        num_issues_declared: Set(parsed.num_issues_declared),
        description: Set(None),
        imported_at: Set(now),
        last_refreshed_at: Set(Some(now)),
        last_match_run_at: Set(Some(now)),
        refresh_schedule: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        preserve_canonical_order: Set(false),
        ..Default::default()
    }
    .insert(&db)
    .await
    .unwrap();
    import::apply_parsed(
        &db,
        list_id,
        &parsed,
        XML,
        Some(SHA),
        RefreshTrigger::Manual,
    )
    .await
    .unwrap();

    let check = refresh::check(&db, list_id, None, REQUEST_BUDGET)
        .await
        .expect("check without a download");
    assert!(!check.upstream_changed);
    assert!(!check.has_changes(), "{check:?}");

    let summary = refresh::refresh(&db, list_id, RefreshTrigger::Manual, false, REQUEST_BUDGET)
        .await
        .expect("refresh without a download");
    assert!(!summary.upstream_changed);
    assert_eq!(summary.missing, 2);
    assert_eq!(summary.added + summary.removed + summary.reordered, 0);

    let entries = cbl_entry::Entity::find()
        .filter(cbl_entry::Column::CblListId.eq(list_id))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(entries, 2);
    let row = cbl_list::Entity::find_by_id(list_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.github_blob_sha.as_deref(), Some(SHA));
    assert_eq!(row.refresh_failure_count, 0);
    assert!(row.last_refresh_error_kind.is_none());
}

/// A path the index does not list is still `PathMissing`, and no download
/// is tried for it.
#[tokio::test]
async fn path_missing_from_the_index_is_reported() {
    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    let now = Utc::now().fixed_offset();
    let source = catalog_source::ActiveModel {
        id: Set(Uuid::now_v7()),
        display_name: Set("Test catalog".into()),
        github_owner: Set(format!("folio-test-owner-{}", Uuid::now_v7())),
        github_repo: Set("no-such-repository".into()),
        github_branch: Set("main".into()),
        enabled: Set(true),
        last_indexed_at: Set(Some(now)),
        index_etag: Set(None),
        index_json: Set(Some(serde_json::json!({ "entries": [] }))),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&db)
    .await
    .unwrap();
    let err = server::cbl::catalog::index_entry(&db, &source, PATH, false)
        .await
        .unwrap_err();
    assert!(
        matches!(err, server::cbl::catalog::CatalogError::PathMissing(ref p) if p == PATH),
        "{err:?}"
    );
}
