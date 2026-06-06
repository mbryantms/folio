//! Composite (multi-provider) metadata merge.
//!
//! Where `diff.rs` / `apply.rs` operate on ONE candidate, this module
//! assembles the best candidate from EACH included provider, fetches
//! their (cache-backed) details, and produces:
//!
//! - [`compute_composite_diff`] — a per-field comparison across
//!   providers (providers as columns, fields as rows) with a
//!   merge-policy default per field. Drives the compare UI.
//! - [`apply_composite_issue`] / [`apply_composite_series`] — assemble a
//!   synthetic merged [`GenericMetadata`] from the user's per-field
//!   source picks + a parallel `field -> SetBy` map, and drive the
//!   shared `apply::write_*_fields` so each field's provenance records
//!   its true contributing provider. (M5.)
//!
//! Everything reuses the single-candidate primitives so the comparison
//! badges + write decisions can't drift: `apply::classify_field`,
//! `apply::fetch_*_detail`, `apply::fetch_field_provenance_map`,
//! `diff::classify_external_ids`, `merge::*`.

use crate::metadata::apply::{
    ApplyArgs, ApplyError, ApplyMode, ApplyOutcome, ProvResolver, ProvSource,
    apply_issue_via_sidecar, apply_series_via_sidecar, build_provider, bump_run_counts,
    classify_field, fetch_field_provenance_map, fetch_issue_detail, fetch_series_detail,
    flip_candidate_applied, load_run, parse_source, write_issue_fields, write_series_fields,
};
use crate::metadata::diff::{
    ExternalIdConflictRow, ExternalIdNewRow, classify_external_ids, fetch_field_provenance_full,
};
use crate::metadata::field::MetadataField;
use crate::metadata::identifier::{Identifier, Source};
use crate::metadata::merge::{self, MergePolicyConfig, MergeScope, ProviderDetail};
use crate::metadata::orchestrator;
use crate::metadata::provider::{GenericMetadata, IssueCandidate, SeriesCandidate};
use crate::metadata::writers::{CoverOverwritePolicy, SetBy};
use crate::state::AppState;
use entity::{issue, metadata_run_candidate, series};
use sea_orm::EntityTrait;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

// ───────── response shape ─────────

/// One provider column in the compare view's header.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct CompositeProviderColumn {
    pub source: String,
    pub ordinal: i32,
    pub external_id: String,
    pub bucket: String,
    pub score: f32,
    /// Cover URL for the "verify this is the same issue" thumbnail.
    pub cover_image_url: Option<String>,
    /// Series name / "name #number" for the column subtitle.
    pub title: Option<String>,
}

/// One candidate's proposed value for a field. Keyed by `ordinal`
/// (unique within a run) so multiple candidates from the same provider
/// are distinct columns.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct CompositeProposal {
    pub source: String,
    pub ordinal: i32,
    pub value: Option<String>,
}

/// One field row across all included candidates.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct CompositeFieldRow {
    pub field: String,
    pub label: String,
    pub current_value: Option<String>,
    pub current_set_by: Option<String>,
    pub current_set_at: Option<String>,
    pub proposals: Vec<CompositeProposal>,
    /// Merge-policy default candidate `ordinal` for this field (`null`
    /// when no included candidate has a value). The UI seeds its
    /// per-field selection here.
    pub chosen_ordinal: Option<i32>,
    /// `DiffDecision` for (current vs the chosen value) — reuses
    /// `apply::classify_field` so the badge matches the single-candidate
    /// pane exactly.
    pub decision: String,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct CompositeDiffResp {
    pub run_id: Uuid,
    pub scope: String,
    pub providers: Vec<CompositeProviderColumn>,
    pub rows: Vec<CompositeFieldRow>,
    pub external_ids_new: Vec<ExternalIdNewRow>,
    pub external_id_conflicts: Vec<ExternalIdConflictRow>,
    pub changes_count: usize,
}

// ───────── per-provider candidate selection ─────────

/// The chosen candidate row + its parsed source for one included
/// provider.
struct PickedCandidate {
    source: Source,
    row: metadata_run_candidate::Model,
}

/// Default candidate set: the lowest-ordinal (best-ranked) candidate per
/// provider. Used when the caller doesn't specify an explicit set of
/// ordinals (the initial compare-view open).
pub fn default_best_per_provider(candidates: &[metadata_run_candidate::Model]) -> Vec<i32> {
    let mut seen: HashSet<Source> = HashSet::new();
    let mut out = Vec::new();
    // `candidates` arrives ordered by ordinal asc (fetch_candidates).
    for c in candidates {
        if let Some(src) = parse_source(&c.source)
            && seen.insert(src)
        {
            out.push(c.ordinal);
        }
    }
    out
}

/// Resolve the requested candidate ordinals into [`PickedCandidate`]s,
/// sorted by `provider_preference` then ordinal for a stable column
/// layout. Unknown ordinals are dropped. When `included` is empty the
/// default best-per-provider set is used.
fn pick_candidates(
    candidates: Vec<metadata_run_candidate::Model>,
    included: &[i32],
    policy: &MergePolicyConfig,
) -> Vec<PickedCandidate> {
    let wanted: Vec<i32> = if included.is_empty() {
        default_best_per_provider(&candidates)
    } else {
        included.to_vec()
    };
    let mut picked: Vec<PickedCandidate> = candidates
        .into_iter()
        .filter(|c| wanted.contains(&c.ordinal))
        .filter_map(|row| parse_source(&row.source).map(|source| PickedCandidate { source, row }))
        .collect();
    picked.sort_by_key(|p| {
        (
            policy
                .provider_preference
                .iter()
                .position(|s| *s == p.source)
                .unwrap_or(usize::MAX),
            p.row.ordinal,
        )
    });
    picked
}

/// Fetch the (cache-backed) detail for each picked candidate. Soft-skips
/// a provider whose detail fetch fails so one flaky provider doesn't
/// sink the whole comparison.
async fn assemble_details(
    state: &AppState,
    scope: MergeScope,
    picked: &[PickedCandidate],
) -> Vec<ProviderDetail> {
    let mut out = Vec::new();
    for p in picked {
        let Some(provider) = build_provider(state, p.source) else {
            continue;
        };
        let detail = match scope {
            MergeScope::Series => fetch_series_detail(state, &*provider, &p.row.external_id).await,
            MergeScope::Issue => fetch_issue_detail(state, &*provider, &p.row.external_id).await,
        };
        match detail {
            Ok(detail) => out.push(ProviderDetail {
                source: p.source,
                external_id: p.row.external_id.clone(),
                ordinal: p.row.ordinal,
                detail,
            }),
            Err(e) => {
                tracing::warn!(source = %p.source, error = %e, "composite: detail fetch failed; provider skipped");
            }
        }
    }
    out
}

fn column_for(scope: MergeScope, picked: &PickedCandidate) -> CompositeProviderColumn {
    let (cover, title) = match scope {
        MergeScope::Series => {
            serde_json::from_value::<SeriesCandidate>(picked.row.candidate.clone())
                .map(|c| (c.cover_image_url, Some(c.name)))
                .unwrap_or((None, None))
        }
        MergeScope::Issue => serde_json::from_value::<IssueCandidate>(picked.row.candidate.clone())
            .map(|c| {
                let title = match (c.series_name.as_deref(), c.issue_number.as_deref()) {
                    (Some(s), Some(n)) => Some(format!("{s} #{n}")),
                    (Some(s), None) => Some(s.to_owned()),
                    (None, Some(n)) => Some(format!("#{n}")),
                    (None, None) => c.name.clone(),
                };
                (c.cover_image_url, title)
            })
            .unwrap_or((None, None)),
    };
    CompositeProviderColumn {
        source: picked.source.as_str().to_owned(),
        ordinal: picked.row.ordinal,
        external_id: picked.row.external_id.clone(),
        bucket: picked.row.bucket.clone(),
        score: picked.row.score,
        cover_image_url: cover,
        title,
    }
}

// ───────── current-value extraction (DB row → field string) ─────────

fn count_csv(csv: Option<&str>) -> usize {
    csv.map(|s| s.split(',').filter(|p| !p.trim().is_empty()).count())
        .unwrap_or(0)
}

fn count_string(n: usize) -> Option<String> {
    match n {
        0 => None,
        1 => Some("1 item".to_owned()),
        n => Some(format!("{n} items")),
    }
}

fn series_current(row: &series::Model, field: MetadataField) -> Option<String> {
    let norm = |s: Option<&str>| {
        s.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    match field {
        MetadataField::Title => norm(Some(row.name.as_str())),
        MetadataField::SortName => norm(row.sort_name.as_deref()),
        MetadataField::SeriesType => norm(row.series_type.as_deref()),
        MetadataField::YearBegan => row.year.map(|n| n.to_string()),
        MetadataField::YearEnd => row.year_end.map(|n| n.to_string()),
        MetadataField::Volume => row.volume.map(|n| n.to_string()),
        MetadataField::Publisher => norm(row.publisher.as_deref()),
        MetadataField::Imprint => norm(row.imprint.as_deref()),
        MetadataField::Deck => norm(row.deck.as_deref()),
        MetadataField::Description => norm(row.summary.as_deref()),
        _ => None,
    }
}

async fn issue_current(
    state: &AppState,
    row: &issue::Model,
    field: MetadataField,
) -> Option<String> {
    let norm = |s: Option<&str>| {
        s.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    match field {
        MetadataField::Title => norm(row.title.as_deref()),
        MetadataField::Deck => norm(row.deck.as_deref()),
        MetadataField::Description => norm(row.summary.as_deref()),
        MetadataField::AgeRating => norm(row.age_rating.as_deref()),
        MetadataField::PageCount => row.page_count.map(|n| n.to_string()),
        MetadataField::Sku => norm(row.sku.as_deref()),
        MetadataField::CoverDate => row
            .year
            .zip(row.month)
            .zip(row.day)
            .and_then(|((y, m), d)| {
                chrono::NaiveDate::from_ymd_opt(y, m as u32, d as u32).map(|d| d.to_string())
            }),
        MetadataField::Credits => count_string(
            count_csv(row.writer.as_deref())
                + count_csv(row.penciller.as_deref())
                + count_csv(row.inker.as_deref())
                + count_csv(row.colorist.as_deref())
                + count_csv(row.letterer.as_deref())
                + count_csv(row.cover_artist.as_deref())
                + count_csv(row.editor.as_deref())
                + count_csv(row.translator.as_deref()),
        ),
        MetadataField::Characters => count_string(count_csv(row.characters.as_deref())),
        MetadataField::Teams => count_string(count_csv(row.teams.as_deref())),
        MetadataField::Locations => count_string(count_csv(row.locations.as_deref())),
        MetadataField::StoryArcs => count_string(count_csv(row.story_arc.as_deref())),
        MetadataField::Tags => count_string(count_csv(row.tags.as_deref())),
        MetadataField::Genres => count_string(count_csv(row.genre.as_deref())),
        MetadataField::CoverVariants => {
            use sea_orm::{ColumnTrait, PaginatorTrait, QueryFilter};
            let n = entity::issue_cover::Entity::find()
                .filter(entity::issue_cover::Column::IssueId.eq(&row.id))
                .filter(entity::issue_cover::Column::Kind.eq("variant"))
                .filter(entity::issue_cover::Column::IsActive.eq(true))
                .count(&state.db)
                .await
                .unwrap_or(0);
            count_string(n as usize)
        }
        _ => None,
    }
}

fn label_for(field: MetadataField) -> &'static str {
    match field {
        MetadataField::Title => "Title",
        MetadataField::SortName => "Sort name",
        MetadataField::SeriesType => "Series type",
        MetadataField::YearBegan => "Year began",
        MetadataField::YearEnd => "Year ended",
        MetadataField::Volume => "Volume",
        MetadataField::Publisher => "Publisher",
        MetadataField::Imprint => "Imprint",
        MetadataField::Deck => "Deck",
        MetadataField::Description => "Description",
        MetadataField::AgeRating => "Age rating",
        MetadataField::PageCount => "Page count",
        MetadataField::Sku => "SKU",
        MetadataField::CoverDate => "Cover date",
        MetadataField::Credits => "Credits",
        MetadataField::Characters => "Characters",
        MetadataField::Teams => "Teams",
        MetadataField::Locations => "Locations",
        MetadataField::StoryArcs => "Story arcs",
        MetadataField::Tags => "Tags",
        MetadataField::Genres => "Genres",
        MetadataField::CoverPrimary => "Primary cover",
        MetadataField::CoverVariants => "Variant covers",
        _ => "Field",
    }
}

/// Union of every included provider's identifiers, deduped first-wins
/// per source (matches `merge_provider_identifiers` precedence). Used
/// for the additive external-id rows + the apply write.
pub(crate) fn union_identifiers(details: &[ProviderDetail]) -> Vec<Identifier> {
    let mut seen: HashSet<Source> = HashSet::new();
    let mut out = Vec::new();
    for d in details {
        for id in &d.detail.identifiers {
            if seen.insert(id.source) {
                out.push(id.clone());
            }
        }
    }
    out
}

// ───────── composite preview ─────────

pub async fn compute_composite_diff(
    state: &AppState,
    run_id: Uuid,
    mode: ApplyMode,
    override_user_edits: bool,
    included: &[i32],
) -> Result<CompositeDiffResp, ApplyError> {
    let run = load_run(&state.db, run_id).await?;
    let scope = match run.scope.as_str() {
        orchestrator::scope::SERIES => MergeScope::Series,
        orchestrator::scope::ISSUE => MergeScope::Issue,
        other => return Err(ApplyError::InvalidScope(other.to_owned())),
    };
    let Some(entity_id_str) = run.scope_entity_id.clone() else {
        return Err(match scope {
            MergeScope::Series => ApplyError::SeriesGone,
            MergeScope::Issue => ApplyError::IssueGone,
        });
    };

    let policy = MergePolicyConfig {
        provider_preference: state.cfg().merge_provider_preference(),
        preferred_cover_provider: None,
    };
    let candidates = orchestrator::fetch_candidates(&state.db, run_id).await?;
    let picked = pick_candidates(candidates, included, &policy);
    let providers: Vec<CompositeProviderColumn> =
        picked.iter().map(|p| column_for(scope, p)).collect();
    let details = assemble_details(state, scope, &picked).await;

    // Args for the per-field classifier (mirrors the single-candidate
    // pane's decision matrix).
    let args = ApplyArgs {
        run_id,
        ordinal: 0,
        mode,
        apply_cover: false,
        cover_overwrite_policy: crate::metadata::writers::CoverOverwritePolicy::WhenMissing,
        override_user_edits,
        actor_id: None,
        selected_fields: None,
        override_external_id_sources: HashSet::new(),
    };

    let entity_type = match scope {
        MergeScope::Series => "series",
        MergeScope::Issue => "issue",
    };
    let provenance = fetch_field_provenance_map(&state.db, entity_type, &entity_id_str).await?;
    let provenance_full =
        fetch_field_provenance_full(&state.db, entity_type, &entity_id_str).await?;

    // Resolve the entity row for current values.
    let series_row = if scope == MergeScope::Series {
        let uuid = Uuid::parse_str(&entity_id_str)
            .map_err(|e| ApplyError::InvalidScope(format!("scope_entity_id not uuid: {e}")))?;
        Some(
            series::Entity::find_by_id(uuid)
                .one(&state.db)
                .await?
                .ok_or(ApplyError::SeriesGone)?,
        )
    } else {
        None
    };
    let issue_row = if scope == MergeScope::Issue {
        Some(
            issue::Entity::find_by_id(&entity_id_str)
                .one(&state.db)
                .await?
                .ok_or(ApplyError::IssueGone)?,
        )
    } else {
        None
    };

    let mut rows = Vec::new();
    for &field in merge::scope_fields(scope) {
        let current_value = match scope {
            MergeScope::Series => series_current(series_row.as_ref().unwrap(), field),
            MergeScope::Issue => issue_current(state, issue_row.as_ref().unwrap(), field).await,
        };
        let proposals: Vec<CompositeProposal> = details
            .iter()
            .map(|d| CompositeProposal {
                source: d.source.as_str().to_owned(),
                ordinal: d.ordinal,
                value: merge::field_value_as_string(&d.detail, field, scope),
            })
            .collect();
        let chosen_ordinal = merge::choose_field_candidate(field, &details, &policy, scope);
        let chosen_value = chosen_ordinal.and_then(|ord| {
            details
                .iter()
                .find(|d| d.ordinal == ord)
                .and_then(|d| merge::field_value_as_string(&d.detail, field, scope))
        });
        let decision = classify_field(
            current_value.as_deref(),
            chosen_value.as_deref(),
            &provenance,
            field,
            &args,
        );
        let (current_set_by, current_set_at) = provenance_full
            .get(&field.key())
            .map(|p| (Some(p.set_by.clone()), Some(p.set_at.to_rfc3339())))
            .unwrap_or((None, None));
        rows.push(CompositeFieldRow {
            field: field.key(),
            label: label_for(field).to_owned(),
            current_value,
            current_set_by,
            current_set_at,
            proposals,
            chosen_ordinal,
            decision: decision.as_str().to_owned(),
        });
    }

    let union = union_identifiers(&details);
    let (conflicts, news) =
        classify_external_ids(&state.db, entity_type, &entity_id_str, &union).await?;

    let changes_count = rows
        .iter()
        .filter(|r| matches!(r.decision.as_str(), "would_fill" | "would_replace"))
        .count()
        + conflicts.len()
        + news.len();

    Ok(CompositeDiffResp {
        run_id,
        scope: run.scope,
        providers,
        rows,
        external_ids_new: news,
        external_id_conflicts: conflicts,
        changes_count,
    })
}

/// Build a synthetic merged [`GenericMetadata`] from the per-field
/// candidate map, plus the parallel `field.key() -> ProvSource`
/// provenance map the apply write body resolves through. `field_sources`
/// maps a `MetadataField::key()` to the candidate `ordinal` whose value
/// wins that field; `set_by` provenance records that candidate's source.
/// Fields absent from the map stay at `Default` (not applied).
pub fn build_merged_detail(
    details: &[ProviderDetail],
    field_sources: &HashMap<String, i32>,
    scope: MergeScope,
    preference: &[Source],
) -> (
    GenericMetadata,
    HashMap<String, crate::metadata::apply::ProvSource>,
) {
    use crate::metadata::apply::ProvSource;
    use crate::metadata::writers::SetBy;

    let by_ordinal = |ord: i32| details.iter().find(|d| d.ordinal == ord);
    let mut merged = GenericMetadata::default();
    let mut prov: HashMap<String, ProvSource> = HashMap::new();

    for &field in merge::scope_fields(scope) {
        let key = field.key();
        let Some(&ordinal) = field_sources.get(&key) else {
            continue;
        };
        let Some(pd) = by_ordinal(ordinal) else {
            continue;
        };
        copy_field(&mut merged, &pd.detail, field, scope);
        prov.insert(
            key,
            ProvSource {
                set_by: SetBy::Provider(pd.source),
                source_ext: pd.detail.source_external_id.clone(),
            },
        );
    }

    // Identifiers are additive — union across ALL included providers.
    merged.identifiers = union_identifiers(details);
    // Attribution: pick the highest-preference included source so the
    // sidecar audit line still fires.
    merged.source_provider = preference
        .iter()
        .find(|s| details.iter().any(|d| d.source == **s))
        .copied()
        .or_else(|| details.first().map(|d| d.source));
    (merged, prov)
}

/// Copy one field's value from `src` detail into `merged`. Mirrors the
/// `GenericMetadata` accessors `apply` reads per field.
fn copy_field(
    merged: &mut GenericMetadata,
    src: &GenericMetadata,
    field: MetadataField,
    scope: MergeScope,
) {
    match field {
        MetadataField::Title => match scope {
            MergeScope::Series => merged.series_name = src.series_name.clone(),
            MergeScope::Issue => merged.title = src.title.clone(),
        },
        MetadataField::SortName => merged.series_sort_name = src.series_sort_name.clone(),
        MetadataField::SeriesType => merged.series_type = src.series_type.clone(),
        MetadataField::YearBegan => merged.year_began = src.year_began,
        MetadataField::YearEnd => merged.year_end = src.year_end,
        MetadataField::Volume => merged.volume = src.volume,
        MetadataField::Publisher => merged.publisher = src.publisher.clone(),
        MetadataField::Imprint => merged.imprint = src.imprint.clone(),
        MetadataField::Deck => merged.deck = src.deck.clone(),
        MetadataField::Description => merged.description = src.description.clone(),
        MetadataField::AgeRating => merged.age_rating = src.age_rating.clone(),
        MetadataField::PageCount => merged.page_count = src.page_count,
        MetadataField::Sku => merged.sku = src.sku.clone(),
        MetadataField::CoverDate => merged.cover_date = src.cover_date,
        MetadataField::Credits => merged.credits = src.credits.clone(),
        MetadataField::Characters => merged.characters = src.characters.clone(),
        MetadataField::Teams => merged.teams = src.teams.clone(),
        MetadataField::Locations => merged.locations = src.locations.clone(),
        MetadataField::StoryArcs => merged.story_arcs = src.story_arcs.clone(),
        MetadataField::Tags => merged.tags = src.tags.clone(),
        MetadataField::Genres => merged.genres = src.genres.clone(),
        MetadataField::CoverPrimary => {
            merged.cover_image_url = src.cover_image_url.clone();
            merged.cover_image_alt_urls = src.cover_image_alt_urls.clone();
        }
        MetadataField::CoverVariants => merged.variants = src.variants.clone(),
        _ => {}
    }
}

// ───────── composite apply (M5) ─────────

/// Inputs for a composite apply. `field_sources` maps each
/// `MetadataField::key()` the user kept to the candidate `ordinal` whose
/// value wins it (absent = not applied). `included` is the candidate
/// `ordinal` set whose details contribute (also the candidates whose
/// `applied_at` gets flipped).
pub struct CompositeApplyArgs {
    pub run_id: Uuid,
    pub field_sources: HashMap<String, i32>,
    pub included: Vec<i32>,
    pub mode: ApplyMode,
    pub apply_cover: bool,
    pub cover_overwrite_policy: CoverOverwritePolicy,
    pub override_user_edits: bool,
    pub override_external_id_sources: HashSet<String>,
    pub actor_id: Option<Uuid>,
}

/// Apply a composite (multi-provider) merge. Builds the synthetic merged
/// detail + per-field provenance map and drives the shared
/// `apply::write_*_fields` (DB-direct) or `apply::apply_*_via_sidecar`
/// (writeback libraries). Flips `applied_at` on every included candidate.
pub async fn apply_composite(
    state: &AppState,
    args: CompositeApplyArgs,
) -> Result<ApplyOutcome, ApplyError> {
    let run = load_run(&state.db, args.run_id).await?;
    let scope = match run.scope.as_str() {
        orchestrator::scope::SERIES => MergeScope::Series,
        orchestrator::scope::ISSUE => MergeScope::Issue,
        other => return Err(ApplyError::InvalidScope(other.to_owned())),
    };
    let Some(entity_id_str) = run.scope_entity_id.clone() else {
        return Err(match scope {
            MergeScope::Series => ApplyError::SeriesGone,
            MergeScope::Issue => ApplyError::IssueGone,
        });
    };

    // Resolve the included candidates → details.
    let candidates = orchestrator::fetch_candidates(&state.db, args.run_id).await?;
    let mut picked: Vec<PickedCandidate> = Vec::new();
    for ordinal in &args.included {
        if let Some(row) = candidates.iter().find(|c| c.ordinal == *ordinal)
            && let Some(source) = parse_source(&row.source)
        {
            picked.push(PickedCandidate {
                source,
                row: row.clone(),
            });
        }
    }
    let details = assemble_details(state, scope, &picked).await;
    if details.is_empty() {
        return Err(ApplyError::InvalidScope(
            "no included providers resolved to a detail".to_owned(),
        ));
    }

    let preference = state.cfg().merge_provider_preference();
    let (merged, prov_map) = build_merged_detail(&details, &args.field_sources, scope, &preference);
    let primary_source = merged.source_provider.unwrap_or(details[0].source);
    let fallback = ProvSource {
        set_by: SetBy::Provider(primary_source),
        source_ext: merged.source_external_id.clone(),
    };
    let resolver = ProvResolver::PerField {
        map: &prov_map,
        fallback,
    };

    // The primary included ordinal — the one the sidecar path flips/bumps.
    let primary_ordinal = picked
        .iter()
        .find(|p| p.source == primary_source)
        .map(|p| p.row.ordinal)
        .unwrap_or(0);

    let apply_args = ApplyArgs {
        run_id: args.run_id,
        ordinal: primary_ordinal,
        mode: args.mode,
        apply_cover: args.apply_cover,
        cover_overwrite_policy: args.cover_overwrite_policy,
        override_user_edits: args.override_user_edits,
        actor_id: args.actor_id,
        // The kept-field set drives the existing per-field opt-in gate.
        selected_fields: Some(args.field_sources.keys().cloned().collect()),
        override_external_id_sources: args.override_external_id_sources.clone(),
    };

    // Writeback dispatch: same flag check as the single-candidate path.
    let (outcome, is_writeback) = match scope {
        MergeScope::Series => {
            let series_uuid = Uuid::parse_str(&entity_id_str)
                .map_err(|e| ApplyError::InvalidScope(format!("scope_entity_id not uuid: {e}")))?;
            let series_row = series::Entity::find_by_id(series_uuid)
                .one(&state.db)
                .await?
                .ok_or(ApplyError::SeriesGone)?;
            let writeback = library_is_writeback(state, series_row.library_id).await?;
            let outcome = if writeback {
                apply_series_via_sidecar(state, &apply_args, &series_row, primary_source, merged)
                    .await?
            } else {
                write_series_fields(
                    state,
                    &series_row,
                    series_uuid,
                    &merged,
                    apply_args,
                    &resolver,
                )
                .await?
            };
            (outcome, writeback)
        }
        MergeScope::Issue => {
            let issue_row = issue::Entity::find_by_id(&entity_id_str)
                .one(&state.db)
                .await?
                .ok_or(ApplyError::IssueGone)?;
            let writeback = library_is_writeback(state, issue_row.library_id).await?;
            let outcome = if writeback {
                apply_issue_via_sidecar(state, &apply_args, &issue_row, primary_source, merged)
                    .await?
            } else {
                // Cover provider = the source of the candidate chosen for
                // the primary cover, else the primary source (cover block
                // no-ops when the merged detail has no cover URL).
                let cover_source = args
                    .field_sources
                    .get(&MetadataField::CoverPrimary.key())
                    .and_then(|ord| details.iter().find(|d| d.ordinal == *ord))
                    .map(|d| d.source)
                    .unwrap_or(primary_source);
                let cover_provider = build_provider(state, cover_source).ok_or_else(|| {
                    ApplyError::InvalidScope(format!("provider {cover_source} not configured"))
                })?;
                write_issue_fields(
                    state,
                    &issue_row,
                    &merged,
                    apply_args,
                    &resolver,
                    &*cover_provider,
                    cover_source,
                )
                .await?
            };
            (outcome, writeback)
        }
    };

    // Flip applied_at on every included candidate. The sidecar path
    // already flipped+bumped the primary ordinal; the DB-direct path did
    // neither, so it bumps once below. flip is idempotent.
    for ordinal in &args.included {
        flip_candidate_applied(&state.db, args.run_id, *ordinal).await?;
    }
    if !is_writeback {
        bump_run_counts(&state.db, args.run_id, &outcome).await?;
    }

    Ok(outcome)
}

/// Fold the merge-policy field choices into the `field_sources` map
/// [`apply_composite`] consumes. Only fields a provider can fill (a `Some`
/// ordinal) are kept. The map MUST be non-empty for a meaningful apply —
/// `apply_composite` derives `selected_fields` from its keys, so an empty
/// map is a silent no-op. See the [`apply_composite_auto`] doc comment.
fn field_sources_from_choices(choices: Vec<merge::FieldChoice>) -> HashMap<String, i32> {
    choices
        .into_iter()
        .filter_map(|fc| fc.ordinal.map(|ord| (fc.field.key(), ord)))
        .collect()
}

/// Inputs for an **auto** composite apply — the bulk "Fill missing /
/// Replace all" path, where there is no human picking per-field sources.
/// The included candidate set defaults to the best-ranked candidate per
/// provider; the per-field winners are derived from the merge policy.
pub struct AutoCompositeArgs {
    pub run_id: Uuid,
    /// Candidate `ordinal`s to merge. Empty → best-per-provider default.
    pub included: Vec<i32>,
    pub mode: ApplyMode,
    pub apply_cover: bool,
    pub cover_overwrite_policy: CoverOverwritePolicy,
    pub override_user_edits: bool,
    pub override_external_id_sources: HashSet<String>,
    /// Cover-only source override (bulk prefers ComicVine).
    pub preferred_cover_provider: Option<Source>,
    pub actor_id: Option<Uuid>,
}

/// Auto composite apply: assemble the **most-complete** record across the
/// run's providers and apply it without any per-field human selection.
///
/// This is the crux of the bulk apply. It computes a fully-populated
/// `field_sources` map from [`merge::build_default_merge`] (every field any
/// included provider can fill) and hands it to [`apply_composite`]. Passing
/// an empty map would be a no-op, because `apply_composite` derives
/// `selected_fields` from `field_sources.keys()` — so the map MUST carry
/// every applicable field, which is exactly what `build_default_merge`
/// returns.
pub async fn apply_composite_auto(
    state: &AppState,
    args: AutoCompositeArgs,
) -> Result<ApplyOutcome, ApplyError> {
    let run = load_run(&state.db, args.run_id).await?;
    let scope = match run.scope.as_str() {
        orchestrator::scope::SERIES => MergeScope::Series,
        orchestrator::scope::ISSUE => MergeScope::Issue,
        other => return Err(ApplyError::InvalidScope(other.to_owned())),
    };

    // Cover-aware policy: the bulk path prefers ComicVine for the primary
    // cover while keeping the global preference for every other field.
    let policy = MergePolicyConfig {
        provider_preference: state.cfg().merge_provider_preference(),
        preferred_cover_provider: args.preferred_cover_provider,
    };

    let candidates = orchestrator::fetch_candidates(&state.db, args.run_id).await?;
    // Effective included set: caller's explicit list, else best-per-provider.
    let included = if args.included.is_empty() {
        default_best_per_provider(&candidates)
    } else {
        args.included.clone()
    };
    let picked = pick_candidates(candidates, &included, &policy);
    let details = assemble_details(state, scope, &picked).await;
    if details.is_empty() {
        return Err(ApplyError::InvalidScope(
            "no included providers resolved to a detail".to_owned(),
        ));
    }

    // Most-complete merge → full field_sources map (every field any provider
    // can fill). Non-empty by construction whenever a provider supplied a
    // value; see the doc comment for why this matters.
    let field_sources =
        field_sources_from_choices(merge::build_default_merge(&details, &policy, scope));

    apply_composite(
        state,
        CompositeApplyArgs {
            run_id: args.run_id,
            field_sources,
            included,
            mode: args.mode,
            apply_cover: args.apply_cover,
            cover_overwrite_policy: args.cover_overwrite_policy,
            override_user_edits: args.override_user_edits,
            override_external_id_sources: args.override_external_id_sources,
            actor_id: args.actor_id,
        },
    )
    .await
}

async fn library_is_writeback(state: &AppState, library_id: Uuid) -> Result<bool, ApplyError> {
    let lib = entity::library::Entity::find_by_id(library_id)
        .one(&state.db)
        .await?;
    Ok(lib
        .map(|l| l.metadata_writeback_enabled && l.allow_archive_writeback)
        .unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::field_sources_from_choices;
    use crate::metadata::field::MetadataField;
    use crate::metadata::merge::FieldChoice;

    /// The auto-composite map must keep exactly the fields a provider can
    /// fill and drop the `None` ones — and stay non-empty, guarding against
    /// the silent no-op where an empty `field_sources` applies nothing.
    #[test]
    fn field_sources_keeps_only_filled_fields() {
        let choices = vec![
            FieldChoice {
                field: MetadataField::Title,
                ordinal: Some(0),
                value: Some("X".into()),
            },
            FieldChoice {
                field: MetadataField::Deck,
                ordinal: None,
                value: None,
            },
            FieldChoice {
                field: MetadataField::CoverPrimary,
                ordinal: Some(1),
                value: Some("https://cv/cover.jpg".into()),
            },
        ];
        let map = field_sources_from_choices(choices);
        assert_eq!(map.len(), 2, "only the two filled fields survive");
        assert!(!map.is_empty(), "non-empty guards against a no-op apply");
        assert_eq!(map.get(&MetadataField::Title.key()), Some(&0));
        assert_eq!(map.get(&MetadataField::CoverPrimary.key()), Some(&1));
        assert!(!map.contains_key(&MetadataField::Deck.key()));
    }

    /// All-`None` choices (no provider supplied anything) → empty map; the
    /// caller treats this as "nothing to apply".
    #[test]
    fn field_sources_empty_when_no_choices_filled() {
        let choices = vec![FieldChoice {
            field: MetadataField::Title,
            ordinal: None,
            value: None,
        }];
        assert!(field_sources_from_choices(choices).is_empty());
    }
}
