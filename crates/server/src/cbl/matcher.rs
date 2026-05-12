//! CBL entry matcher (saved-views M4).
//!
//! Three-tier resolution per [`EntryToMatch`]:
//!
//!   1. **ComicVine issue ID** — exact match against `issues.comicvine_id`.
//!      `match_method = 'comicvine_id'`, confidence `1.0`.
//!   2. **Metron issue ID** — same shape, `match_method = 'metron_id'`.
//!   3. **Series + volume + number fallback** — `pg_trgm` similarity on
//!      `series.normalized_name` (threshold > 0.75) AND exact volume
//!      match (CBL's `Volume` attribute compared against
//!      `series.year::text` OR `series.volume::text`) AND exact
//!      `issues.number_raw`. Single candidate above threshold →
//!      `Matched`. Multiple → `Ambiguous` with top picks preserved.
//!
//! Manual overrides (`match_status = 'manual'`) are not produced here —
//! they're written by the API endpoint and explicitly preserved by the
//! refresh writer.

use entity::{issue, series};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult, QueryFilter, Statement};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const FALLBACK_THRESHOLD: f32 = 0.75;
const AMBIGUOUS_TOP_N: usize = 5;

#[derive(Debug, Clone)]
pub struct EntryToMatch {
    pub series_name: String,
    pub number: String,
    /// CBL's `Volume` attribute — usually the start year (e.g. "2003"),
    /// occasionally a sequential volume number.
    pub volume: Option<String>,
    pub cv_issue_id: Option<i32>,
    pub metron_issue_id: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchStatus {
    Matched,
    Ambiguous,
    Missing,
}

impl MatchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchStatus::Matched => "matched",
            MatchStatus::Ambiguous => "ambiguous",
            MatchStatus::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchCandidate {
    pub issue_id: String,
    pub series_name: String,
    pub similarity: f32,
}

#[derive(Debug, Clone)]
pub struct MatchOutcome {
    pub status: MatchStatus,
    pub issue_id: Option<String>,
    pub method: Option<&'static str>,
    pub confidence: Option<f32>,
    pub ambiguous_candidates: Option<Vec<MatchCandidate>>,
}

impl MatchOutcome {
    pub fn missing() -> Self {
        Self {
            status: MatchStatus::Missing,
            issue_id: None,
            method: None,
            confidence: None,
            ambiguous_candidates: None,
        }
    }
    fn matched(issue_id: String, method: &'static str, confidence: f32) -> Self {
        Self {
            status: MatchStatus::Matched,
            issue_id: Some(issue_id),
            method: Some(method),
            confidence: Some(confidence),
            ambiguous_candidates: None,
        }
    }
    fn ambiguous(candidates: Vec<MatchCandidate>) -> Self {
        Self {
            status: MatchStatus::Ambiguous,
            issue_id: None,
            method: None,
            confidence: None,
            ambiguous_candidates: Some(candidates),
        }
    }
}

/// Resolve every entry in `entries` against the live `issues` table.
/// Returns outcomes in the same order. One batched ID lookup + one
/// fallback query per entry that didn't ID-match. Uses pg_trgm's
/// `similarity()` for the fallback — `series.normalized_name` is the
/// already-normalized form the scanner stores.
pub async fn match_entries<C: ConnectionTrait>(
    db: &C,
    entries: &[EntryToMatch],
) -> Result<Vec<MatchOutcome>, sea_orm::DbErr> {
    let id_lookup = build_id_lookup(db, entries).await?;
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        if let Some(cv) = entry.cv_issue_id
            && let Some(issue_id) = id_lookup.cv.get(&cv)
        {
            out.push(MatchOutcome::matched(issue_id.clone(), "comicvine_id", 1.0));
            continue;
        }
        if let Some(metron) = entry.metron_issue_id
            && let Some(issue_id) = id_lookup.metron.get(&metron)
        {
            out.push(MatchOutcome::matched(issue_id.clone(), "metron_id", 1.0));
            continue;
        }
        out.push(fallback_match(db, entry).await?);
    }
    Ok(out)
}

#[derive(Debug, Default)]
struct IdLookup {
    cv: HashMap<i32, String>,
    metron: HashMap<i32, String>,
}

async fn build_id_lookup<C: ConnectionTrait>(
    db: &C,
    entries: &[EntryToMatch],
) -> Result<IdLookup, sea_orm::DbErr> {
    let cv_ids: Vec<i32> = entries.iter().filter_map(|e| e.cv_issue_id).collect();
    let metron_ids: Vec<i32> = entries.iter().filter_map(|e| e.metron_issue_id).collect();
    if cv_ids.is_empty() && metron_ids.is_empty() {
        return Ok(IdLookup::default());
    }

    let mut cond = sea_orm::Condition::any();
    if !cv_ids.is_empty() {
        let cvs: Vec<i64> = cv_ids.iter().map(|&i| i as i64).collect();
        cond = cond.add(issue::Column::ComicvineId.is_in(cvs));
    }
    if !metron_ids.is_empty() {
        let ms: Vec<i64> = metron_ids.iter().map(|&i| i as i64).collect();
        cond = cond.add(issue::Column::MetronId.is_in(ms));
    }

    let rows = issue::Entity::find()
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .filter(cond)
        .all(db)
        .await?;

    let mut lookup = IdLookup::default();
    for row in rows {
        if let Some(cv) = row.comicvine_id
            && cv >= 0
            && cv <= i32::MAX as i64
        {
            // Last-write-wins on the rare collision; CBL canon is a
            // 1:1 ID→issue mapping so duplicate hits are a data bug.
            lookup.cv.insert(cv as i32, row.id.clone());
        }
        if let Some(m) = row.metron_id
            && m >= 0
            && m <= i32::MAX as i64
        {
            lookup.metron.insert(m as i32, row.id.clone());
        }
    }
    Ok(lookup)
}

async fn fallback_match<C: ConnectionTrait>(
    db: &C,
    entry: &EntryToMatch,
) -> Result<MatchOutcome, sea_orm::DbErr> {
    if entry.series_name.trim().is_empty() || entry.number.trim().is_empty() {
        return Ok(MatchOutcome::missing());
    }
    // Volume gate: when CBL provides one we require it to match either
    // the series's start year or its sequential volume number. Without
    // a volume the fallback would reunite "Tech Jacket 2002" with "Tech
    // Jacket 2014" — refuse rather than guess.
    let Some(volume) = entry
        .volume
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(MatchOutcome::missing());
    };

    let normalized = series::normalize_name(&entry.series_name);
    let backend = db.get_database_backend();

    #[derive(Debug, FromQueryResult)]
    struct Row {
        issue_id: String,
        series_name: String,
        sim: f32,
    }
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        backend,
        r"SELECT i.id AS issue_id, s.name AS series_name,
                similarity(s.normalized_name, $1)::real AS sim
            FROM issues i
            JOIN series s ON s.id = i.series_id
            WHERE i.state = 'active'
              AND i.removed_at IS NULL
              AND i.number_raw = $2
              AND (s.year::text = $3 OR s.volume::text = $3)
              AND similarity(s.normalized_name, $1) > $4
            ORDER BY sim DESC, s.name ASC
            LIMIT $5",
        [
            normalized.into(),
            entry.number.trim().to_string().into(),
            volume.to_string().into(),
            (FALLBACK_THRESHOLD as f64).into(),
            (AMBIGUOUS_TOP_N as i64).into(),
        ],
    ))
    .all(db)
    .await?;

    if rows.is_empty() {
        return Ok(MatchOutcome::missing());
    }
    if rows.len() == 1 {
        let row = &rows[0];
        return Ok(MatchOutcome::matched(
            row.issue_id.clone(),
            "series_volume_number",
            row.sim,
        ));
    }
    // Multiple candidates above the threshold → ambiguous. Keep the top
    // N for the Resolution UI. We don't auto-resolve ties even if the
    // similarity gap is large; the user's library may legitimately have
    // both candidates and only the human can pick.
    let candidates = rows
        .into_iter()
        .map(|r| MatchCandidate {
            issue_id: r.issue_id,
            series_name: r.series_name,
            similarity: r.sim,
        })
        .collect();
    Ok(MatchOutcome::ambiguous(candidates))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_status_strings_round_trip() {
        assert_eq!(MatchStatus::Matched.as_str(), "matched");
        assert_eq!(MatchStatus::Ambiguous.as_str(), "ambiguous");
        assert_eq!(MatchStatus::Missing.as_str(), "missing");
    }

    #[test]
    fn missing_outcome_has_no_match_data() {
        let m = MatchOutcome::missing();
        assert_eq!(m.status, MatchStatus::Missing);
        assert!(m.issue_id.is_none());
        assert!(m.method.is_none());
        assert!(m.confidence.is_none());
        assert!(m.ambiguous_candidates.is_none());
    }
}
