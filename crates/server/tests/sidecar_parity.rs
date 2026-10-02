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
use common::seed::LibrarySeed;
use entity::issue;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use server::jobs::rewrite_sidecars::RewriteIssueSidecarsJob;
use server::library::scanner;
use server::metadata::manual_writeback::{self, Actor, IssueEnqueue};
use std::collections::BTreeMap;
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
        .map(|blob| {
            let v: serde_json::Value = serde_json::from_str(blob).unwrap();
            serde_json::from_value(v["args"].clone()).unwrap()
        })
        .collect();
    let n = jobs.len();
    for job in jobs {
        server::jobs::rewrite_sidecars::handle(job, apalis::prelude::Data::new(app.state()))
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
/// walk (not Folio's parser, which also accepts the legacy shape).
fn assert_metron_credit_shape(xml: &str) {
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
    assert!(credits >= 7, "every ComicTagger credit is present\n{xml}");
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
    // …in the MetronInfo schema's credit shape (WP-8.1).
    assert_metron_credit_shape(&metron_xml);

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
