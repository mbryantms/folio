//! Owner bug (Fantastic Four (2001), 173 issues): a **series-scope**
//! metadata apply wrote the series description into every issue's
//! `<Summary>` and — through the post-rewrite rescan — `issues.summary`.
//!
//! `apply_series_via_sidecar` composed each issue's XML from the
//! series-level provider record, and the composer reads
//! `provider.description` as the *issue* summary, so the series
//! description won over each issue's own text. A series apply must leave
//! issue-level slots alone and put the series description on the series.
//!
//! The issues are first ingested by a real scan (file-tier provenance,
//! like the owner's install); provider details come from the metadata
//! cache, so nothing reaches a real provider. ComicVine is the source
//! because it is a "lumper" (no auto-split provider calls).

mod common;

use chrono::Utc;
use common::TestApp;
use common::seed::LibrarySeed;
use entity::{issue, metadata_run, metadata_run_candidate, series};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
use serde::de::DeserializeOwned;
use serde_json::json;
use server::library::scanner;
use server::metadata::apply::{ApplyArgs, ApplyMode};
use server::metadata::cache::{self, CacheEntity};
use server::metadata::identifier::{Identifier, Source};
use server::metadata::provider::GenericMetadata;
use server::metadata::writers::{self, CoverOverwritePolicy, SetBy};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

const SERIES_DESC: &str = "<p>Continued from Fantastic Four Volume 2.</p>";
const SERIES_URL: &str = "https://comicvine.gamespot.com/fantastic-four/4050-6994/";
const CV_SERIES_ID: &str = "6994";

fn own_summary(n: u32) -> String {
    format!("Issue {n}'s own story: the Thing fights villain #{n}.")
}

fn own_title(n: u32) -> String {
    format!("Own Title {n}")
}

fn comicinfo(n: u32) -> String {
    format!(
        r#"<?xml version="1.0"?>
<ComicInfo>
  <Title>{}</Title>
  <Series>Fantastic Four</Series>
  <Number>{n}</Number>
  <Year>2001</Year>
  <Month>{n}</Month>
  <Summary>{}</Summary>
  <Writer>Carlos Pacheco</Writer>
  <Publisher>Marvel</Publisher>
</ComicInfo>"#,
        own_title(n),
        own_summary(n),
    )
}

fn write_cbz(path: &Path, comic_info: &str, marker: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&marker.to_le_bytes());
    png.extend(std::iter::repeat_n(0u8, 64));
    zw.start_file("page-001.png", opts).unwrap();
    zw.write_all(&png).unwrap();
    zw.start_file("ComicInfo.xml", opts).unwrap();
    zw.write_all(comic_info.as_bytes()).unwrap();
    zw.finish().unwrap();
}

fn archive_comicinfo(path: &Path) -> parsers::comicinfo::ComicInfo {
    let f = std::fs::File::open(path).unwrap();
    let mut zr = zip::ZipArchive::new(f).unwrap();
    let mut entry = zr.by_name("ComicInfo.xml").unwrap();
    let mut s = String::new();
    entry.read_to_string(&mut s).unwrap();
    parsers::comicinfo::parse(s.as_bytes()).unwrap()
}

struct Fixture {
    app: TestApp,
    _dir: tempfile::TempDir,
    series_id: Uuid,
    /// (issue number, archive path), numbers 1..=3.
    archives: Vec<(u32, PathBuf)>,
}

/// `Fantastic Four (2001)/` with three issues, each carrying its own
/// `<Summary>`, ingested by a real scan.
async fn fixture(writeback: bool) -> Fixture {
    let app = TestApp::spawn_with_comicvine("cv-key", true).await;
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Fantastic Four (2001)");
    std::fs::create_dir_all(&folder).unwrap();
    let mut archives = Vec::new();
    for n in 1..=3u32 {
        let path = folder.join(format!("Fantastic Four {n:03}.cbz"));
        write_cbz(&path, &comicinfo(n), n);
        archives.push((n, path));
    }
    let mut seed = LibrarySeed::new(dir.path());
    if writeback {
        seed = seed.with_sidecar_writeback();
    }
    let lib_id = seed.insert(&app.state().db).await;
    let stats = scanner::scan_library(&app.state(), lib_id).await.unwrap();
    assert_eq!(stats.files_added, 3, "{stats:?}");
    let series_id = series::Entity::find()
        .filter(series::Column::LibraryId.eq(lib_id))
        .one(&app.state().db)
        .await
        .unwrap()
        .expect("series row")
        .id;
    let f = Fixture {
        app,
        _dir: dir,
        series_id,
        archives,
    };
    for (n, row) in issues(&f).await {
        assert_eq!(row.summary.as_deref(), Some(own_summary(n).as_str()));
    }
    f
}

/// Issue rows keyed by their number.
async fn issues(f: &Fixture) -> Vec<(u32, issue::Model)> {
    issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(f.series_id))
        .order_by_asc(issue::Column::SortNumber)
        .all(&f.app.state().db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| {
            let n: u32 = r.number_raw.as_deref().unwrap().parse().unwrap();
            (n, r)
        })
        .collect()
}

async fn series_row(f: &Fixture) -> series::Model {
    series::Entity::find_by_id(f.series_id)
        .one(&f.app.state().db)
        .await
        .unwrap()
        .unwrap()
}

/// The ComicVine *volume* record: a series description (raw CV HTML, as
/// on the owner's install), deck, publisher and the series page URL.
fn cv_series_detail() -> GenericMetadata {
    GenericMetadata {
        series_name: Some("Fantastic Four".into()),
        year_began: Some(1998),
        publisher: Some("Marvel".into()),
        deck: Some("The second volume continues.".into()),
        description: Some(SERIES_DESC.into()),
        identifiers: vec![Identifier::with_canonical_url(
            Source::ComicVine,
            CV_SERIES_ID,
            "series",
        )],
        source_provider: Some(Source::ComicVine),
        source_external_id: Some(CV_SERIES_ID.into()),
        source_url: Some(SERIES_URL.into()),
        ..Default::default()
    }
}

/// A completed series-scope run with one ComicVine candidate whose
/// detail sits in the provider cache.
async fn seed_series_run(f: &Fixture) -> Uuid {
    let db = &f.app.state().db;
    let now = Utc::now().fixed_offset();
    let run_id = Uuid::now_v7();
    metadata_run::ActiveModel {
        id: Set(run_id),
        scope: Set("series".into()),
        scope_entity_id: Set(Some(f.series_id.to_string())),
        library_id: Set(None),
        triggered_by: Set(None),
        trigger_kind: Set("manual".into()),
        providers: Set(vec!["comicvine".into()]),
        status: Set("completed".into()),
        started_at: Set(now),
        finished_at: Set(Some(now)),
        items_total: Set(1),
        items_matched_high: Set(1),
        items_matched_medium: Set(0),
        items_matched_low: Set(0),
        items_no_match: Set(0),
        items_applied: Set(0),
        items_skipped: Set(0),
        items_failed: Set(0),
        error_summary: Set(None),
        resume_after: Set(None),
        batch_id: Set(None),
        query: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    metadata_run_candidate::ActiveModel {
        run_id: Set(run_id),
        ordinal: Set(0),
        source: Set("comicvine".into()),
        external_id: Set(CV_SERIES_ID.into()),
        bucket: Set("high".into()),
        score: Set(95.0),
        score_breakdown: Set(json!({})),
        candidate: Set(json!({"kind": "series"})),
        applied_at: Set(None),
    }
    .insert(db)
    .await
    .unwrap();
    cache::put(
        db,
        Source::ComicVine,
        CacheEntity::Series,
        CV_SERIES_ID,
        &cv_series_detail(),
    )
    .await
    .unwrap();
    run_id
}

fn args(run_id: Uuid, mode: ApplyMode) -> ApplyArgs {
    ApplyArgs {
        run_id,
        ordinal: 0,
        mode,
        apply_cover: false,
        cover_overwrite_policy: CoverOverwritePolicy::WhenMissing,
        override_user_edits: false,
        actor_id: None,
        selected_fields: None,
        override_external_id_sources: std::collections::HashSet::new(),
    }
}

/// Pop every job queued on an apalis Redis storage (its `{namespace}:data`
/// hash, `Request { args, parts }` JSON) and clear the hash.
async fn drain<T: DeserializeOwned>(app: &TestApp, data_hash: String) -> Vec<T> {
    let mut conn = app.state().jobs.redis.clone();
    let all: std::collections::HashMap<String, String> = redis::cmd("HGETALL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    let _: i64 = redis::cmd("DEL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    all.values()
        .map(|blob| {
            let v: serde_json::Value = serde_json::from_str(blob).expect("request json");
            serde_json::from_value(v["args"].clone()).expect("job args")
        })
        .collect()
}

/// Series apply (the sidecar path rewrites every archive inline), then the
/// one series-scoped rescan it queued — the step that carried the leaked
/// `<Summary>` into `issues.summary`.
async fn apply_series_and_rescan(f: &Fixture, run_id: Uuid, mode: ApplyMode) {
    let outcome = server::jobs::metadata_apply::apply_series_inline(
        &f.app.state(),
        f.series_id,
        args(run_id, mode),
    )
    .await
    .expect("series apply");
    assert_eq!(outcome.composed_sidecars, 3, "{outcome:?}");
    let scan_hash = f
        .app
        .state()
        .jobs
        .scan_series_storage
        .get_config()
        .job_data_hash();
    let scans = drain::<server::jobs::scan_series::Job>(&f.app, scan_hash).await;
    assert_eq!(scans.len(), 1, "one series-scoped rescan");
    for job in scans {
        server::jobs::scan_series::handle(job, apalis::prelude::Data::new(f.app.state()))
            .await
            .unwrap();
    }
}

fn assert_issue_slots_untouched(n: u32, row: &issue::Model) {
    assert_eq!(
        row.summary.as_deref(),
        Some(own_summary(n).as_str()),
        "#{n}: issues.summary must keep the issue's own description"
    );
    assert_eq!(row.title.as_deref(), Some(own_title(n).as_str()));
}

// ───────── tests ─────────

/// Writeback library: the archives keep each issue's own `<Summary>`, so
/// does `issues.summary` after the rescan, and the series description
/// lands on `series.summary`. Issue #2 carries a provider row on its
/// description from an earlier issue-scope apply — the case #970's
/// timestamp-aware ingest would otherwise hand to the rescan.
#[tokio::test]
async fn series_apply_keeps_issue_descriptions_in_writeback_library() {
    let f = fixture(true).await;
    let db = &f.app.state().db;
    let issue2 = issues(&f).await.remove(1).1;
    writers::write_field_provenance(
        db,
        "issue",
        &issue2.id,
        server::metadata::MetadataField::Description,
        SetBy::Provider(Source::ComicVine),
        None,
    )
    .await
    .unwrap();

    let run_id = seed_series_run(&f).await;
    apply_series_and_rescan(&f, run_id, ApplyMode::ReplaceAll).await;

    for (n, path) in &f.archives {
        let ci = archive_comicinfo(path);
        assert_eq!(
            ci.summary.as_deref(),
            Some(own_summary(*n).as_str()),
            "#{n}: archive <Summary> must not carry the series description"
        );
        assert_eq!(ci.title.as_deref(), Some(own_title(*n).as_str()));
        assert_ne!(
            ci.web.as_deref(),
            Some(SERIES_URL),
            "#{n}: the series page URL is not the issue's <Web>"
        );
        // Series identity still flows into every issue.
        assert_eq!(ci.series.as_deref(), Some("Fantastic Four"));
        assert_eq!(ci.publisher.as_deref(), Some("Marvel"));
    }
    for (n, row) in issues(&f).await {
        assert_issue_slots_untouched(n, &row);
    }
    let s = series_row(&f).await;
    assert_eq!(s.summary.as_deref(), Some(SERIES_DESC));
    assert_eq!(s.deck.as_deref(), Some("The second volume continues."));
}

/// DB-direct library: the series apply writes only the series row.
#[tokio::test]
async fn series_apply_keeps_issue_descriptions_db_direct() {
    let f = fixture(false).await;
    let run_id = seed_series_run(&f).await;
    let outcome = server::jobs::metadata_apply::apply_series_inline(
        &f.app.state(),
        f.series_id,
        args(run_id, ApplyMode::ReplaceAll),
    )
    .await
    .expect("series apply");
    assert!(!outcome.enqueued_rewrite);

    for (n, path) in &f.archives {
        assert_eq!(
            archive_comicinfo(path).summary.as_deref(),
            Some(own_summary(*n).as_str()),
            "#{n}: a DB-direct library's archives are never touched"
        );
    }
    for (n, row) in issues(&f).await {
        assert_issue_slots_untouched(n, &row);
    }
    assert_eq!(series_row(&f).await.summary.as_deref(), Some(SERIES_DESC));
}

/// A user-pinned series description survives a replace-all series apply
/// (user > provider), and the issues still keep their own descriptions.
#[tokio::test]
async fn series_apply_keeps_user_pinned_series_description() {
    for writeback in [true, false] {
        let f = fixture(writeback).await;
        let db = &f.app.state().db;
        let mut am: series::ActiveModel = series_row(&f).await.into();
        am.summary = Set(Some("My own series blurb.".into()));
        am.update(db).await.unwrap();
        writers::write_field_provenance(
            db,
            "series",
            &f.series_id.to_string(),
            server::metadata::MetadataField::Description,
            SetBy::User,
            None,
        )
        .await
        .unwrap();

        let run_id = seed_series_run(&f).await;
        if writeback {
            apply_series_and_rescan(&f, run_id, ApplyMode::ReplaceAll).await;
        } else {
            server::jobs::metadata_apply::apply_series_inline(
                &f.app.state(),
                f.series_id,
                args(run_id, ApplyMode::ReplaceAll),
            )
            .await
            .expect("series apply");
        }

        assert_eq!(
            series_row(&f).await.summary.as_deref(),
            Some("My own series blurb."),
            "writeback={writeback}: a user-pinned series description is never overwritten"
        );
        for (n, row) in issues(&f).await {
            assert_issue_slots_untouched(n, &row);
        }
    }
}
