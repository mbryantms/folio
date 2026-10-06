//! Single audited write surface for metadata.
//!
//! Every metadata-touching code path — scanner, bulk-edit dialog,
//! M4 Apply jobs, manual `<ExternalIdsCard>` edits — funnels through
//! the helpers in this module. Junctions are sole source of truth on
//! writes; CSV columns on `issue` are rebuilt as a denormalized
//! read-cache via [`rebuild_issue_csv_cache`] / [`CsvRebuildBatch`].
//!
//! ## Dedup precedence
//!
//! [`upsert_person`] (and its 9 sibling helpers for character / team /
//! arc / location / concept / object / publisher / imprint / universe)
//! all dedup in the same order:
//!
//! 1. **Identifier match** — if any input [`Identifier`] points at
//!    an existing `external_ids` row for that entity type, use that
//!    entity. Identifiers travel from provider to provider, so
//!    sharing a CV id between a CV response and a Metron response is
//!    the strongest dedup signal.
//! 2. **Normalized-name match** — fall back to `normalized_name` for
//!    rows that arrived without identifiers (ComicInfo CSV credits,
//!    e.g. "Brian Bendis"). This is necessarily lossy when the same
//!    person appears under different spellings ("Brian Michael
//!    Bendis"); the next-best signal is the provider supplying an
//!    identifier in a later call, after which the two get linked.
//! 3. **Create** — neither matches → insert a new row, allocate a
//!    URL-safe slug, and persist every input identifier as an
//!    `external_ids` row so future calls can dedup against it.

use crate::metadata::{Identifier, Source};
use crate::slug::slugify_segment;
use entity::{
    character, concept, external_id, field_provenance, imprint, issue, issue_arc, issue_character,
    issue_concept, issue_cover, issue_credit, issue_genre, issue_location, issue_object,
    issue_reprint, issue_tag, issue_team, issue_universe, location, object, person, publisher,
    series, series_arc, series_character, series_concept, series_location, series_object,
    series_team, series_universe, story_arc, team, universe,
};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend, DbErr,
    EntityTrait, ExprTrait, FromQueryResult, QueryFilter, QueryOrder, QuerySelect, Statement,
    TransactionTrait,
};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────
// Provenance enum — the `set_by` column on external_ids /
// field_provenance.
// ─────────────────────────────────────────────────────────────────

/// Who set this row. Stored as TEXT, serialized via [`Self::as_str`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SetBy {
    User,
    ComicInfo,
    MetronInfo,
    SeriesJson,
    Provider(Source),
    ScannerInference,
    ScannerFolderTag,
    CrossReference,
}

impl SetBy {
    pub fn as_str(self) -> String {
        match self {
            SetBy::User => "user".into(),
            SetBy::ComicInfo => "comicinfo".into(),
            SetBy::MetronInfo => "metroninfo".into(),
            SetBy::SeriesJson => "series_json".into(),
            SetBy::Provider(s) => s.as_str().into(),
            SetBy::ScannerInference => "scanner_inference".into(),
            SetBy::ScannerFolderTag => "scanner_folder_tag".into(),
            SetBy::CrossReference => "cross_reference".into(),
        }
    }

    /// File/scanner-derived codes — the weakest attribution tier. A
    /// re-ingest of a file can't know who put the data *in* the file,
    /// so these codes may refresh each other but never overwrite a
    /// `user` or provider row (see `write_file_field_provenance`).
    pub fn is_file_sourced(self) -> bool {
        matches!(
            self,
            SetBy::ComicInfo
                | SetBy::MetronInfo
                | SetBy::SeriesJson
                | SetBy::ScannerInference
                | SetBy::ScannerFolderTag
        )
    }
}

/// The `set_by` codes `write_file_field_provenance` is allowed to
/// overwrite — kept as a slice so the guard's SQL `IN` list and
/// `SetBy::is_file_sourced` can't drift apart.
pub(crate) const FILE_SOURCED_SET_BY: [&str; 5] = [
    "comicinfo",
    "metroninfo",
    "series_json",
    "scanner_inference",
    "scanner_folder_tag",
];

/// True when a stored `set_by` code is a file-tier source (the weakest
/// attribution tier: **user > provider > file**). Anything else — `user`
/// or a provider name — protects its column against a re-ingest.
pub fn is_file_tier_set_by(set_by: &str) -> bool {
    FILE_SOURCED_SET_BY.contains(&set_by)
}

/// One provenance row as the scanner's tier gate sees it: who set the
/// field and when. `set_at` lets a writeback library tell a provider
/// value the archive's XML already carries (recorded at or before the
/// last sidecar rewrite) from one it doesn't.
#[derive(Clone, Debug)]
pub struct ProvenanceTier {
    pub set_by: String,
    pub set_at: chrono::DateTime<chrono::FixedOffset>,
}

/// `field key → (set_by, set_at)` for every provenance row on an entity.
/// Generic over the connection so the scanner can call it inside its
/// ingest transaction (the apply path has its own `DatabaseConnection`
/// copy).
pub async fn fetch_field_provenance_tiers<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
) -> Result<std::collections::HashMap<String, ProvenanceTier>, DbErr> {
    let rows = field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq(entity_type))
        .filter(field_provenance::Column::EntityId.eq(entity_id))
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.field,
                ProvenanceTier {
                    set_by: r.set_by,
                    set_at: r.set_at,
                },
            )
        })
        .collect())
}

// ─────────────────────────────────────────────────────────────────
// Cover overwrite policy.
// ─────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CoverOverwritePolicy {
    Never,
    WhenMissing,
    Always,
}

// ─────────────────────────────────────────────────────────────────
// Generic slug allocation helper — used by every `upsert_*`.
// Avoids the per-entity SlugAllocator boilerplate by issuing a
// parameterised SELECT against the entity's table.
// ─────────────────────────────────────────────────────────────────

#[derive(FromQueryResult)]
struct Exists {
    #[allow(dead_code)]
    exists: i32,
}

async fn unique_slug<C: ConnectionTrait>(
    db: &C,
    table: &'static str,
    base: &str,
) -> Result<String, DbErr> {
    let base_slug = slugify_segment(base);
    let mut candidate = base_slug.clone();
    let mut n: u32 = 2;
    loop {
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            // SAFETY: `table` is a `&'static str` from the call sites
            // below (every literal in this file), never user-supplied,
            // so no SQL injection. `candidate` is bound.
            format!("SELECT 1 AS exists FROM {table} WHERE slug = $1 LIMIT 1").as_str(),
            [candidate.clone().into()],
        );
        if Exists::find_by_statement(stmt).one(db).await?.is_none() {
            return Ok(candidate);
        }
        candidate = format!("{base_slug}-{n}");
        n += 1;
    }
}

/// Trim + lowercase. Identical to the SQL `btrim(lower(...))` the M0
/// migration uses for backfill, so application-layer + DB-layer
/// dedup agree.
fn normalize(s: &str) -> String {
    s.trim().to_lowercase()
}

// ─────────────────────────────────────────────────────────────────
// External-ID helpers — used both by the public `set_external_id`
// surface and internally by every `upsert_*`.
// ─────────────────────────────────────────────────────────────────

/// Look up an entity by `(source, external_id)` for a given entity
/// type. Returns the entity_id text — interpret as `Uuid::parse_str`
/// for UUID-keyed tables, or use raw for BLAKE3 issue ids.
async fn lookup_by_identifier<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    identifier: &Identifier,
) -> Result<Option<String>, DbErr> {
    let row = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq(entity_type))
        .filter(external_id::Column::Source.eq(identifier.source.as_str()))
        .filter(external_id::Column::ExternalId.eq(&identifier.id))
        .one(db)
        .await?;
    Ok(row.map(|r| r.entity_id))
}

/// Outcome of a single external-ID write. A provider ID can only ever
/// belong to one entity (the `external_ids_source_external_id_entity_type_key`
/// unique), so a write either lands cleanly, reclaims the ID from an
/// owner that's since been removed, or is skipped because a **live**
/// entity still holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetExternalIdOutcome {
    /// Written (or refreshed in place for the same entity).
    Set,
    /// The ID was held by an entity that's gone/removed; we reassigned
    /// it to this entity. `from` is the prior owner's `entity_id`.
    Reclaimed { from: String },
    /// A still-live entity owns this ID; nothing was written. `owner`
    /// is that entity's id. Callers decide how to surface it (the
    /// scanner raises a health finding; a user add returns 409).
    SkippedConflict { owner: String },
    /// A `set_by='user'` row exists and the caller did not override:
    /// the user's value and claim were kept. `same_value` is true when
    /// the incoming id already matched (only `last_synced_at` was
    /// refreshed), false when the caller's differing value was dropped.
    KeptUserValue { same_value: bool },
    /// A provider-set row exists and the caller is a file-tier source
    /// (ComicInfo / MetronInfo re-ingest): the provider's value was kept
    /// (decision D4 — file values never replace provider values on
    /// rescan). Same `same_value` semantics as [`Self::KeptUserValue`].
    KeptProviderValue { same_value: bool },
}

/// One provider ID that [`set_legacy_id_trio`] /
/// [`write_metroninfo_external_ids`] couldn't write because a live
/// entity already owns it. Surfaced by the scanner as a health finding.
#[derive(Debug, Clone)]
pub struct SkippedExternalId {
    pub source: Source,
    pub external_id: String,
    pub owner: String,
}

/// Is the entity currently holding an ID eligible to lose it? True when
/// the owning issue/series is gone or soft-removed (`removed_at` set) —
/// the duplicate-file case where the survivor should reclaim the ID.
/// Other entity types are never auto-reclaimed (returns false → skip).
async fn owner_is_reclaimable<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    owner_entity_id: &str,
) -> Result<bool, DbErr> {
    match entity_type {
        "issue" => Ok(issue::Entity::find_by_id(owner_entity_id.to_owned())
            .one(db)
            .await?
            .is_none_or(|r| r.removed_at.is_some())),
        "series" => {
            let Ok(uuid) = Uuid::parse_str(owner_entity_id) else {
                return Ok(false);
            };
            Ok(series::Entity::find_by_id(uuid)
                .one(db)
                .await?
                .is_none_or(|r| r.removed_at.is_some()))
        }
        _ => Ok(false),
    }
}

/// Internal: write the `external_ids` row, computing the canonical
/// URL when the caller didn't supply one. Honors set_by precedence
/// — never overwrites a `set_by='user'` row with a non-user write.
///
/// Also enforces the cross-entity unique `(source, external_id,
/// entity_type)` *before* inserting (a `SELECT`, not a caught
/// constraint error — the latter poisons the surrounding scan
/// transaction): if a different entity already owns the ID, reclaim it
/// when that owner is gone/removed, else skip and report the conflict.
async fn put_external_id<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
    identifier: &Identifier,
    set_by: SetBy,
    override_user: bool,
) -> Result<SetExternalIdOutcome, DbErr> {
    put_external_id_promoting(
        db,
        entity_type,
        entity_id,
        identifier,
        set_by,
        override_user,
    )
    .await
    .map(|(outcome, _)| outcome)
}

/// [`put_external_id`], also returning how many external links the
/// promotion hook turned into series relationships (WP-8.2).
async fn put_external_id_promoting<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
    identifier: &Identifier,
    set_by: SetBy,
    override_user: bool,
) -> Result<(SetExternalIdOutcome, usize), DbErr> {
    let url = identifier.url.clone().or_else(|| {
        crate::metadata::identifier::canonical_url(identifier.source, entity_type, &identifier.id)
    });
    let now = chrono::Utc::now().fixed_offset();

    // A `set_by='user'` row is never replaced by a non-user write
    // unless the caller explicitly overrides (the conflict pane's "Use
    // theirs" or an admin force-apply). When the incoming value already
    // matches, only `last_synced_at` is refreshed — the row keeps
    // `set_by='user'`. (A matching write used to fall through to the
    // upsert and rewrite `set_by`, silently demoting the user's claim;
    // audit DI-2.)
    //
    // The same shape protects a provider-set row from a file-tier
    // re-ingest (decision D4, roadmap WP-2.5): attribution strength is
    // user > provider > file, and a weaker tier never replaces a
    // stronger one.
    if !override_user
        && let Some(existing) = external_id::Entity::find()
            .filter(external_id::Column::EntityType.eq(entity_type))
            .filter(external_id::Column::EntityId.eq(entity_id))
            .filter(external_id::Column::Source.eq(identifier.source.as_str()))
            .one(db)
            .await?
    {
        let existing_is_user = existing.set_by == SetBy::User.as_str();
        let existing_is_provider = !existing_is_user && !is_file_tier_set_by(&existing.set_by);
        let kept_by_user = existing_is_user && set_by != SetBy::User;
        let kept_by_provider = existing_is_provider && set_by.is_file_sourced();
        if kept_by_user || kept_by_provider {
            let same_value = existing.external_id == identifier.id;
            if same_value {
                let mut am: external_id::ActiveModel = existing.into();
                am.last_synced_at = Set(now);
                am.update(db).await?;
            } else {
                tracing::debug!(
                    entity_type = entity_type,
                    entity_id = entity_id,
                    source = identifier.source.as_str(),
                    kept_tier = if kept_by_user { "user" } else { "provider" },
                    "skipping external_id write: a stronger tier owns this row"
                );
            }
            return Ok((
                if kept_by_user {
                    SetExternalIdOutcome::KeptUserValue { same_value }
                } else {
                    SetExternalIdOutcome::KeptProviderValue { same_value }
                },
                0,
            ));
        }
    }

    // Cross-entity unique: does a *different* entity already own this
    // (source, external_id, entity_type)? Reclaim from a removed owner,
    // otherwise skip so the caller can surface the duplicate.
    let mut reclaimed_from: Option<String> = None;
    if let Some(owner) = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq(entity_type))
        .filter(external_id::Column::Source.eq(identifier.source.as_str()))
        .filter(external_id::Column::ExternalId.eq(&identifier.id))
        .one(db)
        .await?
        && owner.entity_id != entity_id
    {
        if owner_is_reclaimable(db, entity_type, &owner.entity_id).await? {
            external_id::Entity::delete_many()
                .filter(external_id::Column::EntityType.eq(entity_type))
                .filter(external_id::Column::Source.eq(identifier.source.as_str()))
                .filter(external_id::Column::ExternalId.eq(&identifier.id))
                .exec(db)
                .await?;
            reclaimed_from = Some(owner.entity_id);
        } else {
            return Ok((
                SetExternalIdOutcome::SkippedConflict {
                    owner: owner.entity_id,
                },
                0,
            ));
        }
    }

    let am = external_id::ActiveModel {
        entity_type: Set(entity_type.into()),
        entity_id: Set(entity_id.into()),
        source: Set(identifier.source.as_str().into()),
        external_id: Set(identifier.id.clone()),
        external_url: Set(url),
        set_by: Set(set_by.as_str()),
        first_set_at: Set(now),
        last_synced_at: Set(now),
    };
    external_id::Entity::insert(am)
        .on_conflict(
            OnConflict::columns([
                external_id::Column::EntityType,
                external_id::Column::EntityId,
                external_id::Column::Source,
            ])
            .update_columns([
                external_id::Column::ExternalId,
                external_id::Column::ExternalUrl,
                external_id::Column::SetBy,
                external_id::Column::LastSyncedAt,
            ])
            .to_owned(),
        )
        .exec(db)
        .await?;
    // WP-7.8 promotion hooks: a provider id just landed on a local entity,
    // so anything that was waiting for that provider record resolves now —
    // external series links (`series_external_relationship`) and label-only
    // reprints. The suggestion run repeats both per library, so a missed
    // hook only delays them. WP-8.2: the pairs created are returned so a
    // caller holding an `AppState` can drop the similar-series cache.
    let mut promoted_pairs = 0;
    match entity_type {
        "series" => {
            if let Ok(series_id) = Uuid::parse_str(entity_id) {
                promoted_pairs = crate::relationships::external::promote_for_provider_id(
                    db,
                    series_id,
                    identifier.source.as_str(),
                    &identifier.id,
                )
                .await?
                .pairs_created;
            }
        }
        "issue" => {
            resolve_pending_reprints(
                db,
                None,
                Some((identifier.source.as_str(), identifier.id.as_str())),
            )
            .await?;
        }
        _ => {}
    }
    Ok((
        match reclaimed_from {
            Some(from) => SetExternalIdOutcome::Reclaimed { from },
            None => SetExternalIdOutcome::Set,
        },
        promoted_pairs,
    ))
}

/// Public surface for external-ID writes — used by the
/// `<ExternalIdsCard>` CRUD endpoints (M5) and Apply jobs (M4).
pub async fn set_external_id<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
    identifier: &Identifier,
    set_by: SetBy,
) -> Result<SetExternalIdOutcome, DbErr> {
    put_external_id(db, entity_type, entity_id, identifier, set_by, false).await
}

/// [`set_external_id`], also returning how many external links
/// (`series_external_relationship` user rows) the promotion hook turned
/// into series relationships. WP-8.2: the hook has no `AppState`, so a
/// caller that holds one calls `state.similarity.invalidate_all()` when
/// this is non-zero — a new edge is a similar-series signal.
pub async fn set_external_id_promoting<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
    identifier: &Identifier,
    set_by: SetBy,
) -> Result<(SetExternalIdOutcome, usize), DbErr> {
    put_external_id_promoting(db, entity_type, entity_id, identifier, set_by, false).await
}

/// [`set_external_id`] with an explicit user-precedence override.
/// `override_user = true` lets a non-user write replace a
/// `set_by='user'` row — reserved for the apply path when the user
/// chose "Use theirs" on a conflict or an admin forced the apply.
pub async fn set_external_id_with_override<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
    identifier: &Identifier,
    set_by: SetBy,
    override_user: bool,
) -> Result<SetExternalIdOutcome, DbErr> {
    put_external_id(
        db,
        entity_type,
        entity_id,
        identifier,
        set_by,
        override_user,
    )
    .await
}

/// Convenience for the legacy `comicvine_id` + `metron_id` + `gtin`
/// trio that used to live as fixed columns on `series` + `issues`.
/// Used by the scanner (ComicInfo parse), bulk-edit dialog, and
/// per-row PATCH endpoints — every legacy entry-point that knows
/// only these three sources.
///
/// Each input that is `Some` becomes one `external_ids` row. The
/// `set_by` precedence rules from [`set_external_id`] apply per row
/// (user-set values are never silently overwritten).
pub async fn set_legacy_id_trio<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
    comicvine: Option<i64>,
    metron: Option<i64>,
    gtin: Option<&str>,
    set_by: SetBy,
) -> Result<Vec<SkippedExternalId>, DbErr> {
    let mut skipped = Vec::new();
    if let Some(cv) = comicvine {
        let id = cv.to_string();
        if let SetExternalIdOutcome::SkippedConflict { owner } = set_external_id(
            db,
            entity_type,
            entity_id,
            &Identifier::new(Source::ComicVine, id.clone()),
            set_by,
        )
        .await?
        {
            skipped.push(SkippedExternalId {
                source: Source::ComicVine,
                external_id: id,
                owner,
            });
        }
    }
    if let Some(m) = metron {
        let id = m.to_string();
        if let SetExternalIdOutcome::SkippedConflict { owner } = set_external_id(
            db,
            entity_type,
            entity_id,
            &Identifier::new(Source::Metron, id.clone()),
            set_by,
        )
        .await?
        {
            skipped.push(SkippedExternalId {
                source: Source::Metron,
                external_id: id,
                owner,
            });
        }
    }
    if let Some(g) = gtin
        && !g.is_empty()
        && let SetExternalIdOutcome::SkippedConflict { owner } = set_external_id(
            db,
            entity_type,
            entity_id,
            &Identifier::new(Source::Gtin, g),
            set_by,
        )
        .await?
    {
        skipped.push(SkippedExternalId {
            source: Source::Gtin,
            external_id: g.to_string(),
            owner,
        });
    }
    Ok(skipped)
}

/// Inverse of [`set_legacy_id_trio`] — query the trio for a given
/// `(entity_type, entity_id)` so legacy API response shapes that
/// expose `comicvine_id` / `metron_id` / `gtin` keep working
/// through the M0→M4 transition. Apply jobs and the new
/// `<ExternalIdsCard>` payload should read [`fetch_all_external_ids`]
/// instead, which returns the full list.
pub async fn fetch_legacy_id_trio<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
) -> Result<(Option<i64>, Option<i64>, Option<String>), DbErr> {
    let rows = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq(entity_type))
        .filter(external_id::Column::EntityId.eq(entity_id))
        .all(db)
        .await?;
    let mut cv = None;
    let mut metron = None;
    let mut gtin = None;
    for row in rows {
        match row.source.as_str() {
            "comicvine" => cv = row.external_id.parse::<i64>().ok(),
            "metron" => metron = row.external_id.parse::<i64>().ok(),
            "gtin" => gtin = Some(row.external_id),
            _ => {}
        }
    }
    Ok((cv, metron, gtin))
}

/// Fetch every external identifier for `(entity_type, entity_id)`.
/// Used by the M5 `<ExternalIdsCard>` GET endpoint.
pub async fn fetch_all_external_ids<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
) -> Result<Vec<external_id::Model>, DbErr> {
    external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq(entity_type))
        .filter(external_id::Column::EntityId.eq(entity_id))
        .all(db)
        .await
}

/// Delete one (entity, source) external-id row. Used by PATCH
/// handlers when the user explicitly clears a field (`gtin: null`,
/// `comicvine_id: null`, etc. in the request body's double-`Option`
/// shape) and by the M5 `<ExternalIdsCard>` "unlink" action.
pub async fn delete_external_id<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
    source: Source,
) -> Result<(), DbErr> {
    external_id::Entity::delete_many()
        .filter(external_id::Column::EntityType.eq(entity_type))
        .filter(external_id::Column::EntityId.eq(entity_id))
        .filter(external_id::Column::Source.eq(source.as_str()))
        .exec(db)
        .await?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// Field provenance — small helper used everywhere a write touches
// a field that should survive future Apply jobs.
// ─────────────────────────────────────────────────────────────────

pub async fn write_field_provenance<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
    field: crate::metadata::MetadataField,
    set_by: SetBy,
    source_external_id: Option<String>,
) -> Result<(), DbErr> {
    write_field_provenance_at(
        db,
        entity_type,
        entity_id,
        field,
        set_by,
        source_external_id,
        chrono::Utc::now().fixed_offset(),
    )
    .await
}

/// [`write_field_provenance`] with an explicit `set_at`. The sidecar
/// rewrite job records an apply's provider rows at the archive's
/// `last_sidecar_rewrite_at`, so the scanner can see the XML it is about
/// to ingest carries exactly those values (`scanner::process` tier gate).
pub async fn write_field_provenance_at<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
    field: crate::metadata::MetadataField,
    set_by: SetBy,
    source_external_id: Option<String>,
    set_at: chrono::DateTime<chrono::FixedOffset>,
) -> Result<(), DbErr> {
    let am = field_provenance::ActiveModel {
        entity_type: Set(entity_type.into()),
        entity_id: Set(entity_id.into()),
        field: Set(field.key()),
        set_by: Set(set_by.as_str()),
        set_at: Set(set_at),
        source_external_id: Set(source_external_id),
    };
    field_provenance::Entity::insert(am)
        .on_conflict(
            OnConflict::columns([
                field_provenance::Column::EntityType,
                field_provenance::Column::EntityId,
                field_provenance::Column::Field,
            ])
            .update_columns([
                field_provenance::Column::SetBy,
                field_provenance::Column::SetAt,
                field_provenance::Column::SourceExternalId,
            ])
            .to_owned(),
        )
        .exec(db)
        .await?;
    Ok(())
}

/// Scanner-ingest provenance: batch-upsert file-sourced rows for every
/// `(field, set_by)` pair, refusing to overwrite rows whose `set_by`
/// outranks a file re-ingest. Attribution strength is
/// **user > provider > file**: a `user` pin or a provider apply knows
/// *who* chose the value; re-reading the file only proves the value is
/// in the file. The guard is the `DO UPDATE ... WHERE set_by IN (file
/// codes)` clause, so the precedence rule holds atomically even when a
/// writeback-triggered rescan races the apply job that wrote the
/// provider rows.
///
/// Every `set_by` passed here must satisfy [`SetBy::is_file_sourced`]
/// (debug-asserted) — provider/user writes go through
/// [`write_field_provenance`], which overwrites unconditionally.
pub async fn write_file_field_provenance<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
    pairs: &[(crate::metadata::MetadataField, SetBy)],
) -> Result<(), DbErr> {
    if pairs.is_empty() {
        return Ok(());
    }
    debug_assert!(pairs.iter().all(|(_, sb)| sb.is_file_sourced()));
    let now = chrono::Utc::now().fixed_offset();
    let models = pairs
        .iter()
        .map(|(field, set_by)| field_provenance::ActiveModel {
            entity_type: Set(entity_type.into()),
            entity_id: Set(entity_id.into()),
            field: Set(field.key()),
            set_by: Set(set_by.as_str()),
            set_at: Set(now),
            source_external_id: Set(None),
        });
    field_provenance::Entity::insert_many(models)
        .on_conflict(
            OnConflict::columns([
                field_provenance::Column::EntityType,
                field_provenance::Column::EntityId,
                field_provenance::Column::Field,
            ])
            .update_columns([
                field_provenance::Column::SetBy,
                field_provenance::Column::SetAt,
                field_provenance::Column::SourceExternalId,
            ])
            .action_and_where(
                sea_orm::sea_query::Expr::col((
                    field_provenance::Entity,
                    field_provenance::Column::SetBy,
                ))
                .is_in(FILE_SOURCED_SET_BY),
            )
            .to_owned(),
        )
        .exec_without_returning(db)
        .await?;
    Ok(())
}

/// Companion to [`write_file_field_provenance`]: drop file-sourced rows
/// for fields the freshly-parsed sidecar no longer carries, so the
/// provenance table doesn't keep describing a column the same ingest
/// just nulled out. Same precedence guard — `user`/provider rows are
/// never deleted by a scan.
pub async fn delete_file_field_provenance<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
    fields: &[crate::metadata::MetadataField],
) -> Result<(), DbErr> {
    if fields.is_empty() {
        return Ok(());
    }
    field_provenance::Entity::delete_many()
        .filter(field_provenance::Column::EntityType.eq(entity_type))
        .filter(field_provenance::Column::EntityId.eq(entity_id))
        .filter(field_provenance::Column::Field.is_in(fields.iter().map(|f| f.key())))
        .filter(field_provenance::Column::SetBy.is_in(FILE_SOURCED_SET_BY))
        .exec(db)
        .await?;
    Ok(())
}

/// Field keys with a `set_by='user'` provenance row — the scanner's
/// user-precedence source. Generic over [`ConnectionTrait`] (unlike
/// `sidecar_compose::load_user_pins`) so it runs inside the scanner's
/// per-batch transaction handle.
pub async fn fetch_user_pinned_fields<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
) -> Result<HashSet<String>, DbErr> {
    let rows = field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq(entity_type))
        .filter(field_provenance::Column::EntityId.eq(entity_id))
        .filter(field_provenance::Column::SetBy.eq("user"))
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|r| r.field).collect())
}

// ─────────────────────────────────────────────────────────────────
// upsert_* — 10 helpers, one per top-level entity. Each follows the
// identifier-first dedup precedence documented at module-level.
// ─────────────────────────────────────────────────────────────────

/// Macro-free upsert template invoked by each typed wrapper. Returns
/// the entity's UUID. Identifier rows are inserted (or refreshed)
/// for every input [`Identifier`].
///
/// `..Default::default()` is intentional: entities with nullable
/// extras (publisher.founded_year, character.real_name,
/// story_arc.publisher_id, universe.publisher_id) get those fields
/// initialised to `NotSet` so the DB default applies. For entities
/// without extras (team, location, concept, object), the trailing
/// update is a no-op — silenced below.
macro_rules! upsert_entity_helper {
    (
        $fn_name:ident,
        entity = $module:ident,
        entity_type = $entity_type:literal,
        table = $table:literal,
    ) => {
        #[allow(clippy::needless_update)]
        pub async fn $fn_name<C: ConnectionTrait>(
            db: &C,
            name: &str,
            identifiers: &[Identifier],
            set_by: SetBy,
        ) -> Result<Uuid, DbErr> {
            // 1. Identifier match (strongest dedup signal).
            for ident in identifiers {
                if let Some(existing_id) = lookup_by_identifier(db, $entity_type, ident).await? {
                    let uuid = Uuid::parse_str(&existing_id).map_err(|e| {
                        DbErr::Custom(format!(
                            "{} external_ids.entity_id is not a UUID: {e}",
                            $entity_type
                        ))
                    })?;
                    // Refresh / add identifiers we may not have seen.
                    for ident in identifiers {
                        put_external_id(db, $entity_type, &existing_id, ident, set_by, false)
                            .await?;
                    }
                    return Ok(uuid);
                }
            }
            // 2. Normalized-name match.
            let normalized = normalize(name);
            if let Some(row) = $module::Entity::find()
                .filter($module::Column::NormalizedName.eq(&normalized))
                .one(db)
                .await?
            {
                let entity_id_str = row.id.to_string();
                for ident in identifiers {
                    put_external_id(db, $entity_type, &entity_id_str, ident, set_by, false).await?;
                }
                return Ok(row.id);
            }
            // 3. Create.
            let id = Uuid::now_v7();
            let slug = unique_slug(db, $table, name).await?;
            let now = chrono::Utc::now().fixed_offset();
            let am = $module::ActiveModel {
                id: Set(id),
                slug: Set(slug),
                name: Set(name.to_owned()),
                normalized_name: Set(normalized),
                aliases: Set(serde_json::json!([])),
                description: Set(None),
                image_url: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            };
            am.insert(db).await?;
            for ident in identifiers {
                put_external_id(db, $entity_type, &id.to_string(), ident, set_by, false).await?;
            }
            Ok(id)
        }
    };
}

upsert_entity_helper!(
    upsert_person,
    entity = person,
    entity_type = "person",
    table = "person",
);
upsert_entity_helper!(
    upsert_character,
    entity = character,
    entity_type = "character",
    table = "character",
);
upsert_entity_helper!(
    upsert_team,
    entity = team,
    entity_type = "team",
    table = "team",
);
upsert_entity_helper!(
    upsert_story_arc,
    entity = story_arc,
    entity_type = "story_arc",
    table = "story_arc",
);
upsert_entity_helper!(
    upsert_location,
    entity = location,
    entity_type = "location",
    table = "location",
);
upsert_entity_helper!(
    upsert_concept,
    entity = concept,
    entity_type = "concept",
    table = "concept",
);
upsert_entity_helper!(
    upsert_object,
    entity = object,
    entity_type = "object",
    table = "object",
);
upsert_entity_helper!(
    upsert_publisher,
    entity = publisher,
    entity_type = "publisher",
    table = "publisher",
);
upsert_entity_helper!(
    upsert_universe,
    entity = universe,
    entity_type = "universe",
    table = "universe",
);

// ─────────────────────────────────────────────────────────────────
// ensure_series_entity_rows — WP-5.5 entity landing pages.
// ─────────────────────────────────────────────────────────────────

/// `(entity table, SELECT of candidate display names for one series)`.
/// Each SELECT binds the series id as `$1` and yields `nm`; names that
/// already resolve (by `normalized_name`) are filtered out in SQL so
/// the common case — every name known — costs one query per table and
/// zero inserts.
const SERIES_ENTITY_NAME_SOURCES: &[(&str, &str)] = &[
    (
        "character",
        "SELECT DISTINCT j.character AS nm FROM issue_characters j \
         JOIN issues i ON i.id = j.issue_id \
         WHERE i.series_id = $1 AND i.state = 'active' AND i.removed_at IS NULL \
           AND NOT EXISTS (SELECT 1 FROM character e \
                           WHERE e.normalized_name = btrim(lower(j.character)))",
    ),
    (
        "team",
        "SELECT DISTINCT j.team AS nm FROM issue_teams j \
         JOIN issues i ON i.id = j.issue_id \
         WHERE i.series_id = $1 AND i.state = 'active' AND i.removed_at IS NULL \
           AND NOT EXISTS (SELECT 1 FROM team e \
                           WHERE e.normalized_name = btrim(lower(j.team)))",
    ),
    (
        "story_arc",
        // Same split rule as `metadata_rollup::split_csv`.
        "SELECT DISTINCT btrim(x.nm) AS nm FROM issues i \
         CROSS JOIN LATERAL regexp_split_to_table(i.story_arc, \
             CASE WHEN i.story_arc LIKE '%;%' THEN ';' ELSE ',' END) AS x(nm) \
         WHERE i.series_id = $1 AND i.state = 'active' AND i.removed_at IS NULL \
           AND i.story_arc IS NOT NULL AND i.story_arc <> '' \
           AND NOT EXISTS (SELECT 1 FROM story_arc e \
                           WHERE e.normalized_name = btrim(lower(x.nm)))",
    ),
    (
        "publisher",
        "SELECT s.publisher AS nm FROM series s \
         WHERE s.id = $1 AND s.publisher IS NOT NULL \
           AND NOT EXISTS (SELECT 1 FROM publisher e \
                           WHERE e.normalized_name = btrim(lower(s.publisher)))",
    ),
];

/// Ensure a `character` / `team` / `story_arc` / `publisher` row exists
/// for every name this series' active issues (or the series row, for the
/// publisher) carry, so each name has a slug for its landing page
/// (`/characters/{slug}` …, WP-5.5). Mirrors the scanner rollup's
/// `ensure_persons_for_series` for creators.
///
/// Writes **entity rows only**: junction rows, their FK columns and
/// `field_provenance` are untouched — the read side resolves a junction
/// row to its entity by FK, or by `normalized_name` while the FK is NULL.
/// Inserts use `ON CONFLICT DO NOTHING` (both the `slug` and the
/// `normalized_name` unique constraints) so concurrent series rollups
/// racing on a shared name ("Batman") never error; a name that loses a
/// slug race is simply picked up by the next rollup.
pub async fn ensure_series_entity_rows<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
) -> Result<(), DbErr> {
    #[derive(FromQueryResult)]
    struct NameRow {
        nm: Option<String>,
    }
    let backend = db.get_database_backend();
    for &(table, sql) in SERIES_ENTITY_NAME_SOURCES {
        let rows = NameRow::find_by_statement(Statement::from_sql_and_values(
            backend,
            sql,
            [series_id.into()],
        ))
        .all(db)
        .await?;
        let mut seen = HashSet::<String>::new();
        for row in rows {
            let Some(raw) = row.nm else { continue };
            let display = raw.trim();
            let normalized = normalize(display);
            if normalized.is_empty() || !seen.insert(normalized.clone()) {
                continue;
            }
            let slug = unique_slug(db, table, display).await?;
            db.execute_raw(Statement::from_sql_and_values(
                backend,
                // SAFETY: `table` is a `&'static str` literal from
                // SERIES_ENTITY_NAME_SOURCES, never user input.
                format!(
                    "INSERT INTO {table} (slug, name, normalized_name) \
                     VALUES ($1, $2, $3) ON CONFLICT DO NOTHING"
                ),
                [slug.into(), display.to_owned().into(), normalized.into()],
            ))
            .await?;
        }
    }
    Ok(())
}

// Imprint is special — requires a publisher_id parent. Hand-rolled.
pub async fn upsert_imprint<C: ConnectionTrait>(
    db: &C,
    name: &str,
    publisher_id: Uuid,
    identifiers: &[Identifier],
    set_by: SetBy,
) -> Result<Uuid, DbErr> {
    for ident in identifiers {
        if let Some(existing_id) = lookup_by_identifier(db, "imprint", ident).await? {
            let uuid = Uuid::parse_str(&existing_id).map_err(|e| {
                DbErr::Custom(format!("imprint external_ids.entity_id not a UUID: {e}"))
            })?;
            for ident in identifiers {
                put_external_id(db, "imprint", &existing_id, ident, set_by, false).await?;
            }
            return Ok(uuid);
        }
    }
    let normalized = normalize(name);
    if let Some(row) = imprint::Entity::find()
        .filter(imprint::Column::NormalizedName.eq(&normalized))
        .one(db)
        .await?
    {
        let entity_id_str = row.id.to_string();
        for ident in identifiers {
            put_external_id(db, "imprint", &entity_id_str, ident, set_by, false).await?;
        }
        return Ok(row.id);
    }
    let id = Uuid::now_v7();
    let slug = unique_slug(db, "imprint", name).await?;
    let now = chrono::Utc::now().fixed_offset();
    imprint::ActiveModel {
        id: Set(id),
        slug: Set(slug),
        name: Set(name.to_owned()),
        normalized_name: Set(normalized),
        aliases: Set(serde_json::json!([])),
        description: Set(None),
        image_url: Set(None),
        publisher_id: Set(publisher_id),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await?;
    for ident in identifiers {
        put_external_id(db, "imprint", &id.to_string(), ident, set_by, false).await?;
    }
    Ok(id)
}

// ─────────────────────────────────────────────────────────────────
// Junction set helpers — `set_issue_*` and `set_series_*` flavors.
// All follow the same reconcile pattern: caller passes the *full
// desired set* of (entity_id, …) tuples; the helper deletes rows
// no longer in the desired set and inserts new ones. The caller is
// responsible for upserting the entity rows first.
// ─────────────────────────────────────────────────────────────────

/// Per-credit triple: (person_id, role, ordinal).
///
/// `role` may be any provider / ComicInfo spelling (`Writer`,
/// `CoverArtist`, `cover`, …): [`set_issue_credits`] folds it onto the
/// canonical storage key via
/// [`crate::metadata::provider::canonical_credit_role`].
pub type CreditSpec = (Uuid, String, i32);

pub async fn set_issue_credits<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    credits: Vec<CreditSpec>,
    set_by: SetBy,
    source_external_id: Option<String>,
    rebuild_batch: &CsvRebuildBatch,
) -> Result<(), DbErr> {
    issue_credit::Entity::delete_many()
        .filter(issue_credit::Column::IssueId.eq(issue_id))
        .exec(db)
        .await?;
    // WP-8.1: the junction's `person` column is the creator's *name* —
    // the scanner writes it that way, and `/creators`, `/people`, the
    // saved-view credit filters, `series_credits` and the reading stats
    // all key on it. (Pre-WP-8.1 this path stashed the person UUID
    // there, so provider-applied credits were invisible to all of them.)
    // `person.normalized_name` is unique, so two distinct people can't
    // collide on `(issue_id, role, person)`.
    let person_ids: Vec<Uuid> = credits.iter().map(|(id, _, _)| *id).collect();
    let names: HashMap<Uuid, String> = if person_ids.is_empty() {
        HashMap::new()
    } else {
        person::Entity::find()
            .filter(person::Column::Id.is_in(person_ids))
            .all(db)
            .await?
            .into_iter()
            .map(|p| (p.id, p.name))
            .collect()
    };
    let mut seen: HashSet<(String, Uuid)> = HashSet::new();
    let rows: Vec<issue_credit::ActiveModel> = credits
        .into_iter()
        .filter_map(|(person_id, role, ordinal)| {
            // WP-8.1: one canonical lowercase role key at the write
            // surface — the CSV rebuild, the filters and the UI match
            // `writer` / `cover_artist`, never `Writer` / `CoverArtist`.
            let role = crate::metadata::provider::canonical_credit_role(&role)?;
            let name = names.get(&person_id)?.clone();
            seen.insert((role.clone(), person_id))
                .then(|| issue_credit::ActiveModel {
                    issue_id: Set(issue_id.into()),
                    role: Set(role),
                    person: Set(name),
                    person_id: Set(Some(person_id)),
                    ordinal: Set(ordinal),
                })
        })
        .collect();
    if !rows.is_empty() {
        issue_credit::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([
                    issue_credit::Column::IssueId,
                    issue_credit::Column::Role,
                    issue_credit::Column::Person,
                ])
                .do_nothing()
                .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    write_field_provenance(
        db,
        "issue",
        issue_id,
        crate::metadata::MetadataField::Credits,
        set_by,
        source_external_id,
    )
    .await?;
    rebuild_batch.queue(issue_id);
    Ok(())
}

/// Per-character row: (character_id, is_first_appearance, died_in_issue).
pub type CharacterSpec = (Uuid, bool, bool);

pub async fn set_issue_characters<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    characters: Vec<CharacterSpec>,
    set_by: SetBy,
    source_external_id: Option<String>,
    rebuild_batch: &CsvRebuildBatch,
) -> Result<(), DbErr> {
    issue_character::Entity::delete_many()
        .filter(issue_character::Column::IssueId.eq(issue_id))
        .exec(db)
        .await?;
    if !characters.is_empty() {
        let rows: Vec<issue_character::ActiveModel> = characters
            .into_iter()
            .enumerate()
            .map(
                |(ordinal, (character_id, is_first, died))| issue_character::ActiveModel {
                    ordinal: Set(ordinal as i32),
                    issue_id: Set(issue_id.into()),
                    // PK is `(issue_id, character)` with `character` as
                    // the legacy TEXT column. Stash the FK UUID here for
                    // uniqueness; CSV rebuild joins to `character.name`
                    // for display.
                    character: Set(character_id.to_string()),
                    character_id: Set(Some(character_id)),
                    is_first_appearance: Set(is_first),
                    died_in_issue: Set(died),
                },
            )
            .collect();
        issue_character::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([
                    issue_character::Column::IssueId,
                    issue_character::Column::Character,
                ])
                .do_nothing()
                .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    write_field_provenance(
        db,
        "issue",
        issue_id,
        crate::metadata::MetadataField::Characters,
        set_by,
        source_external_id,
    )
    .await?;
    rebuild_batch.queue(issue_id);
    Ok(())
}

pub type TeamSpec = (Uuid, bool, bool);

pub async fn set_issue_teams<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    teams: Vec<TeamSpec>,
    set_by: SetBy,
    source_external_id: Option<String>,
    rebuild_batch: &CsvRebuildBatch,
) -> Result<(), DbErr> {
    issue_team::Entity::delete_many()
        .filter(issue_team::Column::IssueId.eq(issue_id))
        .exec(db)
        .await?;
    if !teams.is_empty() {
        let rows: Vec<issue_team::ActiveModel> = teams
            .into_iter()
            .enumerate()
            .map(
                |(ordinal, (team_id, is_first, disbanded))| issue_team::ActiveModel {
                    ordinal: Set(ordinal as i32),
                    issue_id: Set(issue_id.into()),
                    team: Set(team_id.to_string()),
                    team_id: Set(Some(team_id)),
                    is_first_appearance: Set(is_first),
                    disbanded_in_issue: Set(disbanded),
                },
            )
            .collect();
        issue_team::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([issue_team::Column::IssueId, issue_team::Column::Team])
                    .do_nothing()
                    .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    write_field_provenance(
        db,
        "issue",
        issue_id,
        crate::metadata::MetadataField::Teams,
        set_by,
        source_external_id,
    )
    .await?;
    rebuild_batch.queue(issue_id);
    Ok(())
}

pub type LocationSpec = (Uuid, bool);

pub async fn set_issue_locations<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    locations: Vec<LocationSpec>,
    set_by: SetBy,
    source_external_id: Option<String>,
    rebuild_batch: &CsvRebuildBatch,
) -> Result<(), DbErr> {
    issue_location::Entity::delete_many()
        .filter(issue_location::Column::IssueId.eq(issue_id))
        .exec(db)
        .await?;
    if !locations.is_empty() {
        let rows: Vec<issue_location::ActiveModel> = locations
            .into_iter()
            .enumerate()
            .map(
                |(ordinal, (location_id, is_first))| issue_location::ActiveModel {
                    ordinal: Set(ordinal as i32),
                    issue_id: Set(issue_id.into()),
                    location: Set(location_id.to_string()),
                    location_id: Set(Some(location_id)),
                    is_first_appearance: Set(is_first),
                },
            )
            .collect();
        issue_location::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([
                    issue_location::Column::IssueId,
                    issue_location::Column::Location,
                ])
                .do_nothing()
                .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    write_field_provenance(
        db,
        "issue",
        issue_id,
        crate::metadata::MetadataField::Locations,
        set_by,
        source_external_id,
    )
    .await?;
    rebuild_batch.queue(issue_id);
    Ok(())
}

/// Per-arc: (arc_id, position_in_arc).
pub type ArcSpec = (Uuid, Option<i32>);

pub async fn set_issue_story_arcs<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    arcs: Vec<ArcSpec>,
    set_by: SetBy,
    source_external_id: Option<String>,
    rebuild_batch: &CsvRebuildBatch,
) -> Result<(), DbErr> {
    issue_arc::Entity::delete_many()
        .filter(issue_arc::Column::IssueId.eq(issue_id))
        .exec(db)
        .await?;
    if !arcs.is_empty() {
        let rows: Vec<issue_arc::ActiveModel> = arcs
            .into_iter()
            .map(|(arc_id, pos)| issue_arc::ActiveModel {
                issue_id: Set(issue_id.into()),
                arc_id: Set(arc_id),
                position_in_arc: Set(pos),
            })
            .collect();
        issue_arc::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([issue_arc::Column::IssueId, issue_arc::Column::ArcId])
                    .do_nothing()
                    .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    write_field_provenance(
        db,
        "issue",
        issue_id,
        crate::metadata::MetadataField::StoryArcs,
        set_by,
        source_external_id,
    )
    .await?;
    rebuild_batch.queue(issue_id);
    Ok(())
}

pub type ConceptSpec = (Uuid, bool);

pub async fn set_issue_concepts<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    concepts: Vec<ConceptSpec>,
    set_by: SetBy,
    source_external_id: Option<String>,
) -> Result<(), DbErr> {
    issue_concept::Entity::delete_many()
        .filter(issue_concept::Column::IssueId.eq(issue_id))
        .exec(db)
        .await?;
    if !concepts.is_empty() {
        let rows: Vec<issue_concept::ActiveModel> = concepts
            .into_iter()
            .map(|(concept_id, is_first)| issue_concept::ActiveModel {
                issue_id: Set(issue_id.into()),
                concept_id: Set(concept_id),
                is_first_appearance: Set(is_first),
            })
            .collect();
        issue_concept::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([
                    issue_concept::Column::IssueId,
                    issue_concept::Column::ConceptId,
                ])
                .do_nothing()
                .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    write_field_provenance(
        db,
        "issue",
        issue_id,
        crate::metadata::MetadataField::Concepts,
        set_by,
        source_external_id,
    )
    .await?;
    Ok(())
}

pub type ObjectSpec = (Uuid, bool);

pub async fn set_issue_objects<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    objects: Vec<ObjectSpec>,
    set_by: SetBy,
    source_external_id: Option<String>,
) -> Result<(), DbErr> {
    issue_object::Entity::delete_many()
        .filter(issue_object::Column::IssueId.eq(issue_id))
        .exec(db)
        .await?;
    if !objects.is_empty() {
        let rows: Vec<issue_object::ActiveModel> = objects
            .into_iter()
            .map(|(object_id, is_first)| issue_object::ActiveModel {
                issue_id: Set(issue_id.into()),
                object_id: Set(object_id),
                is_first_appearance: Set(is_first),
            })
            .collect();
        issue_object::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([
                    issue_object::Column::IssueId,
                    issue_object::Column::ObjectId,
                ])
                .do_nothing()
                .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    write_field_provenance(
        db,
        "issue",
        issue_id,
        crate::metadata::MetadataField::Objects,
        set_by,
        source_external_id,
    )
    .await?;
    Ok(())
}

pub async fn set_issue_universes<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    universes: Vec<Uuid>,
    set_by: SetBy,
    source_external_id: Option<String>,
) -> Result<(), DbErr> {
    issue_universe::Entity::delete_many()
        .filter(issue_universe::Column::IssueId.eq(issue_id))
        .exec(db)
        .await?;
    if !universes.is_empty() {
        let rows: Vec<issue_universe::ActiveModel> = universes
            .into_iter()
            .map(|universe_id| issue_universe::ActiveModel {
                issue_id: Set(issue_id.into()),
                universe_id: Set(universe_id),
            })
            .collect();
        issue_universe::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([
                    issue_universe::Column::IssueId,
                    issue_universe::Column::UniverseId,
                ])
                .do_nothing()
                .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    write_field_provenance(
        db,
        "issue",
        issue_id,
        crate::metadata::MetadataField::Universes,
        set_by,
        source_external_id,
    )
    .await?;
    Ok(())
}

/// One reprint of an issue (WP-7.8). At least one of `reprinted_issue_id`
/// / `reprinted_label` must be set (the DB CHECK mirrors this). The
/// provider id (`reprinted_source` + `reprinted_external_id`) is kept even
/// when the reprinted issue isn't in the library, so
/// [`resolve_pending_reprints`] can fill `reprinted_issue_id` once it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReprintSpec {
    pub reprinted_issue_id: Option<String>,
    pub reprinted_label: Option<String>,
    pub reprinted_source: Option<String>,
    pub reprinted_external_id: Option<String>,
}

/// Replace an issue's reprint set. Rows with neither a target nor a label
/// are dropped, duplicates (same target + label) collapse. Writes the
/// `reprints` field provenance. User precedence is the caller's decision
/// (the apply path checks the `reprints` provenance first, like every other
/// junction).
pub async fn set_issue_reprints<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    reprints: Vec<ReprintSpec>,
    set_by: SetBy,
    source_external_id: Option<String>,
) -> Result<(), DbErr> {
    issue_reprint::Entity::delete_many()
        .filter(issue_reprint::Column::IssueId.eq(issue_id))
        .exec(db)
        .await?;
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let rows: Vec<issue_reprint::ActiveModel> = reprints
        .into_iter()
        .filter(|r| r.reprinted_issue_id.is_some() || r.reprinted_label.is_some())
        .filter(|r| r.reprinted_issue_id.as_deref() != Some(issue_id))
        .filter(|r| {
            seen.insert((
                r.reprinted_issue_id.clone().unwrap_or_default(),
                r.reprinted_label.clone().unwrap_or_default(),
            ))
        })
        .map(|r| issue_reprint::ActiveModel {
            id: Set(Uuid::now_v7()),
            issue_id: Set(issue_id.into()),
            reprinted_issue_id: Set(r.reprinted_issue_id),
            reprinted_label: Set(r.reprinted_label),
            reprinted_source: Set(r.reprinted_source),
            reprinted_external_id: Set(r.reprinted_external_id),
        })
        .collect();
    if !rows.is_empty() {
        issue_reprint::Entity::insert_many(rows).exec(db).await?;
    }
    write_field_provenance(
        db,
        "issue",
        issue_id,
        crate::metadata::MetadataField::Reprints,
        set_by,
        source_external_id,
    )
    .await?;
    Ok(())
}

#[derive(Debug, FromQueryResult)]
struct ResolvedIssueRow {
    entity_id: String,
}

/// The local issue a provider issue id points at: a direct `external_ids`
/// match, else the **id bridge** — the provider's cached detail for that
/// issue (`metadata_cache`) lists its other providers' ids (Metron carries
/// `cv_id` / `gcd_id`), and a local issue matched under one of those counts.
/// No network call. Removed issues don't count.
pub async fn resolve_provider_issue<C: ConnectionTrait>(
    db: &C,
    source: &str,
    external_id: &str,
) -> Result<Option<String>, DbErr> {
    let sql = "SELECT x.entity_id FROM external_ids x \
                 JOIN issues i ON i.id = x.entity_id AND i.removed_at IS NULL \
                WHERE x.entity_type = 'issue' AND x.source = $1 AND x.external_id = $2 \
               UNION ALL \
               SELECT x.entity_id FROM metadata_cache c \
                 CROSS JOIN LATERAL jsonb_array_elements( \
                     CASE WHEN jsonb_typeof(c.payload->'identifiers') = 'array' \
                          THEN c.payload->'identifiers' ELSE '[]'::jsonb END) AS ident(v) \
                 JOIN external_ids x ON x.entity_type = 'issue' \
                                    AND x.source = ident.v->>'source' \
                                    AND x.external_id = ident.v->>'id' \
                 JOIN issues i ON i.id = x.entity_id AND i.removed_at IS NULL \
                WHERE c.provider = $1 AND c.entity = 'issue' AND c.external_id = $2 \
                LIMIT 1";
    Ok(
        ResolvedIssueRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [source.into(), external_id.into()],
        ))
        .one(db)
        .await?
        .map(|r| r.entity_id),
    )
}

/// Provider reprint candidates → [`ReprintSpec`]s for `issue_id`: the label
/// is always kept; the reprinted issue is resolved to a local issue through
/// [`resolve_provider_issue`] using the candidate's identifiers (first
/// match wins); the first identifier is kept as the pending provider id.
pub async fn reprint_specs<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    candidates: &[crate::metadata::provider::ReprintCandidate],
) -> Result<Vec<ReprintSpec>, DbErr> {
    let mut out = Vec::with_capacity(candidates.len());
    for c in candidates {
        let label = Some(c.label.trim().to_owned()).filter(|l| !l.is_empty());
        let mut target = None;
        for ident in &c.identifiers {
            if let Some(id) = resolve_provider_issue(db, ident.source.as_str(), &ident.id).await?
                && id != issue_id
            {
                target = Some(id);
                break;
            }
        }
        let first = c.identifiers.first();
        out.push(ReprintSpec {
            reprinted_issue_id: target,
            reprinted_label: label,
            reprinted_source: first.map(|i| i.source.as_str().to_owned()),
            reprinted_external_id: first.map(|i| i.id.clone()),
        });
    }
    Ok(out)
}

/// Fill `reprinted_issue_id` on label-only reprint rows whose provider id
/// now resolves to a local issue (WP-7.8): the reprinted issue was scanned
/// in and matched after the apply that recorded the reprint. Scoped to the
/// reprinting issues of `library_id`, or to rows pointing at
/// `(source, external_id)` when given. Direct `external_ids` matches only;
/// a row whose resolved target would duplicate an existing row of the same
/// issue is left alone. Returns the rows updated. Called by the
/// relationship-suggestion run (before the reprint roll-up) and by
/// [`set_external_id`] when an issue gains a provider id.
pub async fn resolve_pending_reprints<C: ConnectionTrait>(
    db: &C,
    library_id: Option<Uuid>,
    provider_id: Option<(&str, &str)>,
) -> Result<u64, DbErr> {
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut filters = Vec::new();
    if let Some(lib) = library_id {
        values.push(lib.into());
        filters.push(format!(
            "EXISTS (SELECT 1 FROM issues f WHERE f.id = rp.issue_id AND f.library_id = ${})",
            values.len()
        ));
    }
    if let Some((source, ext)) = provider_id {
        values.push(source.into());
        values.push(ext.into());
        filters.push(format!(
            "rp.reprinted_source = ${} AND rp.reprinted_external_id = ${}",
            values.len() - 1,
            values.len()
        ));
    }
    let extra = if filters.is_empty() {
        String::new()
    } else {
        format!(" AND {}", filters.join(" AND "))
    };
    let sql = format!(
        "UPDATE issue_reprints rp SET reprinted_issue_id = x.entity_id \
           FROM external_ids x \
          WHERE rp.reprinted_issue_id IS NULL AND rp.reprinted_external_id IS NOT NULL \
            AND x.entity_type = 'issue' AND x.source = rp.reprinted_source \
            AND x.external_id = rp.reprinted_external_id \
            AND x.entity_id <> rp.issue_id \
            AND EXISTS (SELECT 1 FROM issues t WHERE t.id = x.entity_id AND t.removed_at IS NULL) \
            AND NOT EXISTS (SELECT 1 FROM issue_reprints d \
                             WHERE d.issue_id = rp.issue_id AND d.reprinted_issue_id = x.entity_id \
                               AND COALESCE(d.reprinted_label, '') = COALESCE(rp.reprinted_label, '')){extra}"
    );
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            values,
        ))
        .await?;
    Ok(res.rows_affected())
}

/// Genre + tag setters — these are pure string sets (no entity table).
pub async fn set_issue_genres<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    genres: Vec<String>,
    set_by: SetBy,
    source_external_id: Option<String>,
    rebuild_batch: &CsvRebuildBatch,
) -> Result<(), DbErr> {
    issue_genre::Entity::delete_many()
        .filter(issue_genre::Column::IssueId.eq(issue_id))
        .exec(db)
        .await?;
    if !genres.is_empty() {
        let rows: Vec<issue_genre::ActiveModel> = genres
            .into_iter()
            .enumerate()
            .map(|(ordinal, g)| issue_genre::ActiveModel {
                ordinal: Set(ordinal as i32),
                issue_id: Set(issue_id.into()),
                genre: Set(g),
            })
            .collect();
        issue_genre::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([issue_genre::Column::IssueId, issue_genre::Column::Genre])
                    .do_nothing()
                    .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    write_field_provenance(
        db,
        "issue",
        issue_id,
        crate::metadata::MetadataField::Genres,
        set_by,
        source_external_id,
    )
    .await?;
    rebuild_batch.queue(issue_id);
    Ok(())
}

pub async fn set_issue_tags<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    tags: Vec<String>,
    set_by: SetBy,
    source_external_id: Option<String>,
    rebuild_batch: &CsvRebuildBatch,
) -> Result<(), DbErr> {
    issue_tag::Entity::delete_many()
        .filter(issue_tag::Column::IssueId.eq(issue_id))
        .exec(db)
        .await?;
    if !tags.is_empty() {
        let rows: Vec<issue_tag::ActiveModel> = tags
            .into_iter()
            .enumerate()
            .map(|(ordinal, t)| issue_tag::ActiveModel {
                ordinal: Set(ordinal as i32),
                issue_id: Set(issue_id.into()),
                tag: Set(t),
            })
            .collect();
        issue_tag::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([issue_tag::Column::IssueId, issue_tag::Column::Tag])
                    .do_nothing()
                    .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    write_field_provenance(
        db,
        "issue",
        issue_id,
        crate::metadata::MetadataField::Tags,
        set_by,
        source_external_id,
    )
    .await?;
    rebuild_batch.queue(issue_id);
    Ok(())
}

// Series-level junction setters mirror the issue-level ones but
// don't write field_provenance (series junctions are rollups, not
// authored values) and don't queue a CSV rebuild (series has no
// CSV cache).

pub async fn set_series_characters<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
    characters: Vec<Uuid>,
) -> Result<(), DbErr> {
    series_character::Entity::delete_many()
        .filter(series_character::Column::SeriesId.eq(series_id))
        .exec(db)
        .await?;
    if !characters.is_empty() {
        let rows: Vec<series_character::ActiveModel> = characters
            .into_iter()
            .map(|character_id| series_character::ActiveModel {
                series_id: Set(series_id),
                character: Set(character_id.to_string()),
                character_id: Set(Some(character_id)),
            })
            .collect();
        series_character::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([
                    series_character::Column::SeriesId,
                    series_character::Column::Character,
                ])
                .do_nothing()
                .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    Ok(())
}

pub async fn set_series_teams<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
    teams: Vec<Uuid>,
) -> Result<(), DbErr> {
    series_team::Entity::delete_many()
        .filter(series_team::Column::SeriesId.eq(series_id))
        .exec(db)
        .await?;
    if !teams.is_empty() {
        let rows: Vec<series_team::ActiveModel> = teams
            .into_iter()
            .map(|team_id| series_team::ActiveModel {
                series_id: Set(series_id),
                team: Set(team_id.to_string()),
                team_id: Set(Some(team_id)),
            })
            .collect();
        series_team::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([series_team::Column::SeriesId, series_team::Column::Team])
                    .do_nothing()
                    .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    Ok(())
}

pub async fn set_series_locations<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
    locations: Vec<Uuid>,
) -> Result<(), DbErr> {
    series_location::Entity::delete_many()
        .filter(series_location::Column::SeriesId.eq(series_id))
        .exec(db)
        .await?;
    if !locations.is_empty() {
        let rows: Vec<series_location::ActiveModel> = locations
            .into_iter()
            .map(|location_id| series_location::ActiveModel {
                series_id: Set(series_id),
                location: Set(location_id.to_string()),
                location_id: Set(Some(location_id)),
            })
            .collect();
        series_location::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([
                    series_location::Column::SeriesId,
                    series_location::Column::Location,
                ])
                .do_nothing()
                .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    Ok(())
}

pub async fn set_series_story_arcs<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
    arcs: Vec<Uuid>,
) -> Result<(), DbErr> {
    series_arc::Entity::delete_many()
        .filter(series_arc::Column::SeriesId.eq(series_id))
        .exec(db)
        .await?;
    if !arcs.is_empty() {
        let rows: Vec<series_arc::ActiveModel> = arcs
            .into_iter()
            .map(|arc_id| series_arc::ActiveModel {
                series_id: Set(series_id),
                arc_id: Set(arc_id),
            })
            .collect();
        series_arc::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([series_arc::Column::SeriesId, series_arc::Column::ArcId])
                    .do_nothing()
                    .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    Ok(())
}

pub async fn set_series_concepts<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
    concepts: Vec<Uuid>,
) -> Result<(), DbErr> {
    series_concept::Entity::delete_many()
        .filter(series_concept::Column::SeriesId.eq(series_id))
        .exec(db)
        .await?;
    if !concepts.is_empty() {
        let rows: Vec<series_concept::ActiveModel> = concepts
            .into_iter()
            .map(|concept_id| series_concept::ActiveModel {
                series_id: Set(series_id),
                concept_id: Set(concept_id),
            })
            .collect();
        series_concept::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([
                    series_concept::Column::SeriesId,
                    series_concept::Column::ConceptId,
                ])
                .do_nothing()
                .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    Ok(())
}

pub async fn set_series_objects<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
    objects: Vec<Uuid>,
) -> Result<(), DbErr> {
    series_object::Entity::delete_many()
        .filter(series_object::Column::SeriesId.eq(series_id))
        .exec(db)
        .await?;
    if !objects.is_empty() {
        let rows: Vec<series_object::ActiveModel> = objects
            .into_iter()
            .map(|object_id| series_object::ActiveModel {
                series_id: Set(series_id),
                object_id: Set(object_id),
            })
            .collect();
        series_object::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([
                    series_object::Column::SeriesId,
                    series_object::Column::ObjectId,
                ])
                .do_nothing()
                .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    Ok(())
}

pub async fn set_series_universes<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
    universes: Vec<Uuid>,
) -> Result<(), DbErr> {
    series_universe::Entity::delete_many()
        .filter(series_universe::Column::SeriesId.eq(series_id))
        .exec(db)
        .await?;
    if !universes.is_empty() {
        let rows: Vec<series_universe::ActiveModel> = universes
            .into_iter()
            .map(|universe_id| series_universe::ActiveModel {
                series_id: Set(series_id),
                universe_id: Set(universe_id),
            })
            .collect();
        series_universe::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([
                    series_universe::Column::SeriesId,
                    series_universe::Column::UniverseId,
                ])
                .do_nothing()
                .to_owned(),
            )
            .try_insert()
            .exec(db)
            .await?;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// Cover writes.
// ─────────────────────────────────────────────────────────────────

/// Inputs needed to persist a cover. Bytes + format come from the
/// caller; the helper handles the on-disk write + DB insert.
#[derive(Debug)]
pub struct CoverWrite<'a> {
    pub issue_id: &'a str,
    pub kind: &'a str,
    pub ordinal: i32,
    pub identifier: Option<&'a Identifier>,
    pub source_url: Option<&'a str>,
    pub variant_label: Option<&'a str>,
    pub variant_artist_person_id: Option<Uuid>,
    /// Image bytes. Must carry a recognised image signature
    /// (`archive::image_sniff`) — [`apply_cover`] refuses anything else, and
    /// the stored file's extension (hence the served MIME) comes from the
    /// sniffed format, never from the source URL (SE-6, WP-6.3).
    pub bytes: &'a [u8],
    /// Width / height in pixels.
    pub width: Option<i32>,
    pub height: Option<i32>,
}

/// Persist a cover row + write the image bytes to disk. Honors
/// [`CoverOverwritePolicy`] for `kind='primary' AND ordinal=0`;
/// variants are always additive (the policy doesn't apply).
///
/// Slot uniqueness is a partial index — one **active** row per
/// `(issue_id, kind, ordinal)`, any number of inactive ones (the
/// post-scan phash worker's `archive_extracted` hash row, previously
/// applied covers). Replacing the active primary deactivates it and
/// inserts the new row in **one transaction**: if the insert (or the
/// commit) fails, the deactivation rolls back — the old cover stays
/// served — and the file written for the new row is removed, so neither
/// a cover-less issue nor an orphaned file is left behind.
pub async fn apply_cover(
    db: &sea_orm::DatabaseConnection,
    data_path: &std::path::Path,
    write: CoverWrite<'_>,
    policy: CoverOverwritePolicy,
) -> Result<Option<Uuid>, std::io::Error> {
    let is_primary = write.kind == "primary" && write.ordinal == 0;

    // SE-6 (WP-6.3): magic-sniff before anything is persisted. A provider /
    // CDN that answers with HTML, JSON, SVG, or an error page must not land
    // on disk to be served back as `image/*`.
    let kind = sniff_cover(write.bytes).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "cover bytes are not a recognised image (jpeg/png/gif/webp/avif/jxl)",
        )
    })?;

    let txn = db.begin().await.map_err(|e| cover_db_err("begin", e))?;

    // Policy gate applies only to the primary slot. The row lock
    // serializes concurrent replaces of the same slot.
    let mut replaced: Option<Uuid> = None;
    if is_primary {
        let existing_primary = issue_cover::Entity::find()
            .filter(issue_cover::Column::IssueId.eq(write.issue_id))
            .filter(issue_cover::Column::Kind.eq("primary"))
            .filter(issue_cover::Column::Ordinal.eq(0))
            .filter(issue_cover::Column::IsActive.eq(true))
            .lock_exclusive()
            .one(&txn)
            .await
            .map_err(|e| cover_db_err("lookup", e))?;
        if existing_primary.is_some()
            && matches!(
                policy,
                CoverOverwritePolicy::Never | CoverOverwritePolicy::WhenMissing
            )
        {
            return Ok(None);
        }
        replaced = existing_primary.map(|p| p.id);
    }

    let cover_id = Uuid::now_v7();
    let rel_path = cover_rel_path(write.issue_id, cover_id, kind.ext());
    let on_disk = data_path.join(&rel_path);
    if let Some(parent) = on_disk.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&on_disk, write.bytes)?;

    let written = match insert_cover_row(&txn, &write, cover_id, rel_path, replaced).await {
        Ok(()) => txn.commit().await.map_err(|e| cover_db_err("commit", e)),
        // `txn` drops un-committed → rolled back, deactivation included.
        Err(e) => Err(e),
    };
    if let Err(e) = written {
        // No row points at the file we just wrote; don't orphan it.
        remove_cover_file(&on_disk);
        return Err(e);
    }
    Ok(Some(cover_id))
}

fn cover_db_err(ctx: &str, e: DbErr) -> std::io::Error {
    std::io::Error::other(format!("issue_cover {ctx}: {e}"))
}

/// Best-effort removal of a cover file written for a row that never
/// committed. A missing file is fine; anything else is logged.
fn remove_cover_file(path: &std::path::Path) {
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            path = %path.display(),
            error = %e,
            "cover write: failed to remove the file of an uncommitted cover row",
        );
    }
}

/// The transactional half of [`apply_cover`]: deactivate `replaced` (the
/// current active primary, if any) and insert the new row. The caller
/// commits.
async fn insert_cover_row(
    txn: &sea_orm::DatabaseTransaction,
    write: &CoverWrite<'_>,
    cover_id: Uuid,
    rel_path: String,
    replaced: Option<Uuid>,
) -> Result<(), std::io::Error> {
    // Deactivate first — the partial unique index admits one active row
    // per slot, so the insert below would collide otherwise.
    if let Some(prev_id) = replaced {
        issue_cover::Entity::update_many()
            .col_expr(
                issue_cover::Column::IsActive,
                sea_orm::sea_query::Expr::value(false),
            )
            .filter(issue_cover::Column::Id.eq(prev_id))
            .exec(txn)
            .await
            .map_err(|e| cover_db_err("deactivate", e))?;
    }

    // metadata-providers-1.0 M9: compute perceptual hashes on the
    // bytes as we write. Decode failures don't block the cover write
    // — phash columns stay NULL and the backfill job can recover
    // later. The decode is in-thread because the bytes are already
    // in memory; for archive covers the post-scan thumbnail job
    // hashes from the resized buffer instead.
    let (p, d, a) = match crate::util::image_decode::decode_limited(write.bytes) {
        Ok(img) => {
            let (p, d, a) = crate::metadata::phash::all_hashes(&img);
            (Some(p), Some(d), Some(a))
        }
        Err(e) => {
            tracing::debug!(
                issue_id = write.issue_id,
                error = %e,
                "apply_cover: phash skipped — image decode failed"
            );
            (None, None, None)
        }
    };

    let now = chrono::Utc::now().fixed_offset();
    let am = issue_cover::ActiveModel {
        id: Set(cover_id),
        issue_id: Set(write.issue_id.into()),
        kind: Set(write.kind.into()),
        ordinal: Set(write.ordinal),
        source_provider: Set(write.identifier.map(|i| i.source.as_str().into())),
        source_external_id: Set(write.identifier.map(|i| i.id.clone())),
        source_url: Set(write.source_url.map(str::to_owned)),
        variant_label: Set(write.variant_label.map(str::to_owned)),
        variant_artist_person_id: Set(write.variant_artist_person_id),
        local_path: Set(rel_path),
        width: Set(write.width),
        height: Set(write.height),
        phash: Set(p),
        dhash: Set(d),
        ahash: Set(a),
        fetched_at: Set(now),
        is_active: Set(true),
    };
    am.insert(txn)
        .await
        .map_err(|e| cover_db_err("insert", e))?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// User-pin clear (M5.3 — Revert-pin button surface).
// ─────────────────────────────────────────────────────────────────

/// Delete a user pin (`field_provenance` row with `set_by='user'`) on
/// a single field. Returns `true` if a row was found + deleted,
/// `false` when no user pin existed for that field (caller can ignore
/// — the user-precedence rule was already off).
///
/// Provider-set rows are left untouched: a write of a user-pin clear
/// must not silently nuke a `set_by='comicvine'` row that the next
/// apply would re-overwrite anyway. Guards a hostile / mis-targeted
/// DELETE that could otherwise clobber audit provenance.
pub async fn clear_user_pin<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
    field_key: &str,
) -> Result<bool, sea_orm::DbErr> {
    use entity::field_provenance;
    let Some(row) = field_provenance::Entity::find()
        .filter(field_provenance::Column::EntityType.eq(entity_type))
        .filter(field_provenance::Column::EntityId.eq(entity_id))
        .filter(field_provenance::Column::Field.eq(field_key))
        .filter(field_provenance::Column::SetBy.eq("user"))
        .one(db)
        .await?
    else {
        return Ok(false);
    };
    let am: field_provenance::ActiveModel = row.into();
    am.delete(db).await?;
    Ok(true)
}

// ─────────────────────────────────────────────────────────────────
// Issue column pins (WP-3.7 — replaced the per-issue JSON edit list).
// ─────────────────────────────────────────────────────────────────

/// Every issue column a user edit (`PATCH /series/{s}/issues/{i}` or
/// the bulk-metadata PATCH) can pin, keyed by its column name. This is
/// the closed set of **column-level** pin keys `field_provenance.field`
/// may carry for an issue besides the [`MetadataField::key`] values.
///
/// A user edit records `set_by='user'` under BOTH the column key and,
/// when the column feeds one, the [`MetadataField`] it rolls up into
/// ([`issue_column_pin_field`]): the column key drives the per-field
/// pin UI and the scanner's gates for columns without a `MetadataField`
/// slot (`sort_number`, `number_raw`, `black_and_white`,
/// `alternate_series`, `web_url`); the `MetadataField` key drives the
/// apply / composer / scanner precedence checks. The per-issue JSON
/// edit list retired in WP-3.7 held exactly the column keys; its
/// entries were backfilled here by that WP's migration.
///
/// [`MetadataField`]: crate::metadata::MetadataField
/// [`MetadataField::key`]: crate::metadata::MetadataField::key
pub const ISSUE_COLUMN_PIN_KEYS: &[&str] = &[
    "title",
    "number_raw",
    "sort_number",
    "volume",
    "year",
    "month",
    "day",
    "summary",
    "notes",
    "publisher",
    "imprint",
    "writer",
    "penciller",
    "inker",
    "colorist",
    "letterer",
    "cover_artist",
    "editor",
    "translator",
    "characters",
    "teams",
    "locations",
    "alternate_series",
    "story_arc",
    "story_arc_number",
    "genre",
    "tags",
    "language_code",
    "age_rating",
    "format",
    "manga",
    "black_and_white",
    "web_url",
    "gtin",
    "comicvine_id",
    "metron_id",
];

/// The [`MetadataField`](crate::metadata::MetadataField) an issue
/// column pin key rolls up into, or `None` for columns with no slot
/// (`sort_number`, `black_and_white`, `alternate_series`, `web_url`).
/// The composer / apply pipeline read the `MetadataField` key; the
/// column key stays the precise per-column pin.
pub fn issue_column_pin_field(key: &str) -> Option<crate::metadata::MetadataField> {
    use crate::metadata::MetadataField as F;
    match key {
        "title" => Some(F::Title),
        "summary" => Some(F::Summary),
        "notes" => Some(F::Notes),
        "publisher" => Some(F::Publisher),
        "imprint" => Some(F::Imprint),
        "language_code" => Some(F::LanguageCode),
        "age_rating" => Some(F::AgeRating),
        "format" => Some(F::Format),
        "manga" => Some(F::Manga),
        "volume" => Some(F::Volume),
        "number_raw" => Some(F::Number),
        // The user edits per-role credit columns and the character /
        // team / location CSV strings; the composer reads the
        // junction-shaped fields. Map each to its junction.
        "writer" | "penciller" | "inker" | "colorist" | "letterer" | "cover_artist" | "editor"
        | "translator" => Some(F::Credits),
        "characters" => Some(F::Characters),
        "teams" => Some(F::Teams),
        "locations" => Some(F::Locations),
        "story_arc" | "story_arc_number" => Some(F::StoryArcs),
        "genre" => Some(F::Genres),
        "tags" => Some(F::Tags),
        "gtin" => Some(F::ExternalId(Source::Gtin)),
        "comicvine_id" => Some(F::ExternalId(Source::ComicVine)),
        "metron_id" => Some(F::ExternalId(Source::Metron)),
        // PATCH writes y/m/d separately; the composer reads CoverDate.
        "year" | "month" | "day" => Some(F::CoverDate),
        _ => None,
    }
}

/// Record a user edit of the given issue columns: one `set_by='user'`
/// `field_provenance` row per column key plus one per rolled-up
/// [`MetadataField`](crate::metadata::MetadataField) (see
/// [`ISSUE_COLUMN_PIN_KEYS`]). Keys outside the closed column set are
/// ignored (debug-asserted). Run it in the same transaction as the
/// column update so an edit never lands unpinned.
pub async fn write_issue_user_pins<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    column_keys: &[&str],
) -> Result<(), DbErr> {
    let mut fields: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for key in column_keys {
        debug_assert!(
            ISSUE_COLUMN_PIN_KEYS.contains(key),
            "unknown issue column pin key {key}"
        );
        if !ISSUE_COLUMN_PIN_KEYS.contains(key) {
            continue;
        }
        fields.insert((*key).to_owned());
        if let Some(f) = issue_column_pin_field(key) {
            fields.insert(f.key());
        }
    }
    if fields.is_empty() {
        return Ok(());
    }
    let now = chrono::Utc::now().fixed_offset();
    let models = fields
        .into_iter()
        .map(|field| field_provenance::ActiveModel {
            entity_type: Set("issue".into()),
            entity_id: Set(issue_id.into()),
            field: Set(field),
            set_by: Set(SetBy::User.as_str()),
            set_at: Set(now),
            source_external_id: Set(None),
        });
    field_provenance::Entity::insert_many(models)
        .on_conflict(
            OnConflict::columns([
                field_provenance::Column::EntityType,
                field_provenance::Column::EntityId,
                field_provenance::Column::Field,
            ])
            .update_columns([
                field_provenance::Column::SetBy,
                field_provenance::Column::SetAt,
                field_provenance::Column::SourceExternalId,
            ])
            .to_owned(),
        )
        .exec_without_returning(db)
        .await?;
    Ok(())
}

/// Release a user pin on an issue, keeping the column-level and
/// `MetadataField`-level rows consistent:
///
/// - `field` is a `MetadataField` key (e.g. `credits`) → its user row
///   AND every column pin rolling up into it (`writer`, `inker`, …) go.
/// - `field` is a column key (e.g. `writer`) → its user row goes; the
///   rolled-up `MetadataField` row goes too once no sibling column
///   (`penciller`, …) is still user-pinned.
///
/// Only `set_by='user'` rows are touched. Returns `true` when any row
/// was deleted.
pub async fn clear_issue_user_pin<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    field: &str,
) -> Result<bool, DbErr> {
    let pinned = fetch_user_pinned_fields(db, "issue", issue_id).await?;
    let mut to_clear: HashSet<String> = HashSet::new();
    to_clear.insert(field.to_owned());
    // MetadataField key → drop every column pin rolling up into it.
    if let Ok(mf) = field.parse::<crate::metadata::MetadataField>() {
        for key in ISSUE_COLUMN_PIN_KEYS {
            if issue_column_pin_field(key) == Some(mf) {
                to_clear.insert((*key).to_owned());
            }
        }
    }
    // Column key → drop the rolled-up MetadataField row once no sibling
    // column still holds a pin.
    if let Some(mf) = issue_column_pin_field(field) {
        let sibling_pinned = ISSUE_COLUMN_PIN_KEYS.iter().any(|k| {
            *k != field
                && *k != mf.key()
                && issue_column_pin_field(k) == Some(mf)
                && pinned.contains(*k)
        });
        if !sibling_pinned {
            to_clear.insert(mf.key());
        }
    }
    let res = field_provenance::Entity::delete_many()
        .filter(field_provenance::Column::EntityType.eq("issue"))
        .filter(field_provenance::Column::EntityId.eq(issue_id))
        .filter(field_provenance::Column::Field.is_in(to_clear))
        .filter(field_provenance::Column::SetBy.eq("user"))
        .exec(db)
        .await?;
    Ok(res.rows_affected > 0)
}

// ─────────────────────────────────────────────────────────────────
// Variant covers.
// ─────────────────────────────────────────────────────────────────

/// Largest cover image we'll pull into memory. Covers are typically
/// well under 1 MB; the cap guards against a misbehaving / hostile CDN.
const MAX_COVER_BYTES: usize = crate::util::ssrf::MAX_IMAGE_BYTES;

/// On-disk + hash result of a successful cover download.
struct StoredCover {
    cover_id: Uuid,
    rel_path: String,
    width: i32,
    height: i32,
    phash: i64,
    dhash: i64,
    ahash: i64,
}

/// Identify a cover's image container from its leading bytes. `None` for
/// anything that isn't on the page-bytes allowlist (`archive::image_sniff`)
/// — text, HTML, JSON, SVG, truncated bodies.
pub fn sniff_cover(bytes: &[u8]) -> Option<archive::image_sniff::ImageKind> {
    archive::image_sniff::sniff(&bytes[..bytes.len().min(archive::image_sniff::SNIFF_LEN)])
}

/// Why [`fetch_cover_bytes`] refused a cover.
#[derive(Debug, thiserror::Error)]
pub enum CoverFetchError {
    #[error(transparent)]
    Fetch(#[from] crate::util::ssrf::FetchBytesError),
    #[error("response is not a recognised image")]
    NotAnImage,
    /// The cover host answers non-browser requests with a bot challenge
    /// (see [`crate::metadata::cover_block`]). Not retryable server-side.
    #[error("{host} refuses non-browser downloads (bot challenge)")]
    Blocked { host: String },
}

impl From<CoverFetchError> for crate::metadata::provider::ProviderError {
    fn from(e: CoverFetchError) -> Self {
        match e {
            CoverFetchError::Blocked { .. } => Self::CoverUnavailable(e.to_string()),
            other => Self::Transport(other.to_string()),
        }
    }
}

/// The single fetch path for provider cover images (primary covers via
/// `MetadataProvider::fetch_cover`, variant covers + their backfill via
/// [`download_cover_image`]). SE-6 (WP-6.3):
///
/// - **https only.** ComicVine (`comicvine.gamespot.com/a/uploads/…`) and
///   Metron (`static.metron.cloud/media/…`) both serve covers over https; a
///   plain-http URL — or a redirect hop down to http — is refused instead of
///   being fetched in the clear and persisted.
/// - **magic-sniffed.** The body must carry an image signature; the
///   `Content-Type` header and URL extension are ignored (both are
///   attacker-/CDN-controlled and were how a non-image used to be stored and
///   served as `image/*`).
///
/// SSRF vetting, redirect re-validation, address pinning and the 24 MiB cap
/// are [`crate::util::ssrf::fetch_public_bytes`]'s.
///
/// A host known to answer with a bot challenge is refused up front with
/// [`CoverFetchError::Blocked`] — no request, no retry — and the first
/// challenged response marks it ([`crate::metadata::cover_block`]).
pub async fn fetch_cover_bytes(url: &str) -> Result<Vec<u8>, CoverFetchError> {
    if let Some(host) = crate::metadata::cover_block::blocked_host(url) {
        return Err(CoverFetchError::Blocked { host });
    }
    let fetched = match crate::util::ssrf::fetch_public_bytes(
        url,
        MAX_COVER_BYTES,
        std::time::Duration::from_secs(20),
        crate::build_info::USER_AGENT_COVER,
        2,
        true,
    )
    .await
    {
        Ok(f) => f,
        Err(crate::util::ssrf::FetchBytesError::Challenged { host, .. }) => {
            crate::metadata::cover_block::note_blocked(&host);
            return Err(CoverFetchError::Blocked { host });
        }
        Err(e) => return Err(e.into()),
    };
    if sniff_cover(&fetched.bytes).is_none() {
        return Err(CoverFetchError::NotAnImage);
    }
    Ok(fetched.bytes)
}

/// Relative path (under `data_path`) a cover is stored at. Shared with
/// [`apply_cover`]'s scheme so the serving endpoint + cleanup treat
/// primary and variant covers identically.
fn cover_rel_path(issue_id: &str, cover_id: Uuid, ext: &str) -> String {
    format!("thumbs/issues/{issue_id}/covers/{cover_id}.{ext}")
}

/// GET the image at `url`, decode it, write it under the issue's cover
/// dir, and compute perceptual hashes. Returns `None` on any network /
/// decode / IO failure — the caller soft-falls-back to a metadata-only
/// (hotlink) row that a later backfill can fill in. Decode + write are
/// synchronous (covers are small and applies run off the request path,
/// matching [`apply_cover`]).
async fn download_cover_image(
    data_path: &std::path::Path,
    issue_id: &str,
    url: &str,
) -> Option<StoredCover> {
    let bytes = match fetch_cover_bytes(url).await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::debug!(url, error = %e, "cover download: fetch refused");
            return None;
        }
    };
    if bytes.is_empty() || bytes.len() > MAX_COVER_BYTES {
        tracing::debug!(url, len = bytes.len(), "cover download: empty or oversize");
        return None;
    }
    let img = match crate::util::image_decode::decode_limited(&bytes) {
        Ok(img) => img,
        Err(e) => {
            tracing::debug!(url, error = %e, "cover download: decode failed");
            return None;
        }
    };
    let width = img.width() as i32;
    let height = img.height() as i32;
    let (phash, dhash, ahash) = crate::metadata::phash::all_hashes(&img);
    let cover_id = Uuid::now_v7();
    // Extension (→ served MIME) from the sniffed bytes, not the URL.
    let kind = sniff_cover(&bytes)?;
    let rel_path = cover_rel_path(issue_id, cover_id, kind.ext());
    let on_disk = data_path.join(&rel_path);
    if let Some(parent) = on_disk.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::debug!(path = %parent.display(), error = %e, "cover download: mkdir failed");
        return None;
    }
    if let Err(e) = std::fs::write(&on_disk, &bytes) {
        tracing::debug!(path = %on_disk.display(), error = %e, "cover download: write failed");
        return None;
    }
    Some(StoredCover {
        cover_id,
        rel_path,
        width,
        height,
        phash,
        dhash,
        ahash,
    })
}

/// Persist one [`issue_cover`] row per variant from a provider's
/// `Vec<VariantCoverCandidate>`, **downloading** each image to local
/// storage (mirroring how [`apply_cover`] stores the primary) so the
/// gallery serves bytes from this instance instead of hotlinking the
/// provider CDN. A successful download lands `local_path`, the source
/// dimensions, and perceptual hashes; a failed one soft-falls-back to a
/// metadata-only row (`local_path` empty, `source_url` kept) so the
/// variant still renders via hotlink and [`run_variant_cover_backfill`]
/// can pull the bytes later. Variants with no `image_url` are skipped.
///
/// Idempotency: the issue's whole variant set is **replaced** — the
/// prior rows are deleted and the fresh set inserted in one transaction
/// (downloads happen first, outside it). Variants are presentational — no
/// audit trail needed — so deleting beats deactivating. The prior set's
/// files are removed only after the commit; if the swap fails, the prior
/// rows + files stay and the files just downloaded are removed instead.
/// Primary cover rows are untouched (`apply_cover` owns the
/// `kind='primary'` slot).
///
/// Ordinals are assigned contiguously from 1 (the primary slot is
/// ordinal 0) in provider `Vec` order, so the gallery's
/// `ORDER BY kind, ordinal` shows variants in publisher-supplied
/// sequence. Returns the count of variant rows inserted.
pub async fn set_issue_variants(
    db: &sea_orm::DatabaseConnection,
    data_path: &std::path::Path,
    issue_id: &str,
    variants: &[crate::metadata::provider::VariantCoverCandidate],
    set_by: SetBy,
) -> Result<usize, sea_orm::DbErr> {
    let now = chrono::Utc::now().fixed_offset();
    let provider_str = match set_by {
        SetBy::Provider(s) => Some(s.as_str().to_owned()),
        // For non-provider sources we still emit the variants but
        // leave `source_provider` NULL (the variant has no provider
        // attribution in those cases — typically only the ComicInfo
        // primary is what put it on disk).
        SetBy::User
        | SetBy::ComicInfo
        | SetBy::MetronInfo
        | SetBy::SeriesJson
        | SetBy::ScannerInference
        | SetBy::ScannerFolderTag
        | SetBy::CrossReference => None,
    };
    let mut rows: Vec<issue_cover::ActiveModel> = Vec::new();
    let mut new_files: Vec<std::path::PathBuf> = Vec::new();
    let mut ordinal = 1i32; // primary slot owns ordinal 0
    for v in variants {
        // Skip variants with no image URL — useless to the gallery and
        // pollute the table.
        let Some(image_url) = v.image_url.as_deref().filter(|s| !s.trim().is_empty()) else {
            continue;
        };
        let source_external_id = v.identifiers.first().map(|i| i.id.clone());
        let variant_label = v
            .label
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned);

        let stored = download_cover_image(data_path, issue_id, image_url).await;
        let (id, local_path, width, height, phash, dhash, ahash) = match stored {
            Some(s) => {
                new_files.push(data_path.join(&s.rel_path));
                (
                    s.cover_id,
                    s.rel_path,
                    Some(s.width),
                    Some(s.height),
                    Some(s.phash),
                    Some(s.dhash),
                    Some(s.ahash),
                )
            }
            // Soft fallback: keep the hotlink so the variant still
            // renders; the backfill job pulls the bytes on a later pass.
            None => (Uuid::now_v7(), String::new(), None, None, None, None, None),
        };
        rows.push(issue_cover::ActiveModel {
            id: Set(id),
            issue_id: Set(issue_id.to_owned()),
            kind: Set("variant".into()),
            ordinal: Set(ordinal),
            source_provider: Set(provider_str.clone()),
            source_external_id: Set(source_external_id),
            source_url: Set(Some(image_url.to_owned())),
            variant_label: Set(variant_label),
            variant_artist_person_id: Set(None),
            local_path: Set(local_path),
            width: Set(width),
            height: Set(height),
            phash: Set(phash),
            dhash: Set(dhash),
            ahash: Set(ahash),
            fetched_at: Set(now),
            is_active: Set(true),
        });
        ordinal += 1;
    }
    let inserted = rows.len();

    match replace_variant_rows(db, issue_id, rows).await {
        Ok(prior_paths) => {
            // Committed: the prior set's files are now unreferenced.
            for rel in prior_paths {
                remove_cover_file(&data_path.join(rel));
            }
            Ok(inserted)
        }
        Err(e) => {
            // Rolled back: the prior set is intact; the files downloaded
            // for the new set have no rows.
            for path in &new_files {
                remove_cover_file(path);
            }
            Err(e)
        }
    }
}

/// Swap an issue's variant rows for `rows` in one transaction. Returns the
/// non-empty `local_path`s of the rows it deleted so the caller can remove
/// those files once the swap is durable.
async fn replace_variant_rows(
    db: &sea_orm::DatabaseConnection,
    issue_id: &str,
    rows: Vec<issue_cover::ActiveModel>,
) -> Result<Vec<String>, sea_orm::DbErr> {
    let txn = db.begin().await?;
    let prior_paths: Vec<String> = issue_cover::Entity::find()
        .filter(issue_cover::Column::IssueId.eq(issue_id))
        .filter(issue_cover::Column::Kind.eq("variant"))
        .all(&txn)
        .await?
        .into_iter()
        .map(|r| r.local_path)
        .filter(|p| !p.is_empty())
        .collect();
    issue_cover::Entity::delete_many()
        .filter(issue_cover::Column::IssueId.eq(issue_id))
        .filter(issue_cover::Column::Kind.eq("variant"))
        .exec(&txn)
        .await?;
    for am in rows {
        am.insert(&txn).await?;
    }
    txn.commit().await?;
    Ok(prior_paths)
}

/// Outcome of a variant-cover backfill sweep — surfaced via the admin
/// endpoint so the operator sees how rows fared.
#[derive(Debug, Clone, Default, serde::Serialize, utoipa::ToSchema)]
pub struct VariantCoverBackfillOutcome {
    /// Rows that needed work (no usable on-disk artifact: `local_path`
    /// empty, or set but the file is missing). Rows whose bytes already
    /// exist on disk are skipped before counting.
    pub considered: usize,
    /// Rows whose image downloaded + stored successfully.
    pub stored: usize,
    /// Rows skipped because the download / decode failed (stay hotlinks).
    pub skipped: usize,
}

/// Bounded so a single admin click can't tie up the handler.
pub const VARIANT_BACKFILL_BATCH_CAP: u64 = 500;

/// Download + locally store variant covers that have a `source_url` but
/// no usable on-disk artifact. Two cases qualify:
///   - `local_path = ''` — never stored (hotlink rows applied before
///     local storage shipped, or rows whose original download soft-failed).
///   - `local_path != ''` but the file is **missing on disk** — the row
///     points at bytes that are gone. This is the recovery path for
///     covers reclaimed in error by the thumbnail orphan sweep.
///
/// Used by the admin-triggered backfill and the startup drain. Scans at
/// most [`VARIANT_BACKFILL_BATCH_CAP`] rows per call; rows whose file
/// already exists cost only a `stat` and are skipped. `null` / blank
/// `source_url` rows are unrecoverable and counted as skipped.
///
/// One page from the start of the table — the drain walks the whole
/// backlog with [`run_variant_cover_backfill_page`].
pub async fn run_variant_cover_backfill<C: ConnectionTrait>(
    db: &C,
    data_path: &std::path::Path,
) -> Result<VariantCoverBackfillOutcome, sea_orm::DbErr> {
    run_variant_cover_backfill_page(db, data_path, None)
        .await
        .map(|(outcome, _)| outcome)
}

/// Keyset-paged variant-cover backfill (DI-15). Scans the variant rows with
/// `id > after` in id order, at most [`VARIANT_BACKFILL_BATCH_CAP`], and
/// returns the cursor for the next page (`None` once the table is walked).
///
/// Paging by id is what lets the drain reach the whole backlog: the old
/// single-query shape re-read the same first page every pass, so a page of
/// already-stored rows (or dead URLs) stopped the drain on its
/// "no forward progress" check with recoverable rows still behind it.
pub async fn run_variant_cover_backfill_page<C: ConnectionTrait>(
    db: &C,
    data_path: &std::path::Path,
    after: Option<Uuid>,
) -> Result<(VariantCoverBackfillOutcome, Option<Uuid>), sea_orm::DbErr> {
    let mut query = issue_cover::Entity::find()
        .filter(issue_cover::Column::Kind.eq("variant"))
        .filter(issue_cover::Column::SourceUrl.is_not_null());
    if let Some(after) = after {
        query = query.filter(issue_cover::Column::Id.gt(after));
    }
    let rows = query
        .order_by_asc(issue_cover::Column::Id)
        .limit(VARIANT_BACKFILL_BATCH_CAP)
        .all(db)
        .await?;
    let next = if rows.len() as u64 == VARIANT_BACKFILL_BATCH_CAP {
        rows.last().map(|r| r.id)
    } else {
        None
    };
    let outcome = variant_backfill_rows(db, data_path, rows).await?;
    Ok((outcome, next))
}

async fn variant_backfill_rows<C: ConnectionTrait>(
    db: &C,
    data_path: &std::path::Path,
    rows: Vec<issue_cover::Model>,
) -> Result<VariantCoverBackfillOutcome, sea_orm::DbErr> {
    let mut outcome = VariantCoverBackfillOutcome::default();
    for row in rows {
        // Already have the bytes on disk — nothing to do.
        if !row.local_path.is_empty() && data_path.join(&row.local_path).exists() {
            continue;
        }
        outcome.considered += 1;
        let Some(url) = row.source_url.as_deref().filter(|s| !s.trim().is_empty()) else {
            outcome.skipped += 1;
            continue;
        };
        match download_cover_image(data_path, &row.issue_id, url).await {
            Some(stored) => {
                let mut am: issue_cover::ActiveModel = row.into();
                am.local_path = Set(stored.rel_path);
                am.width = Set(Some(stored.width));
                am.height = Set(Some(stored.height));
                am.phash = Set(Some(stored.phash));
                am.dhash = Set(Some(stored.dhash));
                am.ahash = Set(Some(stored.ahash));
                am.update(db).await?;
                outcome.stored += 1;
            }
            None => outcome.skipped += 1,
        }
    }
    Ok(outcome)
}

// ─────────────────────────────────────────────────────────────────
// CSV cache rebuild (debounced per transaction).
// ─────────────────────────────────────────────────────────────────

/// Batches `(issue_id)` keys queued during a single transaction so a
/// flurry of `set_issue_*` calls flushes one CSV rebuild per touched
/// issue (not one per junction-table write).
///
/// Use:
///   1. Construct at the top of a write path.
///   2. Pass `&CsvRebuildBatch` into every `set_issue_*` call.
///   3. Call [`CsvRebuildBatch::flush`] once the transaction commits.
///
/// Dropping the batch *without* calling flush is a no-op — the CSV
/// columns just stay stale until the next scan touches them, which
/// is acceptable but defeats the read-cache invariant. In production
/// code paths, always flush.
pub struct CsvRebuildBatch {
    issue_ids: Mutex<HashSet<String>>,
}

impl CsvRebuildBatch {
    pub fn new() -> Self {
        Self {
            issue_ids: Mutex::new(HashSet::new()),
        }
    }

    pub fn queue(&self, issue_id: &str) {
        if let Ok(mut s) = self.issue_ids.lock() {
            s.insert(issue_id.to_owned());
        }
    }

    /// Take ownership of the queued set; the batch is empty after.
    pub fn drain(&self) -> Vec<String> {
        if let Ok(mut s) = self.issue_ids.lock() {
            let drained: Vec<String> = s.drain().collect();
            drained
        } else {
            Vec::new()
        }
    }

    /// Rebuild the CSV read-cache for every queued issue. Caller
    /// chooses when to call — typically right before commit.
    pub async fn flush<C: ConnectionTrait>(&self, db: &C) -> Result<(), DbErr> {
        for issue_id in self.drain() {
            rebuild_issue_csv_cache(db, &issue_id).await?;
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.issue_ids.lock().map(|s| s.is_empty()).unwrap_or(true)
    }
}

impl Default for CsvRebuildBatch {
    fn default() -> Self {
        Self::new()
    }
}

/// Rebuild the denormalized CSV columns on `issues` from the
/// junction tables. The CSVs are read-only-cache post-M0; this
/// helper is the only writer.
///
/// Credits are split across the eight role columns
/// (`writer` / `penciller` / `inker` / `colorist` / `letterer` /
/// `cover_artist` / `editor` / `translator`). Other CSVs join one
/// row per per-junction entity, comma-separated, alphabetised so
/// the cache is deterministic.
/// The `SET …` clause of the CSV read-cache rebuild, correlated on
/// `issues.id` so one text serves both the per-issue and the per-series
/// statement. Names are joined with `, ` — or with `; ` when any name in
/// the list contains a comma (`"Capes, Inc."`), the same rule
/// `split_csv` / the sidecar composer use, so every consumer that splits
/// the column recovers the exact set.
fn csv_cache_set_clause() -> String {
    // `order` is the ORDER BY key list inside string_agg. Credits keep the
    // stored order (`issue_credits.ordinal` — the file's / provider's
    // sequence), everything else sorts by name.
    fn agg_by(expr: &str, order: &str) -> String {
        format!(
            "NULLIF(CASE WHEN bool_or({expr} LIKE '%,%') \
                 THEN string_agg({expr}, '; ' ORDER BY {order}) \
                 ELSE string_agg({expr}, ', ' ORDER BY {order}) END, '')"
        )
    }
    // LEFT JOIN + COALESCE: a file-tier row whose entity id hasn't been
    // linked yet (the per-issue write runs before the series rollup links
    // ids) still contributes its own stored name instead of vanishing.
    let credit = |role: &str| {
        format!(
            "(SELECT {} FROM issue_credits ic LEFT JOIN person p ON p.id = ic.person_id \
              WHERE ic.issue_id = issues.id AND ic.role = '{role}')",
            agg_by(
                "COALESCE(p.name, ic.person)",
                "ic.ordinal, COALESCE(p.name, ic.person)"
            )
        )
    };
    format!(
        "writer = {w}, penciller = {pe}, inker = {i}, colorist = {c}, letterer = {l}, \
         cover_artist = {ca}, editor = {e}, translator = {t}, \
         characters = (SELECT {ch} FROM issue_characters ich \
                        LEFT JOIN character c ON c.id = ich.character_id \
                        WHERE ich.issue_id = issues.id), \
         teams = (SELECT {tm} FROM issue_teams it LEFT JOIN team t ON t.id = it.team_id \
                   WHERE it.issue_id = issues.id), \
         locations = (SELECT {lo} FROM issue_locations il \
                       LEFT JOIN location l ON l.id = il.location_id \
                       WHERE il.issue_id = issues.id), \
         story_arc = (SELECT {sa} FROM issue_arcs ia JOIN story_arc sa ON sa.id = ia.arc_id \
                       WHERE ia.issue_id = issues.id), \
         genre = (SELECT {g} FROM issue_genres WHERE issue_id = issues.id), \
         tags = (SELECT {tg} FROM issue_tags WHERE issue_id = issues.id)",
        w = credit("writer"),
        pe = credit("penciller"),
        i = credit("inker"),
        c = credit("colorist"),
        l = credit("letterer"),
        ca = credit("cover_artist"),
        e = credit("editor"),
        t = credit("translator"),
        ch = agg_by(
            "COALESCE(c.name, ich.\"character\")",
            "ich.ordinal, COALESCE(c.name, ich.\"character\")"
        ),
        tm = agg_by(
            "COALESCE(t.name, it.team)",
            "it.ordinal, COALESCE(t.name, it.team)"
        ),
        lo = agg_by(
            "COALESCE(l.name, il.location)",
            "il.ordinal, COALESCE(l.name, il.location)"
        ),
        sa = agg_by("sa.name", "ia.position_in_arc NULLS LAST, sa.name"),
        g = agg_by("genre", "ordinal, genre"),
        tg = agg_by("tag", "ordinal, tag"),
    )
}

pub async fn rebuild_issue_csv_cache<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
) -> Result<(), DbErr> {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!("UPDATE issues SET {} WHERE id = $1", csv_cache_set_clause()),
        [issue_id.into()],
    );
    db.execute_raw(stmt).await?;
    Ok(())
}

/// Rebuild the CSV read-cache for every active issue of one series in a
/// single set-based statement. The scanner's series rollup calls this
/// after it has created the `person` / entity rows and linked their ids
/// onto the junctions, so a file-tagged issue's columns end up holding
/// the normalized names (`"Mike Deodato Jr."`) the junctions hold — the
/// junction tables are the one source of truth for *every* write path,
/// and the columns are strictly derived. The file's literal values stay
/// in `comic_info_raw`. Returns the number of issue rows updated.
pub async fn rebuild_series_issue_csv_cache<C: ConnectionTrait>(
    db: &C,
    series_id: uuid::Uuid,
) -> Result<u64, DbErr> {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!(
            "UPDATE issues SET {} WHERE series_id = $1 AND state = 'active' AND removed_at IS NULL",
            csv_cache_set_clause()
        ),
        [series_id.into()],
    );
    Ok(db.execute_raw(stmt).await?.rows_affected())
}
