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
use crate::metadata::title_norm::{
    FormatClass, classify_format, has_annual_token, infer_format_from_title, issue_number_key,
    strip_annual_prefix, strip_annual_token, strip_volume_prefix,
};

/// Confidence bucket — set by [`Score::bucket`] from the numeric score.
/// Drives the orchestrator's auto-apply / manual-review / discard
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
    ///
    /// Matching-accuracy-1.0 M5: when the candidate carries variant
    /// covers, this holds the **minimum** Hamming across primary +
    /// alternates. The `matched_via_alternate` flag flags which side
    /// the min came from so [`Self::bucket`] can apply the stricter
    /// `MIN_ALTERNATE_SCORE_THRESH` ceiling when needed.
    pub cover_hamming: Option<u32>,
    /// True when the winning cover-Hamming came from an alternate /
    /// variant cover rather than the candidate's primary. The
    /// bucketer applies [`MIN_ALTERNATE_SCORE_THRESH`] instead of
    /// [`MIN_SCORE_THRESH`] for the MEDIUM ceiling — a variant
    /// match needs to be tighter to qualify since the candidate's
    /// "real" cover may differ.
    pub matched_via_alternate: bool,
    /// WP-5.6 format component: `-FORMAT_MISMATCH_PENALTY` when the
    /// local entity and the candidate are known to be different kinds
    /// of publication (collected edition vs single issue, annual vs
    /// either), else `0`. Already folded into [`Self::total`].
    pub format: f32,
    /// WP-5.6: true when both sides carry a known, different
    /// [`FormatClass`](crate::metadata::title_norm::FormatClass).
    /// [`Self::bucket`] caps such a candidate at MEDIUM — it is never
    /// auto-applied, even on a strong cover match (a trade's cover is
    /// usually its first issue's cover).
    pub format_mismatch: bool,
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
        // M5: stricter MEDIUM ceiling when the winning cover is an
        // alternate. HIGH stays at ≤ STRONG_SCORE_THRESH either way
        // — a near-perfect cover match is decisive regardless of
        // which slot it came from.
        let medium_ceiling = if self.matched_via_alternate {
            MIN_ALTERNATE_SCORE_THRESH
        } else {
            MIN_SCORE_THRESH
        };
        let bucket = match self.cover_hamming {
            Some(d) if d <= STRONG_SCORE_THRESH => Confidence::High,
            Some(d) if d <= medium_ceiling => Confidence::Medium,
            Some(_) => Confidence::Low,
            None => Confidence::from_score(self.total, thresholds),
        };
        // WP-5.6: a known format mismatch is a *soft* penalty — it
        // demotes HIGH to MEDIUM (review, never auto-apply) but never
        // vetoes to LOW. Same shape as the gap-to-next-best guard.
        if self.format_mismatch && bucket == Confidence::High {
            Confidence::Medium
        } else {
            bucket
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

/// Tighter MEDIUM-band ceiling that applies when the winning cover
/// is an **alternate** (variant) rather than the candidate's primary
/// cover. Mirrors ComicTagger's `min_alternate_score_thresh`. Beyond
/// 12 bits a variant match is too speculative to surface as MEDIUM
/// — the user would have to verify it manually anyway. Primary-cover
/// matches still use [`MIN_SCORE_THRESH`] (16) as the ceiling.
pub const MIN_ALTERNATE_SCORE_THRESH: u32 = 12;

// ───────── format penalty (WP-5.6) ─────────

/// Text points subtracted from [`Score::total`] when the local entity
/// and the candidate are known to be different kinds of publication
/// (see [`crate::metadata::title_norm::FormatClass`]). Fixed, not
/// operator-tunable: sized so a perfect-text candidate (series 80 /
/// issue 87.5) drops out of HIGH (default 80) but stays MEDIUM
/// (default 60) — a soft penalty, not a veto. Unknown format on either
/// side never penalises. Paired with the HIGH→MEDIUM cap in
/// [`Score::bucket`] so a cover match can't auto-apply a TPB onto a
/// single issue either.
pub const FORMAT_MISMATCH_PENALTY: f32 = 15.0;

// ───────── inputs ─────────

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SeriesQueryFacts {
    pub name: String,
    pub year: Option<i32>,
    pub publisher: Option<String>,
    pub volume: Option<i32>,
    /// WP-5.6: local publication-format hint (`series.series_type`),
    /// any vocabulary [`classify_format`] understands. `None` = unknown
    /// (no format penalty). `#[serde(default)]` so stored queries and
    /// in-flight jobs from before WP-5.6 still deserialize.
    #[serde(default)]
    pub format: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct IssueQueryFacts {
    pub series_name: String,
    pub series_year: Option<i32>,
    pub publisher: Option<String>,
    pub volume: Option<i32>,
    pub issue_number: String,
    /// Cover date year of the local issue (i.e. `issue.year`).
    /// Distinct from `series_year`, which is the series *start*
    /// year. Used as the `cover_year` filter on providers that
    /// support it (Metron) — providers without that filter ignore
    /// it. `#[serde(default)]` so older stored queries deserialize
    /// without it.
    #[serde(default)]
    pub issue_year: Option<i32>,
    /// WP-5.6: local publication-format hint, built by
    /// [`local_issue_format_hint`]. `None` = unknown (no format
    /// penalty).
    #[serde(default)]
    pub format: Option<String>,
}

/// Local columns that feed [`local_issue_format_hint`].
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalIssueFormat<'a> {
    /// `issue.format` (ComicInfo `Format`).
    pub issue_format: Option<&'a str>,
    /// `issue.special_type` (scanner classification).
    pub special_type: Option<&'a str>,
    /// `series.series_type`.
    pub series_type: Option<&'a str>,
    /// `issue.manga` (`Yes` / `YesAndRightToLeft` / `No` / …).
    pub manga: Option<&'a str>,
    /// `series.name`.
    pub series_name: &'a str,
    /// `issue.number_raw` as scanned (not a search override).
    pub issue_number: Option<&'a str>,
}

/// Build the local publication-format hint for an issue query from the
/// columns the scanner already populates, first hit wins:
///
/// 1. manga (`issue.manga` = `Yes` / `YesAndRightToLeft`) → `"Manga"`,
///    which classifies as unknown — a manga "issue" is a tankōbon
///    volume and providers file those as either ongoing issues or
///    trades, so neither side may be penalised;
/// 2. `issue.format` (ComicInfo `Format`), verbatim — an explicit tag
///    the matcher doesn't recognise stays unknown;
/// 3. `issue.special_type` when it's `TPB` or `Annual` (`OneShot` is
///    inferred from a missing number and `Special` says nothing about
///    single vs collected, so both are skipped);
/// 4. `series.series_type` when [`classify_format`] recognises it;
/// 5. a collected / annual marker in the series name
///    ([`infer_format_from_title`]: `"Saga TPB"`, `"X-Men Annual"`);
/// 6. **owner default (2026-09-30):** an otherwise untagged issue whose
///    number is plain (integer, decimal, `½`) with no `Annual` /
///    volume marker and no suffix → `"Single"`. A `"Vol. 1"` / `"v03"`
///    number, a suffixed number, or no number stays unknown.
pub fn local_issue_format_hint(local: LocalIssueFormat<'_>) -> Option<String> {
    fn non_empty(v: Option<&str>) -> Option<&str> {
        v.map(str::trim).filter(|s| !s.is_empty())
    }
    if local
        .manga
        .is_some_and(|m| m.trim().to_ascii_lowercase().starts_with("yes"))
    {
        return Some("Manga".to_owned());
    }
    if let Some(f) = non_empty(local.issue_format) {
        return Some(f.to_owned());
    }
    if let Some(st) = non_empty(local.special_type)
        && matches!(st, "TPB" | "Annual")
    {
        return Some(st.to_owned());
    }
    if let Some(st) = non_empty(local.series_type)
        && classify_format(st).is_some()
    {
        return Some(st.to_owned());
    }
    if let Some(label) = infer_format_from_title(local.series_name, None) {
        return Some(label.to_owned());
    }
    let number = non_empty(local.issue_number)?;
    let bare = number.trim_start_matches('#').trim();
    let key = issue_number_key(bare);
    let plain = strip_annual_prefix(bare).is_none()
        && strip_volume_prefix(bare).is_none()
        && !key.annual
        && key.value.is_some()
        && key.suffix.is_empty();
    plain.then(|| "Single".to_owned())
}

/// Known-format-mismatch test shared by the series + issue scorers.
fn formats_conflict(local: Option<FormatClass>, candidate: Option<FormatClass>) -> bool {
    matches!((local, candidate), (Some(a), Some(b)) if a != b)
}

// ───────── public API ─────────

/// Score a single series candidate against the local series facts.
/// Returns 0–100; never NaN. Convenience wrapper around
/// [`score_series_with_phash`] for the no-phash path — call the
/// `_with_phash` variant directly when both sides have been hashed.
pub fn score_series(query: &SeriesQueryFacts, candidate: &SeriesCandidate) -> Score {
    score_series_with_phash(query, candidate, None, &[])
}

/// Like [`score_series`] but also captures the cover-pHash Hamming
/// distance when both sides have a hash. Post-M4 the Hamming feeds
/// the primary bucketing decision (see [`Score::bucket`]); the text
/// `total` is the tiebreaker and the fallback for the no-phash case.
///
/// `candidate_cover_phashes` — index 0 is the primary cover, indices
/// 1..N are alternates. Each slot is `Option<i64>` so a fetch failure
/// can drop just that variant without losing the others. M5: the
/// minimum Hamming wins; `Score::matched_via_alternate` records
/// whether it came from a non-primary slot.
pub fn score_series_with_phash(
    query: &SeriesQueryFacts,
    candidate: &SeriesCandidate,
    local_cover_phash: Option<i64>,
    candidate_cover_phashes: &[Option<i64>],
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
    let (cover_hamming, matched_via_alternate) =
        best_cover_match(local_cover_phash, candidate_cover_phashes);
    // WP-5.6: series-level format check. The "Annual" name token is a
    // fallback signal on both sides (providers file annuals as their
    // own `"<Series> Annual"` series).
    let local_format = query
        .format
        .as_deref()
        .and_then(classify_format)
        .or_else(|| has_annual_token(&query.name).then_some(FormatClass::Annual));
    let candidate_format = candidate
        .format
        .as_deref()
        .and_then(classify_format)
        .or_else(|| has_annual_token(&candidate.name).then_some(FormatClass::Annual));
    let format_mismatch = formats_conflict(local_format, candidate_format);
    let format = if format_mismatch {
        -FORMAT_MISMATCH_PENALTY
    } else {
        0.0
    };
    let total = (name + year + publisher + issue_number + volume + format).max(0.0);
    Score {
        total,
        name,
        year,
        publisher,
        issue_number,
        volume,
        cover_hamming,
        matched_via_alternate,
        format,
        format_mismatch,
    }
}

/// Score a single issue candidate against the local issue facts.
pub fn score_issue(query: &IssueQueryFacts, candidate: &IssueCandidate) -> Score {
    score_issue_with_phash(query, candidate, None, &[])
}

/// Like [`score_issue`] but also captures the cover-pHash Hamming
/// distance when both sides have a hash. See
/// [`score_series_with_phash`] for the cover-decides rationale and
/// the index-0-is-primary convention.
pub fn score_issue_with_phash(
    query: &IssueQueryFacts,
    candidate: &IssueCandidate,
    local_cover_phash: Option<i64>,
    candidate_cover_phashes: &[Option<i64>],
) -> Score {
    let candidate_series = candidate.series_name.as_deref().unwrap_or("");

    // WP-5.6: annual awareness. Locally an annual is usually numbered
    // "Annual 1" inside the parent series; ComicVine and Metron file it
    // as "1" in a separate "<Series> Annual" series. Resolve "is this
    // an annual?" per side from the number, the format, and the series
    // name, then compare like with like.
    let query_key = issue_number_key(&query.issue_number);
    let local_format_raw = query.format.as_deref().and_then(classify_format);
    let candidate_format_raw = candidate.format.as_deref().and_then(classify_format);
    let query_number_annual = query_key.annual;
    let local_annual = query_number_annual
        || local_format_raw == Some(FormatClass::Annual)
        || has_annual_token(&query.series_name);
    let candidate_key = candidate.issue_number.as_deref().map(issue_number_key);
    let candidate_number_annual = candidate_key.as_ref().is_some_and(|k| k.annual);
    let candidate_annual = candidate_number_annual
        || candidate_format_raw == Some(FormatClass::Annual)
        || has_annual_token(candidate_series);

    // Both sides annual → compare the parent titles ("X-Men" vs
    // "X-Men Annual" are the same annual run).
    let name = if local_annual && candidate_annual {
        W_NAME
            * name_similarity(
                &strip_annual_token(&query.series_name),
                &strip_annual_token(candidate_series),
            )
    } else {
        W_NAME * name_similarity(&query.series_name, candidate_series)
    };
    // An annual series starts after its parent run: when both sides are
    // annual, a candidate start year between the parent's start and
    // this annual's cover year is a full year match.
    let annual_year_window = local_annual
        && candidate_annual
        && matches!(
            (query.series_year, query.issue_year, candidate.series_year),
            (Some(lo), Some(hi), Some(c)) if lo <= c && c <= hi
        );
    let year = if annual_year_window {
        W_YEAR
    } else {
        W_YEAR * year_similarity(query.series_year, candidate.series_year)
    };
    // IssueCandidate has no publisher — let it fall through as a partial
    // match (0.5) so issue queries aren't unfairly penalized. The Apply
    // step pulls the full series detail anyway, which carries publisher.
    let publisher = W_PUBLISHER * 0.5;
    let issue_number = W_ISSUE_NUMBER
        * match candidate_key {
            None => 0.5,
            Some(mut ck) => {
                let mut qk = query_key;
                qk.annual = local_annual;
                ck.annual = candidate_annual;
                let raw_equal = local_annual == candidate_annual
                    && candidate.issue_number.as_deref().map(str::trim)
                        == Some(query.issue_number.trim());
                if raw_equal || qk.same_issue(&ck) {
                    1.0
                } else {
                    0.0
                }
            }
        };
    let volume = 0.0;
    let (cover_hamming, matched_via_alternate) =
        best_cover_match(local_cover_phash, candidate_cover_phashes);

    // WP-5.6 format penalty. An "Annual N" number is the most specific
    // signal (it beats an inherited `series_type = ongoing`); otherwise
    // the explicit format, then the annual resolution as a fallback.
    let resolve = |number_annual: bool, raw: Option<FormatClass>, annual: bool| {
        if number_annual {
            Some(FormatClass::Annual)
        } else {
            raw.or_else(|| annual.then_some(FormatClass::Annual))
        }
    };
    let local_format = resolve(query_number_annual, local_format_raw, local_annual);
    let candidate_format = resolve(
        candidate_number_annual,
        candidate_format_raw,
        candidate_annual,
    );
    let format_mismatch = formats_conflict(local_format, candidate_format);
    let format = if format_mismatch {
        -FORMAT_MISMATCH_PENALTY
    } else {
        0.0
    };
    let total = (name + year + publisher + issue_number + volume + format).max(0.0);
    Score {
        total,
        name,
        year,
        publisher,
        issue_number,
        volume,
        cover_hamming,
        matched_via_alternate,
        format,
        format_mismatch,
    }
}

/// Multi-cover Hamming reducer. `candidate_phashes[0]` is the
/// candidate's primary cover; `candidate_phashes[1..]` are alternates
/// in the order the provider listed them. Returns the minimum Hamming
/// distance plus a flag indicating whether the winning cover came
/// from an alternate slot — the bucketer uses the flag to apply
/// [`MIN_ALTERNATE_SCORE_THRESH`] instead of [`MIN_SCORE_THRESH`].
///
/// Matching-accuracy-1.0 M5. Returns `(None, false)` when no
/// (local, candidate) hash pair is present.
fn best_cover_match(local: Option<i64>, candidate_phashes: &[Option<i64>]) -> (Option<u32>, bool) {
    let Some(local) = local else {
        return (None, false);
    };
    let mut best: Option<(u32, bool)> = None;
    for (i, cand) in candidate_phashes.iter().enumerate() {
        let Some(cand) = cand else {
            continue;
        };
        let d = crate::metadata::phash::hamming_distance(local, *cand);
        let from_alt = i > 0;
        best = match best {
            None => Some((d, from_alt)),
            Some((b, _)) if d < b => Some((d, from_alt)),
            other => other,
        };
    }
    match best {
        Some((d, alt)) => (Some(d), alt),
        None => (None, false),
    }
}

// ───────── similarity primitives ─────────

/// Returns 1.0 for an exact normalized-name match, falling toward
/// 0.0 for divergent strings. Matching-accuracy-1.0 M2 ported the
/// pipeline to ComicTagger's normalization shape:
///
/// 1. [`crate::metadata::title_norm::sanitize_title`] folds case +
///    decomposes NFKD + drops articles, so `"The X-Men"` and
///    `"X-Men"` produce the same key.
/// 2. [`crate::metadata::ratcliff::three_pass_ratio`] is the
///    Ratcliff/Obershelp similarity Python's `difflib` uses; the
///    three-pass upper-bound chain short-circuits when the value
///    can't reach the operator-configurable text threshold.
///
/// Pre-M2 this function used Levenshtein on a simpler ASCII-only
/// normalization, which scored "Spider-Man" vs "Spider Man" lower
/// than ComicTagger would and broke matches on accented-character
/// titles ("Pokémon" vs "Pokemon"). The new shape closes that gap.
pub fn name_similarity(a: &str, b: &str) -> f32 {
    let a = crate::metadata::title_norm::sanitize_title(a);
    let b = crate::metadata::title_norm::sanitize_title(b);
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    // Pass `0.0` as the gate so we always get the real ratio — the
    // bucketing path applies its own threshold (M1's
    // `match_medium_threshold` after `* W_NAME`). The three-pass
    // short-circuit is reserved for the M3 pre-filter that needs to
    // discard candidates *before* scoring.
    crate::metadata::ratcliff::three_pass_ratio(&a, &b, 0.0)
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
/// signal). Shares the title sanitizer with [`name_similarity`] so
/// `"DC Comics"` and `"DC"` both normalize to `"dc"` / `"dc comics"`
/// — substring-equal under the M2 rules.
pub fn publisher_similarity(a: Option<&str>, b: Option<&str>) -> f32 {
    match (a, b) {
        (Some(a), Some(b)) => {
            let na = crate::metadata::title_norm::sanitize_title(a);
            let nb = crate::metadata::title_norm::sanitize_title(b);
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

/// Canonicalize an issue number for provider queries + cross-provider
/// comparison. Scanners emit zero-padded numbers ("014"), but providers store
/// the un-padded form ("14"), so filtering/comparing the raw scan value as a
/// string misses. Strips leading-zero padding and a trailing `.0`
/// ("014" → "14", "1.0" → "1") while leaving fractional ("1.5") values alone.
///
/// WP-5.6 extensions (the result is still a string providers can be
/// queried with, so only unambiguous rewrites):
/// - a leading `#` is dropped (`"#12"` → `"12"`);
/// - `Annual` markers normalise to `"Annual N"` (`"annual #01"`,
///   `"Ann. 1"` → `"Annual 1"`);
/// - volume markers are dropped (`"Vol. 03"`, `"v03"` → `"3"`);
/// - a short letter suffix is upper-cased and un-padded (`"014au"`,
///   `"14 AU"` → `"14AU"`); dotted suffixes (`"1.NOW"`) pass through;
/// - vulgar fractions (`"½"`) pass through unchanged — providers store
///   them verbatim; the matcher compares them numerically via
///   [`crate::metadata::title_norm::issue_number_key`];
/// - a trailing parenthesised legacy number is dropped (`"42 (471)"` →
///   `"42"`): GCD writes dual-numbered runs that way (Fantastic Four
///   1998 #42–70 = legacy #471–499, `"500 (71)"`). Only a plain number
///   followed by a parenthesised plain number is rewritten
///   ([`split_legacy_number`]); `"1 (of 4)"` and the like pass through.
pub(crate) fn canonical_issue_number(raw: &str) -> String {
    let t = raw.trim().trim_start_matches('#').trim();
    if let Some((primary, _)) = split_legacy_number(t) {
        return canonical_issue_number(primary);
    }
    if let Some(rest) = strip_annual_prefix(t) {
        // A bare "Annual" has no number to canonicalize (the numeric
        // path would turn "" into "0").
        return if rest.is_empty() {
            "Annual".to_owned()
        } else {
            format!("Annual {}", canonical_issue_number(rest))
        };
    }
    if let Some(rest) = strip_volume_prefix(t) {
        return canonical_issue_number(rest);
    }
    let Some((whole, fraction)) = t.split_once('.') else {
        if t.chars().all(|c| c.is_ascii_digit()) {
            return strip_integer_padding(t).to_owned();
        }
        // "14AU" / "014au" / "14 AU" → "14AU".
        let digits_end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
        let (digits, rest) = t.split_at(digits_end);
        let suffix = rest.trim_start();
        if !digits.is_empty()
            && (1..=4).contains(&suffix.len())
            && suffix.chars().all(|c| c.is_ascii_alphabetic())
        {
            return format!(
                "{}{}",
                strip_integer_padding(digits),
                suffix.to_ascii_uppercase()
            );
        }
        return t.to_string();
    };

    if whole.is_empty()
        || !whole.chars().all(|c| c.is_ascii_digit())
        || !fraction.chars().all(|c| c.is_ascii_digit())
    {
        return t.to_string();
    }

    let whole = strip_integer_padding(whole);
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        whole.to_owned()
    } else {
        format!("{whole}.{fraction}")
    }
}

/// Split a dual-numbered issue number — `"42 (471)"`, `"500 (71)"`,
/// `"#42(471)"` — into its primary number and the parenthesised legacy
/// alias. Both parts must be plain numbers (digits with an optional
/// decimal part; the primary may carry a short letter suffix, `"14AU
/// (52)"`), so annotations such as `"1 (of 4)"`, `"1 (Direct)"` or
/// `"(1)"` are not split.
pub(crate) fn split_legacy_number(raw: &str) -> Option<(&str, &str)> {
    let t = raw.trim().trim_start_matches('#').trim();
    let inner = t.strip_suffix(')')?;
    let open = inner.rfind('(')?;
    let primary = inner[..open].trim();
    let alias = inner[open + 1..].trim();
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let plain = |s: &str| match s.split_once('.') {
        Some((whole, frac)) => digits(whole) && digits(frac),
        None => digits(s),
    };
    let digits_end = primary
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(primary.len());
    let (num, suffix) = primary.split_at(digits_end);
    let suffix_ok =
        suffix.is_empty() || (suffix.len() <= 4 && suffix.chars().all(|c| c.is_ascii_alphabetic()));
    (plain(num) && suffix_ok && plain(alias)).then_some((primary, alias))
}

/// Comparison key for matching a local issue number against a provider
/// listing: [`canonical_issue_number`] with fractions written as decimals
/// (`"½"`, `"1/2"` → `"0.5"`; `"1½"` → `"1.5"`). ComicVine and Metron list
/// Fantastic Four (1998) #½ as `"½"` while the local file says `"0.5"`.
/// Not for provider queries — providers store the glyph verbatim, so the
/// canonical form is what a search must send.
pub(crate) fn issue_number_compare_key(raw: &str) -> String {
    let canonical = canonical_issue_number(raw);
    if !canonical.contains(['½', '¼', '¾', '/']) {
        return canonical;
    }
    let key = crate::metadata::title_norm::issue_number_key(&canonical);
    match key.value {
        Some(v) if !key.annual && key.suffix.is_empty() && v.is_finite() && v >= 0.0 => {
            let s = format!("{v}");
            canonical_issue_number(&s)
        }
        _ => canonical,
    }
}

fn strip_integer_padding(value: &str) -> &str {
    let stripped = value.trim_start_matches('0');
    if stripped.is_empty() { "0" } else { stripped }
}

/// Issue-number match: 1.0 for the same issue ("1" == "1.0" == "01",
/// "½" == "0.5", "14AU" == "14.AU", "Annual 1" == "annual #01"), 0.5
/// when the candidate side is missing, 0.0 for a hard mismatch. Uses
/// [`crate::metadata::title_norm::issue_number_key`]; the issue scorer
/// additionally resolves annual-ness from the series name + format.
pub fn issue_number_similarity(query: &str, candidate: Option<&str>) -> f32 {
    let Some(candidate) = candidate else {
        return 0.5;
    };
    if query.trim() == candidate.trim() {
        return 1.0;
    }
    if issue_number_key(query).same_issue(&issue_number_key(candidate)) {
        1.0
    } else {
        0.0
    }
}

// ───────── helpers ─────────

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
            alternate_cover_urls: Vec::new(),
            format: None,
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
            alternate_cover_urls: Vec::new(),
            format: None,
        }
    }

    #[test]
    fn name_similarity_exact_vs_off_by_one() {
        // Equal after sanitize → 1.0.
        assert!((name_similarity("Saga", "Saga") - 1.0).abs() < 1e-3);
        // Article + case folding via `sanitize_title`.
        assert!((name_similarity("the saga", "Saga") - 1.0).abs() < 1e-3);
        // Ratcliff/Obershelp matches: "sa" + "g" = 3 chars out of 8 →
        // 0.75. Same number Levenshtein would give for this specific
        // 1-edit pair, by coincidence.
        assert!((name_similarity("Saga", "Sage") - 0.75).abs() < 1e-3);
        // Nothing meaningful in common.
        assert!(name_similarity("Saga", "Watchmen") < 0.4);
    }

    #[test]
    fn name_similarity_handles_unicode_and_articles() {
        // M2: NFKD strips the accent in `Pokémon` so it equals
        // `Pokemon` for matching purposes (was 0.778 under pre-M2
        // Levenshtein because of the multi-byte 'é' bumping the
        // char count).
        assert!((name_similarity("Pokémon", "Pokemon") - 1.0).abs() < 1e-3);
        // Article-strip via `sanitize_title`: `It's` → quotes
        // dropped → `its` → article-stripped. Right side has no
        // article. Both reduce to `wonderful life`.
        assert!((name_similarity("It's a Wonderful Life", "Wonderful Life") - 1.0).abs() < 1e-3,);
        // Punctuation differences are word boundaries, not penalties:
        // `Spider-Man` and `Spider Man` both sanitize to `spider man`.
        assert!((name_similarity("Spider-Man", "Spider Man") - 1.0).abs() < 1e-3);
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
    fn canonical_issue_number_strips_padding() {
        // The Spawn #14 case: "014" must canonicalize to "14" so the provider
        // query/compare matches the un-padded value providers store.
        assert_eq!(canonical_issue_number("014"), "14");
        assert_eq!(canonical_issue_number(" 14 "), "14");
        assert_eq!(canonical_issue_number("0"), "0");
        assert_eq!(canonical_issue_number("1.0"), "1");
        assert_eq!(canonical_issue_number("1.50"), "1.5");
        // Non-numeric variants pass through unchanged (trimmed).
        assert_eq!(canonical_issue_number("Annual 1"), "Annual 1");
        assert_eq!(canonical_issue_number("14AU"), "14AU");
        assert_eq!(canonical_issue_number("½"), "½");
    }

    #[test]
    fn canonical_issue_number_drops_gcd_legacy_alias() {
        // GCD's dual numbering for Fantastic Four (1998).
        assert_eq!(canonical_issue_number("42 (471)"), "42");
        assert_eq!(canonical_issue_number("70 (499)"), "70");
        assert_eq!(canonical_issue_number("500 (71)"), "500");
        assert_eq!(canonical_issue_number("#042(471)"), "42");
        assert_eq!(canonical_issue_number("14AU (52)"), "14AU");
        assert_eq!(split_legacy_number("42 (471)"), Some(("42", "471")));
        assert_eq!(split_legacy_number("500 (71)"), Some(("500", "71")));
        assert_eq!(split_legacy_number("605.1 (12.1)"), Some(("605.1", "12.1")));
        // Annotations and lone parentheses are left alone.
        for raw in [
            "1 (of 4)",
            "1 (Direct)",
            "(1)",
            "Annual 1 (2)",
            "42 ()",
            "42 (471",
            "1.NOW (2)",
        ] {
            assert_eq!(split_legacy_number(raw), None, "{raw}");
        }
        assert_eq!(canonical_issue_number("1 (of 4)"), "1 (of 4)");
        // After an Annual marker the remainder is canonicalised like any
        // number, so a dual-numbered annual keeps its primary number.
        assert_eq!(canonical_issue_number("Annual 1 (2)"), "Annual 1");
    }

    #[test]
    fn compare_key_writes_fractions_as_decimals() {
        assert_eq!(issue_number_compare_key("½"), "0.5");
        assert_eq!(issue_number_compare_key("1/2"), "0.5");
        assert_eq!(issue_number_compare_key("000.5"), "0.5");
        assert_eq!(issue_number_compare_key("1½"), "1.5");
        assert_eq!(issue_number_compare_key("¼"), "0.25");
        assert_eq!(issue_number_compare_key("605.1"), "605.1");
        assert_eq!(issue_number_compare_key("42 (471)"), "42");
        assert_eq!(issue_number_compare_key("Annual 1"), "Annual 1");
        assert_eq!(issue_number_compare_key("14AU"), "14AU");
        // The query form keeps the glyph.
        assert_eq!(canonical_issue_number("½"), "½");
    }

    #[test]
    fn canonical_issue_number_normalises_annual_volume_and_suffix() {
        // WP-5.6.
        assert_eq!(canonical_issue_number("#12"), "12");
        assert_eq!(canonical_issue_number("annual #01"), "Annual 1");
        assert_eq!(canonical_issue_number("Ann. 1"), "Annual 1");
        assert_eq!(canonical_issue_number("Annual 2019"), "Annual 2019");
        assert_eq!(canonical_issue_number("Vol. 03"), "3");
        assert_eq!(canonical_issue_number("v03"), "3");
        assert_eq!(canonical_issue_number("014au"), "14AU");
        assert_eq!(canonical_issue_number("14 AU"), "14AU");
        // Dotted suffixes and words are left alone.
        assert_eq!(canonical_issue_number("1.NOW"), "1.NOW");
        assert_eq!(canonical_issue_number("Annually"), "Annually");
        assert_eq!(canonical_issue_number("Annual"), "Annual");
        assert_eq!(canonical_issue_number("Venom"), "Venom");
    }

    #[test]
    fn issue_number_similarity_uses_structured_key() {
        assert_eq!(issue_number_similarity("½", Some("0.5")), 1.0);
        assert_eq!(issue_number_similarity("14AU", Some("14.AU")), 1.0);
        assert_eq!(issue_number_similarity("14AU", Some("14")), 0.0);
        assert_eq!(issue_number_similarity("Annual 1", Some("annual #01")), 1.0);
        assert_eq!(issue_number_similarity("Annual 1", Some("1")), 0.0);
    }

    #[test]
    fn local_issue_format_hint_precedence() {
        let base = LocalIssueFormat {
            series_name: "Saga",
            issue_number: Some("1"),
            ..Default::default()
        };
        let hint = |l: LocalIssueFormat<'_>| local_issue_format_hint(l);
        // Manga beats everything (neutral class).
        assert_eq!(
            hint(LocalIssueFormat {
                issue_format: Some("TPB"),
                special_type: Some("TPB"),
                series_type: Some("ongoing"),
                manga: Some("Yes"),
                ..base
            }),
            Some("Manga".into())
        );
        assert_eq!(
            hint(LocalIssueFormat {
                issue_format: Some("TPB"),
                series_type: Some("ongoing"),
                manga: Some("No"),
                ..base
            }),
            Some("TPB".into())
        );
        assert_eq!(
            hint(LocalIssueFormat {
                special_type: Some("Annual"),
                series_type: Some("ongoing"),
                ..base
            }),
            Some("Annual".into())
        );
        // OneShot / Special special_types are skipped.
        assert_eq!(
            hint(LocalIssueFormat {
                special_type: Some("OneShot"),
                series_type: Some("ongoing"),
                ..base
            }),
            Some("ongoing".into())
        );
        // A collected series type wins over the plain-number default.
        assert_eq!(
            hint(LocalIssueFormat {
                series_type: Some("Trade Paperback"),
                ..base
            }),
            Some("Trade Paperback".into())
        );
        // An explicit but unrecognised Format stays verbatim (unknown).
        assert_eq!(
            hint(LocalIssueFormat {
                issue_format: Some("Director's Cut"),
                ..base
            }),
            Some("Director's Cut".into())
        );
    }

    #[test]
    fn untagged_plain_number_defaults_to_single() {
        // Owner decision 2026-09-30.
        let untagged = |series_name, number| {
            local_issue_format_hint(LocalIssueFormat {
                series_name,
                issue_number: number,
                ..Default::default()
            })
        };
        for n in ["1", "014", "#12", "0.5", "½", "1.5", "-1"] {
            assert_eq!(untagged("Saga", Some(n)), Some("Single".into()), "{n}");
        }
        // Volume / annual markers, suffixes, no number → unknown.
        for n in [
            "Vol. 1", "v03", "Annual 1", "14AU", "1.NOW", "Alpha", "", "  ",
        ] {
            assert_eq!(untagged("Saga", Some(n)), None, "{n:?}");
        }
        assert_eq!(untagged("Saga", None), None);
        // A series name that says "trade" / "annual" classifies instead.
        assert_eq!(untagged("Saga TPB", Some("1")), Some("TPB".into()));
        assert_eq!(untagged("X-Men Annual", Some("1")), Some("Annual".into()));
        // An unrecognised series type doesn't block the default.
        assert_eq!(
            local_issue_format_hint(LocalIssueFormat {
                series_type: Some("Magazine"),
                series_name: "Heavy Metal",
                issue_number: Some("3"),
                ..Default::default()
            }),
            Some("Single".into())
        );
    }

    #[test]
    fn annual_number_beats_inherited_series_type() {
        // An "Annual 1" in an ongoing series is an annual — no
        // mismatch against the provider's annual series.
        let q = IssueQueryFacts {
            series_name: "X-Men".into(),
            series_year: Some(2019),
            publisher: None,
            volume: None,
            issue_number: "Annual 1".into(),
            issue_year: Some(2020),
            format: Some("ongoing".into()),
        };
        let mut c = issue_candidate("X-Men Annual", Some(2020), "1");
        c.format = Some("Annual Series".into());
        let s = score_issue(&q, &c);
        assert!(!s.format_mismatch);
        assert_eq!(s.bucket(Thresholds::default()), Confidence::High);
    }

    #[test]
    fn series_perfect_match_scores_high() {
        let q = SeriesQueryFacts {
            name: "Saga".into(),
            year: Some(2012),
            publisher: Some("Image Comics".into()),
            volume: None,
            format: None,
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
            format: None,
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
            issue_year: None,
            format: None,
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
            issue_year: None,
            format: None,
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
            format: None,
        };
        let c = series_candidate("Saga", Some(2012), None);
        let identical = score_series_with_phash(&q, &c, Some(0xABCD), &[Some(0xABCD)]);
        assert_eq!(identical.cover_hamming, Some(0));
        assert!(!identical.matched_via_alternate);

        // Bit-set diff: 0 vs 0xFF = 8 bits flipped → Hamming 8.
        let off_by_eight = score_series_with_phash(&q, &c, Some(0), &[Some(0xFF)]);
        assert_eq!(off_by_eight.cover_hamming, Some(8));
    }

    #[test]
    fn score_cover_hamming_is_none_when_either_side_missing() {
        let q = SeriesQueryFacts {
            name: "Saga".into(),
            year: Some(2012),
            publisher: None,
            volume: None,
            format: None,
        };
        let c = series_candidate("Saga", Some(2012), None);
        let only_local = score_series_with_phash(&q, &c, Some(0x1234), &[None]);
        let only_candidate = score_series_with_phash(&q, &c, None, &[Some(0x5678)]);
        let neither = score_series_with_phash(&q, &c, None, &[]);
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

    // ────────────────────────────────────────────────────────────
    // M5 — alternate-cover support
    // ────────────────────────────────────────────────────────────

    #[test]
    fn best_cover_match_picks_minimum_across_alternates() {
        // Local 0; candidate primary 0xFF (8 bits), alternate 0x3
        // (2 bits). Min = 2, won by alternate.
        let (d, alt) = best_cover_match(Some(0), &[Some(0xFF), Some(0x3)]);
        assert_eq!(d, Some(2));
        assert!(alt, "winner came from alternate slot");

        // Same call, alternate is the WORSE match. Min stays primary.
        let (d, alt) = best_cover_match(Some(0), &[Some(0x3), Some(0xFF)]);
        assert_eq!(d, Some(2));
        assert!(!alt);
    }

    #[test]
    fn best_cover_match_returns_none_when_no_local_or_all_candidate_nones() {
        let (d, alt) = best_cover_match(None, &[Some(0), Some(0)]);
        assert_eq!(d, None);
        assert!(!alt);

        let (d, alt) = best_cover_match(Some(0), &[None, None]);
        assert_eq!(d, None);
        assert!(!alt);
    }

    #[test]
    fn alternate_match_within_strong_thresh_still_buckets_high() {
        // Hamming 4 from alternate slot → HIGH. The strict-alternate
        // threshold only kicks in for the MEDIUM band; HIGH is the
        // same for primary + alternate.
        let s = Score {
            total: 30.0,
            cover_hamming: Some(4),
            matched_via_alternate: true,
            ..Default::default()
        };
        assert_eq!(s.bucket(Thresholds::default()), Confidence::High);
    }

    #[test]
    fn alternate_match_in_medium_band_uses_strict_threshold() {
        // Hamming 14 — primary-source candidate is MEDIUM (≤16);
        // alternate-source candidate drops to LOW (>12).
        let primary = Score {
            total: 30.0,
            cover_hamming: Some(14),
            matched_via_alternate: false,
            ..Default::default()
        };
        assert_eq!(primary.bucket(Thresholds::default()), Confidence::Medium);

        let alternate = Score {
            total: 30.0,
            cover_hamming: Some(14),
            matched_via_alternate: true,
            ..Default::default()
        };
        assert_eq!(alternate.bucket(Thresholds::default()), Confidence::Low);
    }

    #[test]
    fn alternate_match_below_strict_threshold_stays_medium() {
        // Hamming 11 — both primary + alternate land MEDIUM.
        for alt in [false, true] {
            let s = Score {
                total: 30.0,
                cover_hamming: Some(11),
                matched_via_alternate: alt,
                ..Default::default()
            };
            assert_eq!(
                s.bucket(Thresholds::default()),
                Confidence::Medium,
                "alt={alt}",
            );
        }
    }

    #[test]
    fn variants_in_score_input_route_to_alternate_path() {
        // Local 0; candidate primary 0xFFFF (16 bits), alternate
        // 0xF (4 bits). Min = 4 via alternate. With M5 default
        // thresholds, alternate Hamming ≤ STRONG → HIGH.
        let q = SeriesQueryFacts {
            name: "Saga".into(),
            year: Some(2012),
            publisher: None,
            volume: None,
            format: None,
        };
        let c = series_candidate("Saga", Some(2012), None);
        let s = score_series_with_phash(&q, &c, Some(0), &[Some(0xFFFF), Some(0xF)]);
        assert_eq!(s.cover_hamming, Some(4));
        assert!(s.matched_via_alternate);
        assert_eq!(s.bucket(Thresholds::default()), Confidence::High);
    }
}
