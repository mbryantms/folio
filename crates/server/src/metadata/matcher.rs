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
    /// Map a total score onto a bucket. Both thresholds are operator-
    /// tunable via the settings registry — `metadata.auto_apply_threshold`
    /// drives HIGH and `metadata.match_medium_threshold` drives MEDIUM
    /// — so calibration is reachable from the admin UI without a
    /// redeploy. Pre-matching-accuracy-M1 the matcher hardcoded
    /// `95 / 70` here, which series text scoring could never reach
    /// (text ceiling = 90); every match landed Medium-or-Low.
    pub fn from_score(score: f32, t: Thresholds) -> Self {
        if score >= t.high {
            Confidence::High
        } else if score >= t.medium {
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

/// Operator-tunable bucket boundaries. Built once per search from
/// the live [`crate::config::Config`] overlay and passed through the
/// orchestrator so every candidate buckets against the same numbers.
///
/// HIGH-side comes from `metadata.auto_apply_threshold` (default 80
/// post-M1); MEDIUM-side from `metadata.match_medium_threshold`
/// (default 60). Inputs are `f32` to avoid an int→float dance at
/// every call.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Thresholds {
    pub high: f32,
    pub medium: f32,
}

impl Thresholds {
    /// Constructor; clamps each value to `[0, 100]` since we want
    /// thresholds in the same units as `Score::total`.
    pub fn new(high: f32, medium: f32) -> Self {
        Self {
            high: high.clamp(0.0, 100.0),
            medium: medium.clamp(0.0, 100.0),
        }
    }
}

impl Default for Thresholds {
    /// The post-M1 defaults — used by the matcher's own unit tests +
    /// any caller that doesn't carry a `Config` (golden-set fixtures,
    /// quick repl drives). Production paths always thread the live
    /// values via `from_config`.
    fn default() -> Self {
        Self::new(80.0, 60.0)
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct Score {
    /// 0–100. Text-only sum of weighted component scores. Post-M4
    /// the cover signal lives in [`Self::cover_hamming`] rather than
    /// being folded into `total` — the bucket() helper consults
    /// cover first and only falls back to `total` when no Hamming
    /// is available.
    pub total: f32,
    /// Per-component breakdown — surfaced in the review UI as a tooltip
    /// so operators can see *why* a candidate scored as it did.
    pub name: f32,
    pub year: f32,
    pub publisher: f32,
    pub issue_number: f32,
    pub volume: f32,
    /// Raw cover-pHash Hamming distance (bits out of 64) when both
    /// local + candidate hashes are present, else `None`. Matching-
    /// accuracy-1.0 M4: this is the **primary** bucket discriminant —
    /// when present, the cover decides the bucket regardless of text
    /// score. Pre-M4 this slot was a `cover_phash: f32` bonus added
    /// to `total`; the inversion is intentional and irreversible
    /// without re-running golden-set calibration.
    pub cover_hamming: Option<u32>,
}

impl Score {
    /// Bucket a candidate. When the cover signal is present, the
    /// ComicTagger Hamming ladder applies (see
    /// [`STRONG_SCORE_THRESH`] / [`MIN_SCORE_THRESH`]) and the text
    /// score is ignored. When absent, fall back to the operator-
    /// tunable text thresholds from M1.
    ///
    /// This is the matching-accuracy-1.0 M4 inversion. Pre-M4 the
    /// matcher used text + a small cover bonus and any candidate
    /// scoring above 95 was HIGH — but text-only ceilings made HIGH
    /// unreachable in practice. After M4 a near-identical cover
    /// match wins HIGH on its own merits, and a wildly different
    /// cover sinks an otherwise-perfect text match to LOW.
    pub fn bucket(self, thresholds: Thresholds) -> Confidence {
        match self.cover_hamming {
            Some(d) if d <= STRONG_SCORE_THRESH => Confidence::High,
            Some(d) if d <= MIN_SCORE_THRESH => Confidence::Medium,
            Some(_) => Confidence::Low,
            None => Confidence::from_score(self.total, thresholds),
        }
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

// ───────── cover-Hamming ladder (matching-accuracy-1.0 M4) ────────
//
// Lifted verbatim from ComicTagger's `IssueIdentifier` defaults
// (`strong_score_thresh=8`, `min_score_thresh=16`,
// `min_score_distance=4`). These are bits out of 64-bit pHash —
// images within 8 bits are visually indistinguishable to a human
// looking for "is this the same cover"; past 16 bits they're
// almost certainly different printings.

/// Cover Hamming distance at or below which a candidate is treated
/// as a **strong** match. M4 changes the bucketing semantics so a
/// strong-cover candidate is HIGH regardless of text score —
/// matches ComicTagger's `strong_score_thresh`.
pub const STRONG_SCORE_THRESH: u32 = 8;

/// Cover Hamming distance ceiling for a MEDIUM bucket — beyond this
/// the cover is decidedly different and the candidate drops to LOW
/// (even if the text scored perfectly). Matches ComicTagger's
/// `min_score_thresh`.
pub const MIN_SCORE_THRESH: u32 = 16;

/// Minimum bit-gap between the top + second cover-Hamming candidates
/// before the top one is allowed to claim HIGH. When two candidates
/// are within `MIN_SCORE_DISTANCE` bits of each other we can't be
/// confident which is right; the winner gets downgraded to MEDIUM
/// so the user picks explicitly. Matches ComicTagger's
/// `min_score_distance`.
pub const MIN_SCORE_DISTANCE: u32 = 4;

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
/// Returns 0–100; never NaN. Convenience wrapper around
/// [`score_series_with_phash`] for the no-phash path — call the
/// `_with_phash` variant directly when both sides have been hashed.
pub fn score_series(query: &SeriesQueryFacts, candidate: &SeriesCandidate) -> Score {
    score_series_with_phash(query, candidate, None, None)
}

/// Like [`score_series`] but also captures the cover-pHash Hamming
/// distance when both sides have a hash. Post-M4 the Hamming feeds
/// the primary bucketing decision (see [`Score::bucket`]); the text
/// `total` is the tiebreaker and the fallback for the no-phash case.
pub fn score_series_with_phash(
    query: &SeriesQueryFacts,
    candidate: &SeriesCandidate,
    local_cover_phash: Option<i64>,
    candidate_cover_phash: Option<i64>,
) -> Score {
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
    let cover_hamming = hamming_distance_opt(local_cover_phash, candidate_cover_phash);
    let total = name + year + publisher + issue_number + volume;
    Score {
        total,
        name,
        year,
        publisher,
        issue_number,
        volume,
        cover_hamming,
    }
}

/// Score a single issue candidate against the local issue facts.
pub fn score_issue(query: &IssueQueryFacts, candidate: &IssueCandidate) -> Score {
    score_issue_with_phash(query, candidate, None, None)
}

/// Like [`score_issue`] but also captures the cover-pHash Hamming
/// distance when both sides have a hash. See
/// [`score_series_with_phash`] for the cover-decides rationale.
pub fn score_issue_with_phash(
    query: &IssueQueryFacts,
    candidate: &IssueCandidate,
    local_cover_phash: Option<i64>,
    candidate_cover_phash: Option<i64>,
) -> Score {
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
    let cover_hamming = hamming_distance_opt(local_cover_phash, candidate_cover_phash);
    let total = name + year + publisher + issue_number + volume;
    Score {
        total,
        name,
        year,
        publisher,
        issue_number,
        volume,
        cover_hamming,
    }
}

/// Convenience: compute Hamming distance only when both sides have
/// a hash. Centralizes the `Option`-unwrap pattern so the score
/// functions stay readable.
fn hamming_distance_opt(a: Option<i64>, b: Option<i64>) -> Option<u32> {
    match (a, b) {
        (Some(x), Some(y)) => Some(crate::metadata::phash::hamming_distance(x, y)),
        _ => None,
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
/// Cover-image perceptual hash similarity. Returns 0..=1.0 — 1.0 for
/// hashes within `0` Hamming distance, scaling linearly down to 0 at
/// `threshold` and beyond. Either-None returns 0 (matcher should
/// fall back to other signals).
///
/// Default `threshold = 20` for `phash` works well across CV/Metron
/// variants per the M9 plan; 8 is the right call for "essentially
/// the same image" matching.
///
/// **Integration status:** the per-candidate search responses don't
/// carry cover hashes today (providers return a thumbnail URL but
/// we'd need to fetch + decode each one to hash, which would burn
/// the per-provider quota during a search). So this helper is
/// surfaced for the Apply-path / diff-preview path where the
/// candidate detail (including the cover URL) is already in hand.
/// Promoting it into [`score_series`] / [`score_issue`] is M9.5.
///
/// metadata-providers-1.0 M9.
pub fn cover_hash_similarity(
    local_hash: Option<i64>,
    candidate_hash: Option<i64>,
    threshold: u32,
) -> f32 {
    match (local_hash, candidate_hash) {
        (Some(a), Some(b)) => {
            let d = crate::metadata::phash::hamming_distance(a, b);
            crate::metadata::phash::similarity_score(d, threshold)
        }
        _ => 0.0,
    }
}

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
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
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
        assert_eq!(
            normalize_for_match("X-Men: First Class"),
            "xmen first class"
        );
        assert_eq!(
            normalize_for_match("  whitespace   chaos  "),
            "whitespace chaos"
        );
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
        assert_eq!(
            publisher_similarity(Some("Image Comics"), Some("image comics")),
            1.0
        );
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
        assert_eq!(s.bucket(Thresholds::new(75.0, 70.0)), Confidence::High);
        assert_eq!(s.bucket(Thresholds::new(95.0, 70.0)), Confidence::Medium);
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
        assert_eq!(s.bucket(Thresholds::new(75.0, 70.0)), Confidence::Low);
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
        assert_eq!(s.bucket(Thresholds::new(80.0, 70.0)), Confidence::High);
        assert_eq!(s.bucket(Thresholds::new(95.0, 70.0)), Confidence::Medium);
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
        assert_eq!(s.bucket(Thresholds::new(75.0, 70.0)), Confidence::Medium);
        assert_eq!(s.bucket(Thresholds::new(95.0, 70.0)), Confidence::Medium);
    }

    // ────────────────────────────────────────────────────────────
    // matching-accuracy-1.0 M1 — operator-tunable thresholds
    // ────────────────────────────────────────────────────────────

    #[test]
    fn default_thresholds_match_post_m1_defaults() {
        let t = Thresholds::default();
        assert!((t.high - 80.0).abs() < 1e-3);
        assert!((t.medium - 60.0).abs() < 1e-3);
    }

    #[test]
    fn default_thresholds_bucket_typical_text_scores() {
        // A 90-score (perfect series text) reaches HIGH under the new
        // defaults — pre-M1 it landed Medium because the matcher
        // hardcoded high=95.
        let s = Score {
            total: 90.0,
            ..Default::default()
        };
        assert_eq!(s.bucket(Thresholds::default()), Confidence::High);

        // A 65-score (one component drift) stays MEDIUM (>=60).
        let s = Score {
            total: 65.0,
            ..Default::default()
        };
        assert_eq!(s.bucket(Thresholds::default()), Confidence::Medium);

        // A 55-score collapses to LOW.
        let s = Score {
            total: 55.0,
            ..Default::default()
        };
        assert_eq!(s.bucket(Thresholds::default()), Confidence::Low);
    }

    #[test]
    fn medium_threshold_is_independent_of_high() {
        // Operator dials HIGH up to 95 but keeps MEDIUM at 60 — score
        // 80 should be MEDIUM (not collapse to LOW).
        let s = Score {
            total: 80.0,
            ..Default::default()
        };
        let strict = Thresholds::new(95.0, 60.0);
        assert_eq!(s.bucket(strict), Confidence::Medium);

        // Same threshold pair, score 50 → LOW (below medium=60).
        let s = Score {
            total: 50.0,
            ..Default::default()
        };
        assert_eq!(s.bucket(strict), Confidence::Low);
    }

    #[test]
    fn thresholds_new_clamps_out_of_range_inputs() {
        // Inputs outside `[0, 100]` get clamped — guards against the
        // settings UI sending `1000` or `-5` after a stray keystroke.
        let t = Thresholds::new(150.0, -25.0);
        assert!((t.high - 100.0).abs() < 1e-3);
        assert!((t.medium - 0.0).abs() < 1e-3);
    }

    // ────────────────────────────────────────────────────────────
    // M4 — cover-pHash as the primary bucket discriminant
    // ────────────────────────────────────────────────────────────

    #[test]
    fn score_captures_cover_hamming_when_both_phashes_present() {
        let q = SeriesQueryFacts {
            name: "Saga".into(),
            year: Some(2012),
            publisher: None,
            volume: None,
        };
        let c = series_candidate("Saga", Some(2012), None);
        let identical = score_series_with_phash(&q, &c, Some(0xABCD), Some(0xABCD));
        assert_eq!(identical.cover_hamming, Some(0));

        // Bit-set diff: 0 vs 0xFF = 8 bits flipped → Hamming 8.
        let off_by_eight = score_series_with_phash(&q, &c, Some(0), Some(0xFF));
        assert_eq!(off_by_eight.cover_hamming, Some(8));
    }

    #[test]
    fn score_cover_hamming_is_none_when_either_side_missing() {
        let q = SeriesQueryFacts {
            name: "Saga".into(),
            year: Some(2012),
            publisher: None,
            volume: None,
        };
        let c = series_candidate("Saga", Some(2012), None);
        let only_local = score_series_with_phash(&q, &c, Some(0x1234), None);
        let only_candidate = score_series_with_phash(&q, &c, None, Some(0x5678));
        let neither = score_series_with_phash(&q, &c, None, None);
        for s in [only_local, only_candidate, neither] {
            assert_eq!(s.cover_hamming, None);
        }
    }

    #[test]
    fn cover_within_strong_thresh_buckets_high_regardless_of_text() {
        // Bad text score (would be LOW on its own) + cover Hamming 4
        // → HIGH because cover decides. M4's central invariant.
        let s = Score {
            total: 30.0,
            cover_hamming: Some(4),
            ..Default::default()
        };
        assert_eq!(s.bucket(Thresholds::default()), Confidence::High);
    }

    #[test]
    fn cover_beyond_min_thresh_buckets_low_even_with_perfect_text() {
        // Perfect text (100) but cover Hamming 30 → LOW because the
        // cover veto overrides the text score.
        let s = Score {
            total: 100.0,
            cover_hamming: Some(30),
            ..Default::default()
        };
        assert_eq!(s.bucket(Thresholds::default()), Confidence::Low);
    }

    #[test]
    fn cover_in_medium_band_buckets_medium() {
        // Hamming 12 sits between STRONG (8) and MIN (16) — MEDIUM.
        let s = Score {
            total: 50.0,
            cover_hamming: Some(12),
            ..Default::default()
        };
        assert_eq!(s.bucket(Thresholds::default()), Confidence::Medium);
    }

    #[test]
    fn no_cover_hash_falls_back_to_text_threshold() {
        // No cover signal → text decides. 90 ≥ 80 (default HIGH).
        let s = Score {
            total: 90.0,
            cover_hamming: None,
            ..Default::default()
        };
        assert_eq!(s.bucket(Thresholds::default()), Confidence::High);

        // 50 < 60 (default MEDIUM) → LOW.
        let s = Score {
            total: 50.0,
            cover_hamming: None,
            ..Default::default()
        };
        assert_eq!(s.bucket(Thresholds::default()), Confidence::Low);
    }

    #[test]
    fn cover_hash_similarity_helper_still_returns_expected_values() {
        // Helper is no longer wired into bucketing but stays in the
        // public API for callers that want a 0..1 similarity (the
        // M5 diff preview surfaces this in the per-field tooltip).
        assert_eq!(cover_hash_similarity(None, None, 20), 0.0);
        assert_eq!(cover_hash_similarity(Some(0), None, 20), 0.0);
        assert_eq!(cover_hash_similarity(Some(0), Some(0), 20), 1.0);
        // 10 bits set on one side → distance 10. similarity = 1 - 10/20 = 0.5.
        assert!((cover_hash_similarity(Some(0), Some(0x3FF), 20) - 0.5).abs() < 1e-3);
    }
}
