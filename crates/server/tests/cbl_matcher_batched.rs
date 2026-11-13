//! Differential test for the batched CBL matcher.
//!
//! `cbl::matcher::match_entries` answers the series + volume + number
//! fallback for every entry in one statement. The per-entry
//! implementation it replaced is kept (debug builds only) as
//! `reference_match_entries_for_tests`; this file seeds a library that
//! exercises every fallback rule and asserts both give the same outcome
//! for every entry.

mod common;

use chrono::Utc;
use common::TestApp;
use entity::library::ActiveModel as LibraryAM;
use entity::series::normalize_name;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DbBackend, Set, Statement};
use server::cbl::matcher::{
    EntryToMatch, MatchOutcome, MatchStatus, match_entries, reference_match_entries_for_tests,
};
use uuid::Uuid;

async fn sql(db: &sea_orm::DatabaseConnection, q: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        q,
        values,
    ))
    .await
    .unwrap_or_else(|e| panic!("{q}: {e}"));
}

pub async fn create_library(db: &sea_orm::DatabaseConnection) -> Uuid {
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    LibraryAM {
        id: Set(id),
        name: Set(format!("Matcher Lib {id}")),
        root_path: Set(format!("/tmp/matcher-{id}")),
        default_language: Set("eng".into()),
        default_reading_direction: Set("ltr".into()),
        dedupe_by_content: Set(true),
        slug: Set(id.to_string()),
        scan_schedule_cron: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_scan_at: Set(None),
        ignore_globs: Set(serde_json::json!([])),
        report_missing_comicinfo: Set(false),
        file_watch_enabled: Set(false),
        soft_delete_days: Set(30),
        thumbnails_enabled: Set(true),
        thumbnail_format: Set("webp".to_owned()),
        thumbnail_cover_quality: Set(server::library::thumbnails::DEFAULT_COVER_QUALITY as i32),
        thumbnail_page_quality: Set(server::library::thumbnails::DEFAULT_STRIP_QUALITY as i32),
        generate_page_thumbs_on_scan: Set(false),
        allow_archive_writeback: Set(false),
        metadata_writeback_enabled: Set(false),
        archive_backup_retain_count: Set(1),
        archive_backup_retain_days: Set(30),
        archive_writeback_jpeg_quality: Set(92),
        cbr_convert_confirmed_at: Set(None),
        metadata_publisher_blacklist: Set(serde_json::json!([])),
        filename_ignore_leading_numbers: Set(false),
        filename_assume_issue_one: Set(false),
        metadata_auto_apply_strong_matches: Set(false),
        auto_convert_cbr_on_scan: Set(false),
        auto_convert_cb7_on_scan: Set(false),
        trust_fingerprint_on_first_import: Set(false),
        auto_search_new_series: Set(false),
        show_in_sidebar_by_default: Set(true),
    }
    .insert(db)
    .await
    .unwrap();
    id
}

async fn series(
    db: &sea_orm::DatabaseConnection,
    lib: Uuid,
    name: &str,
    year: Option<i32>,
    volume: Option<i32>,
) -> Uuid {
    let id = Uuid::now_v7();
    sql(
        db,
        "INSERT INTO series (id, library_id, name, normalized_name, slug, year, volume) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
        vec![
            id.into(),
            lib.into(),
            name.into(),
            normalize_name(name).into(),
            id.to_string().into(),
            year.into(),
            volume.into(),
        ],
    )
    .await;
    id
}

#[derive(Clone, Copy)]
enum Kind {
    Active,
    Removed,
    Encrypted,
}

async fn issue(
    db: &sea_orm::DatabaseConnection,
    lib: Uuid,
    series_id: Uuid,
    number: &str,
    kind: Kind,
) -> String {
    let id = Uuid::now_v7().to_string();
    let state = match kind {
        Kind::Encrypted => "encrypted",
        _ => "active",
    };
    let removed = matches!(kind, Kind::Removed).then(|| Utc::now().fixed_offset());
    sql(
        db,
        "INSERT INTO issues (id, library_id, series_id, file_path, file_size, file_mtime, \
                             content_hash, slug, number_raw, state, removed_at) \
         VALUES ($1, $2, $3, $4, 1, now(), $1, $1, $5, $6, $7)",
        vec![
            id.clone().into(),
            lib.into(),
            series_id.into(),
            format!("/tmp/matcher/{id}.cbz").into(),
            number.into(),
            state.into(),
            removed.into(),
        ],
    )
    .await;
    id
}

/// Seeds the library the matcher tests run against: every fallback rule
/// has a series here. Returns the ids the explicit assertions need.
pub struct Fixture {
    pub batman_2011_1: String,
    pub saga_1: String,
    pub cv_issue: String,
    pub metron_issue: String,
}

pub async fn seed_fixture(db: &sea_orm::DatabaseConnection) -> Fixture {
    let lib = create_library(db).await;
    // Same normalized name in two years.
    let b2011 = series(db, lib, "Batman", Some(2011), None).await;
    let b2016 = series(db, lib, "Batman", Some(2016), Some(3)).await;
    let mut batman_2011_1 = String::new();
    for n in ["1", "2", "3"] {
        let id = issue(db, lib, b2011, n, Kind::Active).await;
        if n == "1" {
            batman_2011_1 = id;
        }
    }
    for n in ["1", "2"] {
        issue(db, lib, b2016, n, Kind::Active).await;
    }
    // Six more series just above the threshold for "batman" (7/9 ≈ 0.78;
    // "Batman B" 7/8),
    // so "Batman" 2011 #1 has seven candidates and keeps five; and one
    // just below (7/10 = 0.7).
    for suffix in ["A", "B", "C", "D", "E", "F", "Ab"] {
        let s = series(db, lib, &format!("Batman {suffix}"), Some(2011), None).await;
        issue(db, lib, s, "1", Kind::Active).await;
    }
    // Exactly on the threshold: similarity("abcde", "abcde x") = 6/8 = 0.75,
    // which `> 0.75` refuses.
    let ab = series(db, lib, "Abcde X", Some(2000), None).await;
    issue(db, lib, ab, "1", Kind::Active).await;
    // Year match vs volume match.
    let saga = series(db, lib, "Saga", Some(2012), Some(1)).await;
    let saga_1 = issue(db, lib, saga, "1", Kind::Active).await;
    issue(db, lib, saga, "2", Kind::Active).await;
    issue(db, lib, saga, "1.5", Kind::Active).await;
    // Year and volume equal: one candidate, not two.
    let weird = series(db, lib, "Weird Tales", Some(2), Some(2)).await;
    issue(db, lib, weird, "1", Kind::Active).await;
    // Removed and non-active issues.
    let removed = series(db, lib, "Removed Run", Some(2020), None).await;
    issue(db, lib, removed, "1", Kind::Removed).await;
    issue(db, lib, removed, "2", Kind::Active).await;
    let enc = series(db, lib, "Encrypted Run", Some(2020), None).await;
    issue(db, lib, enc, "1", Kind::Encrypted).await;
    issue(db, lib, enc, "2", Kind::Active).await;
    // Two active issues with one number in one series.
    let dup = series(db, lib, "Dup Run", Some(2019), None).await;
    issue(db, lib, dup, "1", Kind::Active).await;
    issue(db, lib, dup, "1", Kind::Active).await;
    issue(db, lib, dup, "2", Kind::Active).await;
    // Id tiers.
    let ids = series(db, lib, "Id Run", Some(2005), None).await;
    let cv_issue = issue(db, lib, ids, "1", Kind::Active).await;
    let metron_issue = issue(db, lib, ids, "2", Kind::Active).await;
    for (entity, source, ext) in [
        (&cv_issue, "comicvine", "1001"),
        (&metron_issue, "metron", "2002"),
    ] {
        sql(
            db,
            "INSERT INTO external_ids (entity_type, entity_id, source, external_id, set_by) \
             VALUES ('issue', $1, $2, $3, 'provider')",
            vec![entity.clone().into(), source.into(), ext.into()],
        )
        .await;
    }
    // Background series for the generated entries.
    for g in 0..20 {
        let year = 1990 + (g % 7);
        let volume = (g % 3 == 0).then_some(g % 4 + 1);
        let s = series(
            db,
            lib,
            &format!("Generated Series {g}"),
            Some(year),
            volume,
        )
        .await;
        for n in 1..=4 {
            issue(db, lib, s, &n.to_string(), Kind::Active).await;
        }
    }
    Fixture {
        batman_2011_1,
        saga_1,
        cv_issue,
        metron_issue,
    }
}

fn entry(name: &str, number: &str, volume: Option<&str>) -> EntryToMatch {
    EntryToMatch {
        series_name: name.to_owned(),
        number: number.to_owned(),
        volume: volume.map(str::to_owned),
        cv_issue_id: None,
        metron_issue_id: None,
    }
}

/// The hand-written cases, one per rule.
fn rule_entries() -> Vec<EntryToMatch> {
    let mut cv_hit = entry("Nothing Like It", "9", Some("1999"));
    cv_hit.cv_issue_id = Some(1001);
    let mut metron_hit = entry("Nothing Like It", "9", Some("1999"));
    metron_hit.cv_issue_id = Some(424_242);
    metron_hit.metron_issue_id = Some(2002);
    let mut nowhere = entry("Saga", "1", Some("2012"));
    nowhere.cv_issue_id = Some(777_777);
    nowhere.metron_issue_id = Some(888_888);
    vec![
        entry("Batman", "1", Some("2011")), // 0: seven candidates, top five
        entry("Batman", "2", Some("2011")), // 1: one
        entry("Batman", "1", Some("2016")), // 2: other year
        entry("Batman", "1", Some("3")),    // 3: volume of the 2016 run
        entry("Batman A", "1", Some("2011")),
        entry("Abcde", "1", Some("2000")), // 5: exactly 0.75 → missing
        entry("Abcde X", "1", Some("2000")), // 6: matched
        entry("Saga", "1", Some("2012")),  // 7: year
        entry("Saga", "1", Some("1")),     // 8: volume
        entry("  Saga  ", " 1 ", Some(" 2012 ")),
        entry("Saga", "1", None),       // 10: no volume
        entry("Saga", "1", Some("  ")), // 11: blank volume
        entry("Saga", "   ", Some("2012")),
        entry("   ", "1", Some("2012")),
        entry("Weird Tales", "1", Some("2")),
        entry("Removed Run", "1", Some("2020")), // 15: removed
        entry("Removed Run", "2", Some("2020")),
        entry("Encrypted Run", "1", Some("2020")), // 17: not active
        entry("Encrypted Run", "2", Some("2020")),
        entry("Dup Run", "1", Some("2019")), // 19: two rows, one series
        entry("Dup Run", "2", Some("2019")),
        cv_hit,     // 21
        metron_hit, // 22
        nowhere,    // 23: ids miss, name matches
        entry("SAGA!", "1.5", Some("2012")),
        entry("Saga", "1.50", Some("2012")),
        entry("Batman", "1", Some("2011")), // duplicate of 0
    ]
}

/// A few hundred pseudo-random entries over names (exact, case and
/// punctuation variants, near misses), volumes (years, volume numbers,
/// junk, none) and numbers.
fn generated_entries(n: usize) -> Vec<EntryToMatch> {
    let names: Vec<String> = [
        "Batman",
        "batman",
        "BATMAN!",
        "Batman A",
        "Batman Ab",
        "Saga",
        "Sagas",
        "Abcde",
        "Abcde X",
        "Dup Run",
        "Removed Run",
        "Encrypted Run",
        "Weird Tales",
        "Id Run",
        "",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .chain((0..20).map(|g| format!("Generated Series {g}")))
    .chain((0..5).map(|g| format!("generated-series {g}")))
    .collect();
    let volumes = [
        None,
        Some(""),
        Some("1"),
        Some("2"),
        Some("3"),
        Some("4"),
        Some("2011"),
        Some("2012"),
        Some("2016"),
        Some("2019"),
        Some("2020"),
        Some("2000"),
        Some("1990"),
        Some("1993"),
        Some("1996"),
        Some(" 1991 "),
        Some("vol"),
    ];
    let numbers = ["1", "2", "3", "4", " 2 ", "1.5", "5", ""];
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    (0..n)
        .map(|_| {
            let name = &names[(next() % names.len() as u64) as usize];
            let volume = volumes[(next() % volumes.len() as u64) as usize];
            let number = numbers[(next() % numbers.len() as u64) as usize];
            let mut e = entry(name, number, volume);
            match next() % 10 {
                0 => e.cv_issue_id = Some(1001),
                1 => e.metron_issue_id = Some(2002),
                2 => e.cv_issue_id = Some((next() % 100_000) as i32),
                _ => {}
            }
            e
        })
        .collect()
}

type Cand = (String, String, u32);

/// Candidates as (issue id, series name, similarity bits), sorted by issue
/// id: the comparison is a set comparison, because the reference leaves
/// rows that tie on (similarity, series name) in no defined order.
fn candidate_set(o: &MatchOutcome) -> Option<Vec<Cand>> {
    o.ambiguous_candidates.as_ref().map(|cs| {
        let mut v: Vec<Cand> = cs
            .iter()
            .map(|c| {
                (
                    c.issue_id.clone(),
                    c.series_name.clone(),
                    c.similarity.to_bits(),
                )
            })
            .collect();
        v.sort();
        v
    })
}

fn assert_same(i: usize, e: &EntryToMatch, new: &MatchOutcome, reference: &MatchOutcome) {
    let ctx = format!("entry {i}: {e:?}\n new: {new:?}\n ref: {reference:?}");
    assert_eq!(new.status, reference.status, "{ctx}");
    assert_eq!(new.issue_id, reference.issue_id, "{ctx}");
    assert_eq!(new.method, reference.method, "{ctx}");
    assert_eq!(
        new.confidence.map(f32::to_bits),
        reference.confidence.map(f32::to_bits),
        "{ctx}"
    );
    assert_eq!(candidate_set(new), candidate_set(reference), "{ctx}");
    if let (Some(n), Some(r)) = (&new.ambiguous_candidates, &reference.ambiguous_candidates) {
        // Where the reference's order is defined (no tie on similarity +
        // series name), the order is the same.
        let key = |c: &server::cbl::matcher::MatchCandidate| {
            (c.similarity.to_bits(), c.series_name.clone())
        };
        let mut keys: Vec<_> = r.iter().map(key).collect();
        keys.dedup();
        if keys.len() == r.len() {
            let ids = |cs: &[server::cbl::matcher::MatchCandidate]| {
                cs.iter().map(|c| c.issue_id.clone()).collect::<Vec<_>>()
            };
            assert_eq!(ids(n), ids(r), "{ctx}");
        }
        // The new order breaks such ties by issue id.
        for w in n.windows(2) {
            if key(&w[0]) == key(&w[1]) {
                assert!(w[0].issue_id < w[1].issue_id, "{ctx}");
            }
        }
    }
}

#[tokio::test]
async fn batched_matcher_agrees_with_the_per_entry_reference() {
    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    let fx = seed_fixture(&db).await;

    let mut entries = rule_entries();
    let rules = entries.len();
    entries.extend(generated_entries(400));

    let new = match_entries(&db, &entries).await.unwrap();
    let reference = reference_match_entries_for_tests(&db, &entries)
        .await
        .unwrap();
    assert_eq!(new.len(), entries.len());
    assert_eq!(reference.len(), entries.len());
    for (i, e) in entries.iter().enumerate() {
        assert_same(i, e, &new[i], &reference[i]);
    }

    // The fixture really exercises the rules (the differential check alone
    // would pass on a fixture that matched nothing).
    use MatchStatus::{Ambiguous, Matched, Missing};
    let status = |i: usize| new[i].status;
    let top = new[0].ambiguous_candidates.as_ref().unwrap();
    assert_eq!(status(0), Ambiguous);
    assert_eq!(top.len(), 5, "seven candidates, five kept");
    assert_eq!(top[0].issue_id, fx.batman_2011_1);
    assert_eq!(top[0].similarity, 1.0);
    assert!(top.iter().all(|c| c.series_name != "Batman Ab"));
    assert_eq!(
        top.iter()
            .map(|c| c.series_name.as_str())
            .collect::<Vec<_>>(),
        // "batman b" shares the "  b" trigram with "batman" (7/8), so it
        // ranks above the 7/9 group, which ties and orders by name.
        ["Batman", "Batman B", "Batman A", "Batman C", "Batman D"]
    );
    assert_eq!(status(1), Matched);
    assert_eq!(new[1].method, Some("series_volume_number"));
    assert_eq!(status(2), Matched);
    assert_eq!(status(3), Matched);
    assert_eq!(
        new[2].issue_id, new[3].issue_id,
        "year and volume reach one issue"
    );
    assert_eq!(
        status(5),
        Missing,
        "similarity exactly 0.75 is not above it"
    );
    assert_eq!(status(6), Matched);
    for i in [7, 8, 9] {
        assert_eq!(
            new[i].issue_id.as_deref(),
            Some(fx.saga_1.as_str()),
            "entry {i}"
        );
    }
    for i in [10, 11, 12, 13, 15, 17] {
        assert_eq!(status(i), Missing, "entry {i}");
    }
    assert_eq!(status(14), Matched, "year = volume is one candidate");
    assert_eq!(status(16), Matched);
    assert_eq!(status(18), Matched);
    assert_eq!(status(19), Ambiguous);
    assert_eq!(new[19].ambiguous_candidates.as_ref().unwrap().len(), 2);
    assert_eq!(status(20), Matched);
    assert_eq!(new[21].issue_id.as_deref(), Some(fx.cv_issue.as_str()));
    assert_eq!(new[21].method, Some("comicvine_id"));
    assert_eq!(new[22].issue_id.as_deref(), Some(fx.metron_issue.as_str()));
    assert_eq!(new[22].method, Some("metron_id"));
    assert_eq!(new[23].issue_id.as_deref(), Some(fx.saga_1.as_str()));
    assert_eq!(new[23].method, Some("series_volume_number"));
    assert_eq!(status(24), Matched, "1.5 by normalized name");
    assert_eq!(status(25), Missing, "number_raw compares as text");

    // The generated half is not degenerate either.
    let generated = &new[rules..];
    for s in [Matched, Ambiguous, Missing] {
        assert!(
            generated.iter().any(|o| o.status == s),
            "no generated entry is {s:?}"
        );
    }
}

#[tokio::test]
async fn batched_matcher_handles_no_entries_and_no_fallback() {
    let app = TestApp::spawn().await;
    let db = app.state().db.clone();
    seed_fixture(&db).await;
    assert!(match_entries(&db, &[]).await.unwrap().is_empty());
    // Only id hits and entries the fallback refuses: no fallback statement.
    let mut cv_hit = entry("x", "1", None);
    cv_hit.cv_issue_id = Some(1001);
    let out = match_entries(&db, &[cv_hit, entry("Saga", "1", None)])
        .await
        .unwrap();
    assert_eq!(out[0].status, MatchStatus::Matched);
    assert_eq!(out[1].status, MatchStatus::Missing);
}
