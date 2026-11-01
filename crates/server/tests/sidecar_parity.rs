//! Roadmap WP-6.4: ComicTagger parity.
//!
//! `fixtures/comictagger/ct-1.5.5-tagged.cbz` is a tiny CBZ whose
//! ComicInfo.xml was written by ComicTagger 1.5.5 itself (offline, from
//! `-m` overrides — see `fixtures/comictagger/make-fixture.py`). This test
//! runs it through Folio's real pipeline — scanner ingest → the writeback
//! composer (DB-only compose, the manual-edit / drift-flush path) → the
//! `RewriteIssueSidecarsJob` archive rewrite — and diffs Folio's
//! ComicInfo.xml against ComicTagger's, element by element and page by
//! page. Every difference must be listed in [`KNOWN_DIFFERENCES`] with a
//! reason; the same list is documented in
//! `docs/dev/metadata-sidecar-writeback.md` ("ComicTagger parity").
//!
//! Hermetic: ComicTagger is only needed to *regenerate* the fixture. The
//! XML comparison uses a plain quick-xml leaf walk, not Folio's own
//! parser, so a parser blind spot can't hide a lossy round-trip.
//!
//! Folio's rewritten ComicInfo.xml + MetronInfo.xml are also pinned as
//! golden files (`fixtures/comictagger/folio-rewrite.*.xml`) so a composer or
//! serializer change shows up as a reviewable diff. Re-bless with
//! `FOLIO_PARITY_BLESS=1 cargo test -p server --test sidecar_parity`, then
//! re-run the reverse check (`make-fixture.py verify`, which reads the
//! golden with ComicTagger's own parser).

mod common;

use archive::ArchiveLimits;
use common::TestApp;
use common::seed::{IssueSeed, LibrarySeed, SeriesSeed};
use entity::{external_id, issue, series};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use server::jobs::rewrite_sidecars::RewriteIssueSidecarsJob;
use server::library::scanner;
use server::library::scanner::metadata_rollup;
use server::metadata::identifier::Identifier;
use server::metadata::identifier::Source;
use server::metadata::manual_writeback::{self, Actor, IssueEnqueue};
use server::metadata::writers::{self, CsvRebuildBatch, SetBy};
use std::collections::BTreeMap;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

const CT_VERSION: &str = "1.5.5";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/comictagger")
}

fn fixture_cbz() -> PathBuf {
    fixture_dir().join(format!("ct-{CT_VERSION}-tagged.cbz"))
}

fn golden_path(entry: &str) -> PathBuf {
    fixture_dir().join(format!("folio-rewrite.{entry}"))
}

/// Compare `actual` against the checked-in golden copy of Folio's rewrite
/// (or write it when `FOLIO_PARITY_BLESS` is set).
fn check_golden(entry: &str, actual: &str) {
    let path = golden_path(entry);
    if std::env::var_os("FOLIO_PARITY_BLESS").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let golden = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} — run with FOLIO_PARITY_BLESS=1", path.display()));
    assert_eq!(
        actual, golden,
        "Folio's {entry} rewrite changed; if intended, re-bless with \
         FOLIO_PARITY_BLESS=1 and re-run `make-fixture.py verify`"
    );
}

// ───────── known, intentional differences ─────────

/// How Folio's rewrite is allowed to differ from ComicTagger's file for
/// one element.
#[derive(Debug)]
#[allow(
    dead_code,
    reason = "the full vocabulary stays available for future allowlist entries"
)]
enum Known {
    /// ComicTagger writes the element; Folio's rewrite omits it.
    FolioOmits,
    /// Folio's rewrite adds an element ComicTagger did not write.
    FolioAdds(&'static str),
    /// Both write it; the values differ in the stated way.
    Differs {
        ct: &'static str,
        folio: &'static str,
    },
}

/// Every element-level difference between ComicTagger's ComicInfo.xml and
/// Folio's rewrite of it. Keep in sync with the "ComicTagger parity"
/// section of `docs/dev/metadata-sidecar-writeback.md`. An entry that no
/// longer fires fails the test too, so the list can't rot.
const KNOWN_DIFFERENCES: &[(&str, Known, &str)] = &[(
    "ComicVineID",
    Known::FolioAdds("123456"),
    "Folio derives the ComicVine issue id from the `4000-N` token in \
     <Web> and writes it as the de-facto <ComicVineID> extension element \
     (Metron-Tagger / Mylar3 spelling); ComicTagger 1.5.5 keeps the id \
     only in the URL and ignores the extra element on read.",
)];

/// `<Page DoublePage>` is compared as a boolean, not as text, and an
/// absent attribute equals `false` (the ComicInfo default):
///
/// - ComicTagger serializes a Python bool (`"True"`); Folio writes the
///   xs:boolean form (`"true"`).
/// - Folio omits `DoublePage="false"` (WP-8.1: ComicTagger 1.5.5's page
///   editor ticks the box on attribute presence) except on a landscape
///   page, where it keeps a declared `false` so the next scan doesn't
///   infer a spread.
///
/// Every other page attribute must match exactly.
fn normalize_pages(pages: &[BTreeMap<String, String>]) -> Vec<BTreeMap<String, String>> {
    pages
        .iter()
        .map(|p| {
            let mut p = p.clone();
            let dp = p
                .remove("DoublePage")
                .is_some_and(|v| v.eq_ignore_ascii_case("true"));
            p.insert("DoublePage".into(), dp.to_string());
            p
        })
        .collect()
}

// ───────── generic XML walk ─────────

/// A ComicInfo document reduced to what both tools agree is data: the
/// root's leaf children (name → trimmed text) and each `<Page>`'s
/// attributes. Element order and whitespace are not semantic.
#[derive(Debug, Default)]
struct Doc {
    fields: BTreeMap<String, String>,
    pages: Vec<BTreeMap<String, String>>,
}

fn walk(xml: &str) -> Doc {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;
    let mut doc = Doc::default();
    let mut depth = 0usize;
    let mut name = String::new();
    let mut text = String::new();
    loop {
        match reader.read_event().expect("well-formed XML") {
            Event::Start(e) => {
                depth += 1;
                let tag = e.name().as_ref().to_string();
                if depth == 3 && tag == "Page" {
                    let attrs = e
                        .attributes()
                        .map(|a| {
                            let a = a.unwrap();
                            (
                                a.key.as_ref().to_string(),
                                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                    .unwrap()
                                    .into_owned(),
                            )
                        })
                        .collect();
                    doc.pages.push(attrs);
                }
                if depth == 2 {
                    name = tag;
                    text.clear();
                }
            }
            Event::Text(t) if depth == 2 => {
                text.push_str(&quick_xml::escape::unescape(&t).unwrap());
            }
            Event::GeneralRef(r) if depth == 2 => {
                let ent: &str = &r;
                let resolved = quick_xml::escape::resolve_predefined_entity(ent)
                    .unwrap_or_else(|| panic!("unexpected entity &{ent};"));
                text.push_str(resolved);
            }
            Event::End(_) => {
                if depth == 2 && name != "Pages" {
                    let v = text.trim();
                    if !v.is_empty() {
                        let prev = doc.fields.insert(name.clone(), v.to_owned());
                        assert!(prev.is_none(), "duplicate <{name}>");
                    }
                }
                depth -= 1;
            }
            Event::Eof => break,
            _ => {}
        }
    }
    doc
}

/// One difference between the two documents.
#[derive(Debug, PartialEq)]
struct Diff {
    element: String,
    ct: Option<String>,
    folio: Option<String>,
}

fn field_diffs(ct: &Doc, folio: &Doc) -> Vec<Diff> {
    let mut names: Vec<&String> = ct.fields.keys().chain(folio.fields.keys()).collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter_map(|n| {
            let (a, b) = (ct.fields.get(n), folio.fields.get(n));
            (a != b).then(|| Diff {
                element: n.clone(),
                ct: a.cloned(),
                folio: b.cloned(),
            })
        })
        .collect()
}

fn known_matches(known: &Known, d: &Diff) -> bool {
    match known {
        Known::FolioOmits => d.ct.is_some() && d.folio.is_none(),
        Known::FolioAdds(v) => d.ct.is_none() && d.folio.as_deref() == Some(*v),
        Known::Differs { ct, folio } => {
            d.ct.as_deref() == Some(*ct) && d.folio.as_deref() == Some(*folio)
        }
    }
}

// ───────── pipeline helpers ─────────

fn read_entry(path: &Path, name: &str) -> Option<Vec<u8>> {
    let mut a = archive::open(path, ArchiveLimits::default()).unwrap();
    a.read_entry_bytes(name).ok()
}

/// Run every queued sidecar rewrite job inline, then clear the queue.
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

/// Scan the library, then rewrite the (single) issue's sidecars from the
/// database exactly as a manual edit / drift flush would. Returns Folio's
/// ComicInfo.xml + MetronInfo.xml as they landed in the archive.
async fn scan_and_rewrite(app: &TestApp, lib_id: uuid::Uuid, path: &Path) -> (String, String) {
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.expect("scan");
    let row = issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(lib_id))
        .one(&state.db)
        .await
        .unwrap()
        .expect("fixture ingested as an issue");
    let outcome = manual_writeback::enqueue_issue_rewrite(&state, &row.id, &Actor::default(), true)
        .await
        .unwrap();
    assert_eq!(outcome, IssueEnqueue::Enqueued);
    assert_eq!(run_queued_rewrite_jobs(app).await, 1);
    let ci = String::from_utf8(read_entry(path, "ComicInfo.xml").expect("ComicInfo.xml")).unwrap();
    let mi =
        String::from_utf8(read_entry(path, "MetronInfo.xml").expect("MetronInfo.xml")).unwrap();
    (ci, mi)
}

// ───────── MetronInfo credit shape ─────────

/// Structural check of `<Credits>` against the MetronInfo XSD (v1.0 /
/// v1.1 `creditsType` / `creditType`, Metron-Project/metroninfo):
/// `<Credit>` has no attributes and exactly a `<Creator>` (simple text
/// content, optional `id`) and a `<Roles>` of `<Role>` values from the
/// `roleValues` enumeration. No Rust XSD validator is a dependency, so
/// this is the schema's credit grammar spelled out over a raw quick-xml
/// walk (not Folio's parser, which also accepts the legacy shape). Returns
/// the number of `<Credit>` elements.
fn assert_metron_credit_shape(xml: &str) -> usize {
    let mut reader = Reader::from_str(xml);
    let mut path: Vec<String> = Vec::new();
    let mut credits = 0;
    // Per-credit: (creator text, role values, child element names).
    let mut creator = String::new();
    let mut roles: Vec<String> = Vec::new();
    let mut children: Vec<String> = Vec::new();
    let mut text = String::new();
    loop {
        match reader.read_event().unwrap() {
            Event::Start(e) => {
                let name = e.name().as_ref().to_string();
                let parent = path.last().map(String::as_str);
                match (parent, name.as_str()) {
                    (Some("Credits"), "Credit") => {
                        assert_eq!(
                            e.attributes().count(),
                            0,
                            "<Credit> takes no attributes (legacy `role=`)\n{xml}"
                        );
                        creator.clear();
                        roles.clear();
                        children.clear();
                    }
                    (Some("Credit"), child) => children.push(child.to_owned()),
                    (Some("Roles"), r) => assert_eq!(r, "Role", "{xml}"),
                    (Some("Creator"), c) => {
                        panic!("<Creator> is simple content, found <{c}>\n{xml}")
                    }
                    _ => {}
                }
                path.push(name);
                text.clear();
            }
            Event::Text(t) => text.push_str(&quick_xml::escape::unescape(&t).unwrap()),
            Event::GeneralRef(r) => {
                let ent: &str = &r;
                text.push_str(quick_xml::escape::resolve_predefined_entity(ent).unwrap_or("?"));
            }
            Event::End(_) => {
                let name = path.pop().unwrap();
                match name.as_str() {
                    "Creator" => creator = text.trim().to_owned(),
                    "Role" => roles.push(text.trim().to_owned()),
                    "Credit" => {
                        credits += 1;
                        children.sort();
                        assert_eq!(children, ["Creator", "Roles"], "{xml}");
                        assert!(!creator.is_empty(), "{xml}");
                        assert!(!roles.is_empty(), "{xml}");
                        for r in &roles {
                            assert!(
                                parsers::metroninfo::METRON_ROLES.contains(&r.as_str()),
                                "<Role>{r}</Role> is outside the schema's roleValues\n{xml}"
                            );
                        }
                    }
                    _ => {}
                }
                text.clear();
            }
            Event::Empty(e) => {
                let name = e.name().as_ref().to_string();
                assert!(
                    name != "Credit" && name != "Creator",
                    "empty <{name}/> in credits\n{xml}"
                );
            }
            Event::Eof => break,
            _ => {}
        }
    }
    credits
}

// ───────── MetronInfo XSD validation ─────────

/// The MetronInfo v1.2 XSD, vendored verbatim from
/// Metron-Project/metroninfo `schema/v1.2/MetronInfo.xsd`.
fn metroninfo_xsd() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/metroninfo/MetronInfo-v1.2.xsd")
}

/// The pre-schema MetronInfo Folio wrote until audit 2026-11 WP A4 (the
/// ComicTagger-parity golden as it was before the schema change).
fn legacy_shape_metroninfo() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/metroninfo/folio-legacy-shape.MetronInfo.xml"),
    )
    .unwrap()
}

/// Validate `xml` against the vendored MetronInfo v1.2 XSD with
/// `xmllint --schema`. libxml2 is an XSD 1.0 validator, so the schema's
/// XSD 1.1 `<xs:assert>`s (at most one primary id / URL, EndYear after
/// StartYear, no EndYear on an Ongoing series) are stripped from a temp
/// copy; `parsers::metroninfo::serialize` honours those rules itself.
/// Without `xmllint` the check is skipped locally and fails under CI
/// (`CI` set), where the workflow installs `libxml2-utils`.
#[allow(
    clippy::print_stderr,
    reason = "the skip must be visible in the test log"
)]
fn assert_valid_metroninfo(xml: &str) {
    let Ok(xmllint) = which_xmllint() else {
        assert!(
            std::env::var_os("CI").is_none(),
            "xmllint missing in CI; the Rust test job installs libxml2-utils"
        );
        eprintln!("xmllint not found; skipping MetronInfo XSD validation");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let xsd = dir.path().join("MetronInfo.xsd");
    let doc = dir.path().join("MetronInfo.xml");
    std::fs::write(
        &xsd,
        strip_xsd_asserts(&std::fs::read_to_string(metroninfo_xsd()).unwrap()),
    )
    .unwrap();
    std::fs::write(&doc, xml).unwrap();
    let out = std::process::Command::new(xmllint)
        .arg("--noout")
        .arg("--schema")
        .arg(&xsd)
        .arg(&doc)
        .output()
        .expect("run xmllint");
    assert!(
        out.status.success(),
        "MetronInfo.xml does not validate against the v1.2 XSD:\n{}\n{}\n--- document ---\n{xml}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn which_xmllint() -> Result<PathBuf, ()> {
    let path = std::env::var_os("PATH").ok_or(())?;
    std::env::split_paths(&path)
        .map(|d| d.join("xmllint"))
        .find(|p| p.is_file())
        .ok_or(())
}

/// Remove every `<xs:assert … />` (they span lines) from an XSD.
fn strip_xsd_asserts(xsd: &str) -> String {
    let mut out = String::with_capacity(xsd.len());
    let mut rest = xsd;
    while let Some(start) = rest.find("<xs:assert") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let end = after.find("/>").expect("xs:assert is an empty element") + 2;
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

#[test]
fn xsd_assert_stripping_leaves_the_schema_intact() {
    let xsd = std::fs::read_to_string(metroninfo_xsd()).unwrap();
    assert!(
        xsd.contains("<xs:assert"),
        "vendored XSD is the 1.1-flavoured one"
    );
    let stripped = strip_xsd_asserts(&xsd);
    assert!(!stripped.contains("xs:assert"));
    assert!(stripped.contains("<xs:element name=\"MetronInfo\" type=\"metroninfoType\" />"));
    assert!(
        stripped.contains("<xs:simpleType name=\"seriesStatusType\">"),
        "v1.2"
    );
}

/// Issue-metadata columns a rescan must reproduce from the rewritten
/// archive (the ComicInfo / MetronInfo-fed scalars and read-cache lists)
/// plus the issue's external ids.
#[derive(Debug, PartialEq)]
struct Ingested {
    columns: Vec<(&'static str, Option<String>)>,
    external_ids: BTreeMap<String, String>,
}

async fn ingested(app: &TestApp, lib_id: uuid::Uuid) -> (issue::Model, Ingested) {
    let db = &app.state().db;
    let row = issue::Entity::find()
        .filter(issue::Column::LibraryId.eq(lib_id))
        .one(db)
        .await
        .unwrap()
        .expect("one issue");
    let ids = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq("issue"))
        .filter(external_id::Column::EntityId.eq(row.id.clone()))
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.source, r.external_id))
        .collect();
    let num = |v: Option<i32>| v.map(|n| n.to_string());
    let columns = vec![
        ("title", row.title.clone()),
        ("number_raw", row.number_raw.clone()),
        ("year", num(row.year)),
        ("month", num(row.month)),
        ("day", num(row.day)),
        ("summary", row.summary.clone()),
        ("notes", row.notes.clone()),
        ("age_rating", row.age_rating.clone()),
        ("language_code", row.language_code.clone()),
        ("manga", row.manga.clone()),
        ("publisher", row.publisher.clone()),
        ("imprint", row.imprint.clone()),
        ("story_arc", row.story_arc.clone()),
        ("characters", row.characters.clone()),
        ("teams", row.teams.clone()),
        ("locations", row.locations.clone()),
        ("genre", row.genre.clone()),
        ("writer", row.writer.clone()),
        ("penciller", row.penciller.clone()),
        ("inker", row.inker.clone()),
        ("colorist", row.colorist.clone()),
        ("letterer", row.letterer.clone()),
        ("cover_artist", row.cover_artist.clone()),
        ("editor", row.editor.clone()),
    ];
    (
        row,
        Ingested {
            columns,
            external_ids: ids,
        },
    )
}

// ───────── the test ─────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folio_rewrite_of_a_comictagger_file_agrees_on_every_shared_field() {
    let app = TestApp::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("The Parity Patrol (2021)");
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("The Parity Patrol 003 (2021).cbz");
    std::fs::copy(fixture_cbz(), &path).unwrap();
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;

    let ct_xml = String::from_utf8(read_entry(&path, "ComicInfo.xml").unwrap()).unwrap();
    let original_pages: Vec<(String, Vec<u8>)> = {
        let mut a = archive::open(&path, ArchiveLimits::default()).unwrap();
        let names: Vec<String> = a.pages().iter().map(|p| p.name.clone()).collect();
        names
            .into_iter()
            .map(|n| {
                let b = a.read_entry_bytes(&n).unwrap();
                (n, b)
            })
            .collect()
    };
    assert_eq!(original_pages.len(), 4, "fixture has four real image pages");

    let (folio_xml, metron_xml) = scan_and_rewrite(&app, lib_id, &path).await;

    // 1. Field-by-field parity against ComicTagger's own file.
    let ct = walk(&ct_xml);
    let folio = walk(&folio_xml);
    let diffs = field_diffs(&ct, &folio);
    let unexplained: Vec<&Diff> = diffs
        .iter()
        .filter(|d| {
            !KNOWN_DIFFERENCES
                .iter()
                .any(|(el, k, _)| *el == d.element && known_matches(k, d))
        })
        .collect();
    let stale: Vec<&str> = KNOWN_DIFFERENCES
        .iter()
        .filter(|(el, k, _)| {
            !diffs
                .iter()
                .any(|d| d.element == *el && known_matches(k, d))
        })
        .map(|(el, _, _)| *el)
        .collect();
    assert!(
        unexplained.is_empty() && stale.is_empty(),
        "ComicTagger parity broke.\nunexplained differences: {unexplained:#?}\n\
         KNOWN_DIFFERENCES entries that no longer apply: {stale:?}\n\
         --- ComicTagger ---\n{ct_xml}\n--- Folio ---\n{folio_xml}"
    );
    // Sanity: the comparison actually covered the file.
    assert!(ct.fields.len() >= 35, "{:?}", ct.fields.keys());

    // 2. Per-page metadata survives (ComicTagger's real image sizes /
    //    dimensions, page types, bookmark, the spread it marked double).
    assert_eq!(ct.pages.len(), 4);
    assert_eq!(
        normalize_pages(&ct.pages),
        normalize_pages(&folio.pages),
        "<Pages> differ\n{folio_xml}"
    );
    assert_eq!(
        ct.pages[2].get("DoublePage").map(String::as_str),
        Some("True"),
        "fixture exercises ComicTagger's Python-bool DoublePage"
    );

    // 3. The rewrite never touched the page bytes.
    let mut a = archive::open(&path, ArchiveLimits::default()).unwrap();
    for (name, bytes) in &original_pages {
        assert_eq!(
            &a.read_entry_bytes(name).unwrap(),
            bytes,
            "{name} re-encoded"
        );
    }
    drop(a);

    // 4. MetronInfo.xml (which ComicTagger 1.5.5 neither reads nor writes)
    //    carries the same values for the fields both schemas share.
    let mi = parsers::metroninfo::parse(metron_xml.as_bytes()).expect("MetronInfo parses");
    let f = &ct.fields;
    let csv = |k: &str| -> Vec<String> { f[k].split(", ").map(str::to_owned).collect::<Vec<_>>() };
    assert_eq!(mi.series.as_deref(), Some(f["Series"].as_str()));
    assert_eq!(mi.number.as_deref(), Some(f["Number"].as_str()));
    assert_eq!(mi.title.as_deref(), Some(f["Title"].as_str()));
    assert_eq!(mi.publisher.as_deref(), Some(f["Publisher"].as_str()));
    assert_eq!(mi.imprint.as_deref(), Some(f["Imprint"].as_str()));
    // MetronInfo's <Volume> is a volume *number* (Metron never stores a
    // start year there), so ComicTagger's year-shaped `2021` — kept in
    // ComicInfo — stays out of MetronInfo.
    assert_eq!(f["Volume"], "2021");
    assert_eq!(mi.volume, None);
    assert_eq!(mi.year.map(|v| v.to_string()), Some(f["Year"].clone()));
    assert_eq!(mi.month.map(|v| v.to_string()), Some(f["Month"].clone()));
    assert_eq!(mi.day.map(|v| v.to_string()), Some(f["Day"].clone()));
    assert_eq!(mi.summary.as_deref(), Some(f["Summary"].as_str()));
    assert_eq!(mi.language.as_deref(), Some(f["LanguageISO"].as_str()));
    assert_eq!(mi.characters, csv("Characters"));
    assert_eq!(mi.teams, csv("Teams"));
    assert_eq!(mi.locations, csv("Locations"));
    assert_eq!(mi.genres, csv("Genre"));
    assert_eq!(mi.story_arcs, csv("StoryArc"));
    // Credits: every ComicInfo role column ComicTagger wrote comes back
    // from MetronInfo (CoverArtist travels as the schema's `Cover`).
    assert_eq!(mi.writer().as_deref(), Some(f["Writer"].as_str()));
    assert_eq!(mi.penciller().as_deref(), Some(f["Penciller"].as_str()));
    assert_eq!(mi.inker().as_deref(), Some(f["Inker"].as_str()));
    assert_eq!(mi.colorist().as_deref(), Some(f["Colorist"].as_str()));
    assert_eq!(mi.letterer().as_deref(), Some(f["Letterer"].as_str()));
    assert_eq!(
        mi.cover_artist().as_deref(),
        Some(f["CoverArtist"].as_str())
    );
    assert_eq!(mi.editor().as_deref(), Some(f["Editor"].as_str()));
    // …in the MetronInfo schema's credit shape (WP-8.1), and the whole
    // document validates against the v1.2 XSD (audit 2026-11 WP A4).
    assert!(
        assert_metron_credit_shape(&metron_xml) >= 7,
        "every ComicTagger credit is present\n{metron_xml}"
    );
    assert_valid_metroninfo(&metron_xml);
    assert!(metron_xml.contains("<IDS>"), "{metron_xml}");
    assert!(
        metron_xml.contains("<ID source=\"Comic Vine\" primary=\"true\">123456</ID>"),
        "{metron_xml}"
    );
    assert!(
        metron_xml.contains("<CoverDate>2021-07-14</CoverDate>"),
        "{metron_xml}"
    );
    assert!(!metron_xml.contains("<Title>"), "{metron_xml}");
    assert!(!metron_xml.contains("<Year>"), "{metron_xml}");

    // 5. Golden pin of Folio's rewrite (reviewable diff on any composer /
    //    serializer change; input to `make-fixture.py verify`).
    check_golden("ComicInfo.xml", &folio_xml);
    check_golden("MetronInfo.xml", &metron_xml);

    // 6. Folio's rewrite is a fixed point: rescanning it and rewriting
    //    again produces the same XML (no drift accumulates across cycles).
    let (again_ci, again_mi) = scan_and_rewrite(&app, lib_id, &path).await;
    assert_eq!(again_ci, folio_xml, "second ComicInfo rewrite drifted");
    assert_eq!(again_mi, metron_xml, "second MetronInfo rewrite drifted");
}

// ───────── junctions are the truth (audit 2026-11 N-M2) ─────────

/// A one-page CBZ with a real PNG signature so the rewrite job treats it
/// as an archive with a page, not an empty one.
fn one_page_cbz() -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("page-001.png", opts).unwrap();
        zw.write_all(b"\x89PNG\r\n\x1a\njunction-fixture").unwrap();
        zw.finish().unwrap();
    }
    buf.into_inner()
}

/// Writeback library + series + one seeded issue (no scan: the junctions
/// are written directly, as a provider apply would). Returns
/// `(issue_id, archive path)`.
async fn seed_writeback_issue(app: &TestApp, dir: &Path) -> (String, PathBuf) {
    let db = &app.state().db;
    let lib_id = LibrarySeed::new(dir)
        .with_sidecar_writeback()
        .insert(db)
        .await;
    let series_id = SeriesSeed::new(lib_id, "Saga").insert(db).await;
    let path = dir.join("saga-1.cbz");
    let cbz = one_page_cbz();
    let issue_id = IssueSeed::new(lib_id, series_id, &path, &cbz, 1.0)
        .insert(db)
        .await;
    (issue_id, path)
}

/// Rewrite the issue's sidecars from the database alone (the manual-edit /
/// drift-flush compose) and return the ComicInfo.xml + MetronInfo.xml that
/// landed in the archive.
async fn rewrite_from_db(app: &TestApp, issue_id: &str, path: &Path) -> (String, String) {
    let state = app.state();
    let outcome =
        manual_writeback::enqueue_issue_rewrite(&state, issue_id, &Actor::default(), true)
            .await
            .unwrap();
    assert_eq!(outcome, IssueEnqueue::Enqueued);
    assert_eq!(run_queued_rewrite_jobs(app).await, 1);
    let ci = String::from_utf8(read_entry(path, "ComicInfo.xml").expect("ComicInfo.xml")).unwrap();
    let mi =
        String::from_utf8(read_entry(path, "MetronInfo.xml").expect("MetronInfo.xml")).unwrap();
    (ci, mi)
}

/// A creator whose name carries a generational suffix with a comma
/// (`"Mike Deodato, Jr."`) is one person in the junction and must stay one
/// creator in both sidecars. Pre-fix the composer read the read-cache
/// column and split it on every comma, so MetronInfo carried
/// `<Creator>Mike Deodato</Creator>` + `<Creator>Jr.</Creator>`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn suffixed_creator_name_is_one_creator_in_both_sidecars() {
    let app = TestApp::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (issue_id, path) = seed_writeback_issue(&app, dir.path()).await;
    let db = &app.state().db;

    let set_by = SetBy::Provider(Source::Metron);
    let writer = writers::upsert_person(db, "Wanda Writer", &[], set_by)
        .await
        .unwrap();
    let penciller = writers::upsert_person(db, "Mike Deodato, Jr.", &[], set_by)
        .await
        .unwrap();
    let batch = CsvRebuildBatch::new();
    writers::set_issue_credits(
        db,
        &issue_id,
        vec![
            (writer, "Writer".into(), 0),
            (penciller, "Penciller".into(), 0),
        ],
        set_by,
        None,
        &batch,
    )
    .await
    .unwrap();
    batch.flush(db).await.unwrap();
    // The derived column carries the comma-bearing name verbatim — the
    // very string the old composer split.
    let row = issue::Entity::find_by_id(issue_id.clone())
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.penciller.as_deref(), Some("Mike Deodato, Jr."));

    let (ci_xml, mi_xml) = rewrite_from_db(&app, &issue_id, &path).await;

    assert!(ci_xml.contains("<Writer>Wanda Writer</Writer>"), "{ci_xml}");
    assert!(
        ci_xml.contains("<Penciller>Mike Deodato, Jr.</Penciller>"),
        "{ci_xml}"
    );
    assert_eq!(
        mi_xml.matches("<Credit>").count(),
        2,
        "one <Credit> per creator\n{mi_xml}"
    );
    assert_eq!(
        mi_xml
            .matches("<Creator>Mike Deodato, Jr.</Creator>")
            .count(),
        1,
        "{mi_xml}"
    );
    assert!(!mi_xml.contains("<Creator>Jr.</Creator>"), "{mi_xml}");
    assert!(
        !mi_xml.contains("<Creator>Mike Deodato</Creator>"),
        "{mi_xml}"
    );
    assert_eq!(assert_metron_credit_shape(&mi_xml), 2);

    // Round trip: both parsers hand back the same two credits.
    let mi = parsers::metroninfo::parse(mi_xml.as_bytes()).expect("MetronInfo parses");
    assert_eq!(
        mi.credits.get("Writer").map(Vec::as_slice),
        Some(["Wanda Writer".to_string()].as_slice())
    );
    assert_eq!(
        mi.credits.get("Penciller").map(Vec::as_slice),
        Some(["Mike Deodato, Jr.".to_string()].as_slice())
    );
    assert_eq!(mi.credits.len(), 2);
    let ci = parsers::comicinfo::parse(ci_xml.as_bytes()).expect("ComicInfo parses");
    assert_eq!(ci.writer.as_deref(), Some("Wanda Writer"));
    assert_eq!(ci.penciller.as_deref(), Some("Mike Deodato, Jr."));
    // …and the scanner's own splitter re-ingests the penciller as one
    // person, spelled the way the junction normalises it.
    assert_eq!(
        metadata_rollup::split_csv(ci.penciller.as_deref().unwrap()),
        vec!["Mike Deodato Jr.".to_string()]
    );
}

/// The junction rows are the truth; a read-cache column that disagrees
/// with them (stale, or hand-written around the writers) never reaches
/// the file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn junction_rows_win_over_a_stale_csv_column() {
    let app = TestApp::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (issue_id, path) = seed_writeback_issue(&app, dir.path()).await;
    let db = &app.state().db;

    let set_by = SetBy::Provider(Source::ComicVine);
    let batch = CsvRebuildBatch::new();
    let writer = writers::upsert_person(db, "Wanda Writer", &[], set_by)
        .await
        .unwrap();
    writers::set_issue_credits(
        db,
        &issue_id,
        vec![(writer, "Writer".into(), 0)],
        set_by,
        None,
        &batch,
    )
    .await
    .unwrap();
    let alana = writers::upsert_character(db, "Alana", &[], set_by)
        .await
        .unwrap();
    let marko = writers::upsert_character(db, "Marko", &[], set_by)
        .await
        .unwrap();
    writers::set_issue_characters(
        db,
        &issue_id,
        vec![(alana, true, false), (marko, false, false)],
        set_by,
        None,
        &batch,
    )
    .await
    .unwrap();
    writers::set_issue_genres(
        db,
        &issue_id,
        vec!["Science Fiction".into(), "Romance".into()],
        set_by,
        None,
        &batch,
    )
    .await
    .unwrap();
    batch.flush(db).await.unwrap();

    // Stale the read-cache columns behind the writers' back.
    let mut am: issue::ActiveModel = issue::Entity::find_by_id(issue_id.clone())
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    am.writer = Set(Some("Stale Writer".into()));
    am.characters = Set(Some("Stale Character".into()));
    am.genre = Set(Some("Stale Genre".into()));
    am.update(db).await.unwrap();

    let (ci_xml, mi_xml) = rewrite_from_db(&app, &issue_id, &path).await;

    assert!(ci_xml.contains("<Writer>Wanda Writer</Writer>"), "{ci_xml}");
    assert!(
        ci_xml.contains("<Characters>Alana, Marko</Characters>"),
        "{ci_xml}"
    );
    assert!(
        ci_xml.contains("<Genre>Science Fiction, Romance</Genre>"),
        "{ci_xml}"
    );
    assert!(!ci_xml.contains("Stale"), "{ci_xml}");

    let mi = parsers::metroninfo::parse(mi_xml.as_bytes()).expect("MetronInfo parses");
    assert_eq!(mi.writer().as_deref(), Some("Wanda Writer"));
    assert_eq!(mi.characters, ["Alana", "Marko"]);
    assert_eq!(mi.genres, ["Science Fiction", "Romance"]);
    assert!(!mi_xml.contains("Stale"), "{mi_xml}");
}

// ───────── MetronInfo v1.2 schema (audit 2026-11 WP A4) ─────────

/// A rich issue — every block the composer can emit — rewrites to a
/// MetronInfo.xml that validates against the v1.2 XSD, carries the
/// schema shapes other tools read, and reads back onto the same struct.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rich_metroninfo_validates_against_the_v1_2_schema() {
    let app = TestApp::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let (issue_id, path) = seed_writeback_issue(&app, dir.path()).await;
    let db = &app.state().db;

    // Series: publisher / imprint / years / status / count / sort name.
    let row = issue::Entity::find_by_id(issue_id.clone())
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut sam: series::ActiveModel = series::Entity::find_by_id(row.series_id)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    sam.publisher = Set(Some("Image Comics".into()));
    sam.imprint = Set(Some("Skybound".into()));
    sam.year = Set(Some(2012));
    sam.year_end = Set(Some(2018));
    sam.status = Set("ended".into());
    sam.total_issues = Set(Some(54));
    sam.sort_name = Set(Some("Saga".into()));
    sam.series_type = Set(Some("Ongoing Series".into()));
    sam.summary = Set(Some("Two soldiers from opposite sides of a war.".into()));
    sam.update(db).await.unwrap();
    // Issue scalars.
    let mut am: issue::ActiveModel = row.into();
    am.title = Set(Some("Chapter One".into()));
    am.number_raw = Set(Some("1".into()));
    am.year = Set(Some(2012));
    am.month = Set(Some(3));
    am.day = Set(Some(14));
    am.store_date = Set(chrono::NaiveDate::from_ymd_opt(2012, 3, 14));
    am.summary = Set(Some("Hazel is born. \"Quoted\" & <escaped>.".into()));
    am.notes = Set(Some("Fixture notes.".into()));
    am.age_rating = Set(Some("Mature 17+".into()));
    am.language_code = Set(Some("en".into()));
    am.web_url = Set(Some("https://metron.cloud/issue/saga-2012-1/".into()));
    am.additional_links = Set(serde_json::json!([
        {"label": "ComicVine", "url": "https://comicvine.gamespot.com/saga-1/4000-12345/"},
        {"url": "https://metron.cloud/issue/saga-2012-1/"}
    ]));
    am.page_count = Set(Some(44));
    am.update(db).await.unwrap();

    // External ids: three enumerated sources, two barcodes, one with no
    // schema slot (asin), and the series' Metron id.
    let set_by = SetBy::Provider(Source::Metron);
    for (src, id) in [
        (Source::Metron, "290431"),
        (Source::ComicVine, "4000-12345"),
        (Source::Gcd, "543"),
        (Source::Upc, "70985300470200111"),
        (Source::Isbn, "9781607066019"),
        (Source::Asin, "B00ABCDEF0"),
    ] {
        writers::set_external_id(db, "issue", &issue_id, &Identifier::new(src, id), set_by)
            .await
            .unwrap();
    }
    let series_id = issue::Entity::find_by_id(issue_id.clone())
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .series_id;
    writers::set_external_id(
        db,
        "series",
        &series_id.to_string(),
        &Identifier::new(Source::Metron, "65478"),
        set_by,
    )
    .await
    .unwrap();

    // Junctions: credits, cast, arcs with a position, genres, tags,
    // universes, reprints.
    let batch = CsvRebuildBatch::new();
    let writer = writers::upsert_person(db, "Brian K. Vaughan", &[], set_by)
        .await
        .unwrap();
    let artist = writers::upsert_person(db, "Fiona Staples", &[], set_by)
        .await
        .unwrap();
    writers::set_issue_credits(
        db,
        &issue_id,
        vec![
            (writer, "Writer".into(), 0),
            (artist, "Penciller".into(), 0),
            (artist, "CoverArtist".into(), 0),
        ],
        set_by,
        None,
        &batch,
    )
    .await
    .unwrap();
    let alana = writers::upsert_character(db, "Alana", &[], set_by)
        .await
        .unwrap();
    writers::set_issue_characters(
        db,
        &issue_id,
        vec![(alana, true, false)],
        set_by,
        None,
        &batch,
    )
    .await
    .unwrap();
    let arc = writers::upsert_story_arc(db, "Volume One", &[], set_by)
        .await
        .unwrap();
    writers::set_issue_story_arcs(db, &issue_id, vec![(arc, Some(1))], set_by, None, &batch)
        .await
        .unwrap();
    writers::set_issue_genres(
        db,
        &issue_id,
        vec!["Science Fiction".into(), "Fantasy".into()],
        set_by,
        None,
        &batch,
    )
    .await
    .unwrap();
    writers::set_issue_tags(
        db,
        &issue_id,
        vec!["Image 20th".into()],
        set_by,
        None,
        &batch,
    )
    .await
    .unwrap();
    let universe = writers::upsert_universe(db, "Saga Universe", &[], set_by)
        .await
        .unwrap();
    writers::set_issue_universes(db, &issue_id, vec![universe], set_by, None)
        .await
        .unwrap();
    writers::set_issue_reprints(
        db,
        &issue_id,
        vec![writers::ReprintSpec {
            reprinted_issue_id: None,
            reprinted_label: Some("Saga: Compendium One".into()),
            reprinted_source: None,
            reprinted_external_id: None,
        }],
        set_by,
        None,
    )
    .await
    .unwrap();
    batch.flush(db).await.unwrap();

    let (_ci_xml, mi_xml) = rewrite_from_db(&app, &issue_id, &path).await;

    assert_valid_metroninfo(&mi_xml);
    // The schema shapes, in XSD order.
    let blocks = [
        "  <IDS>\n    <ID source=\"Comic Vine\">4000-12345</ID>\n    \
         <ID source=\"Grand Comics Database\">543</ID>\n    \
         <ID source=\"Metron\" primary=\"true\">290431</ID>\n  </IDS>\n",
        "  <Publisher>\n    <Name>Image Comics</Name>\n    <Imprint>Skybound</Imprint>\n  </Publisher>\n",
        "  <Series id=\"65478\" lang=\"en\">\n    <Name>Saga</Name>\n    <SortName>Saga</SortName>\n    \
         <Format>Single Issue</Format>\n    <StartYear>2012</StartYear>\n    <EndYear>2018</EndYear>\n    \
         <Status>Completed</Status>\n    <IssueCount>54</IssueCount>\n    \
         <Summary>Two soldiers from opposite sides of a war.</Summary>\n  </Series>\n",
        "  <Number>1</Number>\n",
        "  <Stories>\n    <Story>Chapter One</Story>\n  </Stories>\n",
        "  <Summary>Hazel is born. \"Quoted\" &amp; &lt;escaped&gt;.</Summary>\n",
        "  <CoverDate>2012-03-14</CoverDate>\n",
        "  <StoreDate>2012-03-14</StoreDate>\n",
        "  <PageCount>44</PageCount>\n",
        "  <Notes>Fixture notes.</Notes>\n",
        "  <Genres>\n    <Genre>Science Fiction</Genre>\n    <Genre>Fantasy</Genre>\n  </Genres>\n",
        "  <Tags>\n    <Tag>Image 20th</Tag>\n  </Tags>\n",
        "  <Arcs>\n    <Arc>\n      <Name>Volume One</Name>\n      <Number>1</Number>\n    </Arc>\n  </Arcs>\n",
        "  <Characters>\n    <Character>Alana</Character>\n  </Characters>\n",
        "  <Universes>\n    <Universe>\n      <Name>Saga Universe</Name>\n    </Universe>\n  </Universes>\n",
        "  <Reprints>\n    <Reprint>Saga: Compendium One</Reprint>\n  </Reprints>\n",
        "  <GTIN>\n    <ISBN>9781607066019</ISBN>\n    <UPC>70985300470200111</UPC>\n  </GTIN>\n",
        "  <AgeRating>Mature</AgeRating>\n",
        "  <URLs>\n    <URL primary=\"true\">https://metron.cloud/issue/saga-2012-1/</URL>\n    \
         <URL>https://comicvine.gamespot.com/saga-1/4000-12345/</URL>\n  </URLs>\n",
    ];
    let mut cursor = 0;
    for block in blocks {
        let at = mi_xml[cursor..]
            .find(block)
            .unwrap_or_else(|| panic!("missing or out of order:\n{block}\n--- in ---\n{mi_xml}"));
        cursor += at + block.len();
    }
    assert_eq!(assert_metron_credit_shape(&mi_xml), 2, "{mi_xml}");
    assert!(
        mi_xml.contains("<Creator>Fiona Staples</Creator>\n      <Roles>\n        <Role>Cover</Role>\n        <Role>Penciller</Role>"),
        "{mi_xml}"
    );
    assert!(
        !mi_xml.contains("B00ABCDEF0"),
        "asin has no schema slot\n{mi_xml}"
    );
    for old in [
        "<Title>",
        "<Year>",
        "<Month>",
        "<Day>",
        "<Language>",
        "<StoryArcs>",
        "\n  <ID ",
    ] {
        assert!(!mi_xml.contains(old), "pre-schema {old} written\n{mi_xml}");
    }
    // It reads back, and the DB-only compose is a fixed point.
    let mi = parsers::metroninfo::parse(mi_xml.as_bytes()).expect("parses");
    assert_eq!(mi.primary_id_source.as_deref(), Some("metron"));
    assert_eq!(mi.series_id.as_deref(), Some("65478"));
    assert_eq!(mi.arc_numbers.get("Volume One"), Some(&1));
    assert_eq!(mi.universes, ["Saga Universe"]);
    assert_eq!(mi.reprints, ["Saga: Compendium One"]);
    assert_eq!(
        mi.ids.get("upc").map(String::as_str),
        Some("70985300470200111")
    );
    assert_eq!(
        mi.ids.get("isbn").map(String::as_str),
        Some("9781607066019")
    );
    assert_eq!(
        parsers::metroninfo::serialize(&mi),
        mi_xml,
        "serialize ∘ parse is stable"
    );
}

/// An archive Folio wrote in the pre-schema shape (every production
/// archive until it is rewritten): the rescan ingests it as before, the
/// rewrite upgrades it to the v1.2 shape, and a rescan of the upgraded
/// archive reproduces every column and external id. The parsed structs
/// of both shapes agree too, modulo the two documented conventions
/// (`<Manga>` has no schema slot; `<AgeRating>` is the schema value).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_shape_archive_rescans_without_loss_after_the_schema_rewrite() {
    let app = TestApp::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("The Parity Patrol (2021)");
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("The Parity Patrol 003 (2021).cbz");
    let legacy = legacy_shape_metroninfo();
    assert!(
        legacy.contains("<Title>") && legacy.contains("<StoryArcs>"),
        "fixture is pre-schema"
    );
    {
        let f = std::fs::File::create(&path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("page-001.png", opts).unwrap();
        zw.write_all(b"\x89PNG\r\n\x1a\nlegacy-shape-fixture")
            .unwrap();
        zw.start_file("MetronInfo.xml", opts).unwrap();
        zw.write_all(legacy.as_bytes()).unwrap();
        zw.finish().unwrap();
    }
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    let state = app.state();

    // 1. Ingest of the pre-schema archive.
    scanner::scan_library(&state, lib_id).await.expect("scan");
    let (row, before) = ingested(&app, lib_id).await;
    assert_eq!(
        row.title.as_deref(),
        Some("Who's Afraid of <Diffs> & Drift?")
    );
    assert_eq!(
        (row.year, row.month, row.day),
        (Some(2021), Some(7), Some(14))
    );
    assert_eq!(row.imprint.as_deref(), Some("Fixture Press"));
    assert_eq!(
        before.external_ids.get("comicvine").map(String::as_str),
        Some("123456")
    );
    assert_eq!(row.story_arc.as_deref(), Some("The Round Trip"));
    assert_eq!(row.writer.as_deref(), Some("Wanda Writer, Second Scribe"));

    // 2. The rewrite upgrades the file to the schema shape.
    let (_ci, mi_xml) = rewrite_from_db(&app, &row.id, &path).await;
    assert_valid_metroninfo(&mi_xml);
    assert!(
        mi_xml.contains("<ID source=\"Comic Vine\" primary=\"true\">123456</ID>"),
        "{mi_xml}"
    );
    assert!(
        mi_xml.contains("<CoverDate>2021-07-14</CoverDate>"),
        "{mi_xml}"
    );
    assert!(
        mi_xml.contains("<Imprint>Fixture Press</Imprint>"),
        "{mi_xml}"
    );
    assert!(!mi_xml.contains("<Title>"), "{mi_xml}");

    // 3. A rescan of the upgraded archive changes nothing.
    scanner::scan_library(&state, lib_id).await.expect("rescan");
    let (_row, after) = ingested(&app, lib_id).await;
    assert_eq!(after, before);

    // 4. Both shapes parse onto the same struct.
    let old = parsers::metroninfo::parse(legacy.as_bytes()).unwrap();
    let new = parsers::metroninfo::parse(mi_xml.as_bytes()).unwrap();
    let strip = |mut i: parsers::metroninfo::MetronInfo| {
        i.manga = None;
        i.raw.clear();
        i.age_rating = i
            .age_rating
            .as_deref()
            .and_then(parsers::metroninfo::metron_age_rating)
            .map(str::to_owned);
        // The composer adds what the DB knows and the old file lacked.
        i.primary_id_source = None;
        i.series_start_year = None;
        i.series_status = None;
        i.page_count = None;
        i
    };
    assert_eq!(strip(new), strip(old), "{mi_xml}");
}

// ───────── ComicInfo GTIN + StoryArcNumber (audit 2026-11 WP A8) ─────────

/// One-page CBZ whose page bytes differ per `tag` (distinct content
/// hashes), optionally carrying a ComicInfo.xml.
fn tagged_cbz(path: &Path, tag: &str, comic_info: Option<&str>) {
    let f = std::fs::File::create(path).unwrap();
    let mut zw = zip::ZipWriter::new(f);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zw.start_file("page-001.png", opts).unwrap();
    let mut page = b"\x89PNG\r\n\x1a\n".to_vec();
    page.extend_from_slice(tag.as_bytes());
    zw.write_all(&page).unwrap();
    if let Some(xml) = comic_info {
        zw.start_file("ComicInfo.xml", opts).unwrap();
        zw.write_all(xml.as_bytes()).unwrap();
    }
    zw.finish().unwrap();
}

/// Every external id and arc position of one issue: what a rescan must
/// leave exactly as it found it.
#[derive(Debug, PartialEq)]
struct IdsAndArcs {
    external_ids: BTreeMap<String, String>,
    arc_positions: BTreeMap<String, Option<i32>>,
}

async fn ids_and_arcs(app: &TestApp, issue_id: &str) -> IdsAndArcs {
    let db = &app.state().db;
    let external_ids = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq("issue"))
        .filter(external_id::Column::EntityId.eq(issue_id))
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.source, r.external_id))
        .collect();
    let mut arc_positions = BTreeMap::new();
    for row in entity::issue_arc::Entity::find()
        .filter(entity::issue_arc::Column::IssueId.eq(issue_id))
        .all(db)
        .await
        .unwrap()
    {
        let arc = entity::story_arc::Entity::find_by_id(row.arc_id)
            .one(db)
            .await
            .unwrap()
            .expect("arc row");
        arc_positions.insert(arc.name, row.position_in_arc);
    }
    IdsAndArcs {
        external_ids,
        arc_positions,
    }
}

/// A provider- / hand-written issue whose barcode lives only under `upc`
/// (or only under `gtin`) and whose arc positions live only in the
/// junction: ComicInfo carries `<GTIN>` and a parallel `<StoryArcNumber>`,
/// a rescan of the rewritten archive mints no echo id under the other
/// barcode source and keeps every arc position, and a second rewrite is
/// byte-identical. A file-tagged multi-arc issue keeps its own
/// `<StoryArcNumber>` through the rewrite.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gtin_and_arc_numbers_round_trip_through_a_rescan() {
    let app = TestApp::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Saga (2012)");
    std::fs::create_dir_all(&folder).unwrap();
    let paths: Vec<PathBuf> = (1..=3)
        .map(|n| folder.join(format!("Saga {n:03} (2012).cbz")))
        .collect();
    tagged_cbz(&paths[0], "upc", None);
    tagged_cbz(&paths[1], "gtin", None);
    tagged_cbz(
        &paths[2],
        "file-arcs",
        Some(
            "<?xml version=\"1.0\"?>\n<ComicInfo>\n  <Series>Saga</Series>\n  \
             <Number>3</Number>\n  <StoryArc>Zeta, Alpha</StoryArc>\n  \
             <StoryArcNumber>3, 7</StoryArcNumber>\n</ComicInfo>\n",
        ),
    );
    let db = &app.state().db;
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(db)
        .await;
    let state = app.state();
    scanner::scan_library(&state, lib_id).await.expect("scan");
    let issue_id = async |path: &Path| -> String {
        issue::Entity::find()
            .filter(issue::Column::LibraryId.eq(lib_id))
            .filter(issue::Column::FilePath.eq(path.to_string_lossy().into_owned()))
            .one(db)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{} ingested", path.display()))
            .id
    };
    let (upc_issue, gtin_issue, file_issue) = (
        issue_id(&paths[0]).await,
        issue_id(&paths[1]).await,
        issue_id(&paths[2]).await,
    );

    // #1: a provider apply's barcode (`upc` only) and two numbered arcs
    // written to the junction alone (the flat number column stays empty).
    let set_by = SetBy::Provider(Source::Metron);
    writers::set_external_id(
        db,
        "issue",
        &upc_issue,
        &Identifier::new(Source::Upc, "07098530047000111"),
        set_by,
    )
    .await
    .unwrap();
    let batch = CsvRebuildBatch::new();
    let mut arcs = Vec::new();
    for (name, pos) in [("Volume One", 1), ("The War", 3)] {
        let id = writers::upsert_story_arc(db, name, &[], set_by)
            .await
            .unwrap();
        arcs.push((id, Some(pos)));
    }
    writers::set_issue_story_arcs(db, &upc_issue, arcs, set_by, None, &batch)
        .await
        .unwrap();
    batch.flush(db).await.unwrap();
    let row = issue::Entity::find_by_id(upc_issue.clone())
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.story_arc_number, None, "junction-only write");
    // #2, the reverse direction: a ComicInfo-sourced `gtin` row (an
    // ISBN-13, which MetronInfo files under `<ISBN>`).
    writers::set_external_id(
        db,
        "issue",
        &gtin_issue,
        &Identifier::new(Source::Gtin, "9781632154002"),
        SetBy::ComicInfo,
    )
    .await
    .unwrap();

    let (upc_ci, upc_mi) = rewrite_from_db(&app, &upc_issue, &paths[0]).await;
    assert!(
        upc_ci.contains("<GTIN>07098530047000111</GTIN>"),
        "{upc_ci}"
    );
    assert!(
        upc_ci.contains("<StoryArc>Volume One, The War</StoryArc>"),
        "{upc_ci}"
    );
    assert!(
        upc_ci.contains("<StoryArcNumber>1, 3</StoryArcNumber>"),
        "{upc_ci}"
    );
    assert_valid_metroninfo(&upc_mi);
    assert!(upc_mi.contains("<UPC>07098530047000111</UPC>"), "{upc_mi}");
    let (gtin_ci, gtin_mi) = rewrite_from_db(&app, &gtin_issue, &paths[1]).await;
    assert!(gtin_ci.contains("<GTIN>9781632154002</GTIN>"), "{gtin_ci}");
    assert!(gtin_mi.contains("<ISBN>9781632154002</ISBN>"), "{gtin_mi}");
    assert_valid_metroninfo(&gtin_mi);
    let (file_ci, file_mi) = rewrite_from_db(&app, &file_issue, &paths[2]).await;
    // The junction lists the unpositioned arcs by name; each keeps the
    // number the file paired with it.
    assert!(
        file_ci.contains("<StoryArc>Alpha, Zeta</StoryArc>"),
        "{file_ci}"
    );
    assert!(
        file_ci.contains("<StoryArcNumber>7, 3</StoryArcNumber>"),
        "the file's own multi-arc numbers survive the rewrite, paired\n{file_ci}"
    );
    assert!(
        file_mi.contains("<Name>Zeta</Name>\n      <Number>3</Number>"),
        "{file_mi}"
    );

    let before = [
        ids_and_arcs(&app, &upc_issue).await,
        ids_and_arcs(&app, &gtin_issue).await,
        ids_and_arcs(&app, &file_issue).await,
    ];
    assert_eq!(
        before[0].external_ids,
        BTreeMap::from([("upc".to_string(), "07098530047000111".to_string())])
    );
    assert_eq!(
        before[0].arc_positions,
        BTreeMap::from([
            ("The War".to_string(), Some(3)),
            ("Volume One".to_string(), Some(1)),
        ])
    );
    assert_eq!(
        before[1].external_ids,
        BTreeMap::from([("gtin".to_string(), "9781632154002".to_string())])
    );

    // The rescan ingests the rewritten sidecars …
    scanner::scan_library(&state, lib_id).await.expect("rescan");
    let row = issue::Entity::find_by_id(upc_issue.clone())
        .one(db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.story_arc_number.as_deref(),
        Some("1, 3"),
        "the rescan read the rewritten ComicInfo"
    );
    // … and adds no echo id under the other barcode source, nor moves an
    // arc position.
    let after = [
        ids_and_arcs(&app, &upc_issue).await,
        ids_and_arcs(&app, &gtin_issue).await,
        ids_and_arcs(&app, &file_issue).await,
    ];
    assert_eq!(after, before);

    // A second rewrite from the rescanned DB is a fixed point.
    let (again_ci, again_mi) = rewrite_from_db(&app, &upc_issue, &paths[0]).await;
    assert_eq!(again_ci, upc_ci, "ComicInfo rewrite drifted");
    assert_eq!(again_mi, upc_mi, "MetronInfo rewrite drifted");
    let (again_ci, _) = rewrite_from_db(&app, &file_issue, &paths[2]).await;
    assert_eq!(again_ci, file_ci, "file-owned ComicInfo rewrite drifted");
}

/// A first ingest of an archive whose two sidecars name one barcode (the
/// shape every Folio-written archive now has) records it once, under the
/// more specific MetronInfo source.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_ingest_of_a_folio_written_barcode_records_one_row() {
    let app = TestApp::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Saga (2012)");
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("Saga 001 (2012).cbz");
    {
        let f = std::fs::File::create(&path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("page-001.png", opts).unwrap();
        zw.write_all(b"\x89PNG\r\n\x1a\nfirst-ingest").unwrap();
        zw.start_file("ComicInfo.xml", opts).unwrap();
        zw.write_all(
            b"<?xml version=\"1.0\"?>\n<ComicInfo>\n  <Series>Saga</Series>\n  \
              <Number>1</Number>\n  <GTIN>07098530047000111</GTIN>\n</ComicInfo>\n",
        )
        .unwrap();
        zw.start_file("MetronInfo.xml", opts).unwrap();
        zw.write_all(
            b"<?xml version=\"1.0\"?>\n<MetronInfo>\n  <Series>\n    <Name>Saga</Name>\n  \
              </Series>\n  <Number>1</Number>\n  <GTIN>\n    <UPC>0709-8530-0470-00111</UPC>\n  \
              </GTIN>\n</MetronInfo>\n",
        )
        .unwrap();
        zw.finish().unwrap();
    }
    let lib_id = LibrarySeed::new(dir.path())
        .with_sidecar_writeback()
        .insert(&app.state().db)
        .await;
    scanner::scan_library(&app.state(), lib_id)
        .await
        .expect("scan");
    let (_row, ingested) = ingested(&app, lib_id).await;
    assert_eq!(
        ingested.external_ids,
        BTreeMap::from([("upc".to_string(), "0709-8530-0470-00111".to_string())]),
        "one barcode, one row"
    );
}
