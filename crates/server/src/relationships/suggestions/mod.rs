//! Relationship suggestion engine (WP-7.2, spec §5.7 / Phase 7).
//!
//! [`generate_for_library`] proposes candidate relationships from evidence
//! already in the DB ([`sources`]) and upserts them into
//! `series_relationship_suggestion`. It **never** creates
//! `series_relationship` edges: an admin accepts ([`accept`], which calls
//! [`crate::relationships::create_pair_scoped`] — or
//! [`crate::relationships::create_arc_edge`] for an arc target — with
//! [`RelationshipSource::Suggested`]) or rejects ([`reject`]) each one.
//!
//! WP-7.6: a suggestion may target a story arc ([`Target::Arc`], `tie_in_to`
//! only) and carries a proposed [`Scope`] (ranges, coverage, qualifier),
//! which `accept` passes through to the edge.
//!
//! Invariants:
//! - **Canonical rows.** Self-inverse kinds are stored with
//!   `from < to`; directional kinds only in their canonical direction
//!   (`sequel_of`, `continues`, `collects`, …; never `has_*` / `*_by` /
//!   `*_in` / `*_as` — [`RelationshipKind::is_canonical`], [`canonicalize`]).
//!   The DB CHECKs mirror this.
//! - **Rejection memory.** A `(from, to, kind)` whose row is reviewed
//!   (`accepted` / `rejected` / `modified`) is never rewritten or
//!   re-proposed; rows are never deleted. [`reopen`] moves a rejected row
//!   back to `pending` (WP-7.3).
//! - **Stale rows.** A pending row a run no longer produces is marked
//!   `stale` (WP-7.3); a later run that produces it again revives it to
//!   `pending`. Skipped when any evidence source failed, so a transient
//!   error can't empty the review queue.
//! - **Existing edges** with the same kind (or its inverse, which would
//!   contradict on accept) are not suggested. `sequel_of` and `continues`
//!   count as equivalent for this (WP-7.5): an existing `sequel_of` edge
//!   satisfies a `continues` suggestion for the same ordered pair and vice
//!   versa, and the same holds for reviewed rows (rejection memory).
//! - **Cap.** At most [`MAX_SUGGESTIONS_PER_RUN`] rows are written per run
//!   (highest confidence first).
//! - **Scope.** Suggestions only link series in the **same library**; every
//!   source query is library-scoped.
//!
//! See `docs/dev/series-relationships.md` ("Suggestion engine").

pub mod citations;
pub mod detectors;
pub mod sources;

use super::{PairError, RelationshipKind, RelationshipSource, Scope};
use chrono::Utc;
use entity::series_relationship_suggestion as sug;
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DbErr, EntityTrait, FromQueryResult, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Statement, TransactionSession, TransactionTrait, Value,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// Spec Phase 7: "Per-scan candidate cap = 1000 to bound runtime."
pub const MAX_SUGGESTIONS_PER_RUN: usize = 1000;

/// `confidence >= HIGH_MIN` → `high`.
pub const HIGH_MIN: f32 = 0.8;
/// `confidence >= MEDIUM_MIN` → `medium`; below → `low`.
pub const MEDIUM_MIN: f32 = 0.55;

/// Each additional independent source agreeing on the same suggestion adds
/// this much (capped at [`MAX_CONFIDENCE`]).
const CORROBORATION_BONUS: f32 = 0.05;
const MAX_CONFIDENCE: f32 = 0.99;

/// Rows per upsert statement.
const UPSERT_CHUNK: usize = 250;

// ───── enums ─────

/// Confidence bucket shown to reviewers (spec §5.7 "grouped by confidence").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionBucket {
    High,
    Medium,
    Low,
}

impl SuggestionBucket {
    pub fn from_confidence(c: f32) -> Self {
        if c >= HIGH_MIN {
            Self::High
        } else if c >= MEDIUM_MIN {
            Self::Medium
        } else {
            Self::Low
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

impl FromStr for SuggestionBucket {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "high" => Ok(Self::High),
            "medium" => Ok(Self::Medium),
            "low" => Ok(Self::Low),
            _ => Err(()),
        }
    }
}

/// Review state. Reviews (`accepted` / `rejected` / `modified`) are one-way
/// out of `pending`, except [`reopen`] (`rejected` → `pending`). `stale` is
/// set and cleared by the engine itself, not by a reviewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionStatus {
    Pending,
    /// Accepted with the suggested kind.
    Accepted,
    Rejected,
    /// Accepted with a different kind (`accepted_kind`).
    Modified,
    /// Pending, but the latest run no longer produced it (its evidence
    /// went away). Hidden from the default review list; revived to
    /// `pending` if a later run produces it again.
    Stale,
}

impl SuggestionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Modified => "modified",
            Self::Stale => "stale",
        }
    }
}

impl FromStr for SuggestionStatus {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "pending" => Ok(Self::Pending),
            "accepted" => Ok(Self::Accepted),
            "rejected" => Ok(Self::Rejected),
            "modified" => Ok(Self::Modified),
            "stale" => Ok(Self::Stale),
            _ => Err(()),
        }
    }
}

/// Which heuristic produced a candidate (the `source` key of each
/// `evidence.sources[]` entry). WP-7.6 retired `series_group`,
/// `character_density` (pairwise `same_universe`) and the pairwise
/// `story_arc` → `crossover_with` source; old rows keep those names in their
/// stored evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    AlternateSeries,
    NameContinuation,
    CollectedEdition,
    ProviderVolume,
    ProviderRange,
    /// WP-7.6: an annual series → its main series.
    Annual,
    /// WP-7.6: a series → a story arc it ties in to (with role).
    ArcTieIn,
    /// WP-7.6: two series that are both `main` of the same arc.
    ArcCrossover,
    /// WP-7.6: `issue_reprints` rolled up per series pair.
    ReprintRollup,
    /// WP-7.6: same base title, edition marker (Deluxe, Director's Cut, …).
    AlternateEdition,
    /// WP-7.6: "X #N Facsimile Edition" → reprints X #N.
    Facsimile,
    /// WP-7.6: handbook / saga / spotlight / special sharing the parent's title.
    Supplement,
    /// WP-7.6: same work in another language.
    Translation,
}

/// A suggestion's target: a series, or (WP-7.6, `tie_in_to` only) a story
/// arc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    Series(Uuid),
    Arc(Uuid),
}

impl Target {
    pub fn series(self) -> Option<Uuid> {
        match self {
            Self::Series(id) => Some(id),
            Self::Arc(_) => None,
        }
    }

    pub fn arc(self) -> Option<Uuid> {
        match self {
            Self::Arc(id) => Some(id),
            Self::Series(_) => None,
        }
    }

    /// Text key (`s:<uuid>` / `a:<uuid>`) for the stale-marking `unnest`.
    fn key(self) -> String {
        match self {
            Self::Series(id) => format!("s:{id}"),
            Self::Arc(id) => format!("a:{id}"),
        }
    }

    fn of(to_series_id: Option<Uuid>, to_arc_id: Option<Uuid>) -> Option<Self> {
        match (to_series_id, to_arc_id) {
            (Some(s), _) => Some(Self::Series(s)),
            (None, Some(a)) => Some(Self::Arc(a)),
            (None, None) => None,
        }
    }
}

impl From<Uuid> for Target {
    fn from(id: Uuid) -> Self {
        Self::Series(id)
    }
}

/// One proposal from one source, before canonicalization and merging.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub from: Uuid,
    pub to: Target,
    pub kind: RelationshipKind,
    pub confidence: f32,
    pub source: EvidenceSource,
    /// Human-readable; names both series so it reads the same in either
    /// direction.
    pub reason: String,
    /// `{ "source": "<snake_case source>", … }`.
    pub evidence: serde_json::Value,
    /// Proposed scope (WP-7.6), read from `from`'s side. Mirrored when the
    /// candidate is folded onto the canonical direction; fields the kind
    /// doesn't take are dropped.
    pub scope: Scope,
}

/// Fold a `(from, to, kind)` onto the stored canonical form: a
/// non-canonical directional kind (`has_sequel`, `continued_by`,
/// `collected_in`, …) becomes its inverse with the ends swapped;
/// self-inverse kinds order the ends so `from < to`.
pub fn canonicalize(
    from: Uuid,
    to: Uuid,
    kind: RelationshipKind,
) -> (Uuid, Uuid, RelationshipKind) {
    if !kind.is_canonical() {
        (to, from, kind.inverse())
    } else if kind.is_self_inverse() && to < from {
        (to, from, kind)
    } else {
        (from, to, kind)
    }
}

/// Kinds that satisfy a suggestion of `kind` for the same ordered pair
/// (itself, plus `sequel_of` ⇔ `continues`, WP-7.5): an existing edge or a
/// reviewed row of any of them means "already decided".
pub fn equivalent_kinds(kind: RelationshipKind) -> Vec<RelationshipKind> {
    use RelationshipKind as K;
    match kind {
        K::SequelOf | K::Continues => vec![K::SequelOf, K::Continues],
        K::HasSequel | K::ContinuedBy => vec![K::HasSequel, K::ContinuedBy],
        other => vec![other],
    }
}

/// Merged, canonical suggestion ready to upsert.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub from: Uuid,
    pub to: Target,
    pub kind: RelationshipKind,
    pub confidence: f32,
    pub bucket: SuggestionBucket,
    pub reason: String,
    pub evidence: serde_json::Value,
    pub scope: Scope,
}

/// [`canonicalize`] for a [`Target`]; also reports whether the ends were
/// swapped (the candidate's scope must then be mirrored). Arc targets are
/// always `tie_in_to`, which is canonical.
fn canonicalize_target(
    from: Uuid,
    to: Target,
    kind: RelationshipKind,
) -> (Uuid, Target, RelationshipKind, bool) {
    match to {
        Target::Arc(_) => (from, to, kind, false),
        Target::Series(t) => {
            let (f, t2, k) = canonicalize(from, t, kind);
            (f, Target::Series(t2), k, f != from)
        }
    }
}

/// Scope merged across a proposal's sources (strongest first): each field
/// comes from the strongest source that set it. Fields the kind doesn't take
/// and over-long ranges are dropped, so the row always passes the CHECKs.
fn merged_scope<'a>(parts: impl Iterator<Item = &'a Scope>, kind: RelationshipKind) -> Scope {
    let mut out = Scope::default();
    for s in parts {
        out.from_range = out.from_range.or_else(|| s.from_range.clone());
        out.to_range = out.to_range.or_else(|| s.to_range.clone());
        out.coverage = out.coverage.or(s.coverage);
        out.qualifier = out.qualifier.or(s.qualifier);
    }
    let fits = |r: Option<String>| r.filter(|r| r.chars().count() <= super::MAX_RANGE_LEN);
    out.from_range = fits(out.from_range);
    out.to_range = fits(out.to_range);
    out.note = None;
    out.normalized().fitted(kind)
}

/// Canonicalize and merge candidates that land on the same row. Confidence
/// is the strongest source's plus [`CORROBORATION_BONUS`] per additional
/// distinct source; reasons are joined strongest first; evidence keeps
/// every source entry (`{"sources": [...]}`); the scope is taken field by
/// field from the strongest source that has it. Self edges, and arc targets
/// on kinds that can't target an arc, are dropped.
pub fn merge(candidates: Vec<Candidate>) -> Vec<Proposal> {
    struct Acc {
        best: BTreeMap<EvidenceSource, Candidate>,
    }
    let mut by_key: HashMap<(Uuid, Target, RelationshipKind), Acc> = HashMap::new();
    for mut c in candidates {
        if c.to == Target::Series(c.from) || !c.confidence.is_finite() {
            continue;
        }
        if matches!(c.to, Target::Arc(_)) && !c.kind.allows_arc_target() {
            continue;
        }
        let (from, to, kind, swapped) = canonicalize_target(c.from, c.to, c.kind);
        if swapped {
            c.scope = c.scope.mirrored();
        }
        let acc = by_key.entry((from, to, kind)).or_insert_with(|| Acc {
            best: BTreeMap::new(),
        });
        // One entry per source: keep that source's strongest candidate.
        match acc.best.get(&c.source) {
            Some(prev) if prev.confidence >= c.confidence => {}
            _ => {
                acc.best.insert(c.source, c);
            }
        }
    }
    let mut out: Vec<Proposal> = by_key
        .into_iter()
        .map(|((from, to, kind), acc)| {
            let mut parts: Vec<Candidate> = acc.best.into_values().collect();
            parts.sort_by(|a, b| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.source.cmp(&b.source))
            });
            let top = parts.first().map_or(0.0, |p| p.confidence);
            let confidence = (top + CORROBORATION_BONUS * (parts.len().saturating_sub(1)) as f32)
                .clamp(0.0, MAX_CONFIDENCE);
            let confidence = (confidence * 100.0).round() / 100.0;
            let reason = parts
                .iter()
                .map(|p| p.reason.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            let scope = merged_scope(parts.iter().map(|p| &p.scope), kind);
            let evidence = serde_json::json!({
                "sources": parts.iter().map(|p| {
                    let mut e = p.evidence.clone();
                    if let Some(obj) = e.as_object_mut() {
                        obj.insert("confidence".into(), serde_json::json!(p.confidence));
                        obj.insert("reason".into(), serde_json::json!(p.reason));
                    }
                    e
                }).collect::<Vec<_>>(),
            });
            Proposal {
                from,
                to,
                kind,
                confidence,
                bucket: SuggestionBucket::from_confidence(confidence),
                reason,
                evidence,
                scope,
            }
        })
        .collect();
    // Deterministic: strongest first, ties by key.
    out.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.from.cmp(&b.from))
            .then(a.to.cmp(&b.to))
            .then(a.kind.as_str().cmp(b.kind.as_str()))
    });
    out
}

// ───── generation ─────

/// What one run did. Logged, stored in the `library_events` detail, and
/// returned to tests.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RunReport {
    pub library_id: Uuid,
    /// Raw candidates per source (before merge / filtering).
    pub by_source: BTreeMap<String, usize>,
    /// Distinct canonical proposals after merging.
    pub proposals: usize,
    /// Dropped: an edge with this kind (or its inverse) already exists.
    pub skipped_existing_edge: usize,
    /// Dropped: a reviewed (accepted / rejected / modified) row exists.
    pub skipped_reviewed: usize,
    /// Dropped by [`MAX_SUGGESTIONS_PER_RUN`].
    pub capped: usize,
    /// Rows newly inserted as `pending`.
    pub inserted: usize,
    /// Pending rows whose confidence / reason / evidence changed.
    pub updated: usize,
    /// Pending rows re-proposed unchanged.
    pub unchanged: usize,
    /// Stale rows produced again and moved back to `pending` (also counted
    /// in `updated`).
    pub revived: usize,
    /// Pending rows this run did not produce, now `stale`.
    pub marked_stale: usize,
    /// Evidence sources that errored. When non-empty, stale marking is
    /// skipped (their candidates are missing, not gone).
    pub failed_sources: Vec<String>,
    pub elapsed_ms: u64,
}

#[derive(Debug, FromQueryResult)]
struct KeyRow {
    from_series_id: Uuid,
    to_series_id: Option<Uuid>,
    to_arc_id: Option<Uuid>,
    kind: String,
}

type RowKey = (Uuid, Target, String);

fn key_set(rows: Vec<KeyRow>) -> HashSet<RowKey> {
    rows.into_iter()
        .filter_map(|e| {
            Some((
                e.from_series_id,
                Target::of(e.to_series_id, e.to_arc_id)?,
                e.kind,
            ))
        })
        .collect()
}

#[derive(Debug, FromQueryResult)]
struct UpsertRow {
    inserted: bool,
    changed: bool,
}

/// Run every evidence source for `library_id`, merge, drop what is already
/// an edge or already reviewed, cap at [`MAX_SUGGESTIONS_PER_RUN`], and
/// upsert. Pending rows that are proposed again get their confidence,
/// bucket, reason, evidence and scope refreshed; non-pending rows are never
/// touched. Idempotent.
#[tracing::instrument(skip_all, name = "relationship_suggest", fields(library_id = %library_id))]
pub async fn generate_for_library<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
) -> Result<RunReport, DbErr> {
    let started = std::time::Instant::now();
    let mut report = RunReport {
        library_id,
        ..Default::default()
    };

    let (candidates, counts, failed) = sources::collect_all(conn, library_id).await;
    report.by_source = counts.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    report.failed_sources = failed.into_iter().map(str::to_owned).collect();
    let proposals = merge(candidates);
    report.proposals = proposals.len();

    // Existing edges touching this library (bounded by curation), series
    // and arc targets alike.
    let edge_set = key_set(
        KeyRow::find_by_statement(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT r.from_series_id, r.to_series_id, r.to_arc_id, r.kind \
               FROM series_relationship r \
               JOIN series s ON s.id = r.from_series_id \
              WHERE s.library_id = $1",
            [Value::from(library_id)],
        ))
        .all(conn)
        .await?,
    );
    // Reviewed suggestions (the rejection memory).
    let reviewed_set = key_set(
        KeyRow::find_by_statement(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT g.from_series_id, g.to_series_id, g.to_arc_id, g.kind \
               FROM series_relationship_suggestion g \
               JOIN series s ON s.id = g.from_series_id \
              WHERE s.library_id = $1 AND g.status NOT IN ('pending', 'stale')",
            [Value::from(library_id)],
        ))
        .all(conn)
        .await?,
    );
    // Stale rows (to count revivals: the upsert flips them to pending).
    let stale_set = key_set(
        KeyRow::find_by_statement(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT g.from_series_id, g.to_series_id, g.to_arc_id, g.kind \
               FROM series_relationship_suggestion g \
               JOIN series s ON s.id = g.from_series_id \
              WHERE s.library_id = $1 AND g.status = 'stale'",
            [Value::from(library_id)],
        ))
        .all(conn)
        .await?,
    );

    let mut keep: Vec<Proposal> = Vec::with_capacity(proposals.len().min(MAX_SUGGESTIONS_PER_RUN));
    // Every proposal still produced this run (kept or capped): pending rows
    // outside this set go stale.
    let mut produced: Vec<(Uuid, Target, RelationshipKind)> = Vec::with_capacity(proposals.len());
    for p in proposals {
        // Inverse rows are always stored, so checking the forward
        // orientation for both the kind and its inverse covers "already
        // related this way" and "would contradict on accept". Equivalent
        // kinds (`sequel_of` ⇔ `continues`) count as the same decision.
        let same = (p.from, p.to, p.kind.as_str().to_owned());
        let key = |k: RelationshipKind| (p.from, p.to, k.as_str().to_owned());
        let equivalents = equivalent_kinds(p.kind);
        if equivalents
            .iter()
            .any(|k| edge_set.contains(&key(*k)) || edge_set.contains(&key(k.inverse())))
        {
            report.skipped_existing_edge += 1;
            continue;
        }
        if equivalents.iter().any(|k| reviewed_set.contains(&key(*k))) {
            report.skipped_reviewed += 1;
            continue;
        }
        produced.push((p.from, p.to, p.kind));
        if keep.len() >= MAX_SUGGESTIONS_PER_RUN {
            report.capped += 1;
            continue;
        }
        if stale_set.contains(&same) {
            report.revived += 1;
        }
        keep.push(p);
    }

    let (to_series, to_arcs): (Vec<Proposal>, Vec<Proposal>) = keep
        .into_iter()
        .partition(|p| matches!(p.to, Target::Series(_)));
    let kept = to_series.len() + to_arcs.len();
    for (rows, arc) in [(&to_series, false), (&to_arcs, true)] {
        for chunk in rows.chunks(UPSERT_CHUNK) {
            for r in upsert_chunk(conn, chunk, arc).await? {
                if r.inserted {
                    report.inserted += 1;
                } else if r.changed {
                    report.updated += 1;
                }
            }
        }
    }
    // Rows the upsert matched but left alone: pending + identical.
    report.unchanged = kept.saturating_sub(report.inserted + report.updated);
    if report.failed_sources.is_empty() {
        report.marked_stale = mark_stale(conn, library_id, &produced).await?;
    } else {
        tracing::warn!(
            library_id = %library_id,
            failed = ?report.failed_sources,
            "relationship suggestions: a source failed, not marking stale rows"
        );
    }
    report.elapsed_ms = started.elapsed().as_millis() as u64;
    tracing::info!(
        library_id = %library_id,
        proposals = report.proposals,
        inserted = report.inserted,
        updated = report.updated,
        unchanged = report.unchanged,
        skipped_existing_edge = report.skipped_existing_edge,
        skipped_reviewed = report.skipped_reviewed,
        capped = report.capped,
        revived = report.revived,
        marked_stale = report.marked_stale,
        elapsed_ms = report.elapsed_ms,
        "relationship suggestions: run complete"
    );
    Ok(report)
}

/// `INSERT … ON CONFLICT DO UPDATE … WHERE status IN ('pending', 'stale')`,
/// for one target type (`arc` = `to_arc_id`, else `to_series_id`; each has
/// its own partial unique index). A reviewed row conflicts and is left
/// untouched (the `WHERE` guard also covers a review that lands between the
/// pre-filter and this statement); a stale row is revived to `pending`.
/// Returns one row per inserted, *changed* or revived row. Empty scope
/// fields travel as `''` and land as NULL.
async fn upsert_chunk<C: ConnectionTrait>(
    conn: &C,
    chunk: &[Proposal],
    arc: bool,
) -> Result<Vec<UpsertRow>, DbErr> {
    let text = |f: &dyn Fn(&Proposal) -> Option<String>| -> Vec<String> {
        chunk.iter().map(|p| f(p).unwrap_or_default()).collect()
    };
    let from: Vec<String> = chunk.iter().map(|p| p.from.to_string()).collect();
    let to: Vec<String> = chunk
        .iter()
        .map(|p| match p.to {
            Target::Series(id) | Target::Arc(id) => id.to_string(),
        })
        .collect();
    let kind: Vec<String> = chunk.iter().map(|p| p.kind.as_str().to_owned()).collect();
    let conf: Vec<f64> = chunk.iter().map(|p| f64::from(p.confidence)).collect();
    let bucket: Vec<String> = chunk.iter().map(|p| p.bucket.as_str().to_owned()).collect();
    let reason: Vec<String> = chunk.iter().map(|p| p.reason.clone()).collect();
    let evidence: Vec<String> = chunk.iter().map(|p| p.evidence.to_string()).collect();
    let from_range = text(&|p| p.scope.from_range.clone());
    let to_range = text(&|p| p.scope.to_range.clone());
    let coverage = text(&|p| p.scope.coverage.map(|c| c.as_str().to_owned()));
    let qualifier = text(&|p| p.scope.qualifier.map(|q| q.as_str().to_owned()));
    let col = if arc { "to_arc_id" } else { "to_series_id" };
    let t = "series_relationship_suggestion";
    let sql = format!(
        r#"
        INSERT INTO {t}
            (id, from_series_id, {col}, kind, confidence, bucket, reason, evidence,
             from_range, to_range, coverage, qualifier, status, created_at, updated_at)
        SELECT gen_random_uuid(), f::uuid, x.t::uuid, k, c::real, b, r, e::jsonb,
               nullif(fr, ''), nullif(tr, ''), nullif(cv, ''), nullif(q, ''),
               'pending', now(), now()
          FROM unnest($1::text[], $2::text[], $3::text[], $4::float8[], $5::text[], $6::text[],
                      $7::text[], $8::text[], $9::text[], $10::text[], $11::text[])
               AS x(f, t, k, c, b, r, e, fr, tr, cv, q)
        ON CONFLICT (from_series_id, {col}, kind) WHERE {col} IS NOT NULL DO UPDATE
           SET confidence = EXCLUDED.confidence,
               bucket     = EXCLUDED.bucket,
               reason     = EXCLUDED.reason,
               evidence   = EXCLUDED.evidence,
               from_range = EXCLUDED.from_range,
               to_range   = EXCLUDED.to_range,
               coverage   = EXCLUDED.coverage,
               qualifier  = EXCLUDED.qualifier,
               status     = 'pending',
               updated_at = now()
         WHERE {t}.status IN ('pending', 'stale')
           AND ({t}.status = 'stale'
             OR {t}.confidence IS DISTINCT FROM EXCLUDED.confidence
             OR {t}.reason     IS DISTINCT FROM EXCLUDED.reason
             OR {t}.evidence   IS DISTINCT FROM EXCLUDED.evidence
             OR {t}.from_range IS DISTINCT FROM EXCLUDED.from_range
             OR {t}.to_range   IS DISTINCT FROM EXCLUDED.to_range
             OR {t}.coverage   IS DISTINCT FROM EXCLUDED.coverage
             OR {t}.qualifier  IS DISTINCT FROM EXCLUDED.qualifier)
        RETURNING (xmax = 0) AS inserted, true AS changed
    "#
    );
    UpsertRow::find_by_statement(Statement::from_sql_and_values(
        conn.get_database_backend(),
        sql,
        [
            Value::from(from),
            Value::from(to),
            Value::from(kind),
            Value::from(conf),
            Value::from(bucket),
            Value::from(reason),
            Value::from(evidence),
            Value::from(from_range),
            Value::from(to_range),
            Value::from(coverage),
            Value::from(qualifier),
        ],
    ))
    .all(conn)
    .await
}

/// Mark the library's pending rows that are not in `produced` as `stale`.
/// Returns how many rows changed. Keyed on the suggestion's `from` series'
/// library, like every other library-scoped query here. Targets compare as
/// `s:<uuid>` / `a:<uuid>` text keys so series and arc rows share one
/// `unnest`.
async fn mark_stale<C: ConnectionTrait>(
    conn: &C,
    library_id: Uuid,
    produced: &[(Uuid, Target, RelationshipKind)],
) -> Result<usize, DbErr> {
    let from: Vec<String> = produced.iter().map(|k| k.0.to_string()).collect();
    let to: Vec<String> = produced.iter().map(|k| k.1.key()).collect();
    let kind: Vec<String> = produced.iter().map(|k| k.2.as_str().to_owned()).collect();
    let res = conn
        .execute_raw(Statement::from_sql_and_values(
            conn.get_database_backend(),
            r#"
            UPDATE series_relationship_suggestion g
               SET status = 'stale', updated_at = now()
              FROM series s
             WHERE s.id = g.from_series_id
               AND s.library_id = $1
               AND g.status = 'pending'
               AND NOT EXISTS (
                   SELECT 1
                     FROM unnest($2::text[], $3::text[], $4::text[]) AS k(f, t, kd)
                    WHERE k.f::uuid = g.from_series_id
                      AND k.t = CASE WHEN g.to_arc_id IS NOT NULL
                                     THEN 'a:' || g.to_arc_id::text
                                     ELSE 's:' || g.to_series_id::text END
                      AND k.kd = g.kind)
            "#,
            [
                Value::from(library_id),
                Value::from(from),
                Value::from(to),
                Value::from(kind),
            ],
        ))
        .await?;
    Ok(usize::try_from(res.rows_affected()).unwrap_or(usize::MAX))
}

// ───── review ─────

/// Why [`accept`] / [`reject`] refused.
#[derive(Debug)]
pub enum ReviewError {
    NotFound,
    /// Not in the state the action needs: already accepted / rejected /
    /// modified (reviews are one-way), stale, or — for [`reopen`] — not
    /// rejected.
    AlreadyReviewed {
        status: SuggestionStatus,
    },
    /// `create_pair_scoped` / `create_arc_edge` refused (self edge,
    /// contradicting kind, a non-arc kind on an arc suggestion, …).
    Pair(PairError),
    Db(DbErr),
}

impl From<DbErr> for ReviewError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

impl fmt::Display for ReviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("suggestion not found"),
            Self::AlreadyReviewed {
                status: SuggestionStatus::Stale,
            } => f.write_str("suggestion is stale (the engine no longer proposes it)"),
            Self::AlreadyReviewed {
                status: SuggestionStatus::Pending,
            } => f.write_str("suggestion is still pending"),
            Self::AlreadyReviewed { status } => {
                write!(f, "suggestion already reviewed ({})", status.as_str())
            }
            Self::Pair(e) => write!(f, "{e}"),
            Self::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

/// Result of [`accept`].
#[derive(Debug, Clone)]
pub struct AcceptOutcome {
    /// The suggestion row after the status change.
    pub suggestion: sug::Model,
    /// The edge kind that was created (`from kind to`).
    pub kind: RelationshipKind,
    /// The `from → to` edge row (pre-existing or new): a series pair's
    /// forward half, or the series → arc edge.
    pub forward: entity::series_relationship::Model,
    /// The inverse half of a series pair; `None` for an arc edge (WP-7.6).
    pub inverse: Option<entity::series_relationship::Model>,
    /// `false` when the edge already existed.
    pub created: bool,
}

/// The scope a suggestion row proposes (WP-7.6).
pub fn scope_of(row: &sug::Model) -> Scope {
    Scope {
        from_range: row.from_range.clone(),
        to_range: row.to_range.clone(),
        coverage: row.coverage.as_deref().and_then(|c| c.parse().ok()),
        qualifier: row.qualifier.as_deref().and_then(|q| q.parse().ok()),
        note: None,
    }
}

async fn lock_pending<C: ConnectionTrait>(conn: &C, id: Uuid) -> Result<sug::Model, ReviewError> {
    let row = sug::Entity::find_by_id(id)
        .lock_exclusive()
        .one(conn)
        .await?
        .ok_or(ReviewError::NotFound)?;
    let status = row.status.parse().unwrap_or(SuggestionStatus::Pending);
    if status != SuggestionStatus::Pending {
        return Err(ReviewError::AlreadyReviewed { status });
    }
    Ok(row)
}

/// Accept a pending suggestion: create the edge with `source = suggested`,
/// the suggestion's confidence, `created_by = actor` and its proposed scope
/// (WP-7.6) — a series pair via [`crate::relationships::create_pair_scoped`],
/// or a series → arc edge via [`crate::relationships::create_arc_edge`] — and
/// mark the row `accepted`, or `modified` with `accepted_kind` when
/// `kind_override` differs from the suggested kind. The override is read in
/// the suggestion's `from → to` direction; scope fields the overriding kind
/// doesn't take (a coverage on a non-collection kind, a qualifier from
/// another set) are dropped rather than failing the accept
/// ([`Scope::fitted`]). An arc suggestion only accepts arc-capable kinds
/// (`tie_in_to`); anything else is [`PairError::ArcKind`]. Both writes share
/// one transaction (a savepoint when `conn` is already a transaction), and
/// the row is locked `FOR UPDATE`, so a double accept can't create twice.
///
/// **Callers must** call `AppState::similarity.invalidate_all()` after this
/// returns with `created = true` (accepted edges feed WP-7.4's similar
/// series); the HTTP handler does, and so must any bulk path.
pub async fn accept<C>(
    conn: &C,
    id: Uuid,
    actor: Uuid,
    kind_override: Option<RelationshipKind>,
) -> Result<AcceptOutcome, ReviewError>
where
    C: ConnectionTrait + TransactionTrait,
{
    let txn = conn.begin().await?;
    let row = lock_pending(&txn, id).await?;
    let suggested: RelationshipKind = row
        .kind
        .parse()
        .map_err(|()| ReviewError::Db(DbErr::Custom(format!("bad kind {}", row.kind))))?;
    let kind = kind_override.unwrap_or(suggested);
    let scope = scope_of(&row).fitted(kind);
    let confidence = Some(row.confidence.clamp(0.0, 1.0));
    let pair_err = |e: PairError| match e {
        PairError::Db(d) => ReviewError::Db(d),
        other => ReviewError::Pair(other),
    };
    let (forward, inverse, created) = match Target::of(row.to_series_id, row.to_arc_id) {
        Some(Target::Series(to)) => {
            let pair = super::create_pair_scoped(
                &txn,
                row.from_series_id,
                to,
                kind,
                RelationshipSource::Suggested,
                confidence,
                Some(actor),
                &scope,
            )
            .await
            .map_err(pair_err)?;
            (pair.forward, Some(pair.inverse), pair.created)
        }
        Some(Target::Arc(arc)) => {
            let edge = super::create_arc_edge(
                &txn,
                row.from_series_id,
                arc,
                kind,
                RelationshipSource::Suggested,
                confidence,
                Some(actor),
                &scope,
            )
            .await
            .map_err(pair_err)?;
            (edge.row, None, edge.created)
        }
        None => {
            return Err(ReviewError::Db(DbErr::Custom(
                "suggestion has no target".into(),
            )));
        }
    };
    let modified = kind != suggested;
    let now = Utc::now().fixed_offset();
    let status = if modified {
        SuggestionStatus::Modified
    } else {
        SuggestionStatus::Accepted
    };
    let suggestion = set_status(
        &txn,
        row,
        status,
        modified.then(|| kind.as_str().to_owned()),
        actor,
        now,
    )
    .await?;
    txn.commit().await?;
    Ok(AcceptOutcome {
        suggestion,
        kind,
        forward,
        inverse,
        created,
    })
}

/// Reject a pending suggestion. The row stays forever as `rejected`, which
/// is what keeps the engine from proposing it again.
pub async fn reject<C>(conn: &C, id: Uuid, actor: Uuid) -> Result<sug::Model, ReviewError>
where
    C: ConnectionTrait + TransactionTrait,
{
    let txn = conn.begin().await?;
    let row = lock_pending(&txn, id).await?;
    let now = Utc::now().fixed_offset();
    let out = set_status(&txn, row, SuggestionStatus::Rejected, None, actor, now).await?;
    txn.commit().await?;
    Ok(out)
}

/// Reopen a rejected suggestion (`rejected` → `pending`): the manual
/// "clear the rejection" spec §5.7 asks for. The row is kept (append-only:
/// only the status moves) and its review stamp is cleared, so it reads as
/// pending again; the audit row written by the caller keeps who rejected it
/// and when. Returns the row before and after. Anything but `rejected` is
/// [`ReviewError::AlreadyReviewed`] (409).
pub async fn reopen<C>(conn: &C, id: Uuid) -> Result<(sug::Model, sug::Model), ReviewError>
where
    C: ConnectionTrait + TransactionTrait,
{
    use sea_orm::{ActiveModelTrait, Set};
    let txn = conn.begin().await?;
    let row = sug::Entity::find_by_id(id)
        .lock_exclusive()
        .one(&txn)
        .await?
        .ok_or(ReviewError::NotFound)?;
    let status = row.status.parse().unwrap_or(SuggestionStatus::Pending);
    if status != SuggestionStatus::Rejected {
        return Err(ReviewError::AlreadyReviewed { status });
    }
    let before = row.clone();
    let mut am: sug::ActiveModel = row.into();
    am.status = Set(SuggestionStatus::Pending.as_str().to_owned());
    am.accepted_kind = Set(None);
    am.reviewed_at = Set(None);
    am.reviewed_by = Set(None);
    am.updated_at = Set(Utc::now().fixed_offset());
    let after = am.update(&txn).await?;
    txn.commit().await?;
    Ok((before, after))
}

/// Hard cap on one bulk request (explicit ids, or the rows a bucket
/// selects). Larger backlogs take several requests; each is its own
/// batch with its own audit row.
pub const MAX_BULK: usize = 500;

/// One item of a bulk review that was skipped.
#[derive(Debug)]
pub struct BulkFailure {
    pub id: Uuid,
    pub error: ReviewError,
}

/// What [`bulk_accept`] / [`bulk_reject`] did.
#[derive(Debug, Default)]
pub struct BulkOutcome {
    /// Accepted (or rejected) rows, in request order.
    pub succeeded: Vec<sug::Model>,
    /// Edge pairs newly inserted (accept only; `created = true`).
    pub created: usize,
    /// Per-item refusals (not found, already reviewed / stale, conflicting
    /// edge). The rest of the batch still commits.
    pub failed: Vec<BulkFailure>,
}

/// Accept many suggestions in **one** transaction, each [`accept`] in its
/// own savepoint: a per-item refusal (not found, already reviewed, stale,
/// contradicting edge) rolls back only that item and is reported in
/// [`BulkOutcome::failed`]. A database error aborts the whole batch
/// (nothing commits). Duplicate ids are processed once. At most
/// [`MAX_BULK`] ids.
///
/// The caller writes one audit row for the batch and, when
/// `created > 0`, calls `AppState::similarity.invalidate_all()` once.
pub async fn bulk_accept<C>(conn: &C, ids: &[Uuid], actor: Uuid) -> Result<BulkOutcome, DbErr>
where
    C: ConnectionTrait + TransactionTrait,
{
    let txn = conn.begin().await?;
    let mut out = BulkOutcome::default();
    let mut seen = HashSet::new();
    for &id in ids.iter().take(MAX_BULK) {
        if !seen.insert(id) {
            continue;
        }
        match accept(&txn, id, actor, None).await {
            Ok(o) => {
                if o.created {
                    out.created += 1;
                }
                out.succeeded.push(o.suggestion);
            }
            Err(ReviewError::Db(e)) => return Err(e),
            Err(error) => out.failed.push(BulkFailure { id, error }),
        }
    }
    txn.commit().await?;
    Ok(out)
}

/// Reject many suggestions in one transaction; same per-item savepoint and
/// failure reporting as [`bulk_accept`].
pub async fn bulk_reject<C>(conn: &C, ids: &[Uuid], actor: Uuid) -> Result<BulkOutcome, DbErr>
where
    C: ConnectionTrait + TransactionTrait,
{
    let txn = conn.begin().await?;
    let mut out = BulkOutcome::default();
    let mut seen = HashSet::new();
    for &id in ids.iter().take(MAX_BULK) {
        if !seen.insert(id) {
            continue;
        }
        match reject(&txn, id, actor).await {
            Ok(row) => out.succeeded.push(row),
            Err(ReviewError::Db(e)) => return Err(e),
            Err(error) => out.failed.push(BulkFailure { id, error }),
        }
    }
    txn.commit().await?;
    Ok(out)
}

/// Ids of the pending suggestions in `bucket` (optionally one library),
/// highest confidence first, at most `limit`, plus how many match in
/// total (`limit = 0` only counts). Drives bulk accept's bucket mode.
pub async fn pending_ids_in_bucket<C: ConnectionTrait>(
    conn: &C,
    bucket: SuggestionBucket,
    library_id: Option<Uuid>,
    limit: usize,
) -> Result<(Vec<Uuid>, u64), DbErr> {
    let filter = SuggestionFilter {
        status: Some(SuggestionStatus::Pending),
        bucket: Some(bucket),
        library_id,
        series_id: None,
    };
    let cond = filter_condition(&filter).add(sug::Column::Bucket.eq(bucket.as_str()));
    let total = sug::Entity::find().filter(cond.clone()).count(conn).await?;
    if limit == 0 {
        return Ok((Vec::new(), total));
    }
    let ids: Vec<Uuid> = sug::Entity::find()
        .select_only()
        .column(sug::Column::Id)
        .filter(cond)
        .order_by_desc(sug::Column::Confidence)
        .order_by_asc(sug::Column::Id)
        .limit(limit as u64)
        .into_tuple()
        .all(conn)
        .await?;
    Ok((ids, total))
}

async fn set_status<C: ConnectionTrait>(
    conn: &C,
    row: sug::Model,
    status: SuggestionStatus,
    accepted_kind: Option<String>,
    actor: Uuid,
    now: chrono::DateTime<chrono::FixedOffset>,
) -> Result<sug::Model, DbErr> {
    use sea_orm::{ActiveModelTrait, Set};
    let mut am: sug::ActiveModel = row.into();
    am.status = Set(status.as_str().to_owned());
    am.accepted_kind = Set(accepted_kind);
    am.reviewed_at = Set(Some(now));
    am.reviewed_by = Set(Some(actor));
    am.updated_at = Set(now);
    am.update(conn).await
}

/// Filters for [`list`]. `None` means "any".
#[derive(Debug, Clone, Default)]
pub struct SuggestionFilter {
    /// `None` = every status **except** `stale` (stale rows only show when
    /// asked for by name).
    pub status: Option<SuggestionStatus>,
    pub bucket: Option<SuggestionBucket>,
    /// Series' library (both ends share it — suggestions are same-library).
    pub library_id: Option<Uuid>,
    /// Suggestions with this series on either end.
    pub series_id: Option<Uuid>,
}

/// Opaque keyset position for [`list`]: `(confidence DESC, id ASC)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SuggestionCursor {
    #[serde(rename = "c")]
    pub confidence: f32,
    #[serde(rename = "i")]
    pub id: Uuid,
}

/// One page from [`list`].
#[derive(Debug, Clone)]
pub struct SuggestionPage {
    pub items: Vec<sug::Model>,
    /// Pass back for the next page; `None` on the last page.
    pub next_cursor: Option<SuggestionCursor>,
    /// Matching rows across all pages. Only computed for the first page
    /// (`cursor = None`).
    pub total: Option<u64>,
    /// Matching rows per bucket (ignoring the `bucket` filter). First page
    /// only — drives the review UI's confidence tabs.
    pub bucket_counts: Option<BTreeMap<String, u64>>,
}

fn filter_condition(filter: &SuggestionFilter) -> Condition {
    let mut cond = Condition::all();
    match filter.status {
        Some(s) => cond = cond.add(sug::Column::Status.eq(s.as_str())),
        None => cond = cond.add(sug::Column::Status.ne(SuggestionStatus::Stale.as_str())),
    }
    if let Some(lib) = filter.library_id {
        cond = cond.add(
            sug::Column::FromSeriesId.in_subquery(
                sea_orm::sea_query::Query::select()
                    .column(entity::series::Column::Id)
                    .from(entity::series::Entity)
                    .and_where(entity::series::Column::LibraryId.eq(lib))
                    .to_owned(),
            ),
        );
    }
    if let Some(sid) = filter.series_id {
        cond = cond.add(
            Condition::any()
                .add(sug::Column::FromSeriesId.eq(sid))
                .add(sug::Column::ToSeriesId.eq(sid)),
        );
    }
    cond
}

/// Keyset-paginated suggestions, highest confidence first (ties by id).
/// `limit` is clamped to 1..=200.
pub async fn list<C: ConnectionTrait>(
    conn: &C,
    filter: &SuggestionFilter,
    cursor: Option<SuggestionCursor>,
    limit: u64,
) -> Result<SuggestionPage, DbErr> {
    let limit = limit.clamp(1, 200);
    let base = filter_condition(filter);
    let mut cond = base.clone();
    if let Some(b) = filter.bucket {
        cond = cond.add(sug::Column::Bucket.eq(b.as_str()));
    }
    let mut q = sug::Entity::find().filter(cond.clone());
    if let Some(c) = cursor {
        q = q.filter(
            Condition::any()
                .add(sug::Column::Confidence.lt(c.confidence))
                .add(
                    Condition::all()
                        .add(sug::Column::Confidence.eq(c.confidence))
                        .add(sug::Column::Id.gt(c.id)),
                ),
        );
    }
    let mut items = q
        .order_by_desc(sug::Column::Confidence)
        .order_by_asc(sug::Column::Id)
        .limit(limit + 1)
        .all(conn)
        .await?;
    let has_more = items.len() as u64 > limit;
    items.truncate(limit as usize);
    let next_cursor = if has_more {
        items.last().map(|r| SuggestionCursor {
            confidence: r.confidence,
            id: r.id,
        })
    } else {
        None
    };
    let (total, bucket_counts) = if cursor.is_none() {
        let total = sug::Entity::find().filter(cond).count(conn).await?;
        #[derive(FromQueryResult)]
        struct BucketCount {
            bucket: String,
            n: i64,
        }
        let rows = sug::Entity::find()
            .select_only()
            .column(sug::Column::Bucket)
            .column_as(sug::Column::Id.count(), "n")
            .filter(base)
            .group_by(sug::Column::Bucket)
            .into_model::<BucketCount>()
            .all(conn)
            .await?;
        let mut counts: BTreeMap<String, u64> = ["high", "medium", "low"]
            .into_iter()
            .map(|b| (b.to_owned(), 0))
            .collect();
        for r in rows {
            counts.insert(r.bucket, u64::try_from(r.n).unwrap_or(0));
        }
        (Some(total), Some(counts))
    } else {
        (None, None)
    };
    Ok(SuggestionPage {
        items,
        next_cursor,
        total,
        bucket_counts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use RelationshipKind as K;

    fn cand(from: Uuid, to: Uuid, kind: K, c: f32, source: EvidenceSource) -> Candidate {
        Candidate {
            from,
            to: to.into(),
            kind,
            confidence: c,
            source,
            reason: format!("{source:?}"),
            evidence: serde_json::json!({ "source": format!("{source:?}") }),
            scope: Scope::default(),
        }
    }

    #[test]
    fn canonical_forms() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert_eq!(canonicalize(a, b, K::HasSequel), (b, a, K::SequelOf));
        assert_eq!(canonicalize(a, b, K::PrequelOf), (a, b, K::PrequelOf));
        assert_eq!(canonicalize(a, b, K::HasPrequel), (b, a, K::PrequelOf));
        assert_eq!(canonicalize(a, b, K::ContinuedBy), (b, a, K::Continues));
        assert_eq!(canonicalize(a, b, K::HasSpinOff), (b, a, K::SpinOffOf));
        assert_eq!(canonicalize(a, b, K::CollectedIn), (b, a, K::Collects));
        assert_eq!(canonicalize(b, a, K::CompanionTo), (a, b, K::CompanionTo));
        for k in K::ALL {
            let (_, _, c) = canonicalize(a, b, k);
            assert!(c.is_canonical(), "{k} folds onto a canonical kind");
        }
        assert_eq!(canonicalize(b, a, K::SeeAlso), (a, b, K::SeeAlso));
        assert_eq!(
            canonicalize(a, b, K::CrossoverWith),
            (a, b, K::CrossoverWith)
        );
        assert_eq!(canonicalize(b, a, K::SequelOf), (b, a, K::SequelOf));
    }

    #[test]
    fn merge_dedupes_reversed_self_inverse_and_boosts_corroboration() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let merged = merge(vec![
            cand(a, b, K::CrossoverWith, 0.6, EvidenceSource::ArcCrossover),
            cand(b, a, K::CrossoverWith, 0.7, EvidenceSource::AlternateSeries),
            cand(b, a, K::CrossoverWith, 0.5, EvidenceSource::AlternateSeries),
        ]);
        assert_eq!(merged.len(), 1);
        let p = &merged[0];
        assert_eq!((p.from, p.to), (a, Target::Series(b)));
        assert!((p.confidence - 0.75).abs() < 1e-6, "{}", p.confidence);
        assert_eq!(p.bucket, SuggestionBucket::Medium);
        assert_eq!(p.evidence["sources"].as_array().unwrap().len(), 2);
        assert!(p.reason.starts_with("AlternateSeries"));
    }

    #[test]
    fn sequel_and_continues_are_equivalent_for_dedupe() {
        assert_eq!(
            equivalent_kinds(K::Continues),
            vec![K::SequelOf, K::Continues]
        );
        assert_eq!(
            equivalent_kinds(K::SequelOf),
            vec![K::SequelOf, K::Continues]
        );
        assert_eq!(
            equivalent_kinds(K::ContinuedBy),
            vec![K::HasSequel, K::ContinuedBy]
        );
        assert_eq!(equivalent_kinds(K::PrequelOf), vec![K::PrequelOf]);
        assert_eq!(equivalent_kinds(K::Collects), vec![K::Collects]);
    }

    #[test]
    fn merge_drops_self_edges() {
        let a = Uuid::from_u128(1);
        assert!(
            merge(vec![cand(
                a,
                a,
                K::SeeAlso,
                0.9,
                EvidenceSource::AlternateSeries
            )])
            .is_empty()
        );
    }

    #[test]
    fn merge_mirrors_scope_when_folding_and_fits_it_to_the_kind() {
        use crate::relationships::{RelationshipCoverage as Cov, RelationshipQualifier as Q};
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        // `collected_in` folds onto `collects` with the ends swapped: the
        // ranges swap too.
        let mut c = cand(a, b, K::CollectedIn, 0.8, EvidenceSource::ReprintRollup);
        c.scope = Scope {
            from_range: Some("1-6".into()),
            to_range: Some("1".into()),
            coverage: Some(Cov::Full),
            qualifier: Some(Q::Relaunch),
            note: Some("dropped".into()),
        };
        let merged = merge(vec![c]);
        let p = &merged[0];
        assert_eq!((p.from, p.to, p.kind), (b, Target::Series(a), K::Collects));
        assert_eq!(p.scope.from_range.as_deref(), Some("1"));
        assert_eq!(p.scope.to_range.as_deref(), Some("1-6"));
        assert_eq!(p.scope.coverage, Some(Cov::Full));
        assert_eq!(p.scope.qualifier, None, "collects takes no qualifier");
        assert_eq!(p.scope.note, None);

        // Fields come from the strongest source that has them.
        let mut strong = cand(a, b, K::Continues, 0.9, EvidenceSource::NameContinuation);
        strong.scope.from_range = Some("1-12".into());
        let mut weak = cand(a, b, K::Continues, 0.6, EvidenceSource::ProviderVolume);
        weak.scope.qualifier = Some(Q::Relaunch);
        weak.scope.from_range = Some("ignored".into());
        let p = &merge(vec![weak, strong])[0];
        assert_eq!(p.scope.from_range.as_deref(), Some("1-12"));
        assert_eq!(p.scope.qualifier, Some(Q::Relaunch));
    }

    #[test]
    fn merge_keeps_arc_targets_apart_and_drops_non_arc_kinds() {
        let (a, arc) = (Uuid::from_u128(1), Uuid::from_u128(9));
        let mut tie = cand(a, a, K::TieInTo, 0.7, EvidenceSource::ArcTieIn);
        tie.to = Target::Arc(arc);
        let mut bad = cand(a, a, K::SeeAlso, 0.7, EvidenceSource::ArcTieIn);
        bad.to = Target::Arc(arc);
        let merged = merge(vec![tie, bad]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].to, Target::Arc(arc));
        assert_eq!(merged[0].kind, K::TieInTo);
    }

    #[test]
    fn buckets() {
        assert_eq!(
            SuggestionBucket::from_confidence(0.8),
            SuggestionBucket::High
        );
        assert_eq!(
            SuggestionBucket::from_confidence(0.79),
            SuggestionBucket::Medium
        );
        assert_eq!(
            SuggestionBucket::from_confidence(0.55),
            SuggestionBucket::Medium
        );
        assert_eq!(
            SuggestionBucket::from_confidence(0.54),
            SuggestionBucket::Low
        );
    }
}
