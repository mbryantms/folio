//! Hand edits reach the archive (roadmap WP-2.10).
//!
//! For a library with `allow_archive_writeback && metadata_writeback_enabled`
//! the archive is the record and the database a cache
//! (`docs/dev/metadata-sidecar-writeback.md`). Provider applies already take
//! the XML-first path; until WP-2.10 a manual `PATCH` on an issue or a series
//! edited the database only, so the file drifted from the database until the
//! next provider apply or an explicit drift flush — the promise "your
//! archives stay canonical" held for provider data but not for the user's
//! own edits.
//!
//! These helpers compose both sidecars from the database alone (no provider
//! payload — the composer's "DB wins" path, the same one the drift flush
//! uses), enqueue [`RewriteIssueSidecarsJob`], and let the job re-enqueue the
//! scoped rescan that ingests the XML back. The user's `field_provenance`
//! pins protect the values through that rescan (WP-2.5).
//!
//! Invariants (CLAUDE.md, "Metadata writeback"): nothing here writes an
//! entity row, and the job carries no deferred DB writes (`post_apply =
//! None`) — a manual edit already landed its provenance in the handler's
//! transaction. Libraries without writeback are untouched
//! (`docs/dev/archive-writes.md`): the helpers return
//! [`IssueEnqueue::NotWriteback`] without composing anything.

use apalis::prelude::Storage;
use entity::{issue, library, series};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::jobs::rewrite_sidecars::{RewriteIssueSidecarsJob, sidecar_refusal};
use crate::state::AppState;

/// Who made the edit — forwarded onto the job so the rewrite's audit row
/// names the editor rather than a provider run.
#[derive(Clone, Debug, Default)]
pub struct Actor {
    pub id: Option<Uuid>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

/// What happened to one issue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueEnqueue {
    /// A rewrite job was queued.
    Enqueued,
    /// The library is not in writeback mode — the edit is DB-only by
    /// design and nothing touches the file.
    NotWriteback,
    /// Writeback mode, but this archive cannot take the sidecar path
    /// (CBR without conversion, unsupported format). The database holds
    /// the edit; drift surfacing covers the gap.
    Refused(String),
    /// The issue, its series, or its library disappeared underneath us.
    Gone,
}

impl IssueEnqueue {
    /// Short label for audit payloads and logs.
    pub fn label(&self) -> String {
        match self {
            IssueEnqueue::Enqueued => "enqueued".into(),
            IssueEnqueue::NotWriteback => "not_writeback".into(),
            IssueEnqueue::Refused(reason) => format!("refused: {reason}"),
            IssueEnqueue::Gone => "gone".into(),
        }
    }
}

/// Outcome of a series-scoped enqueue.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SeriesEnqueue {
    /// Per-issue rewrite jobs queued (each with `skip_rescan = true`).
    pub enqueued: usize,
    /// `(issue_id, reason)` for archives the sidecar path refused.
    pub refused: Vec<(String, String)>,
    /// True when the library is not in writeback mode (nothing queued).
    pub not_writeback: bool,
}

impl SeriesEnqueue {
    pub fn label(&self) -> String {
        if self.not_writeback {
            "not_writeback".into()
        } else {
            format!(
                "enqueued {} (refused {})",
                self.enqueued,
                self.refused.len()
            )
        }
    }
}

fn writeback_on(lib: &library::Model) -> bool {
    lib.allow_archive_writeback && lib.metadata_writeback_enabled
}

/// Compose both sidecars for one issue from the database and queue the
/// rewrite. `skip_rescan` mirrors the series-scope apply: the caller
/// schedules one series-scoped rescan instead of one per issue.
pub async fn enqueue_issue_rewrite(
    state: &AppState,
    issue_id: &str,
    actor: &Actor,
    skip_rescan: bool,
) -> anyhow::Result<IssueEnqueue> {
    let Some(row) = issue::Entity::find_by_id(issue_id).one(&state.db).await? else {
        return Ok(IssueEnqueue::Gone);
    };
    let Some(lib) = library::Entity::find_by_id(row.library_id)
        .one(&state.db)
        .await?
    else {
        return Ok(IssueEnqueue::Gone);
    };
    if !writeback_on(&lib) {
        return Ok(IssueEnqueue::NotWriteback);
    }
    if let Some(reason) = sidecar_refusal(&lib, &row.file_path) {
        return Ok(IssueEnqueue::Refused(reason));
    }
    let Some(series_row) = series::Entity::find_by_id(row.series_id)
        .one(&state.db)
        .await?
    else {
        return Ok(IssueEnqueue::Gone);
    };
    let series_ctx = SeriesContext::load(state, &series_row).await?;
    push_from_db(state, &row, &series_ctx, actor, skip_rescan).await?;
    Ok(IssueEnqueue::Enqueued)
}

/// Queue a rewrite for every active issue of a series (after a series-level
/// edit changed something the per-issue XML carries — name, year, volume,
/// publisher, imprint, age rating, issue count, language), then one
/// series-scoped rescan, exactly like the series-scope provider apply.
pub async fn enqueue_series_rewrite(
    state: &AppState,
    series_id: Uuid,
    actor: &Actor,
) -> anyhow::Result<SeriesEnqueue> {
    let Some(series_row) = series::Entity::find_by_id(series_id).one(&state.db).await? else {
        return Ok(SeriesEnqueue::default());
    };
    let Some(lib) = library::Entity::find_by_id(series_row.library_id)
        .one(&state.db)
        .await?
    else {
        return Ok(SeriesEnqueue::default());
    };
    if !writeback_on(&lib) {
        return Ok(SeriesEnqueue {
            not_writeback: true,
            ..Default::default()
        });
    }
    let issues = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(series_row.id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .all(&state.db)
        .await?;
    let series_ctx = SeriesContext::load(state, &series_row).await?;
    let mut out = SeriesEnqueue::default();
    for row in &issues {
        if let Some(reason) = sidecar_refusal(&lib, &row.file_path) {
            out.refused.push((row.id.clone(), reason));
            continue;
        }
        push_from_db(state, row, &series_ctx, actor, true).await?;
        out.enqueued += 1;
    }
    // One series-scoped rescan after the fan-out (best-effort: the XML
    // writes land regardless and the next scheduled scan ingests them).
    if out.enqueued > 0
        && let Err(e) = state
            .jobs
            .coalesce_scoped_scan(
                series_row.library_id,
                series_row.id,
                None,
                crate::jobs::scan_series::JobKind::Series,
                None,
                true,
            )
            .await
    {
        tracing::error!(
            series_id = %series_row.id,
            error = %e,
            "manual writeback: series-scoped rescan enqueue failed",
        );
    }
    Ok(out)
}

/// Series-level inputs the composer needs, loaded once per call.
struct SeriesContext {
    row: series::Model,
    external_ids: std::collections::BTreeMap<String, String>,
    user_pins: std::collections::HashSet<String>,
}

impl SeriesContext {
    async fn load(state: &AppState, row: &series::Model) -> anyhow::Result<Self> {
        let id = row.id.to_string();
        let external_ids =
            crate::metadata::sidecar_compose::load_external_ids(&state.db, "series", &id).await?;
        let user_pins =
            crate::metadata::sidecar_compose::load_user_pins(&state.db, "series", &id).await?;
        Ok(Self {
            row: row.clone(),
            external_ids,
            user_pins,
        })
    }
}

async fn push_from_db(
    state: &AppState,
    row: &issue::Model,
    series: &SeriesContext,
    actor: &Actor,
    skip_rescan: bool,
) -> anyhow::Result<()> {
    let issue_external_ids =
        crate::metadata::sidecar_compose::load_external_ids(&state.db, "issue", &row.id).await?;
    let issue_user_pins =
        crate::metadata::sidecar_compose::load_user_pins(&state.db, "issue", &row.id).await?;
    // No provider payload: every field resolves to the database value,
    // which is exactly what a manual edit wants to push into the file.
    let empty_provider = crate::metadata::provider::GenericMetadata::default();
    let ctx = crate::metadata::sidecar_compose::ComposeContext {
        provider: &empty_provider,
        issue: row,
        series: &series.row,
        issue_external_ids: &issue_external_ids,
        series_external_ids: &series.external_ids,
        issue_user_pins: &issue_user_pins,
        series_user_pins: &series.user_pins,
    };
    let comic_info = crate::metadata::sidecar_compose::compose_comicinfo(&ctx);
    let metron_info = crate::metadata::sidecar_compose::compose_metroninfo(&ctx);
    let suppressed_user_pins = crate::metadata::sidecar_compose::enumerate_suppressed_pins(&ctx);

    let mut storage = state.jobs.rewrite_issue_sidecars_storage.clone();
    storage
        .push(RewriteIssueSidecarsJob {
            issue_id: row.id.clone(),
            comic_info_xml: parsers::comicinfo::serialize(&comic_info),
            metron_info_xml: parsers::metroninfo::serialize(&metron_info),
            suppressed_user_pins,
            actor_id: actor.id,
            actor_ip: actor.ip.clone(),
            actor_ua: actor.user_agent.clone(),
            // A manual edit has no metadata run behind it.
            triggering_run_id: None,
            triggering_run_ordinal: None,
            skip_rescan,
            attempt: 0,
            // The handler already wrote the user's provenance in its own
            // transaction; there is nothing to defer until the rewrite.
            post_apply: None,
        })
        .await
        .map_err(|e| anyhow::anyhow!("rewrite job enqueue failed: {e}"))?;
    Ok(())
}
