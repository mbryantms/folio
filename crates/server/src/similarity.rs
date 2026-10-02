//! Explainable "similar series" scoring (WP-7.4).
//!
//! Content-based, no ML: two series are similar when they share
//! metadata entities — creators (by role), characters, teams, story arcs,
//! genres, tags, publisher and imprint. Every shared entity contributes
//! `weight(kind, role) × idf(entity)`, where the IDF is normalised to
//! `0..=1` (`ln(N / df) / ln(N)`, `N` = live series, `df` = live series
//! carrying the entity). Ubiquitous entities ("Marvel", the line editor
//! on every book) therefore count for almost nothing, rare shared ones
//! count fully. Per kind the sum is capped (so fifty shared cover
//! artists can't drown out everything else) and, for the long-list
//! kinds (creators, characters, teams), damped by
//! `min(1, sqrt(|target kind entities| / |candidate kind entities|))` so a
//! 700-issue flagship that shares a handful of names with everything
//! doesn't top every list. The total is the sum over kinds.
//!
//! Every match carries a "because" list — the largest individual
//! contributions, with the entity kind, role (creators only) and name —
//! so the UI can say *why* ("writer Ed Brubaker, character Bucky
//! Barnes, arc Winter Soldier"). Nothing is hidden in the score that is
//! not in the overlap: each reason is a shared entity.
//!
//! **Signals are additive [`Contribution`]s.** The junction overlap is
//! one source ([`fetch_overlap`]); accepted series relationships
//! (WP-7.1, [`fetch_relationships`]) are another, producing
//! `ReasonKind::Relationship` contributions at a flat
//! [`RELATIONSHIP_WEIGHT`] — enough on its own to list a related series
//! even when it shares no metadata. Accepted arc tie-ins (WP-8.2,
//! [`fetch_arc_tie_ins`]) are a third: an arc reason per arc both series
//! tie in to, deduped against the `issue_arcs` arc signal (max) and under
//! the same arc cap.
//!
//! **Cache.** Results are computed on demand (two set-based queries) and
//! kept in a small in-process LRU ([`SimilarityCache`]) keyed by series
//! id, stamped with a global generation. Scans, metadata applies and
//! manual metadata edits call [`SimilarityCache::invalidate_all`] — a
//! change to *any* series can move it into or out of another series'
//! neighbour list and shifts the IDF, so per-entry invalidation would be
//! wrong. Folio is single-instance (roadmap D2), so an in-process cache
//! is sufficient; a TTL bounds staleness from any write path that
//! doesn't hook the invalidation. The cache holds the **unfiltered**
//! neighbour list — library ACL, age-rating caps and per-user hidden
//! series are applied per request, so nothing leaks across users.
//!
//! See `docs/dev/similar-series.md`.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lru::LruCache;
use sea_orm::{ConnectionTrait, DbBackend, DbErr, FromQueryResult, Statement, Value};
use serde::Serialize;
use uuid::Uuid;

/// Entity kinds that can explain a match. `Relationship` is produced by
/// the accepted-relationship signal (WP-7.1); it's part of the enum from
/// day one so wiring that signal doesn't change the response schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = SimilarReasonKind)]
pub enum ReasonKind {
    Creator,
    Character,
    Team,
    Arc,
    Genre,
    Tag,
    Publisher,
    Imprint,
    Relationship,
}

impl ReasonKind {
    fn from_sql(s: &str) -> Option<Self> {
        Some(match s {
            "creator" => Self::Creator,
            "character" => Self::Character,
            "team" => Self::Team,
            "arc" => Self::Arc,
            "genre" => Self::Genre,
            "tag" => Self::Tag,
            "publisher" => Self::Publisher,
            "imprint" => Self::Imprint,
            "relationship" => Self::Relationship,
            _ => return None,
        })
    }

    /// Weight of one fully-rare shared entity of this kind. Creators are
    /// weighted per role ([`creator_role_weight`]).
    fn weight(self) -> f64 {
        match self {
            Self::Creator => 0.5,
            Self::Character | Self::Team => 1.5,
            Self::Arc => 3.0,
            Self::Genre | Self::Tag => 0.75,
            Self::Publisher => 0.5,
            Self::Imprint => 1.0,
            Self::Relationship => RELATIONSHIP_WEIGHT,
        }
    }

    /// Ceiling on the summed contribution of this kind.
    fn cap(self) -> f64 {
        match self {
            Self::Creator | Self::Character | Self::Arc => 6.0,
            Self::Team => 3.0,
            Self::Genre | Self::Tag => 1.5,
            Self::Publisher => 0.5,
            Self::Imprint => 1.0,
            Self::Relationship => RELATIONSHIP_WEIGHT,
        }
    }

    /// Long-list kinds get the breadth damping (see module docs).
    fn damped(self) -> bool {
        matches!(self, Self::Creator | Self::Character | Self::Team)
    }
}

/// Per-role creator weight. Writers and pencillers define a book far
/// more than letterers and editors, who also tend to span whole lines.
pub fn creator_role_weight(role: &str) -> f64 {
    match role {
        "writer" => 3.0,
        "penciller" => 2.0,
        "inker" => 0.75,
        "colorist" | "cover_artist" | "translator" => 0.5,
        "letterer" | "editor" => 0.25,
        _ => 0.5,
    }
}

/// Flat score for an accepted relationship (sequel, spin-off, …). Not
/// IDF-weighted: a curated link is strong evidence on its own.
pub const RELATIONSHIP_WEIGHT: f64 = 6.0;
/// Candidates below this total are dropped (one shared rare writer
/// clears it; a shared genre alone does not).
pub const MIN_SCORE: f64 = 1.0;
/// Neighbours kept per series. "Top matches" — the tail beyond this is
/// noise by construction.
pub const MAX_NEIGHBORS: usize = 100;
/// Reasons kept per neighbour.
pub const MAX_REASONS: usize = 5;
/// Entities carried by more than this fraction of live series are
/// skipped entirely (their IDF is ~0 anyway, and they'd dominate the
/// postings the overlap query has to walk).
const STOP_DF_FRACTION: f64 = 0.5;

const CACHE_CAPACITY: usize = 512;
const CACHE_TTL: Duration = Duration::from_secs(15 * 60);

/// One shared entity explaining a match.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[schema(as = SimilarReason)]
pub struct Reason {
    pub kind: ReasonKind,
    /// Credit role (`writer`, `penciller`, …) for creators; relationship
    /// kind (`sequel_of`, …) for relationships; absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Relationships: the kind's display label, lower-cased ("sequel to",
    /// "continued by"; WP-7.5), so clients don't need the kind catalogue
    /// to caption a reason. Arcs (WP-8.2): `"both tie in to"` when the
    /// shared arc comes from accepted tie-in edges rather than issue
    /// tagging. Absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Display name of the shared entity (or the related series).
    pub name: String,
    /// This entity's contribution to the score (before the per-kind cap).
    pub weight: f64,
}

/// One scored neighbour, unfiltered (ACL is applied per request).
#[derive(Debug, Clone, PartialEq)]
pub struct Neighbor {
    pub series_id: Uuid,
    pub library_id: Uuid,
    pub age_rating: Option<String>,
    pub score: f64,
    pub because: Vec<Reason>,
}

/// One additive piece of evidence that `series_id` resembles the target.
#[derive(Debug, Clone)]
pub struct Contribution {
    pub series_id: Uuid,
    pub kind: ReasonKind,
    pub role: Option<String>,
    pub name: String,
    pub value: f64,
    /// Relationships: the display label (tie-in role folded in, WP-7.7),
    /// lower-cased; `None` falls back to the kind's label. Arcs:
    /// [`TIE_IN_LABEL`] for the accepted-tie-in signal (WP-8.2).
    pub label: Option<String>,
}

/// Library + rating of a candidate, carried so the per-request ACL
/// filter needs no extra query.
#[derive(Debug, Clone)]
pub struct CandidateMeta {
    pub library_id: Uuid,
    pub age_rating: Option<String>,
}

/// Normalised IDF in `0..=1`.
pub fn idf(n: i64, df: i64) -> f64 {
    let n = n.max(2) as f64;
    let df = df.max(1) as f64;
    ((n / df).ln() / n.ln()).clamp(0.0, 1.0)
}

/// Pure scorer. `target_sizes` / `cand_sizes` count each damped kind's
/// distinct entities on the target and on every candidate.
pub fn score(
    contributions: Vec<Contribution>,
    meta: &HashMap<Uuid, CandidateMeta>,
    target_sizes: &HashMap<ReasonKind, f64>,
    cand_sizes: &HashMap<(Uuid, ReasonKind), f64>,
) -> Vec<Neighbor> {
    // Dedupe per (candidate, kind, entity): a person credited as writer
    // *and* penciller on both books counts once, at the strongest role.
    let mut best: HashMap<(Uuid, ReasonKind, String), Contribution> = HashMap::new();
    for c in contributions {
        if c.value <= 0.0 {
            continue;
        }
        let key = (c.series_id, c.kind, c.name.to_lowercase());
        match best.get(&key) {
            Some(prev) if prev.value >= c.value => {}
            _ => {
                best.insert(key, c);
            }
        }
    }

    let mut per_series: HashMap<Uuid, Vec<Contribution>> = HashMap::new();
    for (_, c) in best {
        per_series.entry(c.series_id).or_default().push(c);
    }

    let mut out: Vec<Neighbor> = Vec::new();
    for (series_id, mut contribs) in per_series {
        let Some(m) = meta.get(&series_id) else {
            continue;
        };
        let mut per_kind: HashMap<ReasonKind, f64> = HashMap::new();
        for c in &contribs {
            *per_kind.entry(c.kind).or_default() += c.value;
        }
        let total: f64 = per_kind
            .into_iter()
            .map(|(kind, sum)| {
                let capped = sum.min(kind.cap());
                if kind.damped() {
                    let t = target_sizes.get(&kind).copied().unwrap_or(1.0).max(1.0);
                    let c = cand_sizes
                        .get(&(series_id, kind))
                        .copied()
                        .unwrap_or(1.0)
                        .max(1.0);
                    capped * (t / c).sqrt().min(1.0)
                } else {
                    capped
                }
            })
            .sum();
        if total < MIN_SCORE {
            continue;
        }
        contribs.sort_by(|a, b| {
            b.value
                .total_cmp(&a.value)
                .then_with(|| a.kind.cmp(&b.kind))
                .then_with(|| a.name.cmp(&b.name))
        });
        let because = contribs
            .into_iter()
            .take(MAX_REASONS)
            .map(|c| Reason {
                kind: c.kind,
                label: c
                    .label
                    .or_else(|| relationship_label(c.kind, c.role.as_deref())),
                role: c.role,
                name: c.name,
                weight: round3(c.value),
            })
            .collect();
        out.push(Neighbor {
            series_id,
            library_id: m.library_id,
            age_rating: m.age_rating.clone(),
            score: round3(total),
            because,
        });
    }
    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.series_id.cmp(&b.series_id))
    });
    out.truncate(MAX_NEIGHBORS);
    out
}

/// Rounded so the wire value is stable and the keyset cursor round-trips.
fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

// ───── SQL sources ─────

/// Every (candidate, shared entity) pair for the target, with the
/// entity's document frequency. One statement: the target's entities,
/// their postings across every series' junctions, and `df` per entity.
/// Each posting arm is an index probe (`series_credits(role, person)`,
/// `btrim(lower(character|team))`, `genre`, `tag`, `arc_id`,
/// `btrim(lower(publisher))`); see `docs/dev/similar-series.md` for the
/// EXPLAIN notes.
const OVERLAP_SQL: &str = r#"
WITH feats AS MATERIALIZED (
  SELECT 'creator'::text AS kind, sc.role AS role, sc.person AS key, min(sc.person) AS label
    FROM series_credits sc WHERE sc.series_id = $1 GROUP BY sc.role, sc.person
  UNION ALL
  SELECT 'character', '', btrim(lower(c."character")), min(c."character")
    FROM series_characters c WHERE c.series_id = $1 GROUP BY 3
  UNION ALL
  SELECT 'team', '', btrim(lower(t.team)), min(t.team)
    FROM series_teams t WHERE t.series_id = $1 GROUP BY 3
  UNION ALL
  SELECT 'genre', '', g.genre, g.genre FROM series_genres g WHERE g.series_id = $1
  UNION ALL
  SELECT 'tag', '', t.tag, t.tag FROM series_tags t WHERE t.series_id = $1
  UNION ALL
  SELECT 'arc', '', a.id::text, a.name FROM story_arc a
   WHERE a.id IN (SELECT sa.arc_id FROM series_arcs sa WHERE sa.series_id = $1
                  UNION
                  SELECT ia.arc_id FROM issue_arcs ia JOIN issues i ON i.id = ia.issue_id
                   WHERE i.series_id = $1 AND i.state = 'active' AND i.removed_at IS NULL)
  UNION ALL
  SELECT 'publisher', '', btrim(lower(s.publisher)), s.publisher FROM series s
   WHERE s.id = $1 AND btrim(coalesce(s.publisher, '')) <> ''
  UNION ALL
  SELECT 'imprint', '', btrim(lower(s.imprint)), s.imprint FROM series s
   WHERE s.id = $1 AND btrim(coalesce(s.imprint, '')) <> ''
),
postings AS (
  SELECT f.kind, f.role, f.key, sc.series_id FROM feats f
    JOIN series_credits sc ON f.kind = 'creator' AND sc.role = f.role AND sc.person = f.key
  UNION
  SELECT f.kind, f.role, f.key, c.series_id FROM feats f
    JOIN series_characters c ON f.kind = 'character' AND btrim(lower(c."character")) = f.key
  UNION
  SELECT f.kind, f.role, f.key, t.series_id FROM feats f
    JOIN series_teams t ON f.kind = 'team' AND btrim(lower(t.team)) = f.key
  UNION
  SELECT f.kind, f.role, f.key, g.series_id FROM feats f
    JOIN series_genres g ON f.kind = 'genre' AND g.genre = f.key
  UNION
  SELECT f.kind, f.role, f.key, t.series_id FROM feats f
    JOIN series_tags t ON f.kind = 'tag' AND t.tag = f.key
  UNION
  SELECT f.kind, f.role, f.key, sa.series_id FROM feats f
    JOIN series_arcs sa ON f.kind = 'arc' AND sa.arc_id = f.key::uuid
  UNION
  SELECT f.kind, f.role, f.key, i.series_id FROM feats f
    JOIN issue_arcs ia ON f.kind = 'arc' AND ia.arc_id = f.key::uuid
    JOIN issues i ON i.id = ia.issue_id AND i.state = 'active' AND i.removed_at IS NULL
  UNION
  SELECT f.kind, f.role, f.key, s.id FROM feats f
    JOIN series s ON f.kind = 'publisher' AND btrim(lower(s.publisher)) = f.key
  UNION
  SELECT f.kind, f.role, f.key, s.id FROM feats f
    JOIN series s ON f.kind = 'imprint' AND btrim(lower(s.imprint)) = f.key
),
live AS MATERIALIZED (
  SELECT p.kind, p.role, p.key, p.series_id, s.library_id, s.age_rating
    FROM postings p JOIN series s ON s.id = p.series_id AND s.removed_at IS NULL
),
df AS MATERIALIZED (SELECT kind, role, key, count(*) AS df FROM live GROUP BY 1, 2, 3),
n AS MATERIALIZED (SELECT count(*) AS n FROM series WHERE removed_at IS NULL)
SELECT l.series_id, l.library_id, l.age_rating, l.kind, l.role, f.label,
       df.df, n.n
  FROM live l
  JOIN df USING (kind, role, key)
  JOIN feats f USING (kind, role, key)
 CROSS JOIN n
 WHERE l.series_id <> $1
   AND df.df::float8 <= n.n::float8 * $2
"#;

#[derive(Debug, FromQueryResult)]
struct OverlapRow {
    series_id: Uuid,
    library_id: Uuid,
    age_rating: Option<String>,
    kind: String,
    role: String,
    label: String,
    df: i64,
    n: i64,
}

/// Junction-overlap signal: contributions + candidate metadata.
async fn fetch_overlap<C: ConnectionTrait>(
    db: &C,
    target: Uuid,
) -> Result<(Vec<Contribution>, HashMap<Uuid, CandidateMeta>), DbErr> {
    let rows = OverlapRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        OVERLAP_SQL,
        [target.into(), Value::from(STOP_DF_FRACTION)],
    ))
    .all(db)
    .await?;
    let mut meta = HashMap::new();
    let mut contributions = Vec::with_capacity(rows.len());
    for r in rows {
        let Some(kind) = ReasonKind::from_sql(&r.kind) else {
            continue;
        };
        let weight = if kind == ReasonKind::Creator {
            creator_role_weight(&r.role)
        } else {
            kind.weight()
        };
        meta.entry(r.series_id).or_insert_with(|| CandidateMeta {
            library_id: r.library_id,
            age_rating: r.age_rating.clone(),
        });
        contributions.push(Contribution {
            series_id: r.series_id,
            kind,
            role: (kind == ReasonKind::Creator).then_some(r.role),
            name: r.label,
            value: weight * idf(r.n, r.df),
            label: None,
        });
    }
    Ok((contributions, meta))
}

#[derive(Debug, FromQueryResult)]
struct SizeRow {
    series_id: Uuid,
    kind: String,
    n: i64,
}

/// Distinct entity counts per damped kind for the target + candidates.
async fn fetch_sizes<C: ConnectionTrait>(
    db: &C,
    ids: Vec<Uuid>,
) -> Result<HashMap<(Uuid, ReasonKind), f64>, DbErr> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = SizeRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
        SELECT series_id, 'creator' AS kind, count(DISTINCT person) AS n
          FROM series_credits WHERE series_id = ANY($1) GROUP BY series_id
        UNION ALL
        SELECT series_id, 'character', count(*) FROM series_characters
         WHERE series_id = ANY($1) GROUP BY series_id
        UNION ALL
        SELECT series_id, 'team', count(*) FROM series_teams
         WHERE series_id = ANY($1) GROUP BY series_id
        "#,
        [Value::from(ids)],
    ))
    .all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| ReasonKind::from_sql(&r.kind).map(|k| ((r.series_id, k), r.n as f64)))
        .collect())
}

/// Lower-cased kind label for a relationship reason (`None` otherwise).
fn relationship_label(kind: ReasonKind, role: Option<&str>) -> Option<String> {
    if kind != ReasonKind::Relationship {
        return None;
    }
    let k: crate::relationships::RelationshipKind = role?.parse().ok()?;
    Some(k.label().to_lowercase())
}

#[derive(Debug, FromQueryResult)]
struct RelationshipRow {
    series_id: Uuid,
    library_id: Uuid,
    age_rating: Option<String>,
    kind: String,
    qualifier: Option<String>,
    target_name: String,
}

/// Accepted-relationship signal (WP-7.1 `series_relationship`). Every
/// row counts — manual or suggested — because WP-7.2 suggestions live in
/// their own table and only land here once accepted. Inverse rows are
/// always stored, so the edges out of the target are its whole
/// neighbourhood. The reason reads from the *candidate's* side
/// (`kind.inverse()`, named after the target, with its year so same-name
/// volumes stay distinguishable): on Daredevil (2014)'s page, Daredevil
/// (2011) is "continued by Daredevil (2014)". Arc-target edges (WP-7.5)
/// have no `to_series_id` and drop out of the join. Flat
/// [`RELATIONSHIP_WEIGHT`], not IDF-weighted: a curated link is strong
/// evidence on its own.
async fn fetch_relationships<C: ConnectionTrait>(
    db: &C,
    target: Uuid,
) -> Result<(Vec<Contribution>, HashMap<Uuid, CandidateMeta>), DbErr> {
    let rows = RelationshipRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
        SELECT r.to_series_id AS series_id, s.library_id, s.age_rating,
               r.kind, r.qualifier,
               t.name || COALESCE(' (' || t.year || ')', '') AS target_name
          FROM series_relationship r
          JOIN series s ON s.id = r.to_series_id AND s.removed_at IS NULL
          JOIN series t ON t.id = r.from_series_id
         WHERE r.from_series_id = $1
        "#,
        [target.into()],
    ))
    .all(db)
    .await?;
    let mut meta = HashMap::new();
    let mut contributions = Vec::with_capacity(rows.len());
    for r in rows {
        let Ok(kind) = r.kind.parse::<crate::relationships::RelationshipKind>() else {
            continue;
        };
        meta.entry(r.series_id).or_insert_with(|| CandidateMeta {
            library_id: r.library_id,
            age_rating: r.age_rating.clone(),
        });
        // WP-7.7: a tie-in role reads as itself ("has prelude", not "has
        // tie-in"); the stored qualifier is shared by both halves.
        let qualifier = r.qualifier.as_deref().and_then(|q| q.parse().ok());
        contributions.push(Contribution {
            series_id: r.series_id,
            kind: ReasonKind::Relationship,
            role: Some(kind.inverse().as_str().to_owned()),
            name: r.target_name,
            value: RELATIONSHIP_WEIGHT,
            label: Some(kind.inverse().display_label(qualifier).to_lowercase()),
        });
    }
    Ok((contributions, meta))
}

#[derive(Debug, FromQueryResult)]
struct TieInRow {
    series_id: Uuid,
    library_id: Uuid,
    age_rating: Option<String>,
    arc_name: String,
    df: i64,
    n: i64,
}

/// Accepted-arc-tie-in signal (WP-8.2): two series that both have an
/// accepted `tie_in_to` edge to the same story arc (WP-7.5 arc targets,
/// accepted from the WP-7.6 arc tie-in suggestions or made by hand).
/// Each shared arc contributes `ARC weight × idf`, where `df` counts the
/// live series tied in to that arc — a 60-title event counts for much less
/// than a two-title crossover.
///
/// **No double count with the `issue_arcs` arc signal**: the contribution
/// is an [`ReasonKind::Arc`] reason named after the same arc, so
/// [`score`]'s per-(candidate, kind, entity) dedupe keeps only the larger
/// of the two (the curated edge or the issue tagging, never both), and the
/// arc kind's cap (6.0) bounds every arc reason together. The tie-in can
/// only *add* when the issues aren't tagged with the arc (a manual edge,
/// or tagging the scanner never rolled up) or when fewer series tie in
/// than carry the tag. The reason reads "both tie in to Secret Wars"
/// (`label`).
async fn fetch_arc_tie_ins<C: ConnectionTrait>(
    db: &C,
    target: Uuid,
) -> Result<(Vec<Contribution>, HashMap<Uuid, CandidateMeta>), DbErr> {
    let rows = TieInRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
        WITH mine AS (
            SELECT DISTINCT r.to_arc_id FROM series_relationship r
             WHERE r.from_series_id = $1 AND r.kind = 'tie_in_to' AND r.to_arc_id IS NOT NULL
        ), peers AS MATERIALIZED (
            SELECT DISTINCT r.from_series_id AS series_id, r.to_arc_id
              FROM series_relationship r
              JOIN mine USING (to_arc_id)
              JOIN series s ON s.id = r.from_series_id AND s.removed_at IS NULL
             WHERE r.kind = 'tie_in_to'
        ), df AS (
            SELECT to_arc_id, count(*) AS df FROM peers GROUP BY 1
        ), n AS (SELECT count(*) AS n FROM series WHERE removed_at IS NULL)
        SELECT p.series_id, s.library_id, s.age_rating, a.name AS arc_name, df.df, n.n
          FROM peers p
          JOIN df USING (to_arc_id)
          JOIN story_arc a ON a.id = p.to_arc_id
          JOIN series s ON s.id = p.series_id
         CROSS JOIN n
         WHERE p.series_id <> $1
        "#,
        [target.into()],
    ))
    .all(db)
    .await?;
    let mut meta = HashMap::new();
    let mut contributions = Vec::with_capacity(rows.len());
    for r in rows {
        meta.entry(r.series_id).or_insert_with(|| CandidateMeta {
            library_id: r.library_id,
            age_rating: r.age_rating.clone(),
        });
        contributions.push(Contribution {
            series_id: r.series_id,
            kind: ReasonKind::Arc,
            role: None,
            name: r.arc_name,
            value: ReasonKind::Arc.weight() * idf(r.n, r.df),
            label: Some(TIE_IN_LABEL.to_owned()),
        });
    }
    Ok((contributions, meta))
}

/// `label` of an arc reason that comes from accepted tie-in edges
/// (WP-8.2): the web renders "both tie in to <arc>".
pub const TIE_IN_LABEL: &str = "both tie in to";

/// Compute the unfiltered neighbour list for one series: the junction
/// overlap + accepted relationships + shared accepted arc tie-ins, then
/// the candidate sizes for the breadth damping (4 queries; 3 when nothing
/// is shared).
pub async fn compute<C: ConnectionTrait>(db: &C, target: Uuid) -> Result<Vec<Neighbor>, DbErr> {
    let (mut contributions, mut meta) = fetch_overlap(db, target).await?;
    let (rel_contributions, rel_meta) = fetch_relationships(db, target).await?;
    let (tie_contributions, tie_meta) = fetch_arc_tie_ins(db, target).await?;
    contributions.extend(rel_contributions);
    contributions.extend(tie_contributions);
    for (id, m) in rel_meta.into_iter().chain(tie_meta) {
        meta.entry(id).or_insert(m);
    }
    if contributions.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids: Vec<Uuid> = meta.keys().copied().collect();
    ids.push(target);
    let mut sizes = fetch_sizes(db, ids).await?;
    let target_sizes: HashMap<ReasonKind, f64> =
        [ReasonKind::Creator, ReasonKind::Character, ReasonKind::Team]
            .into_iter()
            .filter_map(|k| sizes.remove(&(target, k)).map(|n| (k, n)))
            .collect();
    Ok(score(contributions, &meta, &target_sizes, &sizes))
}

// ───── Cache ─────

struct CacheEntry {
    generation: u64,
    at: Instant,
    neighbors: Arc<Vec<Neighbor>>,
}

struct CacheInner {
    generation: u64,
    lru: LruCache<Uuid, CacheEntry>,
}

/// Process-local neighbour cache (see module docs). Holds unfiltered
/// lists only; never a per-user view.
pub struct SimilarityCache {
    inner: Mutex<CacheInner>,
}

impl Default for SimilarityCache {
    fn default() -> Self {
        Self::new()
    }
}

impl SimilarityCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(CacheInner {
                generation: 0,
                lru: LruCache::new(NonZeroUsize::new(CACHE_CAPACITY).expect("nonzero")),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CacheInner> {
        // A panic while holding this lock can only come from LruCache
        // internals; the data is a cache, so recover it rather than
        // poisoning every later request.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Drop every cached list. Called after scans, metadata applies and
    /// manual metadata edits.
    pub fn invalidate_all(&self) {
        let mut g = self.lock();
        g.generation = g.generation.wrapping_add(1);
        g.lru.clear();
    }

    /// Current generation (bumped by every invalidation).
    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Number of cached series (tests / diagnostics).
    pub fn len(&self) -> usize {
        self.lock().lru.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn get(&self, id: Uuid) -> Option<Arc<Vec<Neighbor>>> {
        let mut g = self.lock();
        let generation = g.generation;
        let hit = g
            .lru
            .get(&id)
            .filter(|e| e.generation == generation && e.at.elapsed() < CACHE_TTL)
            .map(|e| e.neighbors.clone());
        if hit.is_none() {
            g.lru.pop(&id);
        }
        hit
    }

    /// Store a list computed under `generation`. Dropped when an
    /// invalidation landed mid-compute, so a stale list can't outlive it.
    fn put(&self, id: Uuid, generation: u64, neighbors: Arc<Vec<Neighbor>>) {
        let mut g = self.lock();
        if g.generation != generation {
            return;
        }
        g.lru.put(
            id,
            CacheEntry {
                generation,
                at: Instant::now(),
                neighbors,
            },
        );
    }
}

/// Cached neighbour list for `series_id` (computing on a miss).
pub async fn neighbors(
    state: &crate::state::AppState,
    series_id: Uuid,
) -> Result<Arc<Vec<Neighbor>>, DbErr> {
    let cache = &state.similarity;
    if let Some(hit) = cache.get(series_id) {
        metrics::counter!("folio_similar_series_cache_total", "result" => "hit").increment(1);
        return Ok(hit);
    }
    metrics::counter!("folio_similar_series_cache_total", "result" => "miss").increment(1);
    let generation = cache.generation();
    let list = Arc::new(compute(&state.db, series_id).await?);
    cache.put(series_id, generation, list.clone());
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(
        series: Uuid,
        kind: ReasonKind,
        role: Option<&str>,
        name: &str,
        value: f64,
    ) -> Contribution {
        Contribution {
            series_id: series,
            kind,
            role: role.map(str::to_owned),
            name: name.to_owned(),
            value,
            label: None,
        }
    }

    fn meta(ids: &[Uuid]) -> HashMap<Uuid, CandidateMeta> {
        ids.iter()
            .map(|id| {
                (
                    *id,
                    CandidateMeta {
                        library_id: Uuid::nil(),
                        age_rating: None,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn idf_is_normalised() {
        assert_eq!(idf(100, 100), 0.0);
        assert!((idf(100, 1) - 1.0).abs() < 1e-9);
        assert!(idf(100, 10) > idf(100, 50));
        // Degenerate libraries don't divide by zero.
        assert!(idf(1, 1).is_finite());
        assert!(idf(0, 0).is_finite());
    }

    #[test]
    fn ranks_by_overlap_and_explains_it() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let contribs = vec![
            c(a, ReasonKind::Creator, Some("writer"), "Ed Brubaker", 3.0),
            c(a, ReasonKind::Character, None, "Bucky Barnes", 1.5),
            c(b, ReasonKind::Creator, Some("writer"), "Ed Brubaker", 3.0),
        ];
        let out = score(contribs, &meta(&[a, b]), &HashMap::new(), &HashMap::new());
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].series_id, a);
        assert_eq!(out[0].score, 4.5);
        let names: Vec<_> = out[0].because.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Ed Brubaker", "Bucky Barnes"]);
        assert_eq!(out[0].because[0].role.as_deref(), Some("writer"));
    }

    #[test]
    fn same_person_in_two_roles_counts_once() {
        let a = Uuid::from_u128(1);
        let contribs = vec![
            c(a, ReasonKind::Creator, Some("writer"), "X", 3.0),
            c(a, ReasonKind::Creator, Some("penciller"), "X", 2.0),
        ];
        let out = score(contribs, &meta(&[a]), &HashMap::new(), &HashMap::new());
        assert_eq!(out[0].score, 3.0);
        assert_eq!(out[0].because.len(), 1);
        assert_eq!(out[0].because[0].role.as_deref(), Some("writer"));
    }

    #[test]
    fn per_kind_cap_and_min_score() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let mut contribs: Vec<_> = (0..20)
            .map(|i| c(a, ReasonKind::Genre, None, &format!("g{i}"), 0.75))
            .collect();
        contribs.push(c(b, ReasonKind::Genre, None, "g0", 0.75));
        let out = score(contribs, &meta(&[a, b]), &HashMap::new(), &HashMap::new());
        // Genre is capped at 1.5; b's lone genre (0.75) is under MIN_SCORE.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].score, 1.5);
        assert_eq!(out[0].because.len(), MAX_REASONS);
    }

    #[test]
    fn broad_candidates_are_damped() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let contribs = vec![
            c(a, ReasonKind::Creator, Some("writer"), "W", 3.0),
            c(b, ReasonKind::Creator, Some("writer"), "W", 3.0),
        ];
        let target = HashMap::from([(ReasonKind::Creator, 4.0)]);
        let sizes = HashMap::from([
            ((a, ReasonKind::Creator), 4.0),
            ((b, ReasonKind::Creator), 400.0),
        ]);
        let out = score(contribs, &meta(&[a, b]), &target, &sizes);
        assert_eq!(out[0].series_id, a);
        assert_eq!(out[0].score, 3.0);
        assert!(out.len() == 1 || out[1].score < 1.0 + 1e-9);
    }

    #[test]
    fn tie_in_and_issue_arc_signals_take_the_max() {
        // WP-8.2: the same arc from issue tagging (1.2) and from accepted
        // tie-in edges (2.4) counts once, at the larger value, with the
        // tie-in's label; the arc cap bounds both signals together.
        let a = Uuid::from_u128(1);
        let mut tie = c(a, ReasonKind::Arc, None, "Secret Wars", 2.4);
        tie.label = Some(TIE_IN_LABEL.to_owned());
        let contribs = vec![c(a, ReasonKind::Arc, None, "Secret Wars", 1.2), tie];
        let out = score(contribs, &meta(&[a]), &HashMap::new(), &HashMap::new());
        assert_eq!(out[0].score, 2.4);
        assert_eq!(out[0].because.len(), 1);
        assert_eq!(out[0].because[0].label.as_deref(), Some(TIE_IN_LABEL));
        // Tagging stronger than the edge: the tagged reason wins, no label.
        let mut tie = c(a, ReasonKind::Arc, None, "Secret Wars", 0.5);
        tie.label = Some(TIE_IN_LABEL.to_owned());
        let contribs = vec![tie, c(a, ReasonKind::Arc, None, "secret wars", 1.2)];
        let out = score(contribs, &meta(&[a]), &HashMap::new(), &HashMap::new());
        assert_eq!(out[0].score, 1.2);
        assert_eq!(out[0].because[0].label, None);
        // Many shared arcs stay under the arc cap.
        let contribs: Vec<_> = (0..5)
            .map(|i| {
                let mut t = c(a, ReasonKind::Arc, None, &format!("Event {i}"), 3.0);
                t.label = Some(TIE_IN_LABEL.to_owned());
                t
            })
            .collect();
        let out = score(contribs, &meta(&[a]), &HashMap::new(), &HashMap::new());
        assert_eq!(out[0].score, 6.0);
    }

    #[test]
    fn cache_generation_guards_stale_puts() {
        let cache = SimilarityCache::new();
        let id = Uuid::from_u128(9);
        let generation = cache.generation();
        cache.invalidate_all();
        cache.put(id, generation, Arc::new(Vec::new()));
        assert!(cache.get(id).is_none(), "stale compute must not be cached");
        let generation = cache.generation();
        cache.put(id, generation, Arc::new(Vec::new()));
        assert!(cache.get(id).is_some());
        cache.invalidate_all();
        assert!(cache.get(id).is_none());
    }
}
