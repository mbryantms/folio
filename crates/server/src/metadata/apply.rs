//! Apply jobs — write a chosen candidate's GenericMetadata back to
//! the local series / issue.
//!
//! This is the M4 layer of the metadata-providers-1.0 plan. The
//! orchestrator (M3) ranks candidates from cross-provider searches;
//! the Apply step picks one and walks every field, deciding whether
//! to overwrite based on the field's existing provenance, the
//! caller-chosen mode, and the user-precedence rules.
//!
//! ## Decision matrix (per field)
//!
//! | DB state                         | mode = fill_missing | mode = replace_all |
//! |----------------------------------|---------------------|--------------------|
//! | Empty + no provenance row        | apply               | apply              |
//! | Empty + provenance.set_by='user' | skip (sacred)       | skip *             |
//! | Non-empty + non-user provenance  | skip                | apply              |
//! | Non-empty + provenance.set_by='user' | skip            | skip *             |
//!
//! \* `override_user_edits = true` (admin-only) bypasses the sacred
//! rule. The audit row uses the distinct `…_metadata_apply_force`
//! action so heavy-handed applies are greppable.
//!
//! ## What writes through this layer
//!
//! All writes route through M0's `writers::*` helpers — never direct
//! `UPDATE` on entity rows or junction tables. That gives every
//! mutation:
//! - automatic `field_provenance` row write
//! - automatic CSV cache rebuild on junction touches (debounced)
//! - identifier-first dedup on upserts
//! - user-precedence guard on `set_external_id`

use crate::metadata::cache;
use crate::metadata::comicvine::ComicVineClient;
use crate::metadata::field::MetadataField;
use crate::metadata::gcd::GcdClient;
use crate::metadata::identifier::{Identifier, Source};
use crate::metadata::metron::MetronClient;
use crate::metadata::provider::{GenericMetadata, MetadataProvider, ProviderError, ProviderResult};
use crate::metadata::writers::{self, CoverOverwritePolicy, CoverWrite, CsvRebuildBatch, SetBy};
use crate::state::AppState;
use chrono::{Datelike, NaiveDate, Utc};
use entity::{field_provenance, issue, metadata_run, metadata_run_candidate, series};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, Set,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

// ───────── public API types ─────────

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApplyMode {
    /// Default — only fill fields that are currently empty.
    FillMissing,
    /// Overwrite non-user fields with the provider's value. User-set
    /// fields stay sacred unless `override_user_edits=true`.
    ReplaceAll,
}

impl ApplyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ApplyMode::FillMissing => "fill_missing",
            ApplyMode::ReplaceAll => "replace_all",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ApplyArgs {
    pub run_id: Uuid,
    pub ordinal: i32,
    pub mode: ApplyMode,
    pub apply_cover: bool,
    pub cover_overwrite_policy: CoverOverwritePolicy,
    pub override_user_edits: bool,
    pub actor_id: Option<Uuid>,
    /// Optional per-field opt-in from the M5 preview pane. When
    /// `Some`, only fields whose `MetadataField::key()` is present
    /// will be written (subject to the normal user-precedence rule).
    /// When `None`, the old "apply every field that should_apply
    /// permits" behaviour holds — preserves backwards compat for
    /// callers that haven't been updated to send the opt-in set.
    pub selected_fields: Option<std::collections::HashSet<String>>,
    /// Per-source overrides from the M5 external-IDs conflict
    /// surface. When a source is in this set, the user has
    /// explicitly opted in to "Use theirs" on that conflict — the
    /// candidate's value replaces the user-set row.
    pub override_external_id_sources: std::collections::HashSet<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ApplyOutcome {
    pub applied_fields: Vec<String>,
    pub skipped_fields: Vec<String>,
    pub external_ids_added: Vec<ExternalIdAdded>,
    pub external_ids_skipped: Vec<ExternalIdSkipped>,
    pub junctions_touched: Vec<String>,
    pub cover_replaced: bool,
    pub cover_skipped_reason: Option<String>,
    /// True when this apply queued a `RewriteIssueSidecarsJob` instead
    /// of writing DB rows directly (`library.metadata_writeback_enabled
    /// = true` path). The DB cache will be refreshed by the scoped
    /// rescan the job enqueues; the UI dialog waits for the
    /// `scan.finished` event before closing.
    #[serde(default)]
    pub enqueued_rewrite: bool,
    /// Field keys whose composer output preferred the user-pinned DB
    /// value over the provider's offer. Surfaced in the audit
    /// payload + (M5) the dialog's "{n} of your edits will be
    /// preserved" summary chip.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suppressed_user_pins: Vec<String>,
    /// Number of issue sidecars successfully rewritten — series-scope
    /// apply only (M4). Zero on the issue-scope path.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub composed_sidecars: u32,
    /// Per-issue skip reasons collected by the series-scope sidecar
    /// apply path. One entry per issue that was eligible but couldn't
    /// be rewritten (e.g., `"{issue_id}: no comicvine id"`). Empty on
    /// the issue-scope path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sidecar_skip_reasons: Vec<String>,
    /// Count of variant cover rows persisted to `issue_cover` from
    /// the provider's `variants: Vec<VariantCoverCandidate>`. v1
    /// stores metadata only (source_url + label); the gallery
    /// renders from the CDN URL. Zero when the provider returns no
    /// variants.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub variants_written: u32,
}

fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExternalIdAdded {
    pub source: String,
    pub external_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExternalIdSkipped {
    pub source: String,
    pub external_id: String,
    pub reason: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("candidate not found for run_id={run_id} ordinal={ordinal}")]
    CandidateNotFound { run_id: Uuid, ordinal: i32 },
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("database error: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("series for run is gone")]
    SeriesGone,
    #[error("issue for run is gone")]
    IssueGone,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid run: {0}")]
    InvalidScope(String),
}

// ───────── decision matrix ─────────

/// Whether `field` carries a `set_by='user'` provenance row.
///
/// `summary` / `description` alias the same column on both series and
/// issues: the PATCH endpoints pin under `summary` (their request key)
/// while provider applies key the column as `description`. A pin under
/// either key must protect the field — otherwise a direct-to-DB apply
/// silently overwrites a hand-written summary (audit DI-1). Mirrors
/// `sidecar_pin_matches` and the scanner's alias.
pub(crate) fn user_pinned(provenance: &HashMap<String, String>, field: MetadataField) -> bool {
    let is_user = |k: &str| provenance.get(k).map(|s| s.as_str()) == Some("user");
    if is_user(&field.key()) {
        return true;
    }
    match field {
        MetadataField::Description => is_user("summary"),
        MetadataField::Summary => is_user("description"),
        _ => false,
    }
}

/// Single source of truth for the apply decision. See module-level
/// matrix doc. Visible to siblings so the M5 diff module can mirror
/// the live Apply logic without re-implementing the matrix.
pub(crate) fn should_apply(
    db_has_value: bool,
    provenance: &HashMap<String, String>,
    field: MetadataField,
    args: &ApplyArgs,
) -> bool {
    // M5 per-field opt-in gate: when the preview pane has explicitly
    // chosen a subset of fields, anything not in that set is skipped
    // *before* the matrix runs. None = legacy "apply everything"
    // semantics (preserved for callers that don't yet send the set).
    if let Some(selected) = &args.selected_fields
        && !selected.contains(&field.key())
    {
        return false;
    }
    let user_set = user_pinned(provenance, field);
    if user_set && !args.override_user_edits {
        return false;
    }
    if !db_has_value {
        return true;
    }
    matches!(args.mode, ApplyMode::ReplaceAll)
}

/// Reason a candidate field would (or would not) be applied — drives
/// the diff UI's per-row badge / disabled-checkbox state. M5 diff
/// view.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum DiffDecision {
    /// Field is empty in DB; would write on Apply.
    WouldFill,
    /// Field is set by a non-user source; mode is ReplaceAll so would
    /// overwrite.
    WouldReplace,
    /// Field is set to the same proposed value; nothing to do.
    NoChange,
    /// Field is user-set; user-precedence rule blocks unless override
    /// is on.
    BlockedByUser,
    /// Field has a value and mode is FillMissing; nothing to do.
    SkippedFillMissingHasValue,
    /// Candidate has no value for this field.
    NoIncomingValue,
}

impl DiffDecision {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            DiffDecision::WouldFill => "would_fill",
            DiffDecision::WouldReplace => "would_replace",
            DiffDecision::NoChange => "no_change",
            DiffDecision::BlockedByUser => "blocked_by_user",
            DiffDecision::SkippedFillMissingHasValue => "skipped_fill_missing_has_value",
            DiffDecision::NoIncomingValue => "no_incoming_value",
        }
    }

    /// True for decisions that represent an actionable change the
    /// user can opt in to. Currently used only by the diff-module
    /// unit tests; the runtime API serializes [`as_str`] and the
    /// web client computes default-checked state from that string,
    /// so the helper is `cfg(test)`-only in production builds.
    #[cfg(test)]
    pub(crate) fn would_change(self) -> bool {
        matches!(self, DiffDecision::WouldFill | DiffDecision::WouldReplace)
    }
}

/// Classify a per-field decision into a [`DiffDecision`] without
/// writing. Mirrors [`should_apply`] but tracks the *reason* a write
/// would or would not happen so the preview UI can render
/// per-row status. M5 diff view.
pub(crate) fn classify_field(
    current_value: Option<&str>,
    incoming_value: Option<&str>,
    provenance: &HashMap<String, String>,
    field: MetadataField,
    args: &ApplyArgs,
) -> DiffDecision {
    let has_incoming = incoming_value.is_some_and(|s| !s.trim().is_empty());
    if !has_incoming {
        return DiffDecision::NoIncomingValue;
    }
    let has_current = current_value.is_some_and(|s| !s.trim().is_empty());
    let user_set = user_pinned(provenance, field);
    if user_set && !args.override_user_edits {
        return DiffDecision::BlockedByUser;
    }
    if !has_current {
        return DiffDecision::WouldFill;
    }
    // Same-value short-circuit: nothing to do even in ReplaceAll mode.
    if current_value.map(str::trim) == incoming_value.map(str::trim) {
        return DiffDecision::NoChange;
    }
    if matches!(args.mode, ApplyMode::ReplaceAll) {
        DiffDecision::WouldReplace
    } else {
        DiffDecision::SkippedFillMissingHasValue
    }
}

// ───────── series apply ─────────

/// The provenance a single field's write should be stamped with: the
/// `set_by` source and that source's external id (for the
/// `field_provenance.source_external_id` column).
#[derive(Clone)]
pub struct ProvSource {
    pub set_by: SetBy,
    pub source_ext: Option<String>,
}

/// Resolves the provenance to stamp per field. The single-candidate
/// apply path uses [`ProvResolver::Uniform`] (every field stamped with
/// the one candidate's source → behaviour identical to the pre-refactor
/// code). The composite (multi-provider) apply path uses
/// [`ProvResolver::PerField`] so each merged field records the true
/// provider that supplied its value.
pub enum ProvResolver<'a> {
    Uniform(ProvSource),
    PerField {
        map: &'a std::collections::HashMap<String, ProvSource>,
        fallback: ProvSource,
    },
}

impl ProvResolver<'_> {
    pub fn resolve(&self, field_key: &str) -> ProvSource {
        match self {
            ProvResolver::Uniform(p) => p.clone(),
            ProvResolver::PerField { map, fallback } => map
                .get(field_key)
                .cloned()
                .unwrap_or_else(|| fallback.clone()),
        }
    }

    fn set_by(&self, field_key: &str) -> SetBy {
        self.resolve(field_key).set_by
    }

    fn source_ext(&self, field_key: &str) -> Option<String> {
        self.resolve(field_key).source_ext
    }

    /// Provenance for writes that aren't keyed by a single field — the
    /// external-id batch (additive across sources). Uniform → the one
    /// source; PerField → the fallback (top-preference) source.
    fn primary(&self) -> ProvSource {
        match self {
            ProvResolver::Uniform(p) => p.clone(),
            ProvResolver::PerField { fallback, .. } => fallback.clone(),
        }
    }
}

pub async fn apply_series(state: &AppState, args: ApplyArgs) -> Result<ApplyOutcome, ApplyError> {
    let run_id = args.run_id;
    let outcome = apply_series_impl(state, args).await;
    // WP-7.4: an apply changes the entities similar-series scores over.
    if let Ok(o) = &outcome {
        state.similarity.invalidate_all();
        queue_relationship_suggest(state, run_id, o).await;
    }
    outcome
}

/// WP-7.8: after a successful apply, queue a relationship-suggestion run
/// for the library so what the apply recorded — provider links
/// (`series_external_relationship`, every series apply) and reprints
/// (`issue_reprints`, an issue apply that wrote them DB-direct) — turns
/// into suggestions without waiting for the next scan. The writeback path's
/// reprints land after the rewrite; its scoped rescan queues the run.
/// Best-effort (deduped per library by the job's own key).
pub(crate) async fn queue_relationship_suggest(
    state: &AppState,
    run_id: Uuid,
    outcome: &ApplyOutcome,
) {
    let Ok(run) = load_run(&state.db, run_id).await else {
        return;
    };
    let Some(entity_id) = run.scope_entity_id.as_deref() else {
        return;
    };
    let library_id = if run.scope == crate::metadata::orchestrator::scope::SERIES {
        let Ok(id) = Uuid::parse_str(entity_id) else {
            return;
        };
        series::Entity::find_by_id(id)
            .one(&state.db)
            .await
            .ok()
            .flatten()
            .map(|s| s.library_id)
    } else if outcome.junctions_touched.iter().any(|j| j == "reprints") {
        issue::Entity::find_by_id(entity_id)
            .one(&state.db)
            .await
            .ok()
            .flatten()
            .map(|i| i.library_id)
    } else {
        None
    };
    if let Some(library_id) = library_id {
        crate::jobs::relationship_suggest::enqueue(state, library_id).await;
    }
}

async fn apply_series_impl(state: &AppState, args: ApplyArgs) -> Result<ApplyOutcome, ApplyError> {
    let candidate = load_candidate(&state.db, args.run_id, args.ordinal).await?;
    let run = load_run(&state.db, args.run_id).await?;
    if run.scope != crate::metadata::orchestrator::scope::SERIES {
        return Err(ApplyError::InvalidScope(run.scope.clone()));
    }
    let Some(series_id_str) = run.scope_entity_id.as_deref() else {
        return Err(ApplyError::SeriesGone);
    };
    let series_uuid = Uuid::parse_str(series_id_str)
        .map_err(|e| ApplyError::InvalidScope(format!("scope_entity_id not uuid: {e}")))?;
    let Some(row) = series::Entity::find_by_id(series_uuid)
        .one(&state.db)
        .await?
    else {
        return Err(ApplyError::SeriesGone);
    };

    let source = parse_source(&candidate.source)
        .ok_or_else(|| ApplyError::InvalidScope(format!("unknown source: {}", candidate.source)))?;
    let provider = build_provider(state, source)
        .ok_or_else(|| ApplyError::InvalidScope(format!("provider {source} not configured")))?;
    let detail = fetch_series_detail(state, &*provider, &candidate.external_id).await?;

    // M4 dispatch: when both library writeback toggles are on, walk
    // every issue in the series, write XMLs into each archive, and
    // trigger a single series-scope rescan at the end. Legacy DB-direct
    // path stays for unmigrated libraries (zero behaviour change).
    let lib = entity::library::Entity::find_by_id(row.library_id)
        .one(&state.db)
        .await?;
    let resolver = ProvResolver::Uniform(ProvSource {
        set_by: SetBy::Provider(source),
        source_ext: detail.source_external_id.clone(),
    });
    if let Some(lib) = lib
        && lib.metadata_writeback_enabled
        && lib.allow_archive_writeback
    {
        return apply_series_via_sidecar(state, &args, &row, source, detail, &resolver).await;
    }

    let run_id = args.run_id;
    let ordinal = args.ordinal;
    let outcome = write_series_fields(state, &row, series_uuid, &detail, args, &resolver).await?;
    flip_candidate_applied(&state.db, run_id, ordinal).await?;
    bump_run_counts(&state.db, run_id, &outcome).await?;
    // DB-direct path: signal completion so an open match dialog re-hydrates
    // without a refresh (writeback path uses the rescan's `scan.completed`).
    state
        .events
        .emit(crate::library::events::ScanEvent::MetadataApplied {
            library_id: row.library_id,
            series_id: row.id,
            issue_id: None,
        });
    Ok(outcome)
}

/// Write every series scalar + external-ids + per-field provenance from
/// `detail`, resolving each field's provenance through `resolver`.
/// Shared by single-candidate apply (`ProvResolver::Uniform` → identical
/// to the pre-refactor behaviour) and composite apply
/// (`ProvResolver::PerField`). Does NOT flip run candidates or bump run
/// counts — the caller owns run lifecycle.
pub(crate) async fn write_series_fields(
    state: &AppState,
    row: &series::Model,
    series_uuid: Uuid,
    detail: &GenericMetadata,
    args: ApplyArgs,
    resolver: &ProvResolver<'_>,
) -> Result<ApplyOutcome, ApplyError> {
    let mut outcome = ApplyOutcome::default();
    let entity_id_str = series_uuid.to_string();

    // Only the identifiers that point at *this* series: the provider
    // mappers also list related ids (CV's publisher) and `external_ids`
    // holds one row per (entity, source), so an unfiltered write would
    // replace the series' own id with the publisher's.
    let own_identifiers: Vec<Identifier> = detail
        .identifiers
        .iter()
        .filter(|i| i.is_for_entity("series"))
        .cloned()
        .collect();
    apply_external_ids(
        &state.db,
        "series",
        &entity_id_str,
        &own_identifiers,
        resolver.primary().set_by,
        &args,
        &mut outcome,
    )
    .await?;

    write_series_scalar_fields(
        state,
        row,
        series_uuid,
        detail,
        &args,
        resolver,
        &mut outcome,
    )
    .await?;
    Ok(outcome)
}

/// The scalar half of the series apply: every flat `series` column the
/// provider can fill, decided per-field against provenance +
/// fill/replace mode, written as one UPDATE, provenance stamped.
///
/// Shared by BOTH apply paths. The DB-direct path calls it via
/// [`write_series_fields`] (which also writes external ids). The
/// sidecar-writeback path calls it directly, because series-row scalars
/// do not survive the archive round-trip at all:
///   - `summary` / `deck` / `aliases` / `sort_name` / `series_type` /
///     `year_end` have no series-level slot in either XML schema;
///   - `publisher` / `imprint` are composed per issue but the scanner
///     never refreshes them on the series row;
///   - `name` / `year` are deliberately frozen by the scanner
///     (folder-pinned identity).
///
/// Without this, a series-scope apply on a writeback library rewrote
/// every archive while the series page kept its old description — the
/// Chew regression. External ids are intentionally NOT written here:
/// the sidecar path persists them post-apply in the job handler
/// (`persist_applied_series_external_ids`), per the provider-divergence
/// invariant.
pub(crate) async fn write_series_scalar_fields(
    state: &AppState,
    row: &series::Model,
    series_uuid: Uuid,
    detail: &GenericMetadata,
    args: &ApplyArgs,
    resolver: &ProvResolver<'_>,
    outcome: &mut ApplyOutcome,
) -> Result<(), ApplyError> {
    let entity_id_str = series_uuid.to_string();
    let provenance = fetch_field_provenance_map(&state.db, "series", &entity_id_str).await?;

    // Track which fields we're going to write so we can emit a single
    // UPDATE rather than 10+ round trips.
    let mut new = SeriesUpdates::default();

    if let Some(v) = detail
        .series_name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        let has = !row.name.trim().is_empty();
        decide_scalar(
            has,
            &provenance,
            MetadataField::Title,
            args,
            outcome,
            || {
                new.name = Some(v.to_owned());
            },
        );
    }
    decide_str(
        &row.sort_name,
        &detail.series_sort_name,
        MetadataField::SortName,
        &provenance,
        args,
        outcome,
        |v| new.sort_name = Some(v),
    );
    decide_str(
        &row.series_type,
        &detail.series_type,
        MetadataField::SeriesType,
        &provenance,
        args,
        outcome,
        |v| new.series_type = Some(v),
    );
    decide_i32(
        row.year,
        detail.year_began,
        MetadataField::YearBegan,
        &provenance,
        args,
        outcome,
        |v| new.year = Some(v),
    );
    decide_i32(
        row.year_end,
        detail.year_end,
        MetadataField::YearEnd,
        &provenance,
        args,
        outcome,
        |v| new.year_end = Some(v),
    );
    decide_i32(
        row.volume,
        detail.volume,
        MetadataField::Volume,
        &provenance,
        args,
        outcome,
        |v| new.volume = Some(v),
    );
    decide_str(
        &row.publisher,
        &detail.publisher,
        MetadataField::Publisher,
        &provenance,
        args,
        outcome,
        |v| new.publisher = Some(v),
    );
    decide_str(
        &row.imprint,
        &detail.imprint,
        MetadataField::Imprint,
        &provenance,
        args,
        outcome,
        |v| new.imprint = Some(v),
    );
    decide_str(
        &row.deck,
        &detail.deck,
        MetadataField::Deck,
        &provenance,
        args,
        outcome,
        |v| new.deck = Some(v),
    );
    decide_str(
        &row.summary,
        &detail.description,
        MetadataField::Description,
        &provenance,
        args,
        outcome,
        |v| new.summary = Some(v),
    );

    if !detail.aliases.is_empty() {
        let has = !row.aliases.as_array().map(|a| a.is_empty()).unwrap_or(true);
        decide_scalar(
            has,
            &provenance,
            MetadataField::Aliases,
            args,
            outcome,
            || {
                new.aliases =
                    Some(serde_json::to_value(&detail.aliases).unwrap_or(serde_json::json!([])));
            },
        );
    }

    // Apply pending updates + provenance writes in one pass.
    apply_series_updates(&state.db, series_uuid, &new).await?;
    write_provenance_for_applied(
        &state.db,
        "series",
        &entity_id_str,
        &outcome.applied_fields,
        resolver,
    )
    .await?;

    record_series_provider_links(state, row, detail).await?;

    // Bump sync timestamp.
    bump_series_sync(&state.db, series_uuid).await?;
    Ok(())
}

/// WP-7.8: store the provider's linked series (Metron `associated`) as
/// external relationship rows (`relationships::external`). Metadata-only:
/// neither sidecar schema carries series links, so both apply paths record
/// them here, next to the series scalars. Not gated by `selected_fields`
/// (no preview row; like external ids, the links are additive evidence)
/// and never touches a user-set or dismissed row. A Metron detail with an
/// empty `associated` list clears that series' old Metron rows.
async fn record_series_provider_links(
    state: &AppState,
    row: &series::Model,
    detail: &GenericMetadata,
) -> Result<(), ApplyError> {
    let mut sources: Vec<Source> = Vec::new();
    for l in &detail.related_series {
        if !sources.contains(&l.source) {
            sources.push(l.source);
        }
    }
    if detail.source_provider == Some(Source::Metron) && !sources.contains(&Source::Metron) {
        sources.push(Source::Metron);
    }
    for source in sources {
        let own_id = detail
            .identifiers
            .iter()
            .find(|i| i.source == source && i.is_for_entity("series"))
            .map(|i| i.id.as_str())
            .or(if detail.source_provider == Some(source) {
                detail.source_external_id.as_deref()
            } else {
                None
            });
        let report = crate::relationships::external::record_provider_links(
            &state.db,
            row,
            source,
            own_id,
            detail.series_type.as_deref().or(row.series_type.as_deref()),
            &detail.related_series,
        )
        .await?;
        if report != Default::default() {
            tracing::debug!(
                series_id = %row.id,
                source = source.as_str(),
                upserted = report.upserted,
                removed = report.removed,
                promoted = report.promoted,
                "apply: provider series links recorded"
            );
        }
    }
    Ok(())
}

/// M4 of `metadata-sidecar-writeback-1.0`: series-scope XML-first apply.
///
/// Walks every active issue in the series, composes ComicInfo +
/// MetronInfo per issue from the series-level provider detail merged
/// with the issue's DB state, and writes both XMLs into each archive.
/// One series-scope rescan fires at the end so the scanner ingests all
/// freshly-written XMLs in a single pass.
///
/// The composer gets only the **series-shaped** part of the series-level
/// provider record ([`series_payload_for_issue`]) — no N per-issue
/// provider calls, and no series value lands in an issue-level slot:
/// each issue keeps its own description, title, dates, credits, … The
/// series description goes to `series.summary` via
/// [`write_series_scalar_fields`]. The series apply is structurally a
/// "refresh the series shape across all issues" operation; per-issue
/// refresh is the issue-scope apply path.
///
/// [`series_payload_for_issue`]: crate::metadata::sidecar_compose::series_payload_for_issue
///
/// Failure mode: per-issue errors (no provider id, write failure)
/// accumulate in `ApplyOutcome.sidecar_skip_reasons`. The rescan
/// still fires for the issues that succeeded.
pub(crate) async fn apply_series_via_sidecar(
    state: &AppState,
    args: &ApplyArgs,
    series_row: &series::Model,
    source: Source,
    series_detail: GenericMetadata,
    resolver: &ProvResolver<'_>,
) -> Result<ApplyOutcome, ApplyError> {
    // Eligible issues: state='active' (the scanner's happy-path
    // value — covers ComicInfo-present + MissingComicInfo files).
    // Explicitly skip malformed / encrypted / removed since we can't
    // safely rewrite those archives.
    let issues = entity::issue::Entity::find()
        .filter(entity::issue::Column::SeriesId.eq(series_row.id))
        .filter(entity::issue::Column::State.eq("active"))
        .all(&state.db)
        .await?;

    // Provider series-boundary divergence: issues covered by a
    // `series_provider_range` row for *this apply's source* belong to a
    // DIFFERENT provider series (e.g. legacy-renumbered #600–611 in a
    // "FF (2012)" Metron series). For those issues the composed XML
    // carries the splitter's series identity (name / volume / series id)
    // instead of the main match's, while the rest of the run keeps the
    // main identity. The local `series` row is never mutated — series
    // membership is folder-pinned and the scanner freezes series.name /
    // year on rescan (only series.volume follows the issue-volume MODE,
    // which the small divergent minority can't flip). See
    // `metadata::range_map` + `docs/dev` divergence plan.
    let ranges_for_source: Vec<entity::series_provider_range::Model> =
        entity::series_provider_range::Entity::find()
            .filter(entity::series_provider_range::Column::SeriesId.eq(series_row.id))
            .filter(entity::series_provider_range::Column::Source.eq(source.as_str()))
            .all(&state.db)
            .await
            .unwrap_or_default();

    let series_external_ids_db = crate::metadata::sidecar_compose::load_external_ids(
        &state.db,
        "series",
        &series_row.id.to_string(),
    )
    .await?;
    let series_user_pins = if args.override_user_edits {
        std::collections::HashSet::new()
    } else {
        crate::metadata::sidecar_compose::load_user_pins(
            &state.db,
            "series",
            &series_row.id.to_string(),
        )
        .await?
    };
    // Overlay the provider's freshly-fetched series identifiers (same
    // rationale as the issue path) so series-scope IDs reach the
    // composed XML on every fanned-out issue.
    let series_external_ids = crate::metadata::sidecar_compose::merge_provider_identifiers(
        &series_external_ids_db,
        &series_detail.identifiers,
        &series_user_pins,
    );

    let mut composed_sidecars: u32 = 0;
    let mut skip_reasons: Vec<String> = Vec::new();
    let mut suppressed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    let mut redis = state.jobs.redis.clone();

    for issue_row in &issues {
        let issue_external_ids =
            crate::metadata::sidecar_compose::load_external_ids(&state.db, "issue", &issue_row.id)
                .await?;
        let issue_user_pins = if args.override_user_edits {
            std::collections::HashSet::new()
        } else {
            crate::metadata::sidecar_compose::load_user_pins(&state.db, "issue", &issue_row.id)
                .await?
        };

        // If a range mapping covers this issue for the apply's source,
        // overlay the splitter's series identity (name / volume / series
        // id) onto the provider + series-id map the composer sees. Issues
        // outside every range borrow the main match's data untouched.
        let canonical = issue_row
            .number_raw
            .as_deref()
            .map(crate::metadata::matcher::canonical_issue_number);
        let covering = canonical.as_deref().and_then(|n| {
            ranges_for_source.iter().find(|r| {
                crate::metadata::range_map::issue_in_range(
                    n,
                    r.range_low.as_deref(),
                    r.range_high.as_deref(),
                )
            })
        });
        // Only the series-shaped part of the series record reaches an
        // issue's XML: a series description / notes / URL composed as
        // issue values overwrote every issue's own `<Summary>` (and, via
        // the rescan, `issues.summary`). See `series_payload_for_issue`.
        let mut provider_for_issue =
            crate::metadata::sidecar_compose::series_payload_for_issue(&series_detail, issue_row);
        let series_ids_for_issue = match covering {
            Some(r) => {
                // Override only the series *name* with the splitter's — the
                // identity readers see. Deliberately NOT `volume`: the range
                // row carries the sub-series' start *year* (`declared_year`,
                // for the search year-gate), but the composer writes
                // `provider.volume` into ComicInfo `<Volume>`, which is the
                // numeric volume — writing a year there would corrupt it and
                // disagree with the rest of the run. The year is surfaced for
                // display via the coverage / alternate-series API instead.
                if let Some(name) = r.provider_series_name.as_deref().filter(|s| !s.is_empty()) {
                    provider_for_issue.series_name = Some(name.to_owned());
                }
                let mut ids = series_external_ids.clone();
                ids.insert(source.as_str().to_owned(), r.provider_series_id.clone());
                std::borrow::Cow::Owned(ids)
            }
            None => std::borrow::Cow::Borrowed(&series_external_ids),
        };

        let ctx = crate::metadata::sidecar_compose::ComposeContext {
            provider: &provider_for_issue,
            issue: issue_row,
            series: series_row,
            issue_external_ids: &issue_external_ids,
            series_external_ids: &series_ids_for_issue,
            issue_user_pins: &issue_user_pins,
            series_user_pins: &series_user_pins,
        };

        let pins = crate::metadata::sidecar_compose::enumerate_suppressed_pins(&ctx);
        suppressed.extend(pins);

        let ci = crate::metadata::sidecar_compose::compose_comicinfo(&ctx);
        let mi = crate::metadata::sidecar_compose::compose_metroninfo(&ctx);
        let ci_xml = parsers::comicinfo::serialize(&ci);
        let mi_xml = parsers::metroninfo::serialize(&mi);

        // Per-issue archive-rewrite mutex. Held just long enough to
        // write this one archive — releases between iterations so
        // concurrent issue-scope edits aren't blocked for the whole
        // series fan-out.
        let token = match crate::archive_rewrite::mutex::try_claim(
            &mut redis,
            &issue_row.id,
            crate::archive_rewrite::mutex::SIDECAR_TTL_SECS,
        )
        .await
        {
            Ok(Some(t)) => t,
            // Busy or Redis error — skip this issue, same as before.
            Ok(None) | Err(_) => {
                skip_reasons.push(format!("{}: archive busy (mutex)", issue_row.id));
                continue;
            }
        };

        // Keep the lock alive across the blocking rewrite (WP-2.6 (h)).
        let heartbeat = crate::archive_rewrite::mutex::Heartbeat::start(
            state.jobs.redis.clone(),
            issue_row.id.clone(),
            token.clone(),
            crate::archive_rewrite::mutex::SIDECAR_TTL_SECS,
        );
        let result =
            crate::jobs::rewrite_sidecars::rewrite_one_issue(state, &issue_row.id, ci_xml, mi_xml)
                .await;
        drop(heartbeat);
        crate::archive_rewrite::mutex::release(&mut redis, &issue_row.id, &token).await;

        match result {
            Ok(_) => composed_sidecars += 1,
            Err(e) => {
                skip_reasons.push(format!("{}: {e}", issue_row.id));
            }
        }
    }

    // One series-scoped rescan after the loop so the scanner ingests
    // every freshly-written XML in a single pass. Best-effort — if the
    // enqueue itself fails, the writes already landed and the next
    // scheduled scan will pick them up.
    if composed_sidecars > 0
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
            "series sidecar writeback: scoped rescan enqueue failed",
        );
    }

    // Series-row scalars don't survive the archive round-trip (no XML
    // slot, scanner freeze, or no series-row refresh — see
    // `write_series_scalar_fields`), so the sidecar path persists the
    // same scalar set as the DB-direct path, with identical provenance
    // + fill/replace semantics — resolved through the caller's
    // `resolver` so a composite apply attributes each field to the
    // provider that actually contributed it. External ids stay with
    // the job handler's `persist_applied_series_external_ids`.
    let mut scalar_outcome = ApplyOutcome::default();
    write_series_scalar_fields(
        state,
        series_row,
        series_row.id,
        &series_detail,
        args,
        resolver,
        &mut scalar_outcome,
    )
    .await?;

    flip_candidate_applied(&state.db, args.run_id, args.ordinal).await?;

    // Stamp series-level sync time (parity with the DB-direct `apply_series`,
    // which calls `bump_series_sync`). The XML the rescan re-ingests doesn't
    // carry this bookkeeping field.
    bump_series_sync(&state.db, series_row.id).await?;

    let outcome = ApplyOutcome {
        enqueued_rewrite: composed_sidecars > 0,
        composed_sidecars,
        sidecar_skip_reasons: skip_reasons,
        suppressed_user_pins: suppressed.into_iter().collect(),
        applied_fields: scalar_outcome.applied_fields,
        skipped_fields: scalar_outcome.skipped_fields,
        ..Default::default()
    };
    bump_run_counts(&state.db, args.run_id, &outcome).await?;
    Ok(outcome)
}

// ───────── issue apply ─────────

/// M3 of `metadata-sidecar-writeback-1.0`: XML-first apply path.
///
/// Instead of writing entity rows directly, we compose fresh
/// `ComicInfo.xml` + `MetronInfo.xml` from the provider's data merged
/// with the user's pinned DB values, serialize both, and enqueue a
/// `RewriteIssueSidecarsJob` to swap them into the archive. The
/// scanner's scoped rescan (triggered by that job at the end) is what
/// refreshes the DB cache — so this function returns *before* the UI's
/// underlying data has actually changed. The dialog's WebSocket bridge
/// (M5) waits for the `scan.finished` event before closing.
pub(crate) async fn apply_issue_via_sidecar(
    state: &AppState,
    args: &ApplyArgs,
    row: &entity::issue::Model,
    source: Source,
    detail: GenericMetadata,
    resolver: &ProvResolver<'_>,
) -> Result<ApplyOutcome, ApplyError> {
    // The composer writes every provider value it is handed (unless the
    // field is user-pinned), so the preview pane's per-field opt-in has to
    // be applied to the payload itself — otherwise an unticked row still
    // lands in the XML and, via the scoped rescan, in the DB.
    let detail = mask_unselected_issue_fields(detail, args.selected_fields.as_ref());
    let Some(series_row) = entity::series::Entity::find_by_id(row.series_id)
        .one(&state.db)
        .await?
    else {
        return Err(ApplyError::SeriesGone);
    };

    // Gather context — composer wants external_ids + user_pins on both
    // the issue and its parent series. M3.1 loaders all live in
    // `sidecar_compose`.
    let issue_external_ids_db =
        crate::metadata::sidecar_compose::load_external_ids(&state.db, "issue", &row.id).await?;
    let series_external_ids = crate::metadata::sidecar_compose::load_external_ids(
        &state.db,
        "series",
        &series_row.id.to_string(),
    )
    .await?;
    // `override_user_edits=true` collapses the pin set to empty so
    // the composer behaves as provider-wins (matching legacy
    // `_force` semantics).
    let issue_user_pins = if args.override_user_edits {
        std::collections::HashSet::new()
    } else {
        crate::metadata::sidecar_compose::load_user_pins(&state.db, "issue", &row.id).await?
    };
    // Overlay the provider's freshly-fetched issue identifiers so they
    // reach the composed XML and round-trip back via the scanner ingest.
    // The DB-direct apply path does this through `apply_external_ids`;
    // the sidecar path had no equivalent, silently dropping new IDs.
    let issue_external_ids = crate::metadata::sidecar_compose::merge_provider_identifiers(
        &issue_external_ids_db,
        &detail.identifiers,
        &issue_user_pins,
    );
    let series_user_pins = if args.override_user_edits {
        std::collections::HashSet::new()
    } else {
        crate::metadata::sidecar_compose::load_user_pins(
            &state.db,
            "series",
            &series_row.id.to_string(),
        )
        .await?
    };

    let ctx = crate::metadata::sidecar_compose::ComposeContext {
        provider: &detail,
        issue: row,
        series: &series_row,
        issue_external_ids: &issue_external_ids,
        series_external_ids: &series_external_ids,
        issue_user_pins: &issue_user_pins,
        series_user_pins: &series_user_pins,
    };

    let comic_info = crate::metadata::sidecar_compose::compose_comicinfo(&ctx);
    let metron_info = crate::metadata::sidecar_compose::compose_metroninfo(&ctx);
    let suppressed_user_pins = crate::metadata::sidecar_compose::enumerate_suppressed_pins(&ctx);

    let comic_info_xml = parsers::comicinfo::serialize(&comic_info);
    let metron_info_xml = parsers::metroninfo::serialize(&metron_info);

    // Metadata-only rows the XML can't carry — decided here, **written by
    // the rewrite job only after the XML is in the archive** (WP-2.6 (f),
    // audit DI-10). Pre-fix they were written at enqueue time, so a
    // rewrite that failed at open left provenance, variant rows and
    // `last_metadata_sync_at` describing provider values that never
    // reached the file. The CLAUDE.md invariant still holds: nothing
    // below touches an entity row; it only builds the deferred payload.
    //
    // Variant covers — same gate as the legacy path. Neither ComicInfo
    // nor MetronInfo carries variants, so they land in `issue_cover`
    // straight from the job; the scoped rescan never touches those rows.
    let variants_selected = args
        .selected_fields
        .as_ref()
        .map(|s| s.contains(&MetadataField::CoverVariants.key()))
        .unwrap_or(true);
    let variants: Vec<_> = if variants_selected {
        detail.variants.clone()
    } else {
        Vec::new()
    };
    // Rows the writer will insert: it skips candidates with no image URL.
    let variants_written = variants.iter().filter(|v| v.image_url.is_some()).count() as u32;

    // Field provenance: the XML schemas can't carry "ComicVine set this
    // on date X", and the scoped rescan's file-level attribution is
    // guarded from *overwriting* provider rows — so the apply itself is
    // the only place the true source is known. Skips mirror the
    // composer's decisions: provider-empty fields kept their DB value,
    // `selected_fields` is the per-field opt-in gate, and a user pin
    // means the composer suppressed the provider's value (unless
    // `override_user_edits` collapsed the pins — then the unconditional
    // upsert in the job is also what retires the stale `user` row,
    // letting the follow-up rescan ingest the overridden value).
    let mut provenance = Vec::new();
    for &field in SIDECAR_ISSUE_PROVENANCE_FIELDS {
        if crate::metadata::merge::field_richness(
            &detail,
            field,
            crate::metadata::merge::MergeScope::Issue,
        ) == 0
        {
            continue;
        }
        if let Some(sel) = &args.selected_fields
            && !sel.contains(&field.key())
        {
            continue;
        }
        if !args.override_user_edits && sidecar_pin_matches(&issue_user_pins, field) {
            continue;
        }
        let prov = resolver.resolve(&field.key());
        // The apply path always attributes to a provider; anything else
        // would be a resolver bug, and there's no sensible row to write.
        let SetBy::Provider(prov_source) = prov.set_by else {
            continue;
        };
        provenance.push(crate::jobs::rewrite_sidecars::ProvenanceWrite {
            field: field.key(),
            source: prov_source,
            source_external_id: prov.source_ext,
        });
    }
    // WP-7.8: provider reprints — metadata-only rows neither XML schema
    // carries, so they ride the deferred payload like variant covers and
    // land only after the rewrite succeeded. Same decision as the
    // DB-direct path (user pin + fill/replace; not `selected_fields`).
    let reprints = if detail.reprints.is_empty() {
        Vec::new()
    } else {
        let prov_map = fetch_field_provenance_map(&state.db, "issue", &row.id).await?;
        let has = issue_has_reprints(&state.db, &row.id).await?;
        if reprints_should_apply(has, &prov_map, args) {
            detail.reprints.clone()
        } else {
            Vec::new()
        }
    };
    let reprints_prov = resolver.resolve(&MetadataField::Reprints.key());
    let post_apply = crate::jobs::rewrite_sidecars::PostRewriteWrites {
        provenance,
        variants,
        variants_source: Some(source),
        reprints_source: match reprints_prov.set_by {
            SetBy::Provider(s) if !reprints.is_empty() => Some(s),
            _ => None,
        },
        reprints_source_external_id: reprints_prov.source_ext,
        reprints,
        // `last_metadata_sync_at` is bookkeeping the XML doesn't carry, so
        // the scoped rescan can't set it — the job stamps it on success
        // (the DB-direct `apply_issue` does this via `bump_issue_sync`).
        bump_sync: true,
    };

    use apalis::prelude::Storage;
    let mut storage = state.jobs.rewrite_issue_sidecars_storage.clone();
    storage
        .push(crate::jobs::rewrite_sidecars::RewriteIssueSidecarsJob {
            issue_id: row.id.clone(),
            comic_info_xml,
            metron_info_xml,
            suppressed_user_pins: suppressed_user_pins.clone(),
            actor_id: args.actor_id,
            // Apply-job-level actor IP/UA aren't on `ApplyArgs` (they
            // travel via the apalis job payload in
            // `metadata_apply::ApplyIssueJob`). The rewrite-job audit
            // row still captures actor_id; IP/UA stay on the parent
            // `admin.issue.metadata_apply` row.
            actor_ip: None,
            actor_ua: None,
            triggering_run_id: Some(args.run_id),
            triggering_run_ordinal: Some(args.ordinal),
            // Issue-scope apply path: let the apalis worker enqueue the
            // per-issue scoped rescan when it completes.
            skip_rescan: false,
            attempt: 0,
            post_apply: Some(post_apply),
        })
        .await
        .map_err(|e| ApplyError::InvalidScope(format!("rewrite_sidecars push failed: {e}")))?;

    flip_candidate_applied(&state.db, args.run_id, args.ordinal).await?;

    let outcome = ApplyOutcome {
        enqueued_rewrite: true,
        suppressed_user_pins,
        variants_written,
        ..Default::default()
    };
    bump_run_counts(&state.db, args.run_id, &outcome).await?;
    Ok(outcome)
}

pub async fn apply_issue(state: &AppState, args: ApplyArgs) -> Result<ApplyOutcome, ApplyError> {
    let run_id = args.run_id;
    let outcome = apply_issue_impl(state, args).await;
    // WP-7.4: an apply changes the entities similar-series scores over.
    if let Ok(o) = &outcome {
        state.similarity.invalidate_all();
        queue_relationship_suggest(state, run_id, o).await;
    }
    outcome
}

async fn apply_issue_impl(state: &AppState, args: ApplyArgs) -> Result<ApplyOutcome, ApplyError> {
    let candidate = load_candidate(&state.db, args.run_id, args.ordinal).await?;
    let run = load_run(&state.db, args.run_id).await?;
    if run.scope != crate::metadata::orchestrator::scope::ISSUE {
        return Err(ApplyError::InvalidScope(run.scope.clone()));
    }
    let Some(issue_id_str) = run.scope_entity_id.as_deref() else {
        return Err(ApplyError::IssueGone);
    };
    let Some(row) = issue::Entity::find_by_id(issue_id_str)
        .one(&state.db)
        .await?
    else {
        return Err(ApplyError::IssueGone);
    };

    let source = parse_source(&candidate.source)
        .ok_or_else(|| ApplyError::InvalidScope(format!("unknown source: {}", candidate.source)))?;
    let provider = build_provider(state, source)
        .ok_or_else(|| ApplyError::InvalidScope(format!("provider {source} not configured")))?;
    let detail = fetch_issue_detail(state, &*provider, &candidate.external_id).await?;

    // M3 dispatch: when the library has flipped `metadata_writeback_enabled
    // = true`, route through the XML-first path. The legacy DB-direct
    // flow below stays for unmigrated libraries — zero behaviour change
    // for them.
    let lib = entity::library::Entity::find_by_id(row.library_id)
        .one(&state.db)
        .await?;
    let resolver = ProvResolver::Uniform(ProvSource {
        set_by: SetBy::Provider(source),
        source_ext: detail.source_external_id.clone(),
    });
    // WP-2.6 (f): an archive the sidecar path can't rewrite (CBR / CB7
    // without conversion enabled, unknown extension) falls back to the
    // DB-direct apply with the reason surfaced on the outcome, instead of
    // enqueueing a job that fails at open after the run was marked
    // applied (audit DI-10).
    let mut sidecar_refused: Option<String> = None;
    if let Some(lib) = lib
        && lib.metadata_writeback_enabled
        && lib.allow_archive_writeback
    {
        match crate::jobs::rewrite_sidecars::sidecar_refusal(&lib, &row.file_path) {
            None => {
                return apply_issue_via_sidecar(state, &args, &row, source, detail, &resolver)
                    .await;
            }
            Some(reason) => {
                tracing::warn!(
                    issue_id = %row.id,
                    path = %row.file_path,
                    reason,
                    "apply_issue: sidecar writeback refused; applying DB-direct",
                );
                sidecar_refused = Some(format!("{}: {reason}; applied DB-direct", row.id));
            }
        }
    }

    let run_id = args.run_id;
    let ordinal = args.ordinal;
    let mut outcome =
        write_issue_fields(state, &row, &detail, args, &resolver, &*provider, source).await?;
    outcome.sidecar_skip_reasons.extend(sidecar_refused);
    flip_candidate_applied(&state.db, run_id, ordinal).await?;
    bump_run_counts(&state.db, run_id, &outcome).await?;
    // DB-direct path: rows (covers, fields, notes) are current now. Signal
    // completion so an open match dialog can re-hydrate without a page
    // refresh. The writeback path returns above and signals via the
    // rescan's `scan.completed` instead.
    state
        .events
        .emit(crate::library::events::ScanEvent::MetadataApplied {
            library_id: row.library_id,
            series_id: row.series_id,
            issue_id: Some(row.id.clone()),
        });
    Ok(outcome)
}

/// Write every issue scalar + junction + cover + external-ids + per-field
/// provenance from `detail`, resolving each field's provenance via
/// `resolver`. Shared by single-candidate apply (`ProvResolver::Uniform`
/// → identical to the pre-refactor behaviour) and composite apply
/// (`ProvResolver::PerField`). `cover_provider` / `cover_source` identify
/// the provider whose cover URL sits in `detail.cover_image_url`. Does
/// NOT flip run candidates or bump run counts.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn write_issue_fields(
    state: &AppState,
    row: &issue::Model,
    detail: &GenericMetadata,
    args: ApplyArgs,
    resolver: &ProvResolver<'_>,
    cover_provider: &dyn crate::metadata::provider::MetadataProvider,
    cover_source: Source,
) -> Result<ApplyOutcome, ApplyError> {
    let mut outcome = ApplyOutcome::default();
    let entity_id_str = row.id.clone();
    let provenance = fetch_field_provenance_map(&state.db, "issue", &entity_id_str).await?;

    // See `write_series_fields`: drop the parent-volume / series ids the
    // mappers tuck into the issue detail so they can't overwrite the
    // issue's own row for that source.
    let own_identifiers: Vec<Identifier> = detail
        .identifiers
        .iter()
        .filter(|i| i.is_for_entity("issue"))
        .cloned()
        .collect();
    apply_external_ids(
        &state.db,
        "issue",
        &entity_id_str,
        &own_identifiers,
        resolver.primary().set_by,
        &args,
        &mut outcome,
    )
    .await?;

    let mut new = IssueUpdates::default();

    decide_str(
        &row.title,
        &detail.title,
        MetadataField::Title,
        &provenance,
        &args,
        &mut outcome,
        |v| new.title = Some(v),
    );
    decide_str(
        &row.number_raw,
        &detail.issue_number,
        MetadataField::Number,
        &provenance,
        &args,
        &mut outcome,
        |v| new.number_raw = Some(v),
    );

    // Cover date — split into y/m/d on issue.
    let current_cover_date = row
        .year
        .zip(row.month)
        .zip(row.day)
        .and_then(|((y, m), d)| NaiveDate::from_ymd_opt(y, m as u32, d as u32));
    if let Some(incoming_date) = detail.cover_date {
        let has = current_cover_date.is_some();
        decide_scalar(
            has,
            &provenance,
            MetadataField::CoverDate,
            &args,
            &mut outcome,
            || {
                new.cover_date = Some(incoming_date);
            },
        );
    }

    let current_store = row.store_date.map(date_from_db);
    if let Some(incoming_date) = detail.store_date {
        let has = current_store.is_some();
        decide_scalar(
            has,
            &provenance,
            MetadataField::StoreDate,
            &args,
            &mut outcome,
            || {
                new.store_date = Some(incoming_date);
            },
        );
    }
    let current_foc = row.foc_date.map(date_from_db);
    if let Some(incoming_date) = detail.foc_date {
        let has = current_foc.is_some();
        decide_scalar(
            has,
            &provenance,
            MetadataField::FocDate,
            &args,
            &mut outcome,
            || {
                new.foc_date = Some(incoming_date);
            },
        );
    }

    decide_str(
        &row.deck,
        &detail.deck,
        MetadataField::Deck,
        &provenance,
        &args,
        &mut outcome,
        |v| new.deck = Some(v),
    );
    decide_str(
        &row.summary,
        &detail.description,
        MetadataField::Description,
        &provenance,
        &args,
        &mut outcome,
        |v| new.summary = Some(v),
    );
    decide_str(
        &row.age_rating,
        &detail.age_rating,
        MetadataField::AgeRating,
        &provenance,
        &args,
        &mut outcome,
        |v| new.age_rating = Some(v),
    );
    decide_i32(
        row.page_count,
        detail.page_count,
        MetadataField::PageCount,
        &provenance,
        &args,
        &mut outcome,
        |v| new.page_count = Some(v),
    );
    decide_str(
        &row.sku,
        &detail.sku,
        MetadataField::Sku,
        &provenance,
        &args,
        &mut outcome,
        |v| new.sku = Some(v),
    );
    decide_f64(
        row.price,
        detail.price,
        MetadataField::Price,
        &provenance,
        &args,
        &mut outcome,
        |v| new.price = Some(v),
    );

    apply_issue_updates(&state.db, &entity_id_str, &new).await?;
    write_provenance_for_applied(
        &state.db,
        "issue",
        &entity_id_str,
        &outcome.applied_fields,
        resolver,
    )
    .await?;

    // Junctions.
    let rebuild_batch = CsvRebuildBatch::new();
    if !detail.credits.is_empty()
        && should_apply(
            issue_row_has_credits(row),
            &provenance,
            MetadataField::Credits,
            &args,
        )
    {
        let set_by = resolver.set_by(&MetadataField::Credits.key());
        let source_ext = resolver.source_ext(&MetadataField::Credits.key());
        let mut credits = Vec::with_capacity(detail.credits.len());
        for (i, c) in detail.credits.iter().enumerate() {
            let person_id =
                writers::upsert_person(&state.db, &c.name, &c.identifiers, set_by).await?;
            credits.push((person_id, c.role.clone(), i as i32));
        }
        writers::set_issue_credits(
            &state.db,
            &entity_id_str,
            credits,
            set_by,
            source_ext.clone(),
            &rebuild_batch,
        )
        .await?;
        outcome.applied_fields.push(MetadataField::Credits.key());
        outcome.junctions_touched.push("credits".into());
    } else if !detail.credits.is_empty() {
        outcome.skipped_fields.push(MetadataField::Credits.key());
    }

    if !detail.characters.is_empty()
        && should_apply(
            row.characters
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty()),
            &provenance,
            MetadataField::Characters,
            &args,
        )
    {
        let set_by = resolver.set_by(&MetadataField::Characters.key());
        let source_ext = resolver.source_ext(&MetadataField::Characters.key());
        let mut specs: Vec<writers::CharacterSpec> = Vec::with_capacity(detail.characters.len());
        for c in &detail.characters {
            let id = writers::upsert_character(&state.db, &c.name, &c.identifiers, set_by).await?;
            specs.push((id, c.is_first_appearance, c.died_in_issue.unwrap_or(false)));
        }
        writers::set_issue_characters(
            &state.db,
            &entity_id_str,
            specs,
            set_by,
            source_ext.clone(),
            &rebuild_batch,
        )
        .await?;
        outcome.applied_fields.push(MetadataField::Characters.key());
        outcome.junctions_touched.push("characters".into());
    } else if !detail.characters.is_empty() {
        outcome.skipped_fields.push(MetadataField::Characters.key());
    }

    if !detail.teams.is_empty()
        && should_apply(
            row.teams.as_deref().is_some_and(|s| !s.trim().is_empty()),
            &provenance,
            MetadataField::Teams,
            &args,
        )
    {
        let set_by = resolver.set_by(&MetadataField::Teams.key());
        let source_ext = resolver.source_ext(&MetadataField::Teams.key());
        let mut specs: Vec<writers::TeamSpec> = Vec::with_capacity(detail.teams.len());
        for t in &detail.teams {
            let id = writers::upsert_team(&state.db, &t.name, &t.identifiers, set_by).await?;
            specs.push((
                id,
                t.is_first_appearance,
                t.disbanded_in_issue.unwrap_or(false),
            ));
        }
        writers::set_issue_teams(
            &state.db,
            &entity_id_str,
            specs,
            set_by,
            source_ext.clone(),
            &rebuild_batch,
        )
        .await?;
        outcome.applied_fields.push(MetadataField::Teams.key());
        outcome.junctions_touched.push("teams".into());
    } else if !detail.teams.is_empty() {
        outcome.skipped_fields.push(MetadataField::Teams.key());
    }

    if !detail.locations.is_empty()
        && should_apply(
            row.locations
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty()),
            &provenance,
            MetadataField::Locations,
            &args,
        )
    {
        let set_by = resolver.set_by(&MetadataField::Locations.key());
        let source_ext = resolver.source_ext(&MetadataField::Locations.key());
        let mut specs: Vec<writers::LocationSpec> = Vec::with_capacity(detail.locations.len());
        for l in &detail.locations {
            let id = writers::upsert_location(&state.db, &l.name, &l.identifiers, set_by).await?;
            specs.push((id, l.is_first_appearance));
        }
        writers::set_issue_locations(
            &state.db,
            &entity_id_str,
            specs,
            set_by,
            source_ext.clone(),
            &rebuild_batch,
        )
        .await?;
        outcome.applied_fields.push(MetadataField::Locations.key());
        outcome.junctions_touched.push("locations".into());
    } else if !detail.locations.is_empty() {
        outcome.skipped_fields.push(MetadataField::Locations.key());
    }

    if !detail.story_arcs.is_empty()
        && should_apply(
            row.story_arc
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty()),
            &provenance,
            MetadataField::StoryArcs,
            &args,
        )
    {
        let set_by = resolver.set_by(&MetadataField::StoryArcs.key());
        let source_ext = resolver.source_ext(&MetadataField::StoryArcs.key());
        let mut specs: Vec<writers::ArcSpec> = Vec::with_capacity(detail.story_arcs.len());
        for a in &detail.story_arcs {
            let id = writers::upsert_story_arc(&state.db, &a.name, &a.identifiers, set_by).await?;
            specs.push((id, a.position_in_arc));
        }
        writers::set_issue_story_arcs(
            &state.db,
            &entity_id_str,
            specs,
            set_by,
            source_ext.clone(),
            &rebuild_batch,
        )
        .await?;
        outcome.applied_fields.push(MetadataField::StoryArcs.key());
        outcome.junctions_touched.push("story_arcs".into());
    } else if !detail.story_arcs.is_empty() {
        outcome.skipped_fields.push(MetadataField::StoryArcs.key());
    }

    // WP-7.8: provider reprints (Metron `reprints`) → `issue_reprints`.
    if !detail.reprints.is_empty() {
        let has = issue_has_reprints(&state.db, &entity_id_str).await?;
        if reprints_should_apply(has, &provenance, &args) {
            let set_by = resolver.set_by(&MetadataField::Reprints.key());
            let source_ext = resolver.source_ext(&MetadataField::Reprints.key());
            let specs = writers::reprint_specs(&state.db, &entity_id_str, &detail.reprints).await?;
            writers::set_issue_reprints(&state.db, &entity_id_str, specs, set_by, source_ext)
                .await?;
            outcome.applied_fields.push(MetadataField::Reprints.key());
            outcome.junctions_touched.push("reprints".into());
        } else {
            outcome.skipped_fields.push(MetadataField::Reprints.key());
        }
    }

    // Flush the CSV cache rebuild for any touched issue.
    let _ = rebuild_batch.flush(&state.db).await;

    bump_issue_sync(&state.db, &entity_id_str).await?;

    // Cover.
    if args.apply_cover
        && let Some(url) = detail.cover_image_url.as_deref()
    {
        match cover_provider.fetch_cover(url).await {
            Ok(bytes) => {
                let primary_ident = detail
                    .identifiers
                    .iter()
                    .find(|i| i.source == cover_source)
                    .cloned();
                let cover_write = CoverWrite {
                    issue_id: &entity_id_str,
                    kind: "primary",
                    ordinal: 0,
                    identifier: primary_ident.as_ref(),
                    source_url: Some(url),
                    variant_label: None,
                    variant_artist_person_id: None,
                    bytes: &bytes,
                    width: None,
                    height: None,
                };
                match writers::apply_cover(
                    &state.db,
                    &state.cfg().data_path,
                    cover_write,
                    args.cover_overwrite_policy,
                )
                .await
                {
                    Ok(Some(_id)) => {
                        outcome.cover_replaced = true;
                        outcome
                            .applied_fields
                            .push(MetadataField::CoverPrimary.key());
                    }
                    Ok(None) => {
                        outcome.cover_skipped_reason = Some("policy_denied".into());
                    }
                    Err(e) => {
                        outcome.cover_skipped_reason = Some(format!("write_failed: {e}"));
                    }
                }
            }
            // The provider's CDN refuses server-side downloads (GCD's
            // bot challenge): a clean, non-retryable skip, not a failure.
            Err(ProviderError::CoverUnavailable(why)) => {
                outcome.cover_skipped_reason = Some(format!("cover_unavailable: {why}"));
            }
            Err(e) => {
                outcome.cover_skipped_reason = Some(format!("fetch_failed: {e}"));
            }
        }
    }

    // Variant covers (metadata-only — store source_url, no byte
    // download). The `<CoverGallery>` UI renders directly from
    // `issue_cover.source_url` so the variant tiles show up without a
    // local artifact. The diff preview surfaces a "Variant covers"
    // row keyed on `MetadataField::CoverVariants.key()`; when the
    // user unchecks it, `args.selected_fields` no longer contains
    // that key and we skip the write.
    let variants_selected = args
        .selected_fields
        .as_ref()
        .map(|s| s.contains(&MetadataField::CoverVariants.key()))
        .unwrap_or(true);
    if variants_selected && !detail.variants.is_empty() {
        match writers::set_issue_variants(
            &state.db,
            &state.cfg().data_path,
            &entity_id_str,
            &detail.variants,
            resolver.set_by(&MetadataField::CoverVariants.key()),
        )
        .await
        {
            Ok(n) => outcome.variants_written = n as u32,
            Err(e) => tracing::warn!(
                issue_id = entity_id_str,
                error = %e,
                "apply_issue: variant covers write failed; primary cover (if any) is unaffected",
            ),
        }
    }

    Ok(outcome)
}

/// WP-7.8: the apply decision for provider reprints. Same matrix as
/// [`should_apply`] — a user pin on `reprints` is sacred unless
/// `override_user_edits`, and `fill_missing` keeps an existing set — but
/// **not** gated by `selected_fields`: reprints have no preview-pane row
/// (like external ids, they're additive provider metadata), so a preview
/// selection would otherwise always drop them.
pub(crate) fn reprints_should_apply(
    db_has_value: bool,
    provenance: &HashMap<String, String>,
    args: &ApplyArgs,
) -> bool {
    if user_pinned(provenance, MetadataField::Reprints) && !args.override_user_edits {
        return false;
    }
    !db_has_value || matches!(args.mode, ApplyMode::ReplaceAll)
}

async fn issue_has_reprints(
    db: &DatabaseConnection,
    issue_id: &str,
) -> Result<bool, sea_orm::DbErr> {
    use sea_orm::PaginatorTrait;
    Ok(entity::issue_reprint::Entity::find()
        .filter(entity::issue_reprint::Column::IssueId.eq(issue_id))
        .count(db)
        .await?
        > 0)
}

// ───────── decision wrappers ─────────

fn decide_scalar<F: FnOnce()>(
    db_has: bool,
    provenance: &HashMap<String, String>,
    field: MetadataField,
    args: &ApplyArgs,
    outcome: &mut ApplyOutcome,
    apply: F,
) {
    if should_apply(db_has, provenance, field, args) {
        apply();
        outcome.applied_fields.push(field.key());
    } else {
        outcome.skipped_fields.push(field.key());
    }
}

fn decide_str<F: FnOnce(String)>(
    current: &Option<String>,
    incoming: &Option<String>,
    field: MetadataField,
    provenance: &HashMap<String, String>,
    args: &ApplyArgs,
    outcome: &mut ApplyOutcome,
    apply: F,
) {
    let Some(value) = incoming.as_deref().filter(|s| !s.trim().is_empty()) else {
        return;
    };
    let has = current.as_deref().is_some_and(|s| !s.trim().is_empty());
    decide_scalar(has, provenance, field, args, outcome, || {
        apply(value.to_owned())
    });
}

fn decide_i32<F: FnOnce(i32)>(
    current: Option<i32>,
    incoming: Option<i32>,
    field: MetadataField,
    provenance: &HashMap<String, String>,
    args: &ApplyArgs,
    outcome: &mut ApplyOutcome,
    apply: F,
) {
    let Some(value) = incoming else { return };
    decide_scalar(current.is_some(), provenance, field, args, outcome, || {
        apply(value)
    });
}

fn decide_f64<F: FnOnce(f64)>(
    current: Option<f64>,
    incoming: Option<f64>,
    field: MetadataField,
    provenance: &HashMap<String, String>,
    args: &ApplyArgs,
    outcome: &mut ApplyOutcome,
    apply: F,
) {
    let Some(value) = incoming else { return };
    decide_scalar(current.is_some(), provenance, field, args, outcome, || {
        apply(value)
    });
}

// ───────── update collectors ─────────

#[derive(Default)]
struct SeriesUpdates {
    name: Option<String>,
    sort_name: Option<String>,
    series_type: Option<String>,
    year: Option<i32>,
    year_end: Option<i32>,
    volume: Option<i32>,
    publisher: Option<String>,
    imprint: Option<String>,
    deck: Option<String>,
    summary: Option<String>,
    aliases: Option<serde_json::Value>,
}

async fn apply_series_updates(
    db: &DatabaseConnection,
    series_id: Uuid,
    new: &SeriesUpdates,
) -> Result<(), sea_orm::DbErr> {
    if new.is_noop() {
        return Ok(());
    }
    let Some(row) = series::Entity::find_by_id(series_id).one(db).await? else {
        return Ok(());
    };
    let mut am: series::ActiveModel = row.into();
    if let Some(v) = new.name.clone() {
        am.name = Set(v);
    }
    if let Some(v) = new.sort_name.clone() {
        am.sort_name = Set(Some(v));
    }
    if let Some(v) = new.series_type.clone() {
        am.series_type = Set(Some(v));
    }
    if let Some(v) = new.year {
        am.year = Set(Some(v));
    }
    if let Some(v) = new.year_end {
        am.year_end = Set(Some(v));
    }
    if let Some(v) = new.volume {
        am.volume = Set(Some(v));
    }
    if let Some(v) = new.publisher.clone() {
        am.publisher = Set(Some(v));
    }
    if let Some(v) = new.imprint.clone() {
        am.imprint = Set(Some(v));
    }
    if let Some(v) = new.deck.clone() {
        am.deck = Set(Some(v));
    }
    if let Some(v) = new.summary.clone() {
        am.summary = Set(Some(v));
    }
    if let Some(v) = new.aliases.clone() {
        am.aliases = Set(v);
    }
    am.updated_at = Set(Utc::now().fixed_offset());
    am.update(db).await?;
    Ok(())
}

impl SeriesUpdates {
    fn is_noop(&self) -> bool {
        self.name.is_none()
            && self.sort_name.is_none()
            && self.series_type.is_none()
            && self.year.is_none()
            && self.year_end.is_none()
            && self.volume.is_none()
            && self.publisher.is_none()
            && self.imprint.is_none()
            && self.deck.is_none()
            && self.summary.is_none()
            && self.aliases.is_none()
    }
}

#[derive(Default)]
struct IssueUpdates {
    title: Option<String>,
    number_raw: Option<String>,
    cover_date: Option<NaiveDate>,
    store_date: Option<NaiveDate>,
    foc_date: Option<NaiveDate>,
    deck: Option<String>,
    summary: Option<String>,
    age_rating: Option<String>,
    page_count: Option<i32>,
    sku: Option<String>,
    price: Option<f64>,
}

impl IssueUpdates {
    fn is_noop(&self) -> bool {
        self.title.is_none()
            && self.number_raw.is_none()
            && self.cover_date.is_none()
            && self.store_date.is_none()
            && self.foc_date.is_none()
            && self.deck.is_none()
            && self.summary.is_none()
            && self.age_rating.is_none()
            && self.page_count.is_none()
            && self.sku.is_none()
            && self.price.is_none()
    }
}

async fn apply_issue_updates(
    db: &DatabaseConnection,
    issue_id: &str,
    new: &IssueUpdates,
) -> Result<(), sea_orm::DbErr> {
    if new.is_noop() {
        return Ok(());
    }
    let Some(row) = issue::Entity::find_by_id(issue_id).one(db).await? else {
        return Ok(());
    };
    let mut am: issue::ActiveModel = row.into();
    if let Some(v) = new.title.clone() {
        am.title = Set(Some(v));
    }
    if let Some(v) = new.number_raw.clone() {
        am.number_raw = Set(Some(v));
    }
    if let Some(d) = new.cover_date {
        am.year = Set(Some(d.year()));
        am.month = Set(Some(d.month() as i32));
        am.day = Set(Some(d.day() as i32));
    }
    if let Some(d) = new.store_date {
        am.store_date = Set(Some(date_to_db(d)));
    }
    if let Some(d) = new.foc_date {
        am.foc_date = Set(Some(date_to_db(d)));
    }
    if let Some(v) = new.deck.clone() {
        am.deck = Set(Some(v));
    }
    if let Some(v) = new.summary.clone() {
        am.summary = Set(Some(v));
    }
    if let Some(v) = new.age_rating.clone() {
        am.age_rating = Set(Some(v));
    }
    if let Some(v) = new.page_count {
        am.page_count = Set(Some(v));
    }
    if let Some(v) = new.sku.clone() {
        am.sku = Set(Some(v));
    }
    if let Some(v) = new.price {
        am.price = Set(Some(v));
    }
    am.updated_at = Set(Utc::now().fixed_offset());
    am.update(db).await?;
    Ok(())
}

async fn bump_series_sync(db: &DatabaseConnection, series_id: Uuid) -> Result<(), sea_orm::DbErr> {
    let Some(row) = series::Entity::find_by_id(series_id).one(db).await? else {
        return Ok(());
    };
    let mut am: series::ActiveModel = row.into();
    am.last_metadata_sync_at = Set(Some(Utc::now().fixed_offset()));
    am.updated_at = Set(Utc::now().fixed_offset());
    am.update(db).await?;
    Ok(())
}

async fn bump_issue_sync(db: &DatabaseConnection, issue_id: &str) -> Result<(), sea_orm::DbErr> {
    let Some(row) = issue::Entity::find_by_id(issue_id).one(db).await? else {
        return Ok(());
    };
    let mut am: issue::ActiveModel = row.into();
    am.last_metadata_sync_at = Set(Some(Utc::now().fixed_offset()));
    am.updated_at = Set(Utc::now().fixed_offset());
    am.update(db).await?;
    Ok(())
}

// ───────── shared helpers ─────────

pub(crate) async fn load_candidate(
    db: &DatabaseConnection,
    run_id: Uuid,
    ordinal: i32,
) -> Result<metadata_run_candidate::Model, ApplyError> {
    metadata_run_candidate::Entity::find_by_id((run_id, ordinal))
        .one(db)
        .await?
        .ok_or(ApplyError::CandidateNotFound { run_id, ordinal })
}

pub(crate) async fn load_run(
    db: &DatabaseConnection,
    run_id: Uuid,
) -> Result<metadata_run::Model, ApplyError> {
    metadata_run::Entity::find_by_id(run_id)
        .one(db)
        .await?
        .ok_or_else(|| ApplyError::InvalidScope(format!("run {run_id} not found")))
}

pub(crate) fn parse_source(s: &str) -> Option<Source> {
    use std::str::FromStr;
    Source::from_str(s).ok()
}

pub(crate) fn build_provider(
    state: &AppState,
    source: Source,
) -> Option<Arc<dyn MetadataProvider>> {
    let cfg = state.cfg();
    match source {
        Source::ComicVine => {
            let key = cfg
                .comicvine_api_key
                .clone()
                .filter(|s| !s.trim().is_empty())?;
            if !cfg.comicvine_enabled {
                return None;
            }
            Some(Arc::new(match cfg.comicvine_base_url.clone() {
                Some(base) => ComicVineClient::with_base_url(key, base, state.jobs.redis.clone()),
                None => ComicVineClient::new(key, state.jobs.redis.clone()),
            }))
        }
        Source::Metron => {
            let client = MetronClient::from_config(&cfg, state.jobs.redis.clone())?;
            if !cfg.metron_enabled {
                return None;
            }
            Some(Arc::new(client))
        }
        Source::Gcd => {
            let client = GcdClient::from_config(&cfg, state.jobs.redis.clone())?;
            if !cfg.gcd_enabled {
                return None;
            }
            Some(Arc::new(client))
        }
        _ => None,
    }
}

pub(crate) async fn fetch_series_detail(
    state: &AppState,
    provider: &dyn MetadataProvider,
    external_id: &str,
) -> ProviderResult<GenericMetadata> {
    let source = provider.id();
    let ttl =
        chrono::Duration::from_std(cache::CacheEntity::Series.default_ttl().to_std().unwrap())
            .unwrap_or(chrono::Duration::hours(168));
    // Conditional single-flight (WP-2.9): an expired row is revalidated
    // with its stored `ETag` / `Last-Modified` and a 304 keeps the
    // cached body. Providers without conditional support fall through
    // to a plain fetch via the trait default.
    cache::get_or_revalidate(
        &state.db,
        source,
        cache::CacheEntity::Series,
        external_id,
        ttl,
        |validators| async move {
            provider
                .fetch_series_conditional(external_id, validators.as_ref())
                .await
        },
    )
    .await
}

pub(crate) async fn fetch_issue_detail(
    state: &AppState,
    provider: &dyn MetadataProvider,
    external_id: &str,
) -> ProviderResult<GenericMetadata> {
    fetch_issue_detail_cached(&state.db, provider, external_id).await
}

/// [`fetch_issue_detail`] on a bare connection: the `metadata_cache`
/// issue row (24 h, conditional revalidation) shared by apply, URL lookup
/// and the batch direct lookup — a detail the batch fetched is the one
/// the apply then reads, free.
pub(crate) async fn fetch_issue_detail_cached(
    db: &DatabaseConnection,
    provider: &dyn MetadataProvider,
    external_id: &str,
) -> ProviderResult<GenericMetadata> {
    let source = provider.id();
    let ttl = chrono::Duration::from_std(cache::CacheEntity::Issue.default_ttl().to_std().unwrap())
        .unwrap_or(chrono::Duration::hours(24));
    cache::get_or_revalidate(
        db,
        source,
        cache::CacheEntity::Issue,
        external_id,
        ttl,
        |validators| async move {
            provider
                .fetch_issue_conditional(external_id, validators.as_ref())
                .await
        },
    )
    .await
}

pub(crate) async fn fetch_field_provenance_map(
    db: &DatabaseConnection,
    entity_type: &str,
    entity_id: &str,
) -> Result<HashMap<String, String>, sea_orm::DbErr> {
    let rows = field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq(entity_type))
        .filter(field_provenance::Column::EntityId.eq(entity_id))
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|r| (r.field, r.set_by)).collect())
}

/// Full provenance rows (field + set_by + set_at + source) for an entity —
/// the richer counterpart to [`fetch_field_provenance_map`], used by the
/// issue Metadata tab to render a "field → source → when" table. Ordered by
/// `set_at` descending (most recently touched first).
pub(crate) async fn fetch_field_provenance_rows(
    db: &DatabaseConnection,
    entity_type: &str,
    entity_id: &str,
) -> Result<Vec<field_provenance::Model>, sea_orm::DbErr> {
    field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq(entity_type))
        .filter(field_provenance::Column::EntityId.eq(entity_id))
        .order_by_desc(field_provenance::Column::SetAt)
        .all(db)
        .await
}

/// Write the candidate's identifiers, honouring user precedence per
/// source. A `set_by='user'` row is replaced only when the source is in
/// `args.override_external_id_sources` (the M5 conflict pane's "Use
/// theirs") or the admin `override_user_edits` force flag is set; a
/// kept user value is reported under `external_ids_skipped`, never as
/// added (audit DI-2, DI-4).
async fn apply_external_ids(
    db: &DatabaseConnection,
    entity_type: &str,
    entity_id: &str,
    identifiers: &[Identifier],
    set_by: SetBy,
    args: &ApplyArgs,
    outcome: &mut ApplyOutcome,
) -> Result<(), ApplyError> {
    for id in identifiers {
        let override_user = args.override_user_edits
            || args
                .override_external_id_sources
                .contains(id.source.as_str());
        match writers::set_external_id_with_override(
            db,
            entity_type,
            entity_id,
            id,
            set_by,
            override_user,
        )
        .await
        {
            Ok(writers::SetExternalIdOutcome::SkippedConflict { owner }) => {
                outcome.external_ids_skipped.push(ExternalIdSkipped {
                    source: id.source.as_str().into(),
                    external_id: id.id.clone(),
                    reason: format!("already assigned to another item ({owner})"),
                })
            }
            // Already in sync with the user's own value — nothing to
            // report either way.
            Ok(writers::SetExternalIdOutcome::KeptUserValue { same_value: true }) => {}
            Ok(writers::SetExternalIdOutcome::KeptUserValue { same_value: false }) => {
                outcome.external_ids_skipped.push(ExternalIdSkipped {
                    source: id.source.as_str().into(),
                    external_id: id.id.clone(),
                    reason: "kept the user-set value".into(),
                })
            }
            Ok(_) => outcome.external_ids_added.push(ExternalIdAdded {
                source: id.source.as_str().into(),
                external_id: id.id.clone(),
            }),
            Err(e) => outcome.external_ids_skipped.push(ExternalIdSkipped {
                source: id.source.as_str().into(),
                external_id: id.id.clone(),
                reason: e.to_string(),
            }),
        }
    }
    Ok(())
}

fn issue_row_has_credits(row: &issue::Model) -> bool {
    let any = |s: &Option<String>| s.as_deref().is_some_and(|v| !v.trim().is_empty());
    any(&row.writer)
        || any(&row.penciller)
        || any(&row.inker)
        || any(&row.colorist)
        || any(&row.letterer)
        || any(&row.cover_artist)
        || any(&row.editor)
        || any(&row.translator)
}

pub(crate) async fn flip_candidate_applied(
    db: &DatabaseConnection,
    run_id: Uuid,
    ordinal: i32,
) -> Result<(), sea_orm::DbErr> {
    let Some(row) = metadata_run_candidate::Entity::find_by_id((run_id, ordinal))
        .one(db)
        .await?
    else {
        return Ok(());
    };
    let mut am: metadata_run_candidate::ActiveModel = row.into();
    am.applied_at = Set(Some(Utc::now().fixed_offset()));
    am.update(db).await?;
    Ok(())
}

pub(crate) async fn bump_run_counts(
    db: &DatabaseConnection,
    run_id: Uuid,
    outcome: &ApplyOutcome,
) -> Result<(), sea_orm::DbErr> {
    let Some(row) = metadata_run::Entity::find_by_id(run_id).one(db).await? else {
        return Ok(());
    };
    let any_write = !outcome.applied_fields.is_empty()
        || outcome.cover_replaced
        || !outcome.external_ids_added.is_empty()
        // M3: the XML-first path defers entity writes to the scoped
        // rescan triggered by the rewrite job, but the user-facing
        // contract is still "I applied this candidate" — count it as
        // an apply.
        || outcome.enqueued_rewrite
        // Variant covers (issue_cover rows) are presentational DB
        // writes outside the XML pipeline. Count them as an apply
        // even when nothing else lands (rare: provider has variants
        // but neither primary cover nor any text fields).
        || outcome.variants_written > 0;
    let mut am: metadata_run::ActiveModel = row.into();
    if any_write {
        am.items_applied = Set(active_i32(&am.items_applied) + 1);
    } else {
        am.items_skipped = Set(active_i32(&am.items_skipped) + 1);
    }
    am.update(db).await?;
    Ok(())
}

fn active_i32(v: &ActiveValue<i32>) -> i32 {
    match v {
        ActiveValue::Set(n) | ActiveValue::Unchanged(n) => *n,
        _ => 0,
    }
}

/// Issue-scope fields the sidecar (XML-first) apply records provenance
/// for: the intersection of (a) fields the composer sources from the
/// provider detail, (b) fields either sidecar schema can carry, and
/// (c) fields the scanner ingests back into issue columns on the
/// scoped rescan. Anything outside that intersection would produce a
/// provenance row describing a value that never lands in the DB —
/// e.g. `Sku`/`Price`/`StoreDate`/`FocDate` (composed into MetronInfo
/// but not yet scanner-ingested), `PageCount` (ingested from the
/// archive's real image entries, not the XML's claim), and the covers
/// (no provenance rows on the DB-direct branch either).
const SIDECAR_ISSUE_PROVENANCE_FIELDS: &[MetadataField] = &[
    MetadataField::Title,
    MetadataField::Description,
    MetadataField::AgeRating,
    MetadataField::CoverDate,
    MetadataField::LanguageCode,
    MetadataField::Format,
    MetadataField::Publisher,
    MetadataField::Imprint,
    MetadataField::Volume,
    MetadataField::CommunityRating,
    MetadataField::ScanInformation,
    MetadataField::Credits,
    MetadataField::Characters,
    MetadataField::Teams,
    MetadataField::Locations,
    MetadataField::StoryArcs,
    MetadataField::Tags,
    MetadataField::Genres,
];

/// Blank the preview-pane rows the user left unticked so the composer
/// falls back to the issue's own value for them (the DB-direct path's
/// `should_apply` gate, applied to the sidecar payload). Only the rows the
/// preview shows ([`merge::scope_fields`] for an issue) are masked. Same
/// contract as the DB-direct gate: an ABSENT set (`None`: legacy clients,
/// one-click apply) is "apply everything" and leaves the payload untouched;
/// an EMPTY set means the user unticked every row, so every row is masked.
/// A composite apply's merged payload already carries only its kept fields.
fn mask_unselected_issue_fields(
    mut detail: GenericMetadata,
    selected: Option<&std::collections::HashSet<String>>,
) -> GenericMetadata {
    let Some(selected) = selected else {
        return detail;
    };
    let empty = GenericMetadata::default();
    for &field in crate::metadata::merge::scope_fields(crate::metadata::merge::MergeScope::Issue) {
        if !selected.contains(&field.key()) {
            crate::metadata::composite::copy_field(
                &mut detail,
                &empty,
                field,
                crate::metadata::merge::MergeScope::Issue,
            );
        }
    }
    detail
}

/// Composer pin vocabulary per field: `summary`/`description` alias
/// the same issue-summary column (PATCH pins write `summary`, provider
/// applies write `description`); everything else pins under its own
/// [`MetadataField::key`]. Mirrors `compose_comicinfo`'s check.
fn sidecar_pin_matches(pins: &std::collections::HashSet<String>, field: MetadataField) -> bool {
    pins.contains(&field.key())
        || (matches!(field, MetadataField::Description) && pins.contains("summary"))
}

/// Walk every `applied_fields` entry and emit a `field_provenance`
/// row. Skips entries that don't parse to a `MetadataField` (covers,
/// junctions, external_ids — those already wrote provenance through
/// their own writer helpers).
async fn write_provenance_for_applied(
    db: &DatabaseConnection,
    entity_type: &str,
    entity_id: &str,
    applied: &[String],
    resolver: &ProvResolver<'_>,
) -> Result<(), sea_orm::DbErr> {
    use std::str::FromStr;
    for f in applied {
        // Junctions + covers + external_ids: the underlying writer
        // already wrote provenance. Skip to avoid the double-write.
        let Ok(field) = MetadataField::from_str(f) else {
            continue;
        };
        if field.is_junction() || field.is_cover() {
            continue;
        }
        if matches!(field, MetadataField::ExternalId(_)) {
            continue;
        }
        let prov = resolver.resolve(f);
        writers::write_field_provenance(
            db,
            entity_type,
            entity_id,
            field,
            prov.set_by,
            prov.source_ext,
        )
        .await?;
    }
    Ok(())
}

fn date_from_db(d: sea_orm::prelude::Date) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year(), d.month(), d.day())
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())
}

fn date_to_db(d: NaiveDate) -> sea_orm::prelude::Date {
    sea_orm::prelude::Date::from_ymd_opt(d.year(), d.month(), d.day())
        .unwrap_or_else(|| sea_orm::prelude::Date::from_ymd_opt(1970, 1, 1).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(mode: ApplyMode, override_user: bool) -> ApplyArgs {
        ApplyArgs {
            run_id: Uuid::nil(),
            ordinal: 0,
            mode,
            apply_cover: false,
            cover_overwrite_policy: CoverOverwritePolicy::WhenMissing,
            override_user_edits: override_user,
            actor_id: None,
            selected_fields: None,
            override_external_id_sources: std::collections::HashSet::new(),
        }
    }

    #[test]
    fn decision_matrix_empty_db_applies_under_both_modes() {
        let prov = HashMap::new();
        assert!(should_apply(
            false,
            &prov,
            MetadataField::Title,
            &args(ApplyMode::FillMissing, false)
        ));
        assert!(should_apply(
            false,
            &prov,
            MetadataField::Title,
            &args(ApplyMode::ReplaceAll, false)
        ));
    }

    #[test]
    fn decision_matrix_user_set_is_sacred_unless_override() {
        let mut prov = HashMap::new();
        prov.insert(MetadataField::Title.key(), "user".into());
        assert!(!should_apply(
            false,
            &prov,
            MetadataField::Title,
            &args(ApplyMode::FillMissing, false)
        ));
        assert!(!should_apply(
            false,
            &prov,
            MetadataField::Title,
            &args(ApplyMode::ReplaceAll, false)
        ));
        assert!(should_apply(
            false,
            &prov,
            MetadataField::Title,
            &args(ApplyMode::FillMissing, true)
        ));
        assert!(should_apply(
            true,
            &prov,
            MetadataField::Title,
            &args(ApplyMode::ReplaceAll, true)
        ));
    }

    #[test]
    fn decision_matrix_non_user_provenance_respects_mode() {
        let mut prov = HashMap::new();
        prov.insert(MetadataField::Title.key(), "comicinfo".into());
        assert!(!should_apply(
            true,
            &prov,
            MetadataField::Title,
            &args(ApplyMode::FillMissing, false)
        ));
        assert!(should_apply(
            true,
            &prov,
            MetadataField::Title,
            &args(ApplyMode::ReplaceAll, false)
        ));
    }
}
