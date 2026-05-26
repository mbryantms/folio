//! Matching engine — scores ranked candidates from `MetadataProvider`
//! search calls against the local entity the user wants to identify.
//!
//! Weights are documented in the metadata-providers-1.0 plan (§Matching
//! engine). Short version:
//!
//! Series query:
//! - normalized-name distance: 0.45
//! - year match (±1): 0.20
//! - publisher match (case-insensitive): 0.15
//! - issue-number match (issue queries only): 0.15
//! - volume match: 0.05
//!
//! Total tops out at 100. Buckets:
//! - HIGH   ≥95  — eligible for auto-apply (threshold operator-tunable).
//! - MEDIUM 70-94 — surfaced in the review queue.
//! - LOW    <70  — surfaced with low-confidence flag; never auto-applies.
//!
//! Cover-perceptual-hash distance is a separate weight added in M9 once
//! the post-scan worker writes phashes to `issue_cover`.
//!
//! Score functions are pure: same inputs → same outputs. No DB / HTTP /
//! tracing calls. Trivially unit-testable.

use crate::metadata::provider::{IssueCandidate, SeriesCandidate};

/// Confidence bucket — set by [`Score::bucket`] from the numeric score.
/// Drives the orchestrator's auto-apply / review-queue / discard
/// routing.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    /// Threshold used to compute the bucket. `Score::bucket` honors the
    /// operator-tunable HIGH threshold from `metadata.auto_apply_threshold`
    /// — pass it in rather than reading config here so the matcher stays
    /// pure.
    pub fn from_score(score: f32, high_threshold: f32) -> Self {
        if score >= high_threshold {
            Confidence::High
        } else if score >= 70.0 {
            Confidence::Medium
        } else {
            Confidence::Low
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Score {
    /// 0–100. Sum of weighted component scores.
    pub total: f32,
    /// Per-component breakdown — surfaced in the review UI as a tooltip
    /// so operators can see *why* a candidate scored as it did.
    pub name: f32,
    pub year: f32,
    pub publisher: f32,
    pub issue_number: f32,
    pub volume: f32,
}

impl Score {
    pub fn bucket(self, high_threshold: f32) -> Confidence {
        Confidence::from_score(self.total, high_threshold)
    }
}

// ───────── weights ─────────

const W_NAME: f32 = 45.0;
const W_YEAR: f32 = 20.0;
const W_PUBLISHER: f32 = 15.0;
const W_ISSUE_NUMBER: f32 = 15.0;
/// Volume-number contribution. Provider candidates rarely carry the
/// volume in the search response (only in detail fetches), so today
/// every score lands at 0 here; M3.x can promote candidates to use
/// the actual value once the detail-fetch round-trip is wired.
#[allow(dead_code)]
const W_VOLUME: f32 = 5.0;

// ───────── inputs ─────────

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SeriesQueryFacts {
    pub name: String,
    pub year: Option<i32>,
    pub publisher: Option<String>,
    pub volume: Option<i32>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct IssueQueryFacts {
    pub series_name: String,
    pub series_year: Option<i32>,
    pub publisher: Option<String>,
    pub volume: Option<i32>,
    pub issue_number: String,
}

// ───────── public API ─────────

/// Score a single series candidate against the local series facts.
/// Returns 0–100; never NaN.
pub fn score_series(query: &SeriesQueryFacts, candidate: &SeriesCandidate) -> Score {
    let name = W_NAME * name_similarity(&query.name, &candidate.name);
    let year = W_YEAR * year_similarity(query.year, candidate.year);
    let publisher = W_PUBLISHER
        * publisher_similarity(query.publisher.as_deref(), candidate.publisher.as_deref());
    // Issue-number weight is reserved for issue queries; series queries
    // collapse it to zero so total ranges 0-85 naturally. The threshold
    // tuning accounts for this — `bucket()` is called with the same
    // threshold for both series and issue scores.
    let issue_number = 0.0;
    let volume = 0.0; // SeriesCandidate doesn't carry volume; ignore.
    let total = name + year + publisher + issue_number + volume;
    Score {
        total,
        name,
        year,
        publisher,
        issue_number,
        volume,
    }
}

/// Score a single issue candidate against the local issue facts.
pub fn score_issue(query: &IssueQueryFacts, candidate: &IssueCandidate) -> Score {
    let name = W_NAME
        * name_similarity(
            &query.series_name,
            candidate.series_name.as_deref().unwrap_or(""),
        );
    let year = W_YEAR * year_similarity(query.series_year, candidate.series_year);
    // IssueCandidate has no publisher — let it fall through as a partial
    // match (0.5) so issue queries aren't unfairly penalized. The Apply
    // step pulls the full series detail anyway, which carries publisher.
    let publisher = W_PUBLISHER * 0.5;
    let issue_number = W_ISSUE_NUMBER
        * issue_number_similarity(&query.issue_number, candidate.issue_number.as_deref());
    let volume = 0.0;
    let total = name + year + publisher + issue_number + volume;
    Score {
        total,
        name,
        year,
        publisher,
        issue_number,
        volume,
    }
}

// ───────── similarity primitives ─────────

/// Returns 1.0 for an exact normalized-name match, falling to 0.0 for a
/// completely different string. Uses normalized-Levenshtein:
/// `1 - distance / max(len_a, len_b)`. Both inputs are normalized (case-
/// folded, stripped of leading articles, alphanumerics-only) before the
/// distance is computed.
pub fn name_similarity(a: &str, b: &str) -> f32 {
    let a = normalize_for_match(a);
    let b = normalize_for_match(b);
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    let distance = levenshtein(&a, &b);
    let max_len = a.chars().count().max(b.chars().count()) as f32;
    if max_len <= 0.0 {
        return 0.0;
    }
    (1.0 - (distance as f32 / max_len)).clamp(0.0, 1.0)
}

/// Returns 1.0 for an exact year match, 0.75 for ±1, 0.0 otherwise.
/// Returning 0.5 instead of 0.0 when *either* side is missing makes
/// "no year on the candidate" not penalize the score below medium
/// confidence — provider records for old or obscure runs frequently
/// omit start_year.
pub fn year_similarity(a: Option<i32>, b: Option<i32>) -> f32 {
    match (a, b) {
        (Some(a), Some(b)) => match (a - b).abs() {
            0 => 1.0,
            1 => 0.75,
            _ => 0.0,
        },
        (None, None) => 0.5,
        _ => 0.5,
    }
}

/// Case-insensitive substring match: 1.0 for case-insensitive equality
/// after normalization, 0.7 when one is a substring of the other, 0.0
/// otherwise. Missing on either side scores 0.5 (don't punish lack of
/// signal).
pub fn publisher_similarity(a: Option<&str>, b: Option<&str>) -> f32 {
    match (a, b) {
        (Some(a), Some(b)) => {
            let na = normalize_for_match(a);
            let nb = normalize_for_match(b);
            if na.is_empty() || nb.is_empty() {
                0.5
            } else if na == nb {
                1.0
            } else if na.contains(&nb) || nb.contains(&na) {
                0.7
            } else {
                0.0
            }
        }
        _ => 0.5,
    }
}

/// Issue-number match: 1.0 for parsed-equal numeric values ("1" == "1.0"
/// == "01"), 0.5 when only one side is present, 0.0 for a hard mismatch.
pub fn issue_number_similarity(query: &str, candidate: Option<&str>) -> f32 {
    let Some(candidate) = candidate else {
        return 0.5;
    };
    if query.trim() == candidate.trim() {
        return 1.0;
    }
    let qf: Option<f64> = query.trim().parse().ok();
    let cf: Option<f64> = candidate.trim().parse().ok();
    if let (Some(qf), Some(cf)) = (qf, cf)
        && (qf - cf).abs() < f64::EPSILON
    {
        return 1.0;
    }
    0.0
}

// ───────── helpers ─────────

/// Fold case + strip leading "The ", "A ", "An ", + keep only alphanumerics
/// + collapse repeated whitespace. The result is a deterministic key for
///   comparing comic series names across providers and the local DB.
fn normalize_for_match(s: &str) -> String {
    let lower = s.to_lowercase();
    let trimmed = lower
        .trim()
        .strip_prefix("the ")
        .or_else(|| lower.trim().strip_prefix("a "))
        .or_else(|| lower.trim().strip_prefix("an "))
        .unwrap_or_else(|| lower.trim());
    trimmed
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Standard Levenshtein distance, char-aware (works on non-ASCII without
/// surprises). O(n*m) memory + time; matched strings here are short
/// comic-series names (<60 chars) so the cost is negligible.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (curr[j] + 1)
                .min(prev[j + 1] + 1)
                .min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

// ───────── tests ─────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::identifier::Source;

    fn series_candidate(name: &str, year: Option<i32>, publisher: Option<&str>) -> SeriesCandidate {
        SeriesCandidate {
            source: Source::ComicVine,
            external_id: "1".into(),
            external_url: None,
            name: name.into(),
            year,
            publisher: publisher.map(str::to_owned),
            issue_count: None,
            cover_image_url: None,
            deck: None,
        }
    }

    fn issue_candidate(
        series_name: &str,
        series_year: Option<i32>,
        issue_number: &str,
    ) -> IssueCandidate {
        IssueCandidate {
            source: Source::ComicVine,
            external_id: "1".into(),
            external_url: None,
            issue_number: Some(issue_number.into()),
            name: None,
            cover_date: None,
            series_name: Some(series_name.into()),
            series_year,
            series_external_id: None,
            cover_image_url: None,
        }
    }

    #[test]
    fn normalize_strips_articles_and_punctuation() {
        assert_eq!(normalize_for_match("The Walking Dead"), "walking dead");
        assert_eq!(normalize_for_match("Spider-Man!"), "spiderman");
        assert_eq!(normalize_for_match("X-Men: First Class"), "xmen first class");
        assert_eq!(normalize_for_match("  whitespace   chaos  "), "whitespace chaos");
    }

    #[test]
    fn name_similarity_exact_vs_off_by_one() {
        assert!((name_similarity("Saga", "Saga") - 1.0).abs() < 1e-3);
        // Case + article folding.
        assert!((name_similarity("the saga", "Saga") - 1.0).abs() < 1e-3);
        // One char off in a 4-char string — distance 1 / max 4 → 0.75.
        assert!((name_similarity("Saga", "Sage") - 0.75).abs() < 1e-3);
        // Nothing in common.
        assert!(name_similarity("Saga", "Watchmen") < 0.4);
    }

    #[test]
    fn year_similarity_buckets() {
        assert_eq!(year_similarity(Some(2012), Some(2012)), 1.0);
        assert_eq!(year_similarity(Some(2012), Some(2013)), 0.75);
        assert_eq!(year_similarity(Some(2012), Some(2015)), 0.0);
        // One side missing — partial credit so the candidate isn't
        // hard-penalized.
        assert_eq!(year_similarity(None, Some(2012)), 0.5);
        assert_eq!(year_similarity(Some(2012), None), 0.5);
    }

    #[test]
    fn publisher_similarity_substring_match() {
        assert_eq!(publisher_similarity(Some("Marvel"), Some("Marvel")), 1.0);
        // Case-insensitive equality.
        assert_eq!(publisher_similarity(Some("Image Comics"), Some("image comics")), 1.0);
        // Substring credit.
        assert!((publisher_similarity(Some("DC"), Some("DC Comics")) - 0.7).abs() < 1e-3);
        // Hard mismatch.
        assert_eq!(publisher_similarity(Some("Marvel"), Some("DC")), 0.0);
    }

    #[test]
    fn issue_number_parses_decimal_and_padding() {
        assert_eq!(issue_number_similarity("1", Some("1")), 1.0);
        assert_eq!(issue_number_similarity("1", Some("1.0")), 1.0);
        assert_eq!(issue_number_similarity("1", Some("01")), 1.0);
        assert_eq!(issue_number_similarity("1", Some("2")), 0.0);
        assert_eq!(issue_number_similarity("1.5", Some("1.5")), 1.0);
        // String-only fractional that doesn't parse numerically still
        // wins on string equality.
        assert_eq!(issue_number_similarity("½", Some("½")), 1.0);
        // Missing candidate side falls to partial.
        assert_eq!(issue_number_similarity("1", None), 0.5);
    }

    #[test]
    fn series_perfect_match_scores_high() {
        let q = SeriesQueryFacts {
            name: "Saga".into(),
            year: Some(2012),
            publisher: Some("Image Comics".into()),
            volume: None,
        };
        let c = series_candidate("Saga", Some(2012), Some("Image Comics"));
        let s = score_series(&q, &c);
        // 45 name + 20 year + 15 pub = 80 (max for series query — issue
        // number + volume weights stay zero for series-only matching).
        assert!((s.total - 80.0).abs() < 1e-3);
        // HIGH bucket with the default 75 threshold; MEDIUM with 95.
        assert_eq!(s.bucket(75.0), Confidence::High);
        assert_eq!(s.bucket(95.0), Confidence::Medium);
    }

    #[test]
    fn series_year_drift_lands_medium() {
        let q = SeriesQueryFacts {
            name: "Saga".into(),
            year: Some(2012),
            publisher: Some("Image Comics".into()),
            volume: None,
        };
        let c = series_candidate("Saga", Some(2014), Some("Image Comics"));
        let s = score_series(&q, &c);
        // 45 + 0 (year too far) + 15 = 60 → LOW.
        assert!((s.total - 60.0).abs() < 1e-3);
        assert_eq!(s.bucket(75.0), Confidence::Low);
    }

    #[test]
    fn issue_perfect_match_scores_high() {
        let q = IssueQueryFacts {
            series_name: "Saga".into(),
            series_year: Some(2012),
            publisher: None,
            volume: None,
            issue_number: "1".into(),
        };
        let c = issue_candidate("Saga", Some(2012), "1");
        let s = score_issue(&q, &c);
        // 45 name + 20 year + 7.5 pub (none, half-credit) + 15 issue = 87.5.
        assert!((s.total - 87.5).abs() < 1e-3);
        assert_eq!(s.bucket(80.0), Confidence::High);
        assert_eq!(s.bucket(95.0), Confidence::Medium);
    }

    #[test]
    fn issue_number_mismatch_torpedoes_score() {
        let q = IssueQueryFacts {
            series_name: "Saga".into(),
            series_year: Some(2012),
            publisher: None,
            volume: None,
            issue_number: "1".into(),
        };
        let c = issue_candidate("Saga", Some(2012), "5");
        let s = score_issue(&q, &c);
        // 45 + 20 + 7.5 + 0 = 72.5 — MEDIUM at the 75 threshold.
        assert!((s.total - 72.5).abs() < 1e-3);
        assert_eq!(s.bucket(75.0), Confidence::Medium);
        assert_eq!(s.bucket(95.0), Confidence::Medium);
    }
}
