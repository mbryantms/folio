//! User-edit drift detection (M6 of metadata-sidecar-writeback-1.0).
//!
//! "Drift" means: a user pinned a field in the DB (`field_provenance.set_by='user'`),
//! but the issue's archive hasn't been rewritten since that pin landed.
//! In writeback mode the archive XML is the canonical source for downstream
//! readers (OPDS clients, ComicTagger, Komga, Mylar) — when the DB has a
//! newer truth than the XML, those readers see stale data. M6 surfaces
//! this as an admin-only health row so operators know to flush.
//!
//! Non-drift cases (deliberately not surfaced):
//!   - Library has `metadata_writeback_enabled=false` → DB is canonical;
//!     XML never reflects user edits by design. Not "drift".
//!   - Pin is older than `issue.last_rewrite_at` → the most recent sidecar
//!     write happened after the pin, so the XML already carries the user
//!     value. The composer reads pins at write time.
//!   - Issue was never rewritten (`last_rewrite_at IS NULL`) and there are
//!     no pins → no drift signal.

use entity::{field_provenance, issue};
use sea_orm::sea_query::{Alias, Expr, Query};
use sea_orm::{ConnectionTrait, DatabaseBackend, FromQueryResult, Statement};
use uuid::Uuid;

/// Per-library drift summary fed into the synthesized
/// `MetadataDriftFromXml` health row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriftSummary {
    /// Distinct issues with at least one drifted pin.
    pub drifted_issue_count: u64,
    /// Distinct series containing at least one drifted issue.
    pub drifted_series_count: u64,
    /// Series IDs (up to a cap) the flush button targets. Capped so the
    /// payload stays bounded on libraries with thousands of drifted series.
    pub affected_series_ids: Vec<Uuid>,
}

impl DriftSummary {
    pub fn is_empty(&self) -> bool {
        self.drifted_issue_count == 0
    }
}

/// Cap on `affected_series_ids` returned to admins. Larger libraries
/// surface the same count but truncate the id list — the UI can offer
/// "flush all" without round-tripping every UUID.
const AFFECTED_SERIES_CAP: usize = 200;

/// Compute the drift summary for a single library. Pure read — no
/// writes, no side effects. Cost is one indexed query against
/// `field_provenance` joined to `issues` by entity_id, filtered by
/// `library_id` + the `pin.set_at > last_rewrite_at` predicate.
pub async fn count_drift_in_library<C: ConnectionTrait>(
    db: &C,
    library_id: Uuid,
) -> Result<DriftSummary, sea_orm::DbErr> {
    // The pin.set_at vs issue.last_rewrite_at comparison is the heart of
    // M6. We want issues whose pin landed AFTER the most recent rewrite
    // (or never-rewritten issues that have a pin at all). The query
    // partitions both cases by treating NULL last_rewrite_at as "epoch".
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r#"
        WITH drifted AS (
          SELECT
            i.series_id,
            i.id AS issue_id
          FROM field_provenance fp
          JOIN issues i ON i.id = fp.entity_id
          WHERE fp.entity_type = 'issue'
            AND fp.set_by = 'user'
            AND i.library_id = $1
            AND i.removed_at IS NULL
            AND (i.last_rewrite_at IS NULL OR fp.set_at > i.last_rewrite_at)
          GROUP BY i.series_id, i.id
        )
        SELECT
          (SELECT COUNT(*) FROM drifted) AS issue_count,
          (SELECT COUNT(DISTINCT series_id) FROM drifted) AS series_count
        "#,
        [library_id.into()],
    );
    #[derive(FromQueryResult)]
    struct CountRow {
        issue_count: i64,
        series_count: i64,
    }
    let counts = CountRow::find_by_statement(stmt).one(db).await?;
    let Some(c) = counts else {
        return Ok(DriftSummary::default());
    };
    if c.issue_count == 0 {
        return Ok(DriftSummary::default());
    }

    // Capped series-id payload for the UI / flush endpoint. Ordering
    // is deterministic (sort by id) so a flush retry hits the same
    // first-N series.
    let ids_stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r#"
        SELECT DISTINCT i.series_id::text AS series_id
        FROM field_provenance fp
        JOIN issues i ON i.id = fp.entity_id
        WHERE fp.entity_type = 'issue'
          AND fp.set_by = 'user'
          AND i.library_id = $1
          AND i.removed_at IS NULL
          AND (i.last_rewrite_at IS NULL OR fp.set_at > i.last_rewrite_at)
        ORDER BY series_id
        LIMIT $2
        "#,
        [library_id.into(), (AFFECTED_SERIES_CAP as i64).into()],
    );
    #[derive(FromQueryResult)]
    struct IdRow {
        series_id: String,
    }
    let ids = IdRow::find_by_statement(ids_stmt).all(db).await?;
    let affected_series_ids: Vec<Uuid> = ids
        .into_iter()
        .filter_map(|r| Uuid::parse_str(&r.series_id).ok())
        .collect();

    Ok(DriftSummary {
        drifted_issue_count: c.issue_count as u64,
        drifted_series_count: c.series_count as u64,
        affected_series_ids,
    })
}

/// Same predicate as [`count_drift_in_library`] but returns the
/// drifted issue IDs for a single series. Used by the flush endpoint
/// to enumerate the per-issue rewrite jobs to enqueue.
pub async fn drifted_issues_in_series<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
) -> Result<Vec<String>, sea_orm::DbErr> {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r#"
        SELECT DISTINCT i.id::text AS issue_id
        FROM field_provenance fp
        JOIN issues i ON i.id = fp.entity_id
        WHERE fp.entity_type = 'issue'
          AND fp.set_by = 'user'
          AND i.series_id = $1
          AND i.removed_at IS NULL
          AND i.state = 'active'
          AND (i.last_rewrite_at IS NULL OR fp.set_at > i.last_rewrite_at)
        ORDER BY issue_id
        "#,
        [series_id.into()],
    );
    #[derive(FromQueryResult)]
    struct IdRow {
        issue_id: String,
    }
    let rows = IdRow::find_by_statement(stmt).all(db).await?;
    Ok(rows.into_iter().map(|r| r.issue_id).collect())
}

// Silence the "unused import" warning that fires when the file is
// included via `pub mod drift` but only the public fns are referenced
// from outside — the entity imports above scope macros that *would* be
// needed by a SeaORM `Entity::find()`-style alternative implementation.
#[allow(dead_code)]
fn _entity_imports_unused() {
    let _ = (
        std::marker::PhantomData::<field_provenance::Entity>,
        std::marker::PhantomData::<issue::Entity>,
        std::marker::PhantomData::<Query>,
        std::marker::PhantomData::<Alias>,
        std::marker::PhantomData::<Expr>,
    );
}
