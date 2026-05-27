//! Matching-accuracy-1.0 M9 — golden regression suite.
//!
//! Anchors the matcher's accuracy invariants so they can't silently
//! drift across releases. Two table-driven test cases bracket the
//! known-correct + known-incorrect populations:
//!
//! - `all_known_correct_match_high` walks every fixture in the
//!   HIGH-eligible set and asserts the matcher buckets it HIGH.
//! - `all_known_incorrect_dont_match_high` walks the LOW-eligible
//!   set and asserts no candidate sneaks into the HIGH bucket.
//!
//! Fixtures are inline value objects (no live provider calls, no
//! real cover-image decoding). Cover signals are synthetic i64
//! phashes — the matcher only consumes the Hamming bit-distance, so
//! `Some(0)` + `Some(0xF)` produces a real 4-bit distance the
//! bucketer treats identically to two genuine pHashes.
//!
//! Adding a fixture: see the operator playbook in
//! `docs/dev/matching-accuracy.md` for the full recipe. Short
//! version: append a row to the relevant `GoldenCase` table — the
//! test harness drives every row uniformly. When reporting a missed
//! match in production, capture the (facts, candidate) pair from the
//! `metadata_match_outcome` row and add it here.
//!
//! The seed population intentionally starts small. The harness +
//! playbook are the long-lived deliverable; the test cases grow over
//! time as real misses get curated in.

use server::metadata::identifier::Source;
use server::metadata::matcher::{
    Confidence, IssueQueryFacts, SeriesQueryFacts, Thresholds, score_issue_with_phash,
    score_series_with_phash,
};
use server::metadata::provider::{IssueCandidate, SeriesCandidate};

// ───────── series-shape harness ─────────

struct SeriesGoldenCase {
    /// Human-readable label — printed in assertion failure messages so
    /// a regression names the broken case directly.
    name: &'static str,
    facts: SeriesQueryFacts,
    candidate: SeriesCandidate,
    local_phash: Option<i64>,
    /// `[primary, alternates...]`. Empty when no phash is available.
    candidate_phashes: Vec<Option<i64>>,
}

fn series(name: &str, year: Option<i32>, publisher: Option<&str>) -> SeriesCandidate {
    SeriesCandidate {
        source: Source::ComicVine,
        external_id: name.to_owned(),
        external_url: None,
        name: name.to_owned(),
        year,
        publisher: publisher.map(str::to_owned),
        issue_count: None,
        cover_image_url: None,
        deck: None,
        alternate_cover_urls: Vec::new(),
    }
}

fn series_facts(name: &str, year: Option<i32>, publisher: Option<&str>) -> SeriesQueryFacts {
    SeriesQueryFacts {
        name: name.to_owned(),
        year,
        publisher: publisher.map(str::to_owned),
        volume: None,
    }
}

// ───────── issue-shape harness ─────────

struct IssueGoldenCase {
    name: &'static str,
    facts: IssueQueryFacts,
    candidate: IssueCandidate,
    local_phash: Option<i64>,
    candidate_phashes: Vec<Option<i64>>,
}

fn issue(series_name: &str, series_year: Option<i32>, issue_number: &str) -> IssueCandidate {
    IssueCandidate {
        source: Source::Metron,
        external_id: format!("{series_name}-{issue_number}"),
        external_url: None,
        issue_number: Some(issue_number.to_owned()),
        name: None,
        cover_date: None,
        series_name: Some(series_name.to_owned()),
        series_year,
        series_external_id: None,
        cover_image_url: None,
        alternate_cover_urls: Vec::new(),
    }
}

fn issue_facts(series_name: &str, series_year: Option<i32>, number: &str) -> IssueQueryFacts {
    IssueQueryFacts {
        series_name: series_name.to_owned(),
        series_year,
        publisher: None,
        volume: None,
        issue_number: number.to_owned(),
    }
}

// ───────── known-correct cases ─────────
//
// Each case below must bucket HIGH under the production defaults
// (M1 thresholds: 80 / 60). A regression flips one to MEDIUM-or-LOW
// and fails the test.

fn known_correct_series() -> Vec<SeriesGoldenCase> {
    vec![
        SeriesGoldenCase {
            name: "exact-text + perfect cover",
            facts: series_facts("Saga", Some(2012), Some("Image Comics")),
            candidate: series("Saga", Some(2012), Some("Image Comics")),
            local_phash: Some(0x1234_5678_9ABC_DEF0),
            candidate_phashes: vec![Some(0x1234_5678_9ABC_DEF0)],
        },
        SeriesGoldenCase {
            // Text-only fallback — no phash on either side.
            // Score: 45 (name) + 20 (year) + 15 (publisher) = 80,
            // which sits exactly at the M1 HIGH threshold.
            name: "exact-text, no cover signal",
            facts: series_facts("Saga", Some(2012), Some("Image Comics")),
            candidate: series("Saga", Some(2012), Some("Image Comics")),
            local_phash: None,
            candidate_phashes: vec![],
        },
        SeriesGoldenCase {
            // Cover within STRONG_SCORE_THRESH (8) — HIGH regardless of text.
            // Bad text (different series name) shouldn't sink it.
            name: "cover-decides over weak text",
            facts: series_facts("Saga", Some(2012), Some("Image Comics")),
            candidate: series("Sgaa", Some(2015), Some("Marvel")),
            local_phash: Some(0),
            candidate_phashes: vec![Some(0xF)], // 4 bits flipped
        },
        SeriesGoldenCase {
            // Variant cover wins — primary differs but an alternate
            // is a near-perfect match. M5 invariant.
            name: "alternate-cover wins over off primary",
            facts: series_facts("Saga", Some(2012), None),
            candidate: series("Saga", Some(2012), None),
            local_phash: Some(0),
            candidate_phashes: vec![Some(0xFFFF), Some(0xF)], // primary=16 bits, alt=4 bits
        },
        SeriesGoldenCase {
            // Sanitized title equivalence — article folding in M2
            // makes "The X-Men" and "X-Men" compare equal.
            name: "article folded series name",
            facts: series_facts("The X-Men", Some(1963), Some("Marvel")),
            candidate: series("X-Men", Some(1963), Some("Marvel")),
            local_phash: None,
            candidate_phashes: vec![],
        },
    ]
}

fn known_correct_issues() -> Vec<IssueGoldenCase> {
    vec![
        IssueGoldenCase {
            // Perfect issue text — series name + year + number all match.
            // Score: 45 + 20 + 7.5 (publisher half-credit) + 15 (issue) = 87.5
            name: "exact-text issue, no cover signal",
            facts: issue_facts("Saga", Some(2012), "1"),
            candidate: issue("Saga", Some(2012), "1"),
            local_phash: None,
            candidate_phashes: vec![],
        },
        IssueGoldenCase {
            name: "cover-decides issue match",
            facts: issue_facts("Saga", Some(2012), "1"),
            candidate: issue("Sga", Some(2015), "5"),
            local_phash: Some(0),
            candidate_phashes: vec![Some(0x3)], // 2 bits
        },
    ]
}

// ───────── known-incorrect cases ─────────
//
// Each case must NOT bucket HIGH. Some land MEDIUM (worth surfacing
// in the review queue), others LOW. The harness only asserts
// "not HIGH" — any non-HIGH bucket is acceptable.

fn known_incorrect_series() -> Vec<SeriesGoldenCase> {
    vec![
        SeriesGoldenCase {
            // Wrong publisher + wildly different cover. Pre-M4 this
            // scored ~75 (full name + full year + 0 publisher = 65,
            // tipped over with cover bonus); post-M4 the cover veto
            // sinks it.
            name: "wrong publisher + bad cover",
            facts: series_facts("Saga", Some(2012), Some("Image Comics")),
            candidate: series("Saga", Some(2012), Some("Marvel")),
            local_phash: Some(0),
            candidate_phashes: vec![Some(i64::from_le_bytes([0xFF; 8]))], // 64 bits diff
        },
        SeriesGoldenCase {
            // Right text, wrong cover (Hamming > MIN_SCORE_THRESH=16).
            // M4 cover-veto sinks even a perfect text match.
            name: "right text but wrong cover",
            facts: series_facts("Saga", Some(2012), Some("Image Comics")),
            candidate: series("Saga", Some(2012), Some("Image Comics")),
            local_phash: Some(0),
            candidate_phashes: vec![Some(0xFFFF_FFFF)], // 32 bits diff
        },
        SeriesGoldenCase {
            // Year drift past the M3 pre-filter gate (cand > local + 1)
            // — wouldn't even reach the matcher in practice. Here we
            // exercise the matcher directly; year=0 weight + low text
            // similarity → LOW.
            name: "year drift + different series",
            facts: series_facts("Saga", Some(2012), Some("Image Comics")),
            candidate: series("Aquaman", Some(2018), Some("DC Comics")),
            local_phash: None,
            candidate_phashes: vec![],
        },
        SeriesGoldenCase {
            // M5 strict-alternate ceiling: cover came from an alternate
            // at Hamming 14, primary at 30. Primary-source candidate
            // would be MEDIUM (≤16) but alternate-source drops to LOW
            // (>12).
            name: "alternate-source MEDIUM-ceiling drops to LOW past 12 bits",
            facts: series_facts("Saga", Some(2012), Some("Image Comics")),
            candidate: series("Different Series", Some(2012), None),
            local_phash: Some(0),
            candidate_phashes: vec![
                Some(0x3FFF_FFFF), // primary = 30 bits
                Some(0x3FFF),      // alternate = 14 bits → was MEDIUM at primary-ceiling 16
            ],
        },
    ]
}

fn known_incorrect_issues() -> Vec<IssueGoldenCase> {
    vec![
        IssueGoldenCase {
            // Issue-number mismatch. Score: 45 + 20 + 7.5 + 0 = 72.5
            // (default MEDIUM ceiling = 60 < 72.5 < HIGH = 80) → MEDIUM.
            // Not HIGH — what we care about.
            name: "issue number off",
            facts: issue_facts("Saga", Some(2012), "1"),
            candidate: issue("Saga", Some(2012), "5"),
            local_phash: None,
            candidate_phashes: vec![],
        },
        IssueGoldenCase {
            // Issue match would score perfect on text, but cover Hamming
            // 30 → LOW under M4 cover-veto.
            name: "perfect text + wrong cover",
            facts: issue_facts("Saga", Some(2012), "1"),
            candidate: issue("Saga", Some(2012), "1"),
            local_phash: Some(0),
            candidate_phashes: vec![Some(0x3FFF_FFFF)], // 30 bits
        },
    ]
}

// ───────── drivers ─────────

#[test]
fn all_known_correct_series_match_high() {
    let thresholds = Thresholds::default();
    for case in known_correct_series() {
        let score = score_series_with_phash(
            &case.facts,
            &case.candidate,
            case.local_phash,
            &case.candidate_phashes,
        );
        let bucket = score.bucket(thresholds);
        assert_eq!(
            bucket,
            Confidence::High,
            "expected HIGH for {:?}; got {:?} (score.total={}, cover_hamming={:?}, alt={})",
            case.name,
            bucket,
            score.total,
            score.cover_hamming,
            score.matched_via_alternate,
        );
    }
}

#[test]
fn all_known_correct_issues_match_high() {
    let thresholds = Thresholds::default();
    for case in known_correct_issues() {
        let score = score_issue_with_phash(
            &case.facts,
            &case.candidate,
            case.local_phash,
            &case.candidate_phashes,
        );
        let bucket = score.bucket(thresholds);
        assert_eq!(
            bucket,
            Confidence::High,
            "expected HIGH for {:?}; got {:?} (score.total={}, cover_hamming={:?}, alt={})",
            case.name,
            bucket,
            score.total,
            score.cover_hamming,
            score.matched_via_alternate,
        );
    }
}

#[test]
fn all_known_incorrect_series_dont_match_high() {
    let thresholds = Thresholds::default();
    for case in known_incorrect_series() {
        let score = score_series_with_phash(
            &case.facts,
            &case.candidate,
            case.local_phash,
            &case.candidate_phashes,
        );
        let bucket = score.bucket(thresholds);
        assert_ne!(
            bucket,
            Confidence::High,
            "expected NOT HIGH for {:?}; got HIGH (score.total={}, cover_hamming={:?}, alt={})",
            case.name,
            score.total,
            score.cover_hamming,
            score.matched_via_alternate,
        );
    }
}

#[test]
fn all_known_incorrect_issues_dont_match_high() {
    let thresholds = Thresholds::default();
    for case in known_incorrect_issues() {
        let score = score_issue_with_phash(
            &case.facts,
            &case.candidate,
            case.local_phash,
            &case.candidate_phashes,
        );
        let bucket = score.bucket(thresholds);
        assert_ne!(
            bucket,
            Confidence::High,
            "expected NOT HIGH for {:?}; got HIGH (score.total={}, cover_hamming={:?}, alt={})",
            case.name,
            score.total,
            score.cover_hamming,
            score.matched_via_alternate,
        );
    }
}
