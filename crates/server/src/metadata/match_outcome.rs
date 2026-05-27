//! Per-run match-outcome telemetry. Companion to the orchestrator.
//!
//! Matching-accuracy-1.0 M0. Lands before any matcher tuning so the
//! plan ships with a **before/after** baseline once the
//! ComicTagger-derived heuristics (M2 / M4 / M5) come online. Without
//! this row the only way to validate the plan would be eyeballing
//! `metadata_run` counts, which doesn't separate "1 candidate but
//! cover doesn't match" from "1 candidate + decisive cover match" —
//! the very signal M4 will introduce.
//!
//! The `MatchOutcomeKind` discriminants are the same names M8 will
//! surface to the dialog UX. M0 classifies on `Confidence` alone
//! (`bad_cover` here really means "weak match"; M4 will tighten that
//! to a literal Hamming-distance comparison). The string discriminants
//! stay stable across the migration so historical rows don't need
//! re-bucketing.

use entity::metadata_match_outcome;
use sea_orm::{ActiveModelTrait, ConnectionTrait, Set};
use uuid::Uuid;

use super::matcher::Confidence;
use super::orchestrator::RankedCandidate;

/// Shape of the ranked-candidate list at the end of a search. Drives
/// the M8 dialog state machine and the M0 dashboard distribution.
///
/// Discriminants are stable strings on disk; the enum is just a
/// well-named Rust shim so the orchestrator + dashboard can speak the
/// same vocabulary without re-deriving from the column.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MatchOutcomeKind {
    /// Zero candidates after pre-filter + scoring.
    NoMatch,
    /// Exactly one candidate that crossed the HIGH bucket.
    SingleGood,
    /// 2+ candidates, top bucket is HIGH.
    MultiGood,
    /// Exactly one candidate, bucket below HIGH. Post-M4 this means
    /// "one plausible match but its cover doesn't match yours" — for
    /// M0 it's the broader "one weak match" signal.
    SingleBadCover,
    /// 2+ candidates, top bucket below HIGH.
    MultiBadCover,
}

impl MatchOutcomeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchOutcomeKind::NoMatch => "no_match",
            MatchOutcomeKind::SingleGood => "single_good",
            MatchOutcomeKind::MultiGood => "multi_good",
            MatchOutcomeKind::SingleBadCover => "single_bad_cover",
            MatchOutcomeKind::MultiBadCover => "multi_bad_cover",
        }
    }

    /// Classify a finalized ranked list. The list is assumed
    /// score-descending (orchestrator sorts before calling).
    pub fn classify(ranked: &[RankedCandidate]) -> Self {
        match (ranked.len(), ranked.first().map(|r| r.bucket)) {
            (0, _) => MatchOutcomeKind::NoMatch,
            (1, Some(Confidence::High)) => MatchOutcomeKind::SingleGood,
            (1, _) => MatchOutcomeKind::SingleBadCover,
            (_, Some(Confidence::High)) => MatchOutcomeKind::MultiGood,
            _ => MatchOutcomeKind::MultiBadCover,
        }
    }
}

/// Persist one outcome row for a completed run. Called from
/// [`super::orchestrator::finalize_run`] inside the same transaction
/// so the row + the candidates land atomically.
///
/// Post-M4 the `top_hamming` / `second_hamming` columns receive the
/// actual cover Hamming distance for the top + runner-up candidates
/// when their phashes were available. The orchestrator's sort puts
/// the bucket-priority winner first, so `top_hamming` will be the
/// HIGH-bucket candidate's Hamming when one exists.
pub async fn record<C: ConnectionTrait>(
    db: &C,
    run_id: Uuid,
    scope: &str,
    ranked: &[RankedCandidate],
) -> Result<(), sea_orm::DbErr> {
    let kind = MatchOutcomeKind::classify(ranked);
    let top_score = ranked.first().map(|r| r.score.total).unwrap_or(0.0);
    let second_score = ranked.get(1).map(|r| r.score.total);
    let top_hamming = ranked
        .first()
        .and_then(|r| r.score.cover_hamming)
        .map(|d| d as i32);
    let second_hamming = ranked
        .get(1)
        .and_then(|r| r.score.cover_hamming)
        .map(|d| d as i32);
    let am = metadata_match_outcome::ActiveModel {
        id: Set(Uuid::new_v4()),
        run_id: Set(run_id),
        scope: Set(scope.to_owned()),
        outcome_kind: Set(kind.as_str().to_owned()),
        top_score: Set(top_score),
        top_hamming: Set(top_hamming),
        second_score: Set(second_score),
        second_hamming: Set(second_hamming),
        candidate_count: Set(ranked.len() as i32),
        created_at: Set(chrono::Utc::now().into()),
    };
    am.insert(db).await?;
    Ok(())
}

/// Prune outcome rows older than `cutoff_days`. The `metadata_run`
/// FK CASCADE handles run-deletion side; this sweep targets the
/// "run was retained, outcome no longer interesting" case + acts as a
/// safety net against an indefinitely-growing telemetry table.
pub async fn prune<C: ConnectionTrait>(db: &C, cutoff_days: i64) -> Result<u64, sea_orm::DbErr> {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(cutoff_days);
    let res = db
        .execute(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "DELETE FROM metadata_match_outcome WHERE created_at < $1",
            [sea_orm::Value::from(cutoff.fixed_offset())],
        ))
        .await?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::Source;
    use crate::metadata::matcher::Score;
    use crate::metadata::orchestrator::CandidatePayload;
    use crate::metadata::provider::SeriesCandidate;

    fn ranked(bucket: Confidence) -> RankedCandidate {
        RankedCandidate {
            source: Source::ComicVine,
            external_id: "x".into(),
            score: Score::default(),
            bucket,
            payload: CandidatePayload::Series(SeriesCandidate {
                source: Source::ComicVine,
                external_id: "x".into(),
                external_url: None,
                name: "x".into(),
                year: None,
                publisher: None,
                issue_count: None,
                cover_image_url: None,
                deck: None,
            }),
        }
    }

    #[test]
    fn classify_no_match_on_empty() {
        assert_eq!(MatchOutcomeKind::classify(&[]), MatchOutcomeKind::NoMatch);
    }

    #[test]
    fn classify_single_good_on_lone_high() {
        let r = vec![ranked(Confidence::High)];
        assert_eq!(MatchOutcomeKind::classify(&r), MatchOutcomeKind::SingleGood);
    }

    #[test]
    fn classify_single_bad_cover_on_lone_medium_or_low() {
        for c in [Confidence::Medium, Confidence::Low] {
            let r = vec![ranked(c)];
            assert_eq!(
                MatchOutcomeKind::classify(&r),
                MatchOutcomeKind::SingleBadCover
            );
        }
    }

    #[test]
    fn classify_multi_good_on_top_high() {
        let r = vec![ranked(Confidence::High), ranked(Confidence::Medium)];
        assert_eq!(MatchOutcomeKind::classify(&r), MatchOutcomeKind::MultiGood);
    }

    #[test]
    fn classify_multi_bad_cover_on_top_below_high() {
        let r = vec![ranked(Confidence::Medium), ranked(Confidence::Low)];
        assert_eq!(
            MatchOutcomeKind::classify(&r),
            MatchOutcomeKind::MultiBadCover
        );
    }
}
