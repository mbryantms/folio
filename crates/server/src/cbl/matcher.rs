//! CBL entry matcher (saved-views M4).
//!
//! Three-tier resolution per [`EntryToMatch`]:
//!
//!   1. **ComicVine issue ID** — exact match against the issue's
//!      `external_ids` row (`source = 'comicvine'`).
//!      `match_method = 'comicvine_id'`, confidence `1.0`.
//!   2. **Metron issue ID** — same shape, `match_method = 'metron_id'`.
//!   3. **Series + volume + number fallback** — `pg_trgm` similarity on
//!      `series.normalized_name` (threshold > 0.75) AND exact volume
//!      match (CBL's `Volume` attribute compared against
//!      `series.year::text` OR `series.volume::text`) AND exact
//!      `issues.number_raw`. Single candidate above threshold →
//!      `Matched`. Multiple → `Ambiguous` with the top
//!      [`AMBIGUOUS_TOP_N`] preserved.
//!
//! The statement count of [`match_entries`] does not grow with the
//! number of entries: one id lookup, and one fallback statement that
//! covers every entry the ids left over (see [`fallback_batch`]).
//!
//! Manual overrides (`match_status = 'manual'`) are not produced here —
//! they're written by the API endpoint and explicitly preserved by the
//! refresh writer.

use entity::series;
use sea_orm::{ConnectionTrait, FromQueryResult, Statement};
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
/// Returns outcomes in the same order.
///
/// The number of SQL statements is fixed, whatever the number of
/// entries: at most one ID lookup (both id tiers, [`build_id_lookup`])
/// and at most one fallback statement that covers every entry the ids
/// did not resolve ([`fallback_batch`]). Uses pg_trgm's `similarity()` for the
/// fallback — `series.normalized_name` is the already-normalized form
/// the scanner stores.
pub async fn match_entries<C: ConnectionTrait>(
    db: &C,
    entries: &[EntryToMatch],
) -> Result<Vec<MatchOutcome>, sea_orm::DbErr> {
    let id_lookup = build_id_lookup(db, entries).await?;
    let mut out: Vec<Option<MatchOutcome>> = Vec::with_capacity(entries.len());
    let mut batch = FallbackBatch::default();
    for (idx, entry) in entries.iter().enumerate() {
        if let Some(outcome) = id_match(&id_lookup, entry) {
            out.push(Some(outcome));
            continue;
        }
        match FallbackQuery::of(entry) {
            Some(query) => {
                batch.push(idx, query);
                out.push(None);
            }
            None => out.push(Some(MatchOutcome::missing())),
        }
    }
    let rows = fallback_batch(db, &batch).await?;
    for (idx, wanted) in batch.entries {
        // Several entries can share one wanted (key, number) pair.
        out[idx] = Some(outcome_of(rows[wanted].clone()));
    }
    Ok(out
        .into_iter()
        .map(|o| o.unwrap_or_else(MatchOutcome::missing))
        .collect())
}

/// The id tiers: ComicVine id, then Metron id.
fn id_match(lookup: &IdLookup, entry: &EntryToMatch) -> Option<MatchOutcome> {
    if let Some(cv) = entry.cv_issue_id
        && let Some(issue_id) = lookup.cv.get(&cv)
    {
        return Some(MatchOutcome::matched(issue_id.clone(), "comicvine_id", 1.0));
    }
    if let Some(metron) = entry.metron_issue_id
        && let Some(issue_id) = lookup.metron.get(&metron)
    {
        return Some(MatchOutcome::matched(issue_id.clone(), "metron_id", 1.0));
    }
    None
}

/// One row of the fallback: a candidate issue and its series.
#[derive(Debug, Clone)]
struct FallbackRow {
    issue_id: String,
    series_name: String,
    sim: f32,
}

/// Rows (already ordered, at most [`AMBIGUOUS_TOP_N`]) → outcome: none is
/// missing, one is a match, more are ambiguous with those rows kept for
/// the Resolution UI. We don't auto-resolve ties even if the similarity
/// gap is large; the user's library may legitimately have both
/// candidates and only the human can pick.
fn outcome_of(rows: Vec<FallbackRow>) -> MatchOutcome {
    match rows.len() {
        0 => MatchOutcome::missing(),
        1 => {
            let row = rows.into_iter().next().expect("one row");
            MatchOutcome::matched(row.issue_id, "series_volume_number", row.sim)
        }
        _ => MatchOutcome::ambiguous(
            rows.into_iter()
                .map(|r| MatchCandidate {
                    issue_id: r.issue_id,
                    series_name: r.series_name,
                    similarity: r.sim,
                })
                .collect(),
        ),
    }
}

/// What the series + volume + number fallback compares for one entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FallbackQuery {
    /// `series::normalize_name` of the entry's series name.
    normalized: String,
    /// The trimmed volume; compared with `series.year::text` and
    /// `series.volume::text`.
    volume: String,
    /// The trimmed number; compared with `issues.number_raw`.
    number: String,
}

impl FallbackQuery {
    /// `None` when the fallback cannot match the entry: an empty (after
    /// trim) series name or number, or no volume. Volume gate: when the
    /// CBL provides one we require it to match either the series's start
    /// year or its sequential volume number. Without a volume the
    /// fallback would reunite "Tech Jacket 2002" with "Tech Jacket 2014"
    /// — refuse rather than guess.
    fn of(entry: &EntryToMatch) -> Option<Self> {
        if entry.series_name.trim().is_empty() || entry.number.trim().is_empty() {
            return None;
        }
        let volume = entry
            .volume
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        Some(Self {
            normalized: series::normalize_name(&entry.series_name),
            volume: volume.to_owned(),
            number: entry.number.trim().to_owned(),
        })
    }
}

/// The fallback work of one [`match_entries`] call, deduplicated: a
/// reading list names the same series (and often the same issue) many
/// times.
#[derive(Debug, Default)]
struct FallbackBatch {
    /// Distinct (normalized name, volume) pairs: the series lookups.
    keys: Vec<(String, String)>,
    key_index: HashMap<(String, String), usize>,
    /// Distinct (key, number) pairs: the issue lookups.
    wanted: Vec<(usize, String)>,
    wanted_index: HashMap<(usize, String), usize>,
    /// (entry index, wanted index) for every entry in the fallback.
    entries: Vec<(usize, usize)>,
}

impl FallbackBatch {
    fn push(&mut self, entry_idx: usize, q: FallbackQuery) {
        let key = (q.normalized, q.volume);
        let next = self.keys.len();
        let k = *self.key_index.entry(key.clone()).or_insert_with(|| {
            self.keys.push(key);
            next
        });
        let want = (k, q.number);
        let next = self.wanted.len();
        let w = *self.wanted_index.entry(want.clone()).or_insert_with(|| {
            self.wanted.push(want);
            next
        });
        self.entries.push((entry_idx, w));
    }
}

/// The series + volume + number fallback for every entry of `batch`, in
/// one statement. Returns the candidate rows per wanted (key, number)
/// pair, indexed like `batch.wanted`.
///
/// Per entry, the rows are the (issue, series) pairs where the issue is
/// active and not removed, its `number_raw` equals the trimmed number,
/// the series's `year::text` or `volume::text` equals the trimmed volume,
/// and `similarity(series.normalized_name, normalized name) > 0.75`. They
/// are ordered by similarity descending, then series name ascending (the
/// column's collation, as Postgres sorts it), then issue id ascending,
/// and cut at [`AMBIGUOUS_TOP_N`]. The issue-id key makes the order
/// deterministic; the per-entry statement this replaced
/// ([`reference_match_entries_for_tests`]) left ties on (similarity, name)
/// in no defined order.
///
/// Shape: the distinct (name, volume) keys are joined to `series` on the
/// volume equality (two hash-joinable equalities, `UNION`-ed so a series
/// whose year and volume both equal the key appears once), and only those
/// pairs get a `similarity`. That ordering is the gain over the per-entry
/// statement, whose plan evaluated `similarity` on every series row
/// before the year/volume test. The similarity condition needs both the
/// key and the series, so it can only run as a join condition; under a
/// hash join the volume equality is the hash clause and runs first in any
/// case. `MATERIALIZED` covers the remaining case, a nested-loop join,
/// where both conditions would be join-filter quals evaluated in
/// estimated-cost order, and Postgres costs `similarity()` below the
/// cast-and-compare: the fence makes the volume equality come first
/// whatever join the planner picks. The issues are then looked up by
/// (candidate series, number) through the `issues (series_id, …)` index.
/// The similarity and the threshold comparison are Postgres's, with the
/// same types as before (`similarity()` is `real`, the threshold is bound
/// as `float8`, the reported value is cast to `real`).
async fn fallback_batch<C: ConnectionTrait>(
    db: &C,
    batch: &FallbackBatch,
) -> Result<Vec<Vec<FallbackRow>>, sea_orm::DbErr> {
    let mut out: Vec<Vec<FallbackRow>> = vec![Vec::new(); batch.wanted.len()];
    if batch.wanted.is_empty() {
        return Ok(out);
    }
    let names: Vec<String> = batch.keys.iter().map(|(n, _)| n.clone()).collect();
    let volumes: Vec<String> = batch.keys.iter().map(|(_, v)| v.clone()).collect();
    // `WITH ORDINALITY` is 1-based.
    let wanted_keys: Vec<i64> = batch.wanted.iter().map(|(k, _)| *k as i64 + 1).collect();
    let wanted_numbers: Vec<String> = batch.wanted.iter().map(|(_, n)| n.clone()).collect();

    #[derive(Debug, FromQueryResult)]
    struct Row {
        wanted: i64,
        issue_id: String,
        series_name: String,
        sim: f32,
    }
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        FALLBACK_BATCH_SQL,
        [
            names.into(),
            volumes.into(),
            (FALLBACK_THRESHOLD as f64).into(),
            wanted_keys.into(),
            wanted_numbers.into(),
            (AMBIGUOUS_TOP_N as i64).into(),
        ],
    ))
    .all(db)
    .await?;
    for row in rows {
        let Some(slot) = usize::try_from(row.wanted - 1)
            .ok()
            .and_then(|w| out.get_mut(w))
        else {
            // `wanted` is an ordinal of `$4`, so this cannot happen; skip
            // the row in release builds rather than fail the match.
            debug_assert!(
                false,
                "fallback row with out-of-range wanted {}",
                row.wanted
            );
            continue;
        };
        slot.push(FallbackRow {
            issue_id: row.issue_id,
            series_name: row.series_name,
            sim: row.sim,
        });
    }
    Ok(out)
}

/// See [`fallback_batch`]. `$1` / `$2`: the keys' normalized names and
/// volumes; `$3`: the threshold; `$4` / `$5`: per wanted pair, its key's
/// ordinal and the number; `$6`: rows kept per wanted pair.
const FALLBACK_BATCH_SQL: &str = r"
WITH k AS (
    SELECT name, volume, ord
      FROM unnest($1::text[], $2::text[]) WITH ORDINALITY AS k(name, volume, ord)
),
pairs AS MATERIALIZED (
    SELECT k.ord, k.name, s.id, s.name AS series_name, s.normalized_name
      FROM k JOIN series s ON s.year::text = k.volume
    UNION
    SELECT k.ord, k.name, s.id, s.name AS series_name, s.normalized_name
      FROM k JOIN series s ON s.volume::text = k.volume
),
cand AS (
    SELECT p.ord, p.id AS series_id, p.series_name,
           similarity(p.normalized_name, p.name)::real AS sim
      FROM pairs p
     WHERE similarity(p.normalized_name, p.name) > $3
),
w AS (
    SELECT key_ord, number, ord
      FROM unnest($4::bigint[], $5::text[]) WITH ORDINALITY AS w(key_ord, number, ord)
),
ranked AS (
    SELECT w.ord AS wanted, i.id AS issue_id, c.series_name, c.sim,
           row_number() OVER (PARTITION BY w.ord
                              ORDER BY c.sim DESC, c.series_name ASC, i.id ASC) AS rn
      FROM w
      JOIN cand c ON c.ord = w.key_ord
      JOIN issues i ON i.series_id = c.series_id AND i.number_raw = w.number
     WHERE i.state = 'active'
       AND i.removed_at IS NULL
)
SELECT wanted, issue_id, series_name, sim
  FROM ranked
 WHERE rn <= $6
 ORDER BY wanted, rn";

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

    // CV / Metron ids on `external_ids` are stored as TEXT; format
    // the i32 inputs to match. Single query covers both sources.
    let cv_strs: Vec<String> = cv_ids.iter().map(i32::to_string).collect();
    let metron_strs: Vec<String> = metron_ids.iter().map(i32::to_string).collect();
    let mut cond = sea_orm::Condition::any();
    if !cv_strs.is_empty() {
        cond = cond.add(
            sea_orm::Condition::all()
                .add(entity::external_id::Column::Source.eq("comicvine"))
                .add(entity::external_id::Column::ExternalId.is_in(cv_strs)),
        );
    }
    if !metron_strs.is_empty() {
        cond = cond.add(
            sea_orm::Condition::all()
                .add(entity::external_id::Column::Source.eq("metron"))
                .add(entity::external_id::Column::ExternalId.is_in(metron_strs)),
        );
    }
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let id_rows = entity::external_id::Entity::find()
        .filter(entity::external_id::Column::EntityType.eq("issue"))
        .filter(cond)
        .all(db)
        .await?;

    let mut lookup = IdLookup::default();
    for row in id_rows {
        let Ok(numeric) = row.external_id.parse::<i64>() else {
            continue;
        };
        if numeric < 0 || numeric > i64::from(i32::MAX) {
            continue;
        }
        let k = numeric as i32;
        match row.source.as_str() {
            // Last-write-wins on the rare collision; CBL canon is a
            // 1:1 ID→issue mapping so duplicate hits are a data bug.
            "comicvine" => {
                lookup.cv.insert(k, row.entity_id.clone());
            }
            "metron" => {
                lookup.metron.insert(k, row.entity_id.clone());
            }
            _ => {}
        }
    }
    Ok(lookup)
}

// ───── debug-build reference implementation ─────
//
// The per-entry matcher that `match_entries` replaced (one fallback
// statement per entry), kept verbatim as the reference for the
// differential test in `tests/cbl_matcher_batched.rs`. Not compiled into
// release builds; nothing but tests may call it.

/// REFERENCE ONLY (debug builds, for integration tests): the previous
/// per-entry implementation of [`match_entries`] — the same id tiers,
/// then one fallback statement per entry. Its candidate order is
/// undefined among rows that tie on (similarity, series name).
#[cfg(debug_assertions)]
pub async fn reference_match_entries_for_tests<C: ConnectionTrait>(
    db: &C,
    entries: &[EntryToMatch],
) -> Result<Vec<MatchOutcome>, sea_orm::DbErr> {
    let id_lookup = build_id_lookup(db, entries).await?;
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        if let Some(outcome) = id_match(&id_lookup, entry) {
            out.push(outcome);
            continue;
        }
        out.push(reference_fallback_match(db, entry).await?);
    }
    Ok(out)
}

#[cfg(debug_assertions)]
async fn reference_fallback_match<C: ConnectionTrait>(
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
