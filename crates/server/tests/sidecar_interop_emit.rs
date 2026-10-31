//! Audit 2026-11 WP A5: the sidecar interop **producer**.
//!
//! `sidecar_parity.rs` proves Folio's ComicInfo.xml agrees with a
//! ComicTagger-written file and that MetronInfo.xml validates against the
//! v1.2 XSD — but nothing there runs another tool. This test seeds one rich
//! issue in a writeback library (credits incl. a comma-suffixed name,
//! cast, arcs with numbers, genres, tags, universes, reprints, ComicVine /
//! Metron / GCD ids, GTINs, cover + store date, age rating, URLs), writes
//! both sidecars into a real CBZ through the production rewrite job, and
//! emits what Folio *meant* as `expected.json`. When `FOLIO_INTEROP_OUT`
//! names a directory the archive, both XML files and the JSON land there,
//! and `scripts/ci/sidecar_interop.py` reads the archive with ComicTagger
//! (`comicapi`) and Metron-Tagger (`darkseid`) and diffs what each tool
//! reads against the JSON. The CI job `sidecar-interop` runs the pair;
//! `just sidecar-interop` does the same locally.
//!
//! Without the env var the test still runs end to end (so the producer
//! can't rot) and only checks that Folio's own parsers read the archive
//! back as `expected.json` says.

mod common;

use archive::ArchiveLimits;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{issue, series};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use server::jobs::rewrite_sidecars::RewriteIssueSidecarsJob;
use server::metadata::identifier::{Identifier, Source};
use server::metadata::manual_writeback::{self, Actor, IssueEnqueue};
use server::metadata::writers::{self, CsvRebuildBatch, SetBy};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

/// Directory the producer copies its outputs into, when set.
const OUT_ENV: &str = "FOLIO_INTEROP_OUT";

// ───────── the seeded issue ─────────

const SERIES: &str = "Saga";
const PUBLISHER: &str = "Image Comics";
const IMPRINT: &str = "Skybound";
const TITLE: &str = "Chapter One";
const NUMBER: &str = "1";
const SUMMARY: &str = "Hazel is born. \"Quoted\" & <escaped>; café, 日本語.";
const NOTES: &str = "Interop fixture written by Folio's rewrite job.";
/// ComicInfo keeps the finer ComicInfo vocabulary …
const AGE_RATING: &str = "Mature 17+";
/// … while MetronInfo carries the schema's `ageRatingType` value
/// (`parsers::metroninfo::metron_age_rating`).
const AGE_RATING_METRON: &str = "Mature";
const WEB: &str = "https://metron.cloud/issue/saga-2012-1/";
const WEB_CV: &str = "https://comicvine.gamespot.com/saga-1/4000-123456/";
const METRON_ID: &str = "290431";
const COMICVINE_ID: &str = "123456";
const GCD_ID: &str = "543";
const SERIES_METRON_ID: &str = "65478";
/// Leading zero on purpose: darkseid parses GTINs as integers.
const UPC: &str = "07098530047000111";
const ISBN: &str = "9781607066019";
const PAGE_COUNT: u32 = 3;

/// `(role, name)` — ComicInfo role labels; the second penciller carries
/// the generational-suffix comma the audit asked the producer to cover.
const CREDITS: &[(&str, &str)] = &[
    ("Writer", "Brian K. Vaughan"),
    ("Penciller", "Fiona Staples"),
    ("Penciller", "Mike Deodato, Jr."),
    ("Inker", "Fiona Staples"),
    ("Colorist", "Fiona Staples"),
    ("Letterer", "Fonografiks"),
    ("CoverArtist", "Fiona Staples"),
    ("Editor", "Eric Stephenson"),
];
const CHARACTERS: &[&str] = &["Alana", "Marko", "Hazel"];
const TEAMS: &[&str] = &["Landfall Coalition", "Wreath High Command"];
const LOCATIONS: &[&str] = &["Cleave", "Landfall"];
const ARCS: &[(&str, i32)] = &[("Volume One", 1)];
const GENRES: &[&str] = &["Science Fiction", "Fantasy"];
const TAGS: &[&str] = &["Image 20th", "Eisner Winner"];
const UNIVERSES: &[&str] = &["Saga Universe"];
const REPRINTS: &[&str] = &["Saga: Compendium One"];

/// What Folio meant to say, in both sidecars' vocabularies. The Python
/// reader compares each tool's reading against this (per-tool coverage
/// and tolerated normalisations live in the script).
fn expected() -> serde_json::Value {
    let mut credits = serde_json::Map::new();
    for (role, name) in CREDITS {
        credits
            .entry((*role).to_owned())
            .or_insert_with(|| serde_json::Value::Array(Vec::new()))
            .as_array_mut()
            .unwrap()
            .push(serde_json::Value::String((*name).to_owned()));
    }
    serde_json::json!({
        "series": SERIES,
        "series_sort_name": SERIES,
        "series_start_year": 2012,
        "series_end_year": 2018,
        "series_issue_count": 54,
        "series_status_metron": "Completed",
        "series_format_metron": "Single Issue",
        "series_metron_id": SERIES_METRON_ID,
        "number": NUMBER,
        "title": TITLE,
        "cover_date": "2012-03-14",
        "store_date": "2012-03-14",
        "publisher": PUBLISHER,
        "imprint": IMPRINT,
        "language": "en",
        "summary": SUMMARY,
        "notes": NOTES,
        "page_count": PAGE_COUNT,
        "credits": credits,
        "characters": CHARACTERS,
        "teams": TEAMS,
        "locations": LOCATIONS,
        "story_arcs": ARCS.iter().map(|(n, p)| serde_json::json!({"name": n, "number": p})).collect::<Vec<_>>(),
        "genres": GENRES,
        "tags": TAGS,
        "universes": UNIVERSES,
        "reprints": REPRINTS,
        "ids": {"comicvine": COMICVINE_ID, "metron": METRON_ID, "gcd": GCD_ID},
        "primary_id_source": "metron",
        "gtin": {"upc": UPC, "isbn": ISBN},
        "age_rating": AGE_RATING,
        "age_rating_metron": AGE_RATING_METRON,
        "web": WEB,
        "urls": [WEB, WEB_CV],
    })
}

// ───────── pipeline helpers ─────────

/// A CBZ with `PAGE_COUNT` real PNG pages (the readers sniff image magic
/// and count pages against `<PageCount>`).
fn cbz_with_pages() -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for i in 0..PAGE_COUNT {
            let shade = 40 + (i as u8) * 60;
            let img = image::RgbImage::from_pixel(40, 60, image::Rgb([200, shade, 40]));
            let mut png = Vec::new();
            img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap();
            zw.start_file(format!("page-{:03}.png", i + 1), opts)
                .unwrap();
            zw.write_all(&png).unwrap();
        }
        zw.finish().unwrap();
    }
    buf.into_inner()
}

fn read_entry(path: &Path, name: &str) -> String {
    let mut a = archive::open(path, ArchiveLimits::default()).unwrap();
    String::from_utf8(
        a.read_entry_bytes(name)
            .unwrap_or_else(|e| panic!("{name}: {e}")),
    )
    .unwrap()
}

/// Run every queued sidecar rewrite job inline (same as
/// `sidecar_parity.rs`), then clear the queue.
async fn run_queued_rewrite_jobs(app: &TestApp) -> usize {
    let storage = app.state().jobs.rewrite_issue_sidecars_storage.clone();
    let data_hash = storage.get_config().job_data_hash();
    let mut conn = app.state().jobs.redis.clone();
    let all: std::collections::HashMap<String, String> = redis::cmd("HGETALL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    let jobs: Vec<RewriteIssueSidecarsJob> = all
        .values()
        .map(|blob| serde_json::from_str(blob).unwrap())
        .collect();
    let n = jobs.len();
    for job in jobs {
        server::jobs::rewrite_sidecars::handle(
            job,
            apalis::prelude::Data::new(app.state()),
            apalis::prelude::Attempt::new_with_value(1),
        )
        .await
        .expect("rewrite job");
    }
    let _: i64 = redis::cmd("DEL")
        .arg(&data_hash)
        .query_async(&mut conn)
        .await
        .unwrap();
    n
}

/// Seed the series + issue described by the constants above. Returns the
/// issue id and the archive path.
async fn seed(app: &TestApp, dir: &Path) -> (String, PathBuf) {
    let db = &app.state().db;
    let lib_id = LibrarySeed::new(dir)
        .with_sidecar_writeback()
        .insert(db)
        .await;
    let series_id = SeriesSeed::new(lib_id, SERIES).insert(db).await;
    let folder = dir.join(format!("{SERIES} (2012)"));
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join(format!("{SERIES} 001 (2012).cbz"));
    let cbz = cbz_with_pages();
    let issue_id = IssueSeed::new(lib_id, series_id, &path, &cbz, 1.0)
        .with_title(TITLE)
        .with_page_count(PAGE_COUNT as i32)
        .insert(db)
        .await;

    let mut sam: series::ActiveModel = series::Entity::find_by_id(series_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    sam.publisher = Set(Some(PUBLISHER.into()));
    sam.imprint = Set(Some(IMPRINT.into()));
    sam.year = Set(Some(2012));
    sam.year_end = Set(Some(2018));
    sam.status = Set("ended".into());
    sam.total_issues = Set(Some(54));
    sam.sort_name = Set(Some(SERIES.into()));
    sam.series_type = Set(Some("Ongoing Series".into()));
    sam.update(db).await.unwrap();

    let mut am: issue::ActiveModel = issue::Entity::find_by_id(issue_id.clone())
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.number_raw = Set(Some(NUMBER.into()));
    am.year = Set(Some(2012));
    am.month = Set(Some(3));
    am.day = Set(Some(14));
    am.store_date = Set(chrono::NaiveDate::from_ymd_opt(2012, 3, 14));
    am.summary = Set(Some(SUMMARY.into()));
    am.notes = Set(Some(NOTES.into()));
    am.age_rating = Set(Some(AGE_RATING.into()));
    am.language_code = Set(Some("en".into()));
    am.web_url = Set(Some(WEB.into()));
    am.additional_links = Set(serde_json::json!([{"label": "ComicVine", "url": WEB_CV}]));
    am.update(db).await.unwrap();

    let set_by = SetBy::Provider(Source::Metron);
    for (src, id) in [
        (Source::Metron, METRON_ID),
        (Source::ComicVine, COMICVINE_ID),
        (Source::Gcd, GCD_ID),
        (Source::Upc, UPC),
        (Source::Isbn, ISBN),
    ] {
        writers::set_external_id(db, "issue", &issue_id, &Identifier::new(src, id), set_by)
            .await
            .unwrap();
    }
    writers::set_external_id(
        db,
        "series",
        &series_id.to_string(),
        &Identifier::new(Source::Metron, SERIES_METRON_ID),
        set_by,
    )
    .await
    .unwrap();

    let batch = CsvRebuildBatch::new();
    let mut credits = Vec::new();
    for (role, name) in CREDITS {
        let person = writers::upsert_person(db, name, &[], set_by).await.unwrap();
        credits.push((person, (*role).to_owned(), 0));
    }
    writers::set_issue_credits(db, &issue_id, credits, set_by, None, &batch)
        .await
        .unwrap();
    let mut characters = Vec::new();
    for (i, name) in CHARACTERS.iter().enumerate() {
        let id = writers::upsert_character(db, name, &[], set_by)
            .await
            .unwrap();
        characters.push((id, i == 0, false));
    }
    writers::set_issue_characters(db, &issue_id, characters, set_by, None, &batch)
        .await
        .unwrap();
    let mut teams = Vec::new();
    for name in TEAMS {
        let id = writers::upsert_team(db, name, &[], set_by).await.unwrap();
        teams.push((id, false, false));
    }
    writers::set_issue_teams(db, &issue_id, teams, set_by, None, &batch)
        .await
        .unwrap();
    let mut locations = Vec::new();
    for name in LOCATIONS {
        let id = writers::upsert_location(db, name, &[], set_by)
            .await
            .unwrap();
        locations.push((id, false));
    }
    writers::set_issue_locations(db, &issue_id, locations, set_by, None, &batch)
        .await
        .unwrap();
    let mut arcs = Vec::new();
    for (name, position) in ARCS {
        let id = writers::upsert_story_arc(db, name, &[], set_by)
            .await
            .unwrap();
        arcs.push((id, Some(*position)));
    }
    writers::set_issue_story_arcs(db, &issue_id, arcs, set_by, None, &batch)
        .await
        .unwrap();
    writers::set_issue_genres(
        db,
        &issue_id,
        GENRES.iter().map(|g| (*g).to_owned()).collect(),
        set_by,
        None,
        &batch,
    )
    .await
    .unwrap();
    writers::set_issue_tags(
        db,
        &issue_id,
        TAGS.iter().map(|t| (*t).to_owned()).collect(),
        set_by,
        None,
        &batch,
    )
    .await
    .unwrap();
    let mut universes = Vec::new();
    for name in UNIVERSES {
        universes.push(
            writers::upsert_universe(db, name, &[], set_by)
                .await
                .unwrap(),
        );
    }
    writers::set_issue_universes(db, &issue_id, universes, set_by, None)
        .await
        .unwrap();
    writers::set_issue_reprints(
        db,
        &issue_id,
        REPRINTS
            .iter()
            .map(|label| writers::ReprintSpec {
                reprinted_issue_id: None,
                reprinted_label: Some((*label).to_owned()),
                reprinted_source: None,
                reprinted_external_id: None,
            })
            .collect(),
        set_by,
        None,
    )
    .await
    .unwrap();
    batch.flush(db).await.unwrap();
    (issue_id, path)
}

// ───────── the test ─────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[allow(
    clippy::print_stderr,
    reason = "the output location must be visible in the CI log"
)]
async fn folio_written_archive_and_expected_json_for_the_interop_readers() {
    let app = TestApp::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (issue_id, path) = seed(&app, dir.path()).await;

    // Write both sidecars through the real job (the manual-edit /
    // drift-flush compose: no provider payload, junctions + columns only).
    let state = app.state();
    let outcome =
        manual_writeback::enqueue_issue_rewrite(&state, &issue_id, &Actor::default(), true)
            .await
            .unwrap();
    assert_eq!(outcome, IssueEnqueue::Enqueued);
    assert_eq!(run_queued_rewrite_jobs(&app).await, 1);

    let ci_xml = read_entry(&path, "ComicInfo.xml");
    let mi_xml = read_entry(&path, "MetronInfo.xml");
    let expected = expected();

    // The archive is what the readers will see: pages intact, both
    // sidecars present.
    {
        let a = archive::open(&path, ArchiveLimits::default()).unwrap();
        assert_eq!(a.pages().len(), PAGE_COUNT as usize, "pages survived");
    }

    // Folio's own parsers agree with `expected.json` on the fields the
    // readers compare, so a producer bug can't masquerade as a tool gap.
    let ci = parsers::comicinfo::parse(ci_xml.as_bytes()).expect("ComicInfo parses");
    assert_eq!(ci.series.as_deref(), Some(SERIES));
    assert_eq!(ci.number.as_deref(), Some(NUMBER));
    assert_eq!(ci.title.as_deref(), Some(TITLE));
    assert_eq!((ci.year, ci.month, ci.day), (Some(2012), Some(3), Some(14)));
    assert_eq!(ci.publisher.as_deref(), Some(PUBLISHER));
    assert_eq!(ci.imprint.as_deref(), Some(IMPRINT));
    assert_eq!(ci.age_rating.as_deref(), Some(AGE_RATING));
    assert_eq!(ci.web.as_deref(), Some(WEB));
    assert_eq!(
        ci.penciller.as_deref(),
        Some("Fiona Staples; Mike Deodato, Jr."),
        "the comma-bearing name switches the ComicInfo joiner to `; `"
    );
    assert_eq!(ci.characters.as_deref(), Some("Alana, Marko, Hazel"));
    assert_eq!(ci.tags.as_deref(), Some("Image 20th, Eisner Winner"));
    assert_eq!(ci.comicvine_id, Some(123_456));
    assert_eq!(ci.metron_id, Some(290_431));
    let mi = parsers::metroninfo::parse(mi_xml.as_bytes()).expect("MetronInfo parses");
    assert_eq!(mi.series.as_deref(), Some(SERIES));
    assert_eq!(mi.number.as_deref(), Some(NUMBER));
    assert_eq!(mi.primary_id_source.as_deref(), Some("metron"));
    assert_eq!(mi.ids.get("upc").map(String::as_str), Some(UPC));
    assert_eq!(mi.ids.get("isbn").map(String::as_str), Some(ISBN));
    assert_eq!(mi.series_id.as_deref(), Some(SERIES_METRON_ID));
    assert_eq!(mi.age_rating.as_deref(), Some(AGE_RATING_METRON));
    assert_eq!(mi.arc_numbers.get("Volume One"), Some(&1));
    assert_eq!(
        mi.credits.get("Penciller").map(Vec::as_slice),
        Some(["Fiona Staples".to_string(), "Mike Deodato, Jr.".to_string()].as_slice())
    );
    assert_eq!(mi.universes, UNIVERSES);
    assert_eq!(mi.reprints, REPRINTS);
    assert_eq!(mi.tags, TAGS);
    assert_eq!(
        expected["credits"]["Penciller"],
        serde_json::json!(["Fiona Staples", "Mike Deodato, Jr."])
    );

    let Some(out) = std::env::var_os(OUT_ENV) else {
        eprintln!(
            "{OUT_ENV} unset; archive not exported (set it to feed scripts/ci/sidecar_interop.py)"
        );
        return;
    };
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(&path, out.join("issue.cbz")).unwrap();
    std::fs::write(out.join("ComicInfo.xml"), &ci_xml).unwrap();
    std::fs::write(out.join("MetronInfo.xml"), &mi_xml).unwrap();
    std::fs::write(
        out.join("expected.json"),
        serde_json::to_string_pretty(&expected).unwrap() + "\n",
    )
    .unwrap();
    eprintln!("sidecar interop fixture written to {}", out.display());
}
