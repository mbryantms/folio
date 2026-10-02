//! Series relationships (WP-7.1, spec §5.2 / Phase 7): typed, directed
//! edges between series, always stored as inverse pairs.
//!
//! This module is the **only** write surface for `series_relationship`:
//!
//! - [`create_pair`] inserts the forward edge and its inverse in one
//!   transaction (idempotent — an existing pair is returned, not an error).
//! - [`delete_pair`] removes both halves together.
//! - [`traverse`] / [`chain`] walk the graph with a depth-capped,
//!   cycle-safe recursive CTE (one of the spec's raw-SQL escape hatches,
//!   §17 "(d) recursive series-relationship traversal").
//!
//! The HTTP surface lives in [`crate::api::series_relationships`]; the
//! suggestion engine (WP-7.2) accepts a suggestion by calling
//! [`create_pair`] with [`RelationshipSource::Suggested`].
//!
//! Edge semantics read left to right: `from sequel_of to` means *from* is
//! the sequel, so *to* comes first in reading order. See
//! `docs/dev/series-relationships.md`.

use chrono::Utc;
use entity::series_relationship as rel;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbErr, EntityTrait, FromQueryResult, QueryFilter, Statement,
    Value,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// Hard cap on traversal depth (spec Phase 7: "CTE depth capped at 6").
/// Callers asking for more are clamped.
pub const MAX_TRAVERSAL_DEPTH: u32 = 6;

/// The kind of a directed relationship edge. Wire + DB form is snake_case
/// (`sequel_of`, …); the DB CHECK in the migration mirrors this list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipKind {
    /// `from` continues `to` (`to` is read first).
    SequelOf,
    /// `from` precedes `to` (inverse of `sequel_of`).
    PrequelOf,
    /// `from` was spun off from `to`.
    SpinOffOf,
    /// `from` spawned the spin-off `to` (inverse of `spin_off_of`; not in the
    /// roadmap's original list, added so `spin_off_of` has an inverse).
    HasSpinOff,
    /// Self-inverse.
    CrossoverWith,
    /// `from` (e.g. a TPB / omnibus series) collects `to`.
    Collects,
    /// `from` is collected in `to` (inverse of `collects`).
    CollectedIn,
    /// Self-inverse.
    SameUniverse,
    /// Self-inverse catch-all.
    SeeAlso,
}

impl RelationshipKind {
    pub const ALL: [Self; 9] = [
        Self::SequelOf,
        Self::PrequelOf,
        Self::SpinOffOf,
        Self::HasSpinOff,
        Self::CrossoverWith,
        Self::Collects,
        Self::CollectedIn,
        Self::SameUniverse,
        Self::SeeAlso,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SequelOf => "sequel_of",
            Self::PrequelOf => "prequel_of",
            Self::SpinOffOf => "spin_off_of",
            Self::HasSpinOff => "has_spin_off",
            Self::CrossoverWith => "crossover_with",
            Self::Collects => "collects",
            Self::CollectedIn => "collected_in",
            Self::SameUniverse => "same_universe",
            Self::SeeAlso => "see_also",
        }
    }

    /// The kind of the reverse edge (`to → from`).
    pub fn inverse(self) -> Self {
        match self {
            Self::SequelOf => Self::PrequelOf,
            Self::PrequelOf => Self::SequelOf,
            Self::SpinOffOf => Self::HasSpinOff,
            Self::HasSpinOff => Self::SpinOffOf,
            Self::Collects => Self::CollectedIn,
            Self::CollectedIn => Self::Collects,
            Self::CrossoverWith => Self::CrossoverWith,
            Self::SameUniverse => Self::SameUniverse,
            Self::SeeAlso => Self::SeeAlso,
        }
    }

    pub fn is_self_inverse(self) -> bool {
        self.inverse() == self
    }

    /// Human label from the subject's point of view ("Sequel of").
    pub fn label(self) -> &'static str {
        match self {
            Self::SequelOf => "Sequel of",
            Self::PrequelOf => "Prequel of",
            Self::SpinOffOf => "Spin-off of",
            Self::HasSpinOff => "Has spin-off",
            Self::CrossoverWith => "Crossover with",
            Self::Collects => "Collects",
            Self::CollectedIn => "Collected in",
            Self::SameUniverse => "Same universe as",
            Self::SeeAlso => "See also",
        }
    }
}

impl fmt::Display for RelationshipKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RelationshipKind {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL.into_iter().find(|k| k.as_str() == s).ok_or(())
    }
}

/// Where an edge came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipSource {
    /// Created by an admin by hand.
    Manual,
    /// An accepted suggestion from the WP-7.2 engine.
    Suggested,
}

impl RelationshipSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Suggested => "suggested",
        }
    }
}

impl FromStr for RelationshipSource {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "manual" => Ok(Self::Manual),
            "suggested" => Ok(Self::Suggested),
            _ => Err(()),
        }
    }
}

/// Why [`create_pair`] refused.
#[derive(Debug)]
pub enum PairError {
    /// `from == to` (also enforced by the DB CHECK).
    SelfEdge,
    /// The opposite directional kind already links the same ordered pair
    /// (e.g. asking for `A sequel_of B` while `A prequel_of B` exists).
    Conflict {
        existing: RelationshipKind,
    },
    /// `confidence` outside 0.0–1.0.
    InvalidConfidence,
    Db(DbErr),
}

impl From<DbErr> for PairError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

impl fmt::Display for PairError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SelfEdge => f.write_str("a series cannot be related to itself"),
            Self::Conflict { existing } => {
                write!(f, "conflicts with existing `{existing}` relationship")
            }
            Self::InvalidConfidence => f.write_str("confidence must be between 0 and 1"),
            Self::Db(e) => write!(f, "database error: {e}"),
        }
    }
}

/// Result of [`create_pair`].
#[derive(Debug, Clone)]
pub struct PairOutcome {
    /// The `from → to` row (pre-existing or freshly inserted).
    pub forward: rel::Model,
    /// The `to → from` row.
    pub inverse: rel::Model,
    /// `true` when this call inserted the forward edge; `false` when the
    /// pair already existed (idempotent no-op apart from healing a
    /// missing inverse half).
    pub created: bool,
}

/// Insert `from —kind→ to` and its inverse `to —kind.inverse()→ from`.
///
/// Run it inside a transaction (`conn` is usually a
/// `DatabaseTransaction`) so both halves land together. Idempotent: an
/// existing pair is returned with `created = false`; a missing inverse
/// half (should never happen, but the old row might predate a bug fix) is
/// re-created. Concurrent inserts are safe — both statements are
/// `ON CONFLICT DO NOTHING` on the `(from, to, kind)` unique key.
///
/// Does **not** verify the series exist or that the caller may see them;
/// the FK rejects unknown ids and the HTTP layer does the ACL check.
#[allow(clippy::too_many_arguments)]
pub async fn create_pair<C: ConnectionTrait>(
    conn: &C,
    from: Uuid,
    to: Uuid,
    kind: RelationshipKind,
    source: RelationshipSource,
    confidence: Option<f32>,
    created_by: Option<Uuid>,
) -> Result<PairOutcome, PairError> {
    if from == to {
        return Err(PairError::SelfEdge);
    }
    if let Some(c) = confidence
        && !(0.0..=1.0).contains(&c)
    {
        return Err(PairError::InvalidConfidence);
    }
    // A directional kind contradicts its own inverse on the same ordered
    // pair (`A sequel_of B` + `A prequel_of B` would make each the other's
    // sequel). Self-inverse kinds can't conflict this way.
    if !kind.is_self_inverse() {
        let contradiction = kind.inverse();
        let clash = rel::Entity::find()
            .filter(rel::Column::FromSeriesId.eq(from))
            .filter(rel::Column::ToSeriesId.eq(to))
            .filter(rel::Column::Kind.eq(contradiction.as_str()))
            .one(conn)
            .await?;
        if clash.is_some() {
            return Err(PairError::Conflict {
                existing: contradiction,
            });
        }
    }

    let now = Utc::now().fixed_offset();
    let insert = |a: Uuid, b: Uuid, k: RelationshipKind| {
        Statement::from_sql_and_values(
            conn.get_database_backend(),
            "INSERT INTO series_relationship \
               (id, from_series_id, to_series_id, kind, source, confidence, created_by, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (from_series_id, to_series_id, kind) DO NOTHING",
            [
                Value::from(Uuid::now_v7()),
                Value::from(a),
                Value::from(b),
                Value::from(k.as_str()),
                Value::from(source.as_str()),
                Value::from(confidence),
                Value::from(created_by),
                Value::from(now),
            ],
        )
    };
    let fwd = conn.execute_raw(insert(from, to, kind)).await?;
    conn.execute_raw(insert(to, from, kind.inverse())).await?;

    let forward = find_edge(conn, from, to, kind)
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("series_relationship forward row".into()))?;
    let inverse = find_edge(conn, to, from, kind.inverse())
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("series_relationship inverse row".into()))?;
    Ok(PairOutcome {
        forward,
        inverse,
        created: fwd.rows_affected() > 0,
    })
}

/// Delete `from —kind→ to` and its inverse. Returns the deleted forward
/// row, or `None` when the edge didn't exist (a stray inverse half is
/// still removed). Run inside a transaction.
pub async fn delete_pair<C: ConnectionTrait>(
    conn: &C,
    from: Uuid,
    to: Uuid,
    kind: RelationshipKind,
) -> Result<Option<rel::Model>, DbErr> {
    let forward = find_edge(conn, from, to, kind).await?;
    let pair_filter = sea_orm::Condition::any()
        .add(
            sea_orm::Condition::all()
                .add(rel::Column::FromSeriesId.eq(from))
                .add(rel::Column::ToSeriesId.eq(to))
                .add(rel::Column::Kind.eq(kind.as_str())),
        )
        .add(
            sea_orm::Condition::all()
                .add(rel::Column::FromSeriesId.eq(to))
                .add(rel::Column::ToSeriesId.eq(from))
                .add(rel::Column::Kind.eq(kind.inverse().as_str())),
        );
    rel::Entity::delete_many()
        .filter(pair_filter)
        .exec(conn)
        .await?;
    Ok(forward)
}

/// [`delete_pair`] keyed by either half's row id. `None` when no such row.
pub async fn delete_pair_by_id<C: ConnectionTrait>(
    conn: &C,
    id: Uuid,
) -> Result<Option<rel::Model>, DbErr> {
    let Some(row) = rel::Entity::find_by_id(id).one(conn).await? else {
        return Ok(None);
    };
    let Ok(kind) = row.kind.parse::<RelationshipKind>() else {
        // Unreachable under the DB CHECK; drop the lone row anyway.
        rel::Entity::delete_by_id(id).exec(conn).await?;
        return Ok(Some(row));
    };
    delete_pair(conn, row.from_series_id, row.to_series_id, kind).await
}

async fn find_edge<C: ConnectionTrait>(
    conn: &C,
    from: Uuid,
    to: Uuid,
    kind: RelationshipKind,
) -> Result<Option<rel::Model>, DbErr> {
    rel::Entity::find()
        .filter(rel::Column::FromSeriesId.eq(from))
        .filter(rel::Column::ToSeriesId.eq(to))
        .filter(rel::Column::Kind.eq(kind.as_str()))
        .one(conn)
        .await
}

/// Direct (depth-1) edges out of `series_id`, oldest first. Because every
/// edge is stored with its inverse, this is the complete neighbourhood.
pub async fn direct<C: ConnectionTrait>(
    conn: &C,
    series_id: Uuid,
) -> Result<Vec<rel::Model>, DbErr> {
    use sea_orm::QueryOrder;
    rel::Entity::find()
        .filter(rel::Column::FromSeriesId.eq(series_id))
        .order_by_asc(rel::Column::CreatedAt)
        .order_by_asc(rel::Column::Id)
        .all(conn)
        .await
}

/// One node reached by [`traverse`].
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct TraversalNode {
    pub series_id: Uuid,
    /// Shortest hop count from the start (≥ 1).
    pub depth: i32,
    /// The node this one was reached from on a shortest path (the start
    /// series for depth-1 nodes). Lets callers prune a subtree when an
    /// intermediate node is hidden from the viewer.
    pub parent_id: Uuid,
}

/// Every series reachable from `start` by following edges whose kind is in
/// `kinds`, up to `max_depth` hops (clamped to [`MAX_TRAVERSAL_DEPTH`]).
/// Cycle-safe: a path never revisits a node (the CTE carries the visited
/// path), and each node is reported once at its shortest depth. The start
/// series itself is never returned. Ordered by depth, then id.
pub async fn traverse<C: ConnectionTrait>(
    conn: &C,
    start: Uuid,
    kinds: &[RelationshipKind],
    max_depth: u32,
) -> Result<Vec<TraversalNode>, DbErr> {
    let depth = max_depth.min(MAX_TRAVERSAL_DEPTH);
    if depth == 0 || kinds.is_empty() {
        return Ok(Vec::new());
    }
    let kinds: Vec<String> = kinds.iter().map(|k| k.as_str().to_owned()).collect();
    let sql = r#"
        WITH RECURSIVE walk(series_id, parent_id, depth, path) AS (
            SELECT r.to_series_id, r.from_series_id, 1, ARRAY[r.from_series_id, r.to_series_id]
              FROM series_relationship r
             WHERE r.from_series_id = $1 AND r.kind = ANY($2)
            UNION ALL
            SELECT r.to_series_id, w.series_id, w.depth + 1, w.path || r.to_series_id
              FROM walk w
              JOIN series_relationship r ON r.from_series_id = w.series_id
             WHERE w.depth < $3
               AND r.kind = ANY($2)
               AND NOT (r.to_series_id = ANY(w.path))
        )
        SELECT DISTINCT ON (series_id) series_id, depth, parent_id
          FROM walk
         ORDER BY series_id, depth, parent_id
    "#;
    let stmt = Statement::from_sql_and_values(
        conn.get_database_backend(),
        sql,
        [
            Value::from(start),
            Value::from(kinds),
            Value::from(i32::try_from(depth).unwrap_or(6)),
        ],
    );
    let mut nodes = TraversalNode::find_by_statement(stmt).all(conn).await?;
    nodes.sort_by(|a, b| a.depth.cmp(&b.depth).then(a.series_id.cmp(&b.series_id)));
    Ok(nodes)
}

/// One step of a reading-order chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainNode {
    pub series_id: Uuid,
    /// Signed reading-order offset from the start series: negative =
    /// read before (prequels), `0` = the start series, positive = read
    /// after (sequels). Several nodes can share a position when the chain
    /// branches.
    pub position: i32,
    pub parent_id: Option<Uuid>,
}

/// The sequel/prequel chain through `start`, in reading order: walks
/// `sequel_of` edges backwards (everything read before) and `prequel_of`
/// edges forwards (everything read after), each up to
/// [`MAX_TRAVERSAL_DEPTH`] hops. Returns an empty list when `start` has no
/// sequel/prequel edges; otherwise the start series is included at
/// position 0.
pub async fn chain<C: ConnectionTrait>(conn: &C, start: Uuid) -> Result<Vec<ChainNode>, DbErr> {
    // `start sequel_of X` ⇒ X is read before start.
    let before = traverse(
        conn,
        start,
        &[RelationshipKind::SequelOf],
        MAX_TRAVERSAL_DEPTH,
    )
    .await?;
    // `start prequel_of Y` ⇒ Y is read after start.
    let after = traverse(
        conn,
        start,
        &[RelationshipKind::PrequelOf],
        MAX_TRAVERSAL_DEPTH,
    )
    .await?;
    if before.is_empty() && after.is_empty() {
        return Ok(Vec::new());
    }
    // A cycle (A sequel_of B sequel_of A) can put the same node on both
    // sides; keep it on the "before" side only so each node shows once.
    let before_ids: std::collections::HashSet<Uuid> = before.iter().map(|n| n.series_id).collect();
    let mut out: Vec<ChainNode> = before
        .iter()
        .map(|n| ChainNode {
            series_id: n.series_id,
            position: -n.depth,
            parent_id: Some(n.parent_id),
        })
        .collect();
    // Furthest-back first (stable, so ties keep id order).
    out.sort_by_key(|n| n.position);
    out.push(ChainNode {
        series_id: start,
        position: 0,
        parent_id: None,
    });
    out.extend(
        after
            .iter()
            .filter(|n| !before_ids.contains(&n.series_id))
            .map(|n| ChainNode {
                series_id: n.series_id,
                position: n.depth,
                parent_id: Some(n.parent_id),
            }),
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_is_an_involution() {
        for k in RelationshipKind::ALL {
            assert_eq!(k.inverse().inverse(), k, "{k}");
        }
    }

    #[test]
    fn self_inverse_kinds() {
        let selfies: Vec<_> = RelationshipKind::ALL
            .into_iter()
            .filter(|k| k.is_self_inverse())
            .collect();
        assert_eq!(
            selfies,
            vec![
                RelationshipKind::CrossoverWith,
                RelationshipKind::SameUniverse,
                RelationshipKind::SeeAlso
            ]
        );
    }

    #[test]
    fn str_round_trip_matches_serde() {
        for k in RelationshipKind::ALL {
            assert_eq!(k.as_str().parse::<RelationshipKind>(), Ok(k));
            assert_eq!(
                serde_json::to_value(k).unwrap(),
                serde_json::Value::from(k.as_str())
            );
        }
        assert!("sequel".parse::<RelationshipKind>().is_err());
    }
}
