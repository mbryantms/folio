//! Relationship suggestion engine (WP-7.2, spec §5.7 / Phase 7).
//!
//! [`generate_for_library`] proposes candidate relationships from evidence
//! already in the DB ([`sources`]) and upserts them into
//! `series_relationship_suggestion`. It **never** creates
//! `series_relationship` edges: an admin accepts ([`accept`], which calls
//! [`crate::relationships::create_pair`] with
//! [`RelationshipSource::Suggested`]) or rejects ([`reject`]) each one.
//!
//! Invariants:
//! - **Canonical rows.** Self-inverse kinds are stored with
//!   `from < to`; directional kinds only as `sequel_of` / `spin_off_of` /
//!   `collects` ([`canonicalize`]). The DB CHECKs mirror this.
//! - **Rejection memory.** A `(from, to, kind)` whose row is not `pending`
//!   is never rewritten or re-proposed; rows are never deleted.
//! - **Existing edges** with the same kind (or its inverse, which would
//!   contradict on accept) are not suggested.
//! - **Cap.** At most [`MAX_SUGGESTIONS_PER_RUN`] rows are written per run
//!   (highest confidence first).
//! - **Scope.** Suggestions only link series in the **same library**; every
//!   source query is library-scoped.
//!
//! See `docs/dev/series-relationships.md` ("Suggestion engine").

pub mod citations;
pub mod sources;

use super::{PairError, PairOutcome, RelationshipKind, RelationshipSource};
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

/// Review state. Transitions are one-way out of `pending`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionStatus {
    Pending,
    /// Accepted with the suggested kind.
    Accepted,
    Rejected,
    /// Accepted with a different kind (`accepted_kind`).
    Modified,
}

impl SuggestionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Modified => "modified",
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
            _ => Err(()),
        }
    }
}

/// Which heuristic produced a candidate (the `source` key of each
/// `evidence.sources[]` entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    AlternateSeries,
    SeriesGroup,
    StoryArc,
    NameContinuation,
    CollectedEdition,
    ProviderVolume,
    ProviderRange,
    CharacterDensity,
}

/// One proposal from one source, before canonicalization and merging.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub from: Uuid,
    pub to: Uuid,
    pub kind: RelationshipKind,
    pub confidence: f32,
    pub source: EvidenceSource,
    /// Human-readable; names both series so it reads the same in either
    /// direction.
    pub reason: String,
    /// `{ "source": "<snake_case source>", … }`.
    pub evidence: serde_json::Value,
}

/// Fold a `(from, to, kind)` onto the stored canonical form: directional
/// kinds become `sequel_of` / `spin_off_of` / `collects` (swapping ends for
/// their inverses), self-inverse kinds order the ends so `from < to`.
pub fn canonicalize(
    from: Uuid,
    to: Uuid,
    kind: RelationshipKind,
) -> (Uuid, Uuid, RelationshipKind) {
    use RelationshipKind as K;
    match kind {
        K::PrequelOf | K::HasSpinOff | K::CollectedIn => (to, from, kind.inverse()),
        K::CrossoverWith | K::SameUniverse | K::SeeAlso if to < from => (to, from, kind),
        _ => (from, to, kind),
    }
}

/// Merged, canonical suggestion ready to upsert.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub from: Uuid,
    pub to: Uuid,
    pub kind: RelationshipKind,
    pub confidence: f32,
    pub bucket: SuggestionBucket,
    pub reason: String,
    pub evidence: serde_json::Value,
}

/// Canonicalize and merge candidates that land on the same row. Confidence
/// is the strongest source's plus [`CORROBORATION_BONUS`] per additional
/// distinct source; reasons are joined strongest first; evidence keeps
/// every source entry (`{"sources": [...]}`). Self edges are dropped.
pub fn merge(candidates: Vec<Candidate>) -> Vec<Proposal> {
    struct Acc {
        best: BTreeMap<EvidenceSource, Candidate>,
    }
    let mut by_key: HashMap<(Uuid, Uuid, RelationshipKind), Acc> = HashMap::new();
    for c in candidates {
        if c.from == c.to || !c.confidence.is_finite() {
            continue;
        }
        let (from, to, kind) = canonicalize(c.from, c.to, c.kind);
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
    pub elapsed_ms: u64,
}

#[derive(Debug, FromQueryResult)]
struct KeyRow {
    from_series_id: Uuid,
    to_series_id: Uuid,
    kind: String,
}

#[derive(Debug, FromQueryResult)]
struct UpsertRow {
    inserted: bool,
    changed: bool,
}

/// Run every evidence source for `library_id`, merge, drop what is already
/// an edge or already reviewed, cap at [`MAX_SUGGESTIONS_PER_RUN`], and
/// upsert. Pending rows that are proposed again get their confidence,
/// bucket, reason and evidence refreshed; non-pending rows are never
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

    let (candidates, counts) = sources::collect_all(conn, library_id).await;
    report.by_source = counts.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    let proposals = merge(candidates);
    report.proposals = proposals.len();

    // Existing edges touching this library (bounded by curation).
    let edges = KeyRow::find_by_statement(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "SELECT r.from_series_id, r.to_series_id, r.kind \
           FROM series_relationship r \
           JOIN series s ON s.id = r.from_series_id \
          WHERE s.library_id = $1",
        [Value::from(library_id)],
    ))
    .all(conn)
    .await?;
    let edge_set: HashSet<(Uuid, Uuid, String)> = edges
        .into_iter()
        .map(|e| (e.from_series_id, e.to_series_id, e.kind))
        .collect();
    // Reviewed suggestions (the rejection memory).
    let reviewed = KeyRow::find_by_statement(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "SELECT g.from_series_id, g.to_series_id, g.kind \
           FROM series_relationship_suggestion g \
           JOIN series s ON s.id = g.from_series_id \
          WHERE s.library_id = $1 AND g.status <> 'pending'",
        [Value::from(library_id)],
    ))
    .all(conn)
    .await?;
    let reviewed_set: HashSet<(Uuid, Uuid, String)> = reviewed
        .into_iter()
        .map(|e| (e.from_series_id, e.to_series_id, e.kind))
        .collect();

    let mut keep: Vec<Proposal> = Vec::with_capacity(proposals.len().min(MAX_SUGGESTIONS_PER_RUN));
    for p in proposals {
        // Inverse rows are always stored, so checking the forward
        // orientation for both the kind and its inverse covers "already
        // related this way" and "would contradict on accept".
        let same = (p.from, p.to, p.kind.as_str().to_owned());
        let inv = (p.from, p.to, p.kind.inverse().as_str().to_owned());
        if edge_set.contains(&same) || edge_set.contains(&inv) {
            report.skipped_existing_edge += 1;
            continue;
        }
        if reviewed_set.contains(&same) {
            report.skipped_reviewed += 1;
            continue;
        }
        if keep.len() >= MAX_SUGGESTIONS_PER_RUN {
            report.capped += 1;
            continue;
        }
        keep.push(p);
    }

    for chunk in keep.chunks(UPSERT_CHUNK) {
        let rows = upsert_chunk(conn, chunk).await?;
        for r in rows {
            if r.inserted {
                report.inserted += 1;
            } else if r.changed {
                report.updated += 1;
            }
        }
    }
    // Rows the upsert matched but left alone: pending + identical.
    report.unchanged = keep.len().saturating_sub(report.inserted + report.updated);
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
        elapsed_ms = report.elapsed_ms,
        "relationship suggestions: run complete"
    );
    Ok(report)
}

/// `INSERT … ON CONFLICT DO UPDATE … WHERE status = 'pending'`. A reviewed
/// row conflicts and is left untouched (the `WHERE` guard also covers a
/// review that lands between the pre-filter and this statement). Returns
/// one row per inserted or *changed* pending row.
async fn upsert_chunk<C: ConnectionTrait>(
    conn: &C,
    chunk: &[Proposal],
) -> Result<Vec<UpsertRow>, DbErr> {
    let from: Vec<String> = chunk.iter().map(|p| p.from.to_string()).collect();
    let to: Vec<String> = chunk.iter().map(|p| p.to.to_string()).collect();
    let kind: Vec<String> = chunk.iter().map(|p| p.kind.as_str().to_owned()).collect();
    let conf: Vec<f64> = chunk.iter().map(|p| f64::from(p.confidence)).collect();
    let bucket: Vec<String> = chunk.iter().map(|p| p.bucket.as_str().to_owned()).collect();
    let reason: Vec<String> = chunk.iter().map(|p| p.reason.clone()).collect();
    let evidence: Vec<String> = chunk.iter().map(|p| p.evidence.to_string()).collect();
    let sql = r#"
        INSERT INTO series_relationship_suggestion
            (id, from_series_id, to_series_id, kind, confidence, bucket, reason, evidence,
             status, created_at, updated_at)
        SELECT gen_random_uuid(), f::uuid, t::uuid, k, c::real, b, r, e::jsonb, 'pending', now(), now()
          FROM unnest($1::text[], $2::text[], $3::text[], $4::float8[], $5::text[], $6::text[], $7::text[])
               AS x(f, t, k, c, b, r, e)
        ON CONFLICT (from_series_id, to_series_id, kind) DO UPDATE
           SET confidence = EXCLUDED.confidence,
               bucket     = EXCLUDED.bucket,
               reason     = EXCLUDED.reason,
               evidence   = EXCLUDED.evidence,
               updated_at = now()
         WHERE series_relationship_suggestion.status = 'pending'
           AND (series_relationship_suggestion.confidence IS DISTINCT FROM EXCLUDED.confidence
             OR series_relationship_suggestion.reason     IS DISTINCT FROM EXCLUDED.reason
             OR series_relationship_suggestion.evidence   IS DISTINCT FROM EXCLUDED.evidence)
        RETURNING (xmax = 0) AS inserted, true AS changed
    "#;
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
        ],
    ))
    .all(conn)
    .await
}

// ───── review (WP-7.3 builds on these) ─────

/// Why [`accept`] / [`reject`] refused.
#[derive(Debug)]
pub enum ReviewError {
    NotFound,
    /// Already accepted / rejected / modified — reviews are one-way.
    AlreadyReviewed {
        status: SuggestionStatus,
    },
    /// `create_pair` refused (self edge, contradicting kind, …).
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
    /// The pair from [`crate::relationships::create_pair`] (`created =
    /// false` when the edge already existed).
    pub pair: PairOutcome,
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

/// Accept a pending suggestion: create the edge pair via
/// [`crate::relationships::create_pair`] (`source = suggested`, the
/// suggestion's confidence, `created_by = actor`) and mark the row
/// `accepted` — or `modified` with `accepted_kind` when `kind_override`
/// differs from the suggested kind. The override is read in the
/// suggestion's `from → to` direction. Both writes share one transaction
/// (a savepoint when `conn` is already a transaction), and the row is
/// locked `FOR UPDATE`, so a double accept can't create twice.
///
/// **Callers must** call `AppState::similarity.invalidate_all()` after this
/// returns with `pair.created = true` (accepted edges feed WP-7.4's similar
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
    let pair = super::create_pair(
        &txn,
        row.from_series_id,
        row.to_series_id,
        kind,
        RelationshipSource::Suggested,
        Some(row.confidence.clamp(0.0, 1.0)),
        Some(actor),
    )
    .await
    .map_err(|e| match e {
        PairError::Db(d) => ReviewError::Db(d),
        other => ReviewError::Pair(other),
    })?;
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
        pair,
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
    if let Some(s) = filter.status {
        cond = cond.add(sug::Column::Status.eq(s.as_str()));
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
            to,
            kind,
            confidence: c,
            source,
            reason: format!("{source:?}"),
            evidence: serde_json::json!({ "source": format!("{source:?}") }),
        }
    }

    #[test]
    fn canonical_forms() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert_eq!(canonicalize(a, b, K::PrequelOf), (b, a, K::SequelOf));
        assert_eq!(canonicalize(a, b, K::HasSpinOff), (b, a, K::SpinOffOf));
        assert_eq!(canonicalize(a, b, K::CollectedIn), (b, a, K::Collects));
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
            cand(a, b, K::CrossoverWith, 0.6, EvidenceSource::StoryArc),
            cand(b, a, K::CrossoverWith, 0.7, EvidenceSource::AlternateSeries),
            cand(b, a, K::CrossoverWith, 0.5, EvidenceSource::AlternateSeries),
        ]);
        assert_eq!(merged.len(), 1);
        let p = &merged[0];
        assert_eq!((p.from, p.to), (a, b));
        assert!((p.confidence - 0.75).abs() < 1e-6, "{}", p.confidence);
        assert_eq!(p.bucket, SuggestionBucket::Medium);
        assert_eq!(p.evidence["sources"].as_array().unwrap().len(), 2);
        assert!(p.reason.starts_with("AlternateSeries"));
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
                EvidenceSource::SeriesGroup
            )])
            .is_empty()
        );
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
