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
    Confidence, FORMAT_MISMATCH_PENALTY, IssueQueryFacts, SeriesQueryFacts, Thresholds,
    local_issue_format_hint, score_issue_with_phash, score_series_with_phash,
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
        format: None,
    }
}

/// WP-5.6: series candidate carrying a provider format hint.
fn series_fmt(
    name: &str,
    year: Option<i32>,
    publisher: Option<&str>,
    format: &str,
) -> SeriesCandidate {
    SeriesCandidate {
        format: Some(format.to_owned()),
        ..series(name, year, publisher)
    }
}

fn series_facts(name: &str, year: Option<i32>, publisher: Option<&str>) -> SeriesQueryFacts {
    SeriesQueryFacts {
        name: name.to_owned(),
        year,
        publisher: publisher.map(str::to_owned),
        volume: None,
        format: None,
    }
}

/// WP-5.6: series facts carrying a local format hint (`series_type`).
fn series_facts_fmt(
    name: &str,
    year: Option<i32>,
    publisher: Option<&str>,
    format: &str,
) -> SeriesQueryFacts {
    SeriesQueryFacts {
        format: Some(format.to_owned()),
        ..series_facts(name, year, publisher)
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
        format: None,
    }
}

/// WP-5.6: issue candidate whose series carries a provider format hint.
fn issue_fmt(
    series_name: &str,
    series_year: Option<i32>,
    issue_number: &str,
    format: &str,
) -> IssueCandidate {
    IssueCandidate {
        format: Some(format.to_owned()),
        ..issue(series_name, series_year, issue_number)
    }
}

fn issue_facts(series_name: &str, series_year: Option<i32>, number: &str) -> IssueQueryFacts {
    IssueQueryFacts {
        series_name: series_name.to_owned(),
        series_year,
        publisher: None,
        volume: None,
        issue_number: number.to_owned(),
        issue_year: None,
        format: None,
    }
}

/// WP-5.6: issue facts carrying a local format hint.
fn issue_facts_fmt(
    series_name: &str,
    series_year: Option<i32>,
    number: &str,
    format: Option<String>,
) -> IssueQueryFacts {
    IssueQueryFacts {
        format,
        ..issue_facts(series_name, series_year, number)
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
        SeriesGoldenCase {
            // WP-5.6 TPB <-> TPB: a local trade series (`series_type`
            // in Metron's vocabulary) against a ComicVine volume whose
            // format was inferred from its name. Same class -> no
            // penalty -> 80, HIGH.
            name: "TPB series matched to TPB volume",
            facts: series_facts_fmt("Saga", Some(2012), Some("Image Comics"), "Trade Paperback"),
            candidate: series_fmt("Saga", Some(2012), Some("Image Comics"), "TPB"),
            local_phash: None,
            candidate_phashes: vec![],
        },
        SeriesGoldenCase {
            // Unknown format on the candidate side never penalises.
            name: "ongoing series vs candidate with no format signal",
            facts: series_facts_fmt("Saga", Some(2012), Some("Image Comics"), "Ongoing Series"),
            candidate: series("Saga", Some(2012), Some("Image Comics")),
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
        // ── WP-5.6 format awareness ──
        IssueGoldenCase {
            // A local TPB (ComicInfo Format) against Metron's trade
            // series of the same name. Same class -> 87.5, HIGH.
            name: "TPB volume matched to TPB series",
            facts: issue_facts_fmt("Saga", Some(2012), "1", Some("TPB".into())),
            candidate: issue_fmt("Saga", Some(2012), "1", "Trade Paperback"),
            local_phash: None,
            candidate_phashes: vec![],
        },
        IssueGoldenCase {
            // A local "Annual 1" inside the parent series against the
            // provider's separate "<Series> Annual" series numbered "1".
            // Annual-aware name + number + year window -> 87.5, HIGH.
            name: "annual: local 'Annual 1' <-> provider '<Series> Annual' #1",
            facts: IssueQueryFacts {
                issue_year: Some(2020),
                ..issue_facts("X-Men", Some(2019), "Annual 1")
            },
            candidate: issue("X-Men Annual", Some(2020), "1"),
            local_phash: None,
            candidate_phashes: vec![],
        },
        IssueGoldenCase {
            // Padding / '#' / case variants of the annual marker, with
            // Metron's `series_type` on the candidate.
            name: "annual: 'annual #01' <-> Annual Series #1",
            facts: issue_facts("Amazing Spider-Man", Some(2018), "annual #01"),
            candidate: issue_fmt(
                "Amazing Spider-Man Annual",
                Some(2018),
                "1",
                "Annual Series",
            ),
            local_phash: None,
            candidate_phashes: vec![],
        },
        IssueGoldenCase {
            // Manga volume tagged TPB locally; the provider files each
            // volume as an issue of an ongoing series. Manga is neutral
            // (never penalised) and "Vol. 03" normalises to "3".
            name: "manga volume 'Vol. 03' <-> ongoing-series #3",
            facts: issue_facts_fmt(
                "One Piece",
                Some(2003),
                "Vol. 03",
                local_issue_format_hint(Some("TPB"), None, None, Some("YesAndRightToLeft")),
            ),
            candidate: issue_fmt("One Piece", Some(2003), "3", "Ongoing Series"),
            local_phash: None,
            candidate_phashes: vec![],
        },
        IssueGoldenCase {
            // Same manga volume against a provider that files it as a
            // trade: still neutral, still HIGH.
            name: "manga volume 'v03' <-> Trade Paperback #3",
            facts: issue_facts_fmt(
                "One Piece",
                Some(2003),
                "v03",
                local_issue_format_hint(None, None, None, Some("Yes")),
            ),
            candidate: issue_fmt("One Piece", Some(2003), "3", "Trade Paperback"),
            local_phash: None,
            candidate_phashes: vec![],
        },
        IssueGoldenCase {
            name: "fractional issue '½' <-> '0.5'",
            facts: issue_facts("Amazing Spider-Man", Some(1963), "½"),
            candidate: issue("Amazing Spider-Man", Some(1963), "0.5"),
            local_phash: None,
            candidate_phashes: vec![],
        },
        IssueGoldenCase {
            name: "suffixed issue '14 au' <-> '14AU'",
            facts: issue_facts("Avengers Assemble", Some(2012), "14 au"),
            candidate: issue("Avengers Assemble", Some(2012), "14AU"),
            local_phash: None,
            candidate_phashes: vec![],
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
        SeriesGoldenCase {
            // WP-5.6: an ongoing local series against a trade volume of
            // the same name. Perfect text 80 - 15 penalty = 65 -> MEDIUM.
            name: "ongoing series vs TPB volume",
            facts: series_facts_fmt("Saga", Some(2012), Some("Image Comics"), "ongoing"),
            candidate: series_fmt("Saga", Some(2012), Some("Image Comics"), "TPB"),
            local_phash: None,
            candidate_phashes: vec![],
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
        // ── WP-5.6 format awareness ──
        IssueGoldenCase {
            // Single issue vs the trade that collects it — the classic
            // false match. Text 87.5 - 15 = 72.5 -> MEDIUM.
            name: "TPB vs ongoing: single issue <-> Trade Paperback #1",
            facts: issue_facts_fmt("Saga", Some(2012), "1", Some("Series".into())),
            candidate: issue_fmt("Saga", Some(2012), "1", "Trade Paperback"),
            local_phash: None,
            candidate_phashes: vec![],
        },
        IssueGoldenCase {
            // Same, but the trade's cover *is* issue #1's cover (2 bits).
            // Cover alone would say HIGH; the format cap holds it at
            // MEDIUM so it is never auto-applied.
            name: "TPB vs ongoing: trade reuses the single's cover",
            facts: issue_facts_fmt("Saga", Some(2012), "1", Some("Series".into())),
            candidate: issue_fmt("Saga", Some(2012), "1", "Trade Paperback"),
            local_phash: Some(0),
            candidate_phashes: vec![Some(0x3)],
        },
        IssueGoldenCase {
            // Reverse direction: a local trade (scanner special_type)
            // against an ongoing series' #1.
            name: "TPB vs ongoing: local TPB <-> Ongoing Series #1",
            facts: issue_facts_fmt(
                "Saga",
                Some(2012),
                "1",
                local_issue_format_hint(None, Some("TPB"), None, None),
            ),
            candidate: issue_fmt("Saga", Some(2012), "1", "Ongoing Series"),
            local_phash: None,
            candidate_phashes: vec![],
        },
        IssueGoldenCase {
            // A local annual must not match the parent run's regular #1.
            name: "annual: local 'Annual 1' <-> regular #1",
            facts: issue_facts("X-Men", Some(2019), "Annual 1"),
            candidate: issue("X-Men", Some(2019), "1"),
            local_phash: None,
            candidate_phashes: vec![],
        },
        IssueGoldenCase {
            // Suffix is part of the identity: 14AU is not #14.
            name: "suffixed issue '14AU' <-> '14'",
            facts: issue_facts("Avengers Assemble", Some(2012), "14AU"),
            candidate: issue("Avengers Assemble", Some(2012), "14"),
            local_phash: None,
            candidate_phashes: vec![],
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

// ───────── WP-5.6 format-penalty boundaries ─────────

/// The penalty is soft: a perfect-text mismatch lands exactly
/// `FORMAT_MISMATCH_PENALTY` below the match — below the default HIGH
/// threshold but still at-or-above MEDIUM.
#[test]
fn format_penalty_demotes_perfect_text_to_medium_not_low() {
    let t = Thresholds::default();
    let facts = series_facts_fmt("Saga", Some(2012), Some("Image Comics"), "Ongoing Series");
    let same = score_series_with_phash(
        &facts,
        &series_fmt("Saga", Some(2012), Some("Image Comics"), "Limited Series"),
        None,
        &[],
    );
    let tpb = score_series_with_phash(
        &facts,
        &series_fmt("Saga", Some(2012), Some("Image Comics"), "Trade Paperback"),
        None,
        &[],
    );
    assert!(!same.format_mismatch);
    assert_eq!(same.format, 0.0);
    assert_eq!(same.bucket(t), Confidence::High);
    assert!(tpb.format_mismatch);
    assert!((same.total - tpb.total - FORMAT_MISMATCH_PENALTY).abs() < 1e-3);
    assert!(tpb.total < t.high);
    assert!(tpb.total >= t.medium);
    assert_eq!(tpb.bucket(t), Confidence::Medium);
}

/// Unknown on either side (or a neutral class like manga) never
/// penalises.
#[test]
fn format_penalty_needs_both_sides_known() {
    let cand_tpb = issue_fmt("Saga", Some(2012), "1", "Trade Paperback");
    for local in [
        None,
        Some("Manga".to_owned()),
        Some("Special".to_owned()),
        local_issue_format_hint(Some("Series"), None, None, Some("Yes")),
    ] {
        let s = score_issue_with_phash(
            &issue_facts_fmt("Saga", Some(2012), "1", local.clone()),
            &cand_tpb,
            None,
            &[],
        );
        assert!(!s.format_mismatch, "local {local:?} must not penalise");
        assert_eq!(s.bucket(Thresholds::default()), Confidence::High);
    }
    let s = score_issue_with_phash(
        &issue_facts_fmt("Saga", Some(2012), "1", Some("TPB".into())),
        &issue("Saga", Some(2012), "1"),
        None,
        &[],
    );
    assert!(
        !s.format_mismatch,
        "unknown candidate format must not penalise"
    );
}

/// With a cover signal the mismatch caps HIGH at MEDIUM but never moves
/// MEDIUM or LOW — the ComicTagger ladder is otherwise untouched.
#[test]
fn format_mismatch_caps_cover_high_only() {
    let t = Thresholds::default();
    let facts = issue_facts_fmt("Saga", Some(2012), "1", Some("Series".into()));
    let cand = issue_fmt("Saga", Some(2012), "1", "Trade Paperback");
    let at = |bits: i64| score_issue_with_phash(&facts, &cand, Some(0), &[Some(bits)]).bucket(t);
    assert_eq!(at(0xFF), Confidence::Medium); // 8 bits: HIGH -> capped
    assert_eq!(at(0x1FF), Confidence::Medium); // 9 bits: MEDIUM stays
    assert_eq!(at(0xFFFF), Confidence::Medium); // 16 bits: MEDIUM stays
    assert_eq!(at(0x1FFFF), Confidence::Low); // 17 bits: LOW stays
}

/// A total can't go negative however bad the text is.
#[test]
fn format_penalty_total_floors_at_zero() {
    let s = score_series_with_phash(
        &series_facts_fmt("Saga", None, None, "TPB"),
        &series_fmt("Watchmen", Some(1986), Some("DC"), "Ongoing Series"),
        None,
        &[],
    );
    assert!(s.format_mismatch);
    assert!(s.total >= 0.0);
}
