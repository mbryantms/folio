//! Proposed-diff computation — drives the M5 dialog's preview pane.
//!
//! `compute_series_diff` / `compute_issue_diff` mirror the live
//! Apply pipeline up to (but excluding) the write step, returning a
//! structured [`DiffResp`] the web client renders as a per-field
//! diff table.
//!
//! Why a separate module instead of folding diff into apply:
//!
//! - The apply pipeline's hot path is fast precisely because it
//!   collects updates into a `SeriesUpdates` / `IssueUpdates` struct
//!   then emits one SQL UPDATE. Threading diff collection through
//!   it would force ~30 closure invocations to allocate `String`s
//!   they otherwise didn't need.
//! - The diff endpoint runs every time the preview pane opens (one
//!   per candidate the user inspects); apply runs once per
//!   confirmation. Keeping them parallel-but-separate lets diff
//!   pre-compute friendly labels while apply stays terse.
//! - Crucially, both paths share [`apply::classify_field`] +
//!   [`apply::should_apply`] + [`apply::fetch_*_detail`] +
//!   [`apply::fetch_field_provenance_map`] so they CAN'T drift —
//!   the decision matrix lives in apply.rs and both callers read it.
//!
//! metadata-providers-1.0 M5 — diff view.

use crate::metadata::apply::{
    ApplyArgs, ApplyError, build_provider, classify_field, fetch_field_provenance_map,
    fetch_issue_detail, fetch_series_detail, load_candidate, load_run, parse_source,
};
use crate::metadata::field::MetadataField;
use crate::metadata::identifier::Source;
use crate::metadata::orchestrator;
use crate::metadata::writers;
use crate::state::AppState;
use entity::{external_id, field_provenance, issue, series};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Serialize;
use std::collections::HashMap;
use uuid::Uuid;

// ───────── response shape ─────────

/// What the preview pane needs to render one row of the diff table.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct ScalarDiffRow {
    /// `MetadataField::key()` — stable string identifier the apply
    /// payload echoes back in `selected_fields`.
    pub field: String,
    /// Human-readable label for the field, derived once on the
    /// server so every client renders the same string.
    pub label: String,
    /// String-formatted current value, or `null` if the DB cell is
    /// empty / NULL. Always a string so client rendering is uniform
    /// across int/str/date/float fields.
    pub current_value: Option<String>,
    /// String-formatted proposed value from the provider candidate.
    pub proposed_value: Option<String>,
    /// One of the [`DiffDecision`] variants — drives default checked
    /// state + per-row badge in the preview pane.
    pub decision: String,
    /// `set_by` for the current row's provenance (`"user"` /
    /// `"comicvine"` / `"metroninfo"` / …). `null` when the field has
    /// no provenance row.
    pub current_set_by: Option<String>,
    /// ISO-8601 timestamp of the last write to this field, or `null`.
    pub current_set_at: Option<String>,
}

/// External-ID conflict row — surfaces in the preview pane as an
/// amber row with a Keep mine / Use theirs control.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct ExternalIdConflictRow {
    /// `"comicvine" | "metron" | …` from [`Source::as_str`].
    pub source: String,
    /// Current user-set external id.
    pub current_external_id: String,
    /// The candidate's proposed external id (always different from
    /// `current_external_id`; same-value rows are filtered out).
    pub proposed_external_id: String,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct DiffResp {
    pub run_id: Uuid,
    pub ordinal: i32,
    /// `"series" | "issue"` — echoes the run scope so the client
    /// can render the right header copy.
    pub scope: String,
    /// `"comicvine" | "metron"` — which provider supplied the
    /// candidate. Drives the "Apply from <provider>" copy.
    pub source: String,
    pub source_external_id: String,
    /// Per-field rows, ordered by enum declaration (groups
    /// scalars-then-junctions-then-covers).
    pub rows: Vec<ScalarDiffRow>,
    /// Only present when the candidate carries external_ids that
    /// disagree with user-set rows on the same entity.
    pub external_id_conflicts: Vec<ExternalIdConflictRow>,
    /// Provider IDs the candidate brought that the entity doesn't
    /// have yet (no conflict — would just add).
    pub external_ids_new: Vec<ExternalIdNewRow>,
    /// Count of rows whose decision would actually write a value
    /// (`would_fill` + `would_replace`). Lets the preview header
    /// summarize "5 changes" without the client re-walking the rows.
    pub changes_count: usize,
}

/// Provider external id the entity doesn't carry yet — apply will
/// just add the row, no conflict.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct ExternalIdNewRow {
    pub source: String,
    pub external_id: String,
}

// ───────── series diff ─────────

pub async fn compute_series_diff(
    state: &AppState,
    args: ApplyArgs,
) -> Result<DiffResp, ApplyError> {
    let candidate = load_candidate(&state.db, args.run_id, args.ordinal).await?;
    let run = load_run(&state.db, args.run_id).await?;
    if run.scope != orchestrator::scope::SERIES {
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

    let entity_id_str = series_uuid.to_string();
    let provenance = fetch_field_provenance_map(&state.db, "series", &entity_id_str).await?;
    let provenance_full = fetch_field_provenance_full(&state.db, "series", &entity_id_str).await?;

    let mut rows = Vec::new();

    push_scalar(
        &mut rows,
        MetadataField::Title,
        "Title",
        Some(row.name.as_str()),
        detail.series_name.as_deref(),
        &provenance,
        &provenance_full,
        &args,
    );
    push_scalar(
        &mut rows,
        MetadataField::SortName,
        "Sort name",
        row.sort_name.as_deref(),
        detail.series_sort_name.as_deref(),
        &provenance,
        &provenance_full,
        &args,
    );
    push_scalar(
        &mut rows,
        MetadataField::SeriesType,
        "Series type",
        row.series_type.as_deref(),
        detail.series_type.as_deref(),
        &provenance,
        &provenance_full,
        &args,
    );
    push_scalar_int(
        &mut rows,
        MetadataField::YearBegan,
        "Year began",
        row.year,
        detail.year_began,
        &provenance,
        &provenance_full,
        &args,
    );
    push_scalar_int(
        &mut rows,
        MetadataField::YearEnd,
        "Year ended",
        row.year_end,
        detail.year_end,
        &provenance,
        &provenance_full,
        &args,
    );
    push_scalar_int(
        &mut rows,
        MetadataField::Volume,
        "Volume",
        row.volume,
        detail.volume,
        &provenance,
        &provenance_full,
        &args,
    );
    push_scalar(
        &mut rows,
        MetadataField::Publisher,
        "Publisher",
        row.publisher.as_deref(),
        detail.publisher.as_deref(),
        &provenance,
        &provenance_full,
        &args,
    );
    push_scalar(
        &mut rows,
        MetadataField::Imprint,
        "Imprint",
        row.imprint.as_deref(),
        detail.imprint.as_deref(),
        &provenance,
        &provenance_full,
        &args,
    );
    push_scalar(
        &mut rows,
        MetadataField::Deck,
        "Deck",
        row.deck.as_deref(),
        detail.deck.as_deref(),
        &provenance,
        &provenance_full,
        &args,
    );
    push_scalar(
        &mut rows,
        MetadataField::Description,
        "Description",
        row.summary.as_deref(),
        detail.description.as_deref(),
        &provenance,
        &provenance_full,
        &args,
    );

    let (conflicts, news) =
        classify_external_ids(&state.db, "series", &entity_id_str, &detail.identifiers).await?;

    let changes_count = rows
        .iter()
        .filter(|r| matches!(r.decision.as_str(), "would_fill" | "would_replace"))
        .count()
        + conflicts.len()
        + news.len();
    Ok(DiffResp {
        run_id: args.run_id,
        ordinal: args.ordinal,
        scope: "series".to_owned(),
        source: source.as_str().to_owned(),
        source_external_id: detail.source_external_id.unwrap_or_default(),
        rows,
        external_id_conflicts: conflicts,
        external_ids_new: news,
        changes_count,
    })
}

// ───────── issue diff ─────────

pub async fn compute_issue_diff(state: &AppState, args: ApplyArgs) -> Result<DiffResp, ApplyError> {
    let candidate = load_candidate(&state.db, args.run_id, args.ordinal).await?;
    let run = load_run(&state.db, args.run_id).await?;
    if run.scope != orchestrator::scope::ISSUE {
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

    let entity_id_str = row.id.clone();
    let provenance = fetch_field_provenance_map(&state.db, "issue", &entity_id_str).await?;
    let provenance_full = fetch_field_provenance_full(&state.db, "issue", &entity_id_str).await?;

    let mut rows = Vec::new();
    push_scalar(
        &mut rows,
        MetadataField::Title,
        "Title",
        row.title.as_deref(),
        detail.title.as_deref(),
        &provenance,
        &provenance_full,
        &args,
    );
    push_scalar(
        &mut rows,
        MetadataField::Deck,
        "Deck",
        row.deck.as_deref(),
        detail.deck.as_deref(),
        &provenance,
        &provenance_full,
        &args,
    );
    push_scalar(
        &mut rows,
        MetadataField::Description,
        "Description",
        row.summary.as_deref(),
        detail.description.as_deref(),
        &provenance,
        &provenance_full,
        &args,
    );
    push_scalar(
        &mut rows,
        MetadataField::AgeRating,
        "Age rating",
        row.age_rating.as_deref(),
        detail.age_rating.as_deref(),
        &provenance,
        &provenance_full,
        &args,
    );
    push_scalar_int(
        &mut rows,
        MetadataField::PageCount,
        "Page count",
        row.page_count,
        detail.page_count,
        &provenance,
        &provenance_full,
        &args,
    );
    push_scalar(
        &mut rows,
        MetadataField::Sku,
        "SKU",
        row.sku.as_deref(),
        detail.sku.as_deref(),
        &provenance,
        &provenance_full,
        &args,
    );
    // Cover date — render as ISO date string for diff. The actual
    // write side splits into y/m/d; the user-facing diff doesn't
    // need that subdivision.
    let current_cover_date = row
        .year
        .zip(row.month)
        .zip(row.day)
        .and_then(|((y, m), d)| {
            chrono::NaiveDate::from_ymd_opt(y, m as u32, d as u32).map(|d| d.to_string())
        });
    let incoming_cover_date = detail.cover_date.as_ref().map(|d| d.to_string());
    push_scalar(
        &mut rows,
        MetadataField::CoverDate,
        "Cover date",
        current_cover_date.as_deref(),
        incoming_cover_date.as_deref(),
        &provenance,
        &provenance_full,
        &args,
    );

    // ── junction + variant rows ───────────────────────────────────
    // The apply path writes more than the 7 scalars above: credits,
    // characters, teams, locations, story arcs, tags, genres, variant
    // covers. M5.1 surfaces these as synthetic count-shaped rows in
    // the diff so the user sees what's actually going to land + the
    // Apply button enables when the only changes are these
    // non-scalar writes.
    // Credits "proposed" count is restricted to roles ComicInfo can
    // actually represent. The provider may return `"journalist"` /
    // `"other"` rows that won't land in the per-role CSV cache (which
    // is what `count_credits` reads); without this filter the proposed
    // count overstates by those rows and the diff sticks at "16 → 18"
    // even after a successful apply. See
    // [`crate::metadata::provider::canonicalize_role`] for the mapping.
    let proposed_credits_count = detail
        .credits
        .iter()
        .filter(|c| {
            // Roles arrive at the provider boundary already
            // canonicalized for the high-volume cases (CV `cover` →
            // `CoverArtist`). Treat any role that resolves to a known
            // canonical name as "in scope", plus anything that's
            // already PascalCase canonical.
            crate::metadata::provider::canonicalize_role(&c.role).is_some()
                || matches!(
                    c.role.as_str(),
                    "Writer"
                        | "Penciller"
                        | "Inker"
                        | "Colorist"
                        | "Letterer"
                        | "CoverArtist"
                        | "Editor"
                        | "Translator"
                )
        })
        .count();
    // Each junction row carries the *contents* comparison too: equal
    // counts used to read as "no change", which hid a provider's clean
    // credits behind file-tagged ones of the same length (`"John Doe
    // [15487]"` ×3 vs `"John Doe"` ×3). Compare normalized name sets.
    push_count_row(
        &mut rows,
        MetadataField::Credits,
        "Credits",
        count_credits(&row),
        proposed_credits_count,
        JunctionContents {
            same: issue_junction_signature(&CsvColumns::from(&row), MetadataField::Credits)
                == detail_junction_signature(&detail, MetadataField::Credits),
            low_quality: issue_row_credits_tagged(&CsvColumns::from(&row)),
        },
        &provenance,
        &provenance_full,
        &args,
    );
    for (field, label, csv, incoming_count) in [
        (
            MetadataField::Characters,
            "Characters",
            row.characters.as_deref(),
            detail.characters.len(),
        ),
        (
            MetadataField::Teams,
            "Teams",
            row.teams.as_deref(),
            detail.teams.len(),
        ),
        (
            MetadataField::Locations,
            "Locations",
            row.locations.as_deref(),
            detail.locations.len(),
        ),
        (
            MetadataField::StoryArcs,
            "Story arcs",
            row.story_arc.as_deref(),
            detail.story_arcs.len(),
        ),
        (
            MetadataField::Tags,
            "Tags",
            row.tags.as_deref(),
            detail.tags.len(),
        ),
        (
            MetadataField::Genres,
            "Genres",
            row.genre.as_deref(),
            detail.genres.len(),
        ),
    ] {
        push_count_row(
            &mut rows,
            field,
            label,
            count_csv(csv),
            incoming_count,
            JunctionContents {
                same: issue_junction_signature(&CsvColumns::from(&row), field)
                    == detail_junction_signature(&detail, field),
                low_quality: csv.is_some_and(csv_has_tagger_ids),
            },
            &provenance,
            &provenance_full,
            &args,
        );
    }
    // Variant covers — count the active variant rows in `issue_cover`
    // and the provider's non-empty `variants` Vec. The composer skips
    // entries with no image URL so we match that filter here too.
    let current_variant_count = count_active_variant_rows(&state.db, &entity_id_str).await?;
    let incoming_variant_count = detail
        .variants
        .iter()
        .filter(|v| v.image_url.as_deref().is_some_and(|s| !s.trim().is_empty()))
        .count();
    push_count_row(
        &mut rows,
        MetadataField::CoverVariants,
        "Variant covers",
        current_variant_count,
        incoming_variant_count,
        // Variant rows aren't name-shaped; equal counts stay the heuristic.
        JunctionContents {
            same: current_variant_count == incoming_variant_count,
            low_quality: false,
        },
        &provenance,
        &provenance_full,
        &args,
    );

    let (conflicts, news) =
        classify_external_ids(&state.db, "issue", &entity_id_str, &detail.identifiers).await?;

    let changes_count = rows
        .iter()
        .filter(|r| matches!(r.decision.as_str(), "would_fill" | "would_replace"))
        .count()
        + conflicts.len()
        + news.len();
    Ok(DiffResp {
        run_id: args.run_id,
        ordinal: args.ordinal,
        scope: "issue".to_owned(),
        source: source.as_str().to_owned(),
        source_external_id: detail.source_external_id.unwrap_or_default(),
        rows,
        external_id_conflicts: conflicts,
        external_ids_new: news,
        changes_count,
    })
}

// ───────── helpers ─────────

/// How a junction's current contents relate to the proposal — what the
/// count-shaped diff row can't tell from the two numbers alone.
#[derive(Debug, Clone, Copy)]
pub(crate) struct JunctionContents {
    /// Normalized name sets are equal: nothing to write even in
    /// `replace_all`.
    pub same: bool,
    /// The current names are file-tagged junk (`"John Doe [15487]"`): the
    /// field counts as *missing* for the fill-missing matrix, so a
    /// provider's clean names replace it without `replace_all`.
    pub low_quality: bool,
}

/// Synthetic diff row for an issue's junction / variant fields. Counts
/// stand in for the per-row contents in the *display* (names would be too
/// verbose for a pre-apply preview), but the decision compares the actual
/// normalized name sets ([`JunctionContents::same`]) — equal counts with
/// different names is a change, not `no_change`. Mirrors
/// [`crate::metadata::apply::classify_field`] otherwise.
#[allow(clippy::too_many_arguments)]
fn push_count_row(
    rows: &mut Vec<ScalarDiffRow>,
    field: MetadataField,
    label: &str,
    current_count: usize,
    incoming_count: usize,
    contents: JunctionContents,
    provenance: &HashMap<String, String>,
    provenance_full: &HashMap<String, field_provenance::Model>,
    args: &ApplyArgs,
) {
    let user_set = provenance.get(&field.key()).map(|s| s.as_str()) == Some("user");
    let decision = if incoming_count == 0 {
        crate::metadata::apply::DiffDecision::NoIncomingValue
    } else if user_set && !args.override_user_edits {
        crate::metadata::apply::DiffDecision::BlockedByUser
    } else if current_count == 0 || contents.low_quality {
        crate::metadata::apply::DiffDecision::WouldFill
    } else if contents.same {
        crate::metadata::apply::DiffDecision::NoChange
    } else if matches!(args.mode, crate::metadata::apply::ApplyMode::ReplaceAll) {
        crate::metadata::apply::DiffDecision::WouldReplace
    } else {
        crate::metadata::apply::DiffDecision::SkippedFillMissingHasValue
    };

    let (current_set_by, current_set_at) = provenance_full
        .get(&field.key())
        .map(|p| (Some(p.set_by.clone()), Some(p.set_at.to_rfc3339())))
        .unwrap_or((None, None));
    rows.push(ScalarDiffRow {
        field: field.key(),
        label: label.to_owned(),
        current_value: Some(format_count(current_count)),
        proposed_value: Some(format_count(incoming_count)),
        decision: decision.as_str().to_owned(),
        current_set_by,
        current_set_at,
    });
}

fn format_count(n: usize) -> String {
    match n {
        0 => "none".to_owned(),
        1 => "1 item".to_owned(),
        n => format!("{n} items"),
    }
}

/// Count entries in a comma-separated string field. Empty / NULL → 0.
fn count_csv(csv: Option<&str>) -> usize {
    csv.map(|s| s.split(',').filter(|p| !p.trim().is_empty()).count())
        .unwrap_or(0)
}

/// Aggregate count across the per-role credit columns on the issue
/// row. Mirrors the data the legacy apply path writes through
/// [`writers::set_issue_credits`].
fn count_credits(row: &issue::Model) -> usize {
    count_csv(row.writer.as_deref())
        + count_csv(row.penciller.as_deref())
        + count_csv(row.inker.as_deref())
        + count_csv(row.colorist.as_deref())
        + count_csv(row.letterer.as_deref())
        + count_csv(row.cover_artist.as_deref())
        + count_csv(row.editor.as_deref())
        + count_csv(row.translator.as_deref())
}

// ───────── junction contents ─────────

/// Trim + lowercase, the same key `writers::normalize` uses for person /
/// character dedupe, so the signature agrees with what an apply would
/// upsert.
fn norm_name(s: &str) -> String {
    s.trim().to_lowercase()
}

/// The name-shaped CSV read-cache columns of an issue row — the slice of
/// `issue::Model` the junction comparison reads, borrowed so unit tests
/// can build one without a full row.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CsvColumns<'a> {
    pub writer: Option<&'a str>,
    pub penciller: Option<&'a str>,
    pub inker: Option<&'a str>,
    pub colorist: Option<&'a str>,
    pub letterer: Option<&'a str>,
    pub cover_artist: Option<&'a str>,
    pub editor: Option<&'a str>,
    pub translator: Option<&'a str>,
    pub characters: Option<&'a str>,
    pub teams: Option<&'a str>,
    pub locations: Option<&'a str>,
    pub story_arc: Option<&'a str>,
    pub tags: Option<&'a str>,
    pub genre: Option<&'a str>,
}

impl<'a> From<&'a issue::Model> for CsvColumns<'a> {
    fn from(r: &'a issue::Model) -> Self {
        Self {
            writer: r.writer.as_deref(),
            penciller: r.penciller.as_deref(),
            inker: r.inker.as_deref(),
            colorist: r.colorist.as_deref(),
            letterer: r.letterer.as_deref(),
            cover_artist: r.cover_artist.as_deref(),
            editor: r.editor.as_deref(),
            translator: r.translator.as_deref(),
            characters: r.characters.as_deref(),
            teams: r.teams.as_deref(),
            locations: r.locations.as_deref(),
            story_arc: r.story_arc.as_deref(),
            tags: r.tags.as_deref(),
            genre: r.genre.as_deref(),
        }
    }
}

impl CsvColumns<'_> {
    /// Role columns keyed by the lowercased canonical ComicInfo role
    /// (`coverartist`, not `cover_artist`) so provider roles compare
    /// against the same key.
    fn credit_columns(&self) -> [(&'static str, Option<&str>); 8] {
        [
            ("writer", self.writer),
            ("penciller", self.penciller),
            ("inker", self.inker),
            ("colorist", self.colorist),
            ("letterer", self.letterer),
            ("coverartist", self.cover_artist),
            ("editor", self.editor),
            ("translator", self.translator),
        ]
    }
}

/// Any role CSV column on the issue carries a tagger id.
pub(crate) fn issue_row_credits_tagged(cols: &CsvColumns<'_>) -> bool {
    cols.credit_columns()
        .iter()
        .any(|(_, csv)| csv.is_some_and(csv_has_tagger_ids))
}

/// Does this CSV column carry a tagger id (`"John Doe [15487]"`)?
pub(crate) fn csv_has_tagger_ids(csv: &str) -> bool {
    crate::library::scanner::metadata_rollup::csv_has_tagger_id(csv)
}

/// Separator between entries of a signature — never part of a name.
const SIG_SEP: char = '\u{1f}';

/// The issue's current names for a junction field, as a sorted, normalized
/// signature (`"role|name"` for credits, bare names otherwise). `None`
/// when the column is empty. The CSV read-cache mirrors the junction
/// (`writers::rebuild_series_issue_csv_cache`), so reading it here costs
/// no extra query; names go through the scanner's `split_csv`, so a
/// stale tagger id is dropped before comparing.
pub(crate) fn issue_junction_signature(
    cols: &CsvColumns<'_>,
    field: MetadataField,
) -> Option<String> {
    use crate::library::scanner::metadata_rollup::split_csv;
    let mut set: std::collections::BTreeSet<String> = Default::default();
    match field {
        MetadataField::Credits => {
            for (role, csv) in cols.credit_columns() {
                for name in csv.map(split_csv).unwrap_or_default() {
                    set.insert(format!("{role}|{}", norm_name(&name)));
                }
            }
        }
        _ => {
            let csv = match field {
                MetadataField::Characters => cols.characters,
                MetadataField::Teams => cols.teams,
                MetadataField::Locations => cols.locations,
                MetadataField::StoryArcs => cols.story_arc,
                MetadataField::Tags => cols.tags,
                MetadataField::Genres => cols.genre,
                _ => return None,
            };
            for name in csv.map(split_csv).unwrap_or_default() {
                set.insert(norm_name(&name));
            }
        }
    }
    signature(set)
}

/// The proposal's names for a junction field, in the same shape as
/// [`issue_junction_signature`]. Credits keep only the roles ComicInfo can
/// represent (the same filter the proposed count uses) and key by the
/// canonical role, so `"cover"` and `"CoverArtist"` compare equal.
pub(crate) fn detail_junction_signature(
    detail: &crate::metadata::provider::GenericMetadata,
    field: MetadataField,
) -> Option<String> {
    let mut set: std::collections::BTreeSet<String> = Default::default();
    match field {
        MetadataField::Credits => {
            for c in &detail.credits {
                let canonical = crate::metadata::provider::canonicalize_role(&c.role).or({
                    match c.role.as_str() {
                        "Writer" | "Penciller" | "Inker" | "Colorist" | "Letterer"
                        | "CoverArtist" | "Editor" | "Translator" => Some(c.role.as_str()),
                        _ => None,
                    }
                });
                if let Some(role) = canonical {
                    set.insert(format!("{}|{}", role.to_lowercase(), norm_name(&c.name)));
                }
            }
        }
        MetadataField::Characters => {
            set.extend(detail.characters.iter().map(|e| norm_name(&e.name)));
        }
        MetadataField::Teams => set.extend(detail.teams.iter().map(|e| norm_name(&e.name))),
        MetadataField::Locations => {
            set.extend(detail.locations.iter().map(|e| norm_name(&e.name)));
        }
        MetadataField::StoryArcs => {
            set.extend(detail.story_arcs.iter().map(|e| norm_name(&e.name)));
        }
        MetadataField::Tags => set.extend(detail.tags.iter().map(|s| norm_name(s))),
        MetadataField::Genres => set.extend(detail.genres.iter().map(|s| norm_name(s))),
        _ => return None,
    }
    set.retain(|s| !s.is_empty() && !s.ends_with('|'));
    signature(set)
}

fn signature(set: std::collections::BTreeSet<String>) -> Option<String> {
    (!set.is_empty()).then(|| {
        set.into_iter()
            .collect::<Vec<_>>()
            .join(&SIG_SEP.to_string())
    })
}

/// Count active variant cover rows in `issue_cover` for the issue.
async fn count_active_variant_rows(
    db: &DatabaseConnection,
    issue_id: &str,
) -> Result<usize, sea_orm::DbErr> {
    use entity::issue_cover;
    use sea_orm::PaginatorTrait;
    let n = issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(issue_id))
        .filter(issue_cover::Column::Kind.eq("variant"))
        .filter(issue_cover::Column::IsActive.eq(true))
        .count(db)
        .await?;
    Ok(n as usize)
}

#[allow(clippy::too_many_arguments)]
fn push_scalar(
    rows: &mut Vec<ScalarDiffRow>,
    field: MetadataField,
    label: &str,
    current: Option<&str>,
    incoming: Option<&str>,
    provenance: &HashMap<String, String>,
    provenance_full: &HashMap<String, field_provenance::Model>,
    args: &ApplyArgs,
) {
    let decision = classify_field(current, incoming, provenance, field, args);
    let (current_set_by, current_set_at) = provenance_full
        .get(&field.key())
        .map(|p| (Some(p.set_by.clone()), Some(p.set_at.to_rfc3339())))
        .unwrap_or((None, None));
    rows.push(ScalarDiffRow {
        field: field.key(),
        label: label.to_owned(),
        current_value: current.map(str::to_owned),
        proposed_value: incoming.map(str::to_owned),
        decision: decision.as_str().to_owned(),
        current_set_by,
        current_set_at,
    });
}

#[allow(clippy::too_many_arguments)]
fn push_scalar_int(
    rows: &mut Vec<ScalarDiffRow>,
    field: MetadataField,
    label: &str,
    current: Option<i32>,
    incoming: Option<i32>,
    provenance: &HashMap<String, String>,
    provenance_full: &HashMap<String, field_provenance::Model>,
    args: &ApplyArgs,
) {
    let current_s = current.map(|n| n.to_string());
    let incoming_s = incoming.map(|n| n.to_string());
    push_scalar(
        rows,
        field,
        label,
        current_s.as_deref(),
        incoming_s.as_deref(),
        provenance,
        provenance_full,
        args,
    );
}

pub(crate) async fn fetch_field_provenance_full(
    db: &DatabaseConnection,
    entity_type: &str,
    entity_id: &str,
) -> Result<HashMap<String, field_provenance::Model>, sea_orm::DbErr> {
    let rows = field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq(entity_type))
        .filter(field_provenance::Column::EntityId.eq(entity_id))
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|r| (r.field.clone(), r)).collect())
}

/// Partition the candidate's external_ids into (conflicts, news).
/// A conflict is a row where the candidate's id disagrees with a
/// user-set row for the same source. "News" are sources the entity
/// doesn't have a row for yet — the apply will just add them.
pub(crate) async fn classify_external_ids(
    db: &DatabaseConnection,
    entity_type: &str,
    entity_id: &str,
    incoming: &[crate::metadata::identifier::Identifier],
) -> Result<(Vec<ExternalIdConflictRow>, Vec<ExternalIdNewRow>), sea_orm::DbErr> {
    let existing = writers::fetch_all_external_ids(db, entity_type, entity_id).await?;
    let mut by_source: HashMap<String, external_id::Model> = HashMap::new();
    for r in existing {
        by_source.insert(r.source.clone(), r);
    }
    let mut conflicts = Vec::new();
    let mut news = Vec::new();
    let mut seen: std::collections::HashSet<Source> = std::collections::HashSet::new();
    for id in incoming {
        if !seen.insert(id.source) {
            continue;
        }
        let source_key = id.source.as_str().to_owned();
        match by_source.get(&source_key) {
            Some(existing) if existing.external_id != id.id => {
                // Only surface as a conflict when the existing row is
                // user-pinned — non-user rows get silently updated by
                // apply.
                if existing.set_by == "user" {
                    conflicts.push(ExternalIdConflictRow {
                        source: source_key,
                        current_external_id: existing.external_id.clone(),
                        proposed_external_id: id.id.clone(),
                    });
                }
            }
            Some(_) => {} // same value, no diff
            None => {
                news.push(ExternalIdNewRow {
                    source: source_key,
                    external_id: id.id.clone(),
                });
            }
        }
    }
    Ok((conflicts, news))
}

// Pure unit tests for the classifier — no DB / no provider call.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::apply::{ApplyMode, DiffDecision};

    fn make_args(mode: ApplyMode, override_user: bool) -> ApplyArgs {
        ApplyArgs {
            run_id: Uuid::nil(),
            ordinal: 0,
            mode,
            apply_cover: false,
            cover_overwrite_policy: writers::CoverOverwritePolicy::WhenMissing,
            override_user_edits: override_user,
            actor_id: None,
            selected_fields: None,
            override_external_id_sources: std::collections::HashSet::new(),
        }
    }

    fn credit(name: &str, role: &str) -> crate::metadata::provider::CreditCandidate {
        crate::metadata::provider::CreditCandidate {
            name: name.into(),
            role: role.into(),
            ordinal: None,
            identifiers: vec![],
        }
    }

    #[test]
    fn junction_signatures_compare_names_not_counts() {
        let row = CsvColumns {
            writer: Some("John Doe [15487]"),
            cover_artist: Some("Jane Roe"),
            characters: Some("Batman [1699], Martian Manhunter"),
            ..Default::default()
        };
        assert!(issue_row_credits_tagged(&row));
        assert!(csv_has_tagger_ids(row.characters.unwrap()));

        let detail = crate::metadata::provider::GenericMetadata {
            credits: vec![
                credit("john doe", "Writer"),
                credit("Jane Roe", "cover"),
                // Not a ComicInfo role: ignored, as in the proposed count.
                credit("Someone", "journalist"),
            ],
            characters: vec![
                crate::metadata::provider::EntityCandidate {
                    name: "Martian Manhunter".into(),
                    identifiers: vec![],
                    is_first_appearance: false,
                    died_in_issue: None,
                    disbanded_in_issue: None,
                    position_in_arc: None,
                },
                crate::metadata::provider::EntityCandidate {
                    name: "BATMAN".into(),
                    identifiers: vec![],
                    is_first_appearance: false,
                    died_in_issue: None,
                    disbanded_in_issue: None,
                    position_in_arc: None,
                },
            ],
            ..Default::default()
        };
        // The tagger id is dropped before comparing, roles canonicalize,
        // case and order don't matter: same contents.
        assert_eq!(
            issue_junction_signature(&row, MetadataField::Credits),
            detail_junction_signature(&detail, MetadataField::Credits)
        );
        assert_eq!(
            issue_junction_signature(&row, MetadataField::Characters),
            detail_junction_signature(&detail, MetadataField::Characters)
        );
        // Same count, different person: a change.
        let other = crate::metadata::provider::GenericMetadata {
            credits: vec![
                credit("John Doe", "Writer"),
                credit("Someone Else", "cover"),
            ],
            ..Default::default()
        };
        assert_ne!(
            issue_junction_signature(&row, MetadataField::Credits),
            detail_junction_signature(&other, MetadataField::Credits)
        );
        // Empty either side is None.
        assert_eq!(issue_junction_signature(&row, MetadataField::Teams), None);
        assert_eq!(
            detail_junction_signature(&other, MetadataField::Teams),
            None
        );
    }

    #[test]
    fn count_row_decides_on_contents_and_tagged_names() {
        use crate::metadata::apply::ApplyMode;
        let prov = HashMap::new();
        let full = HashMap::new();
        let decide = |mode, contents| {
            let mut rows = Vec::new();
            push_count_row(
                &mut rows,
                MetadataField::Credits,
                "Credits",
                3,
                3,
                contents,
                &prov,
                &full,
                &make_args(mode, false),
            );
            rows.pop().unwrap().decision
        };
        let same = JunctionContents {
            same: true,
            low_quality: false,
        };
        let differ = JunctionContents {
            same: false,
            low_quality: false,
        };
        let tagged = JunctionContents {
            same: false,
            low_quality: true,
        };
        assert_eq!(decide(ApplyMode::FillMissing, same), "no_change");
        assert_eq!(decide(ApplyMode::ReplaceAll, same), "no_change");
        // Equal counts, different names: no longer "no change".
        assert_eq!(
            decide(ApplyMode::FillMissing, differ),
            "skipped_fill_missing_has_value"
        );
        assert_eq!(decide(ApplyMode::ReplaceAll, differ), "would_replace");
        // Tagged names count as missing in either mode.
        assert_eq!(decide(ApplyMode::FillMissing, tagged), "would_fill");
        assert_eq!(decide(ApplyMode::ReplaceAll, tagged), "would_fill");
    }

    #[test]
    fn classify_returns_would_fill_when_db_empty() {
        let provenance = HashMap::new();
        let d = classify_field(
            None,
            Some("Saga"),
            &provenance,
            MetadataField::Title,
            &make_args(ApplyMode::FillMissing, false),
        );
        assert_eq!(d, DiffDecision::WouldFill);
        assert!(d.would_change());
    }

    #[test]
    fn classify_blocks_user_set_unless_override() {
        let mut provenance = HashMap::new();
        provenance.insert("title".to_owned(), "user".to_owned());
        let d = classify_field(
            Some("My Title"),
            Some("Saga"),
            &provenance,
            MetadataField::Title,
            &make_args(ApplyMode::ReplaceAll, false),
        );
        assert_eq!(d, DiffDecision::BlockedByUser);
        assert!(!d.would_change());

        let d2 = classify_field(
            Some("My Title"),
            Some("Saga"),
            &provenance,
            MetadataField::Title,
            &make_args(ApplyMode::ReplaceAll, true),
        );
        assert_eq!(d2, DiffDecision::WouldReplace);
    }

    #[test]
    fn classify_no_change_when_values_equal() {
        let provenance = HashMap::new();
        let d = classify_field(
            Some("Saga"),
            Some("Saga"),
            &provenance,
            MetadataField::Title,
            &make_args(ApplyMode::ReplaceAll, false),
        );
        assert_eq!(d, DiffDecision::NoChange);
    }

    #[test]
    fn classify_no_incoming_value() {
        let provenance = HashMap::new();
        let d = classify_field(
            Some("Saga"),
            None,
            &provenance,
            MetadataField::Title,
            &make_args(ApplyMode::ReplaceAll, false),
        );
        assert_eq!(d, DiffDecision::NoIncomingValue);
    }

    #[test]
    fn classify_skipped_fill_missing_when_db_has_other_value() {
        let provenance = HashMap::new();
        let d = classify_field(
            Some("Old"),
            Some("New"),
            &provenance,
            MetadataField::Title,
            &make_args(ApplyMode::FillMissing, false),
        );
        assert_eq!(d, DiffDecision::SkippedFillMissingHasValue);
        assert!(!d.would_change());
    }
}
