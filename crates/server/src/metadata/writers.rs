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
    character, concept, external_id, field_provenance, imprint, issue_arc, issue_character,
    issue_concept, issue_cover, issue_credit, issue_genre, issue_location, issue_object,
    issue_reprint, issue_tag, issue_team, issue_universe, location, object, person, publisher,
    series_arc, series_character, series_concept, series_location, series_object, series_team,
    series_universe, story_arc, team, universe,
};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend, DbErr,
    EntityTrait, FromQueryResult, QueryFilter, Statement,
};
use std::collections::HashSet;
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
            SetBy::Provider(s) => s.as_str().into(),
            SetBy::ScannerInference => "scanner_inference".into(),
            SetBy::ScannerFolderTag => "scanner_folder_tag".into(),
            SetBy::CrossReference => "cross_reference".into(),
        }
    }
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

/// Internal: write the `external_ids` row, computing the canonical
/// URL when the caller didn't supply one. Honors set_by precedence
/// — never overwrites a `set_by='user'` row with a non-user write.
async fn put_external_id<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
    identifier: &Identifier,
    set_by: SetBy,
) -> Result<(), DbErr> {
    let url = identifier.url.clone().or_else(|| {
        crate::metadata::identifier::canonical_url(identifier.source, entity_type, &identifier.id)
    });
    let now = chrono::Utc::now().fixed_offset();

    // If a row exists with set_by='user' and the value disagrees,
    // skip (user wins). Same-value writes always pass through to
    // refresh last_synced_at.
    if let Some(existing) = external_id::Entity::find()
        .filter(external_id::Column::EntityType.eq(entity_type))
        .filter(external_id::Column::EntityId.eq(entity_id))
        .filter(external_id::Column::Source.eq(identifier.source.as_str()))
        .one(db)
        .await?
        && existing.set_by == SetBy::User.as_str()
        && existing.external_id != identifier.id
        && set_by != SetBy::User
    {
        tracing::debug!(
            entity_type = entity_type,
            entity_id = entity_id,
            source = identifier.source.as_str(),
            "skipping external_id write: user-set value differs"
        );
        return Ok(());
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
    Ok(())
}

/// Public surface for external-ID writes — used by the
/// `<ExternalIdsCard>` CRUD endpoints (M5) and Apply jobs (M4).
pub async fn set_external_id<C: ConnectionTrait>(
    db: &C,
    entity_type: &str,
    entity_id: &str,
    identifier: &Identifier,
    set_by: SetBy,
) -> Result<(), DbErr> {
    put_external_id(db, entity_type, entity_id, identifier, set_by).await
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
) -> Result<(), DbErr> {
    if let Some(cv) = comicvine {
        set_external_id(
            db,
            entity_type,
            entity_id,
            &Identifier::new(Source::ComicVine, cv.to_string()),
            set_by,
        )
        .await?;
    }
    if let Some(m) = metron {
        set_external_id(
            db,
            entity_type,
            entity_id,
            &Identifier::new(Source::Metron, m.to_string()),
            set_by,
        )
        .await?;
    }
    if let Some(g) = gtin
        && !g.is_empty()
    {
        set_external_id(
            db,
            entity_type,
            entity_id,
            &Identifier::new(Source::Gtin, g),
            set_by,
        )
        .await?;
    }
    Ok(())
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
    let am = field_provenance::ActiveModel {
        entity_type: Set(entity_type.into()),
        entity_id: Set(entity_id.into()),
        field: Set(field.key()),
        set_by: Set(set_by.as_str()),
        set_at: Set(chrono::Utc::now().fixed_offset()),
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
                        put_external_id(db, $entity_type, &existing_id, ident, set_by).await?;
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
                    put_external_id(db, $entity_type, &entity_id_str, ident, set_by).await?;
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
                put_external_id(db, $entity_type, &id.to_string(), ident, set_by).await?;
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
                put_external_id(db, "imprint", &existing_id, ident, set_by).await?;
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
            put_external_id(db, "imprint", &entity_id_str, ident, set_by).await?;
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
        put_external_id(db, "imprint", &id.to_string(), ident, set_by).await?;
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
    if !credits.is_empty() {
        let rows: Vec<issue_credit::ActiveModel> = credits
            .into_iter()
            .map(|(person_id, role, ordinal)| issue_credit::ActiveModel {
                issue_id: Set(issue_id.into()),
                role: Set(role),
                // Junction PK is `(issue_id, role, person)` with
                // `person` as the legacy TEXT name column. Stash the
                // person UUID as text so multi-person-per-role
                // inserts don't collide on the PK. CSV rebuild reads
                // names from the `person` table via `person_id`, not
                // this column. Follow-up cleanup (post-M0c) will
                // migrate the PK to use `person_id` directly.
                person: Set(person_id.to_string()),
                person_id: Set(Some(person_id)),
                ordinal: Set(ordinal),
            })
            .collect();
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
            .do_nothing()
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
            .map(
                |(character_id, is_first, died)| issue_character::ActiveModel {
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
            .do_nothing()
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
            .map(|(team_id, is_first, disbanded)| issue_team::ActiveModel {
                issue_id: Set(issue_id.into()),
                team: Set(team_id.to_string()),
                team_id: Set(Some(team_id)),
                is_first_appearance: Set(is_first),
                disbanded_in_issue: Set(disbanded),
            })
            .collect();
        issue_team::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([issue_team::Column::IssueId, issue_team::Column::Team])
                    .do_nothing()
                    .to_owned(),
            )
            .do_nothing()
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
            .map(|(location_id, is_first)| issue_location::ActiveModel {
                issue_id: Set(issue_id.into()),
                location: Set(location_id.to_string()),
                location_id: Set(Some(location_id)),
                is_first_appearance: Set(is_first),
            })
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
            .do_nothing()
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
            .do_nothing()
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
            .do_nothing()
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
            .do_nothing()
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
            .do_nothing()
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

/// Per-reprint: (reprinted_issue_id, reprinted_label).
/// At least one of the two must be `Some` (DB-level CHECK
/// constraint mirrors this).
pub type ReprintSpec = (Option<String>, Option<String>);

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
    if !reprints.is_empty() {
        let rows: Vec<issue_reprint::ActiveModel> = reprints
            .into_iter()
            .filter(|(target, label)| target.is_some() || label.is_some())
            .map(
                |(reprinted_issue_id, reprinted_label)| issue_reprint::ActiveModel {
                    id: Set(Uuid::now_v7()),
                    issue_id: Set(issue_id.into()),
                    reprinted_issue_id: Set(reprinted_issue_id),
                    reprinted_label: Set(reprinted_label),
                },
            )
            .collect();
        if !rows.is_empty() {
            issue_reprint::Entity::insert_many(rows).exec(db).await?;
        }
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
            .map(|g| issue_genre::ActiveModel {
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
            .do_nothing()
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
            .map(|t| issue_tag::ActiveModel {
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
            .do_nothing()
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
            .do_nothing()
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
            .do_nothing()
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
            .do_nothing()
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
            .do_nothing()
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
            .do_nothing()
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
            .do_nothing()
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
            .do_nothing()
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
    pub bytes: &'a [u8],
    /// File extension (no leading dot) — e.g. `"webp"` / `"jpg"`.
    pub ext: &'a str,
    /// Width / height in pixels.
    pub width: Option<i32>,
    pub height: Option<i32>,
}

/// Persist a cover row + write the image bytes to disk. Honors
/// [`CoverOverwritePolicy`] for `kind='primary' AND ordinal=0`;
/// variants are always additive (the policy doesn't apply).
pub async fn apply_cover<C: ConnectionTrait>(
    db: &C,
    data_path: &std::path::Path,
    write: CoverWrite<'_>,
    policy: CoverOverwritePolicy,
) -> Result<Option<Uuid>, std::io::Error> {
    let is_primary = write.kind == "primary" && write.ordinal == 0;

    // Policy gate applies only to the primary slot.
    if is_primary {
        let existing_primary = issue_cover::Entity::find()
            .filter(issue_cover::Column::IssueId.eq(write.issue_id))
            .filter(issue_cover::Column::Kind.eq("primary"))
            .filter(issue_cover::Column::Ordinal.eq(0))
            .filter(issue_cover::Column::IsActive.eq(true))
            .one(db)
            .await
            .map_err(|e| std::io::Error::other(format!("issue_cover lookup: {e}")))?;
        match (existing_primary.as_ref(), policy) {
            (Some(_), CoverOverwritePolicy::Never) => return Ok(None),
            (Some(_), CoverOverwritePolicy::WhenMissing) => return Ok(None),
            _ => {}
        }
        // Replacing: deactivate the existing row so unique
        // (issue_id, kind, ordinal) doesn't fire.
        if let Some(prev) = existing_primary {
            let mut am: issue_cover::ActiveModel = prev.into();
            am.is_active = Set(false);
            am.update(db)
                .await
                .map_err(|e| std::io::Error::other(format!("issue_cover deactivate: {e}")))?;
        }
    }

    let cover_id = Uuid::now_v7();
    let rel_dir = format!("thumbs/issues/{}/covers", write.issue_id);
    let rel_path = format!("{rel_dir}/{cover_id}.{}", write.ext);
    let on_disk = data_path.join(&rel_path);
    if let Some(parent) = on_disk.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&on_disk, write.bytes)?;

    // metadata-providers-1.0 M9: compute perceptual hashes on the
    // bytes as we write. Decode failures don't block the cover write
    // — phash columns stay NULL and the backfill job can recover
    // later. The decode is in-thread because the bytes are already
    // in memory; for archive covers the post-scan thumbnail job
    // hashes from the resized buffer instead.
    let (p, d, a) = match image::load_from_memory(write.bytes) {
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
    am.insert(db)
        .await
        .map_err(|e| std::io::Error::other(format!("issue_cover insert: {e}")))?;
    Ok(Some(cover_id))
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
// Variant covers.
// ─────────────────────────────────────────────────────────────────

/// Upsert one [`issue_cover`] row per variant from a provider's
/// `Vec<VariantCoverCandidate>`. Variants are stored *metadata-only*
/// — `source_url` (CDN) and `variant_label` are persisted, but no
/// bytes are downloaded. The UI's [`web/components/library/
/// CoverGallery.tsx`] renders straight from `source_url`.
///
/// Idempotency: every existing variant row for the issue is
/// **deleted** before inserting the fresh set. Variants are
/// presentational — no audit trail needed — and the table has a
/// `UNIQUE (issue_id, kind, ordinal)` constraint that would block a
/// re-apply if we merely deactivated. Primary cover rows are
/// untouched (`apply_cover` handles that slot via `kind='primary'`).
///
/// Ordinals start at 1 (the primary slot is ordinal 0 with kind
/// `'primary'`). Insert order matches the provider's `Vec` order so
/// the gallery's `ORDER BY kind, ordinal` displays variants in
/// publisher-supplied sequence.
///
/// Returns the count of variant rows inserted.
pub async fn set_issue_variants<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    variants: &[crate::metadata::provider::VariantCoverCandidate],
    set_by: SetBy,
) -> Result<usize, sea_orm::DbErr> {
    // Delete every existing variant row so the upsert reflects only
    // the provider's current variant set. Primary rows
    // (`kind='primary'`) are not touched.
    issue_cover::Entity::delete_many()
        .filter(issue_cover::Column::IssueId.eq(issue_id))
        .filter(issue_cover::Column::Kind.eq("variant"))
        .exec(db)
        .await?;

    if variants.is_empty() {
        return Ok(0);
    }

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
        | SetBy::ScannerInference
        | SetBy::ScannerFolderTag
        | SetBy::CrossReference => None,
    };
    let mut inserted = 0usize;
    for (idx, v) in variants.iter().enumerate() {
        // Skip variants with no image URL — they're useless to the
        // gallery surface and pollute the table.
        let Some(image_url) = v.image_url.as_deref().filter(|s| !s.trim().is_empty()) else {
            continue;
        };
        let primary_ident = v.identifiers.first();
        let am = issue_cover::ActiveModel {
            id: Set(Uuid::now_v7()),
            issue_id: Set(issue_id.to_owned()),
            kind: Set("variant".into()),
            // 1-based; the primary slot owns ordinal 0.
            ordinal: Set((idx + 1) as i32),
            source_provider: Set(provider_str.clone()),
            source_external_id: Set(primary_ident.map(|i| i.id.clone())),
            source_url: Set(Some(image_url.to_owned())),
            variant_label: Set(v
                .label
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)),
            variant_artist_person_id: Set(None),
            // Metadata-only: no bytes downloaded. The gallery renders
            // from `source_url`; a future backfill job could populate
            // `local_path` if/when CDN takedowns become a real concern.
            local_path: Set(String::new()),
            width: Set(None),
            height: Set(None),
            phash: Set(None),
            dhash: Set(None),
            ahash: Set(None),
            fetched_at: Set(now),
            is_active: Set(true),
        };
        am.insert(db).await?;
        inserted += 1;
    }
    Ok(inserted)
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
pub async fn rebuild_issue_csv_cache<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
) -> Result<(), DbErr> {
    // Single UPDATE pulling values from the junctions via subselects.
    // Sea-orm doesn't model this neatly so we use raw SQL.
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        r#"
        UPDATE issues SET
            writer = (
                SELECT NULLIF(string_agg(p.name, ', ' ORDER BY p.name), '')
                FROM issue_credits ic JOIN person p ON p.id = ic.person_id
                WHERE ic.issue_id = $1 AND ic.role = 'writer'
            ),
            penciller = (
                SELECT NULLIF(string_agg(p.name, ', ' ORDER BY p.name), '')
                FROM issue_credits ic JOIN person p ON p.id = ic.person_id
                WHERE ic.issue_id = $1 AND ic.role = 'penciller'
            ),
            inker = (
                SELECT NULLIF(string_agg(p.name, ', ' ORDER BY p.name), '')
                FROM issue_credits ic JOIN person p ON p.id = ic.person_id
                WHERE ic.issue_id = $1 AND ic.role = 'inker'
            ),
            colorist = (
                SELECT NULLIF(string_agg(p.name, ', ' ORDER BY p.name), '')
                FROM issue_credits ic JOIN person p ON p.id = ic.person_id
                WHERE ic.issue_id = $1 AND ic.role = 'colorist'
            ),
            letterer = (
                SELECT NULLIF(string_agg(p.name, ', ' ORDER BY p.name), '')
                FROM issue_credits ic JOIN person p ON p.id = ic.person_id
                WHERE ic.issue_id = $1 AND ic.role = 'letterer'
            ),
            cover_artist = (
                SELECT NULLIF(string_agg(p.name, ', ' ORDER BY p.name), '')
                FROM issue_credits ic JOIN person p ON p.id = ic.person_id
                WHERE ic.issue_id = $1 AND ic.role = 'cover_artist'
            ),
            editor = (
                SELECT NULLIF(string_agg(p.name, ', ' ORDER BY p.name), '')
                FROM issue_credits ic JOIN person p ON p.id = ic.person_id
                WHERE ic.issue_id = $1 AND ic.role = 'editor'
            ),
            translator = (
                SELECT NULLIF(string_agg(p.name, ', ' ORDER BY p.name), '')
                FROM issue_credits ic JOIN person p ON p.id = ic.person_id
                WHERE ic.issue_id = $1 AND ic.role = 'translator'
            ),
            characters = (
                SELECT NULLIF(string_agg(c.name, ', ' ORDER BY c.name), '')
                FROM issue_characters ich JOIN character c ON c.id = ich.character_id
                WHERE ich.issue_id = $1
            ),
            teams = (
                SELECT NULLIF(string_agg(t.name, ', ' ORDER BY t.name), '')
                FROM issue_teams it JOIN team t ON t.id = it.team_id
                WHERE it.issue_id = $1
            ),
            locations = (
                SELECT NULLIF(string_agg(l.name, ', ' ORDER BY l.name), '')
                FROM issue_locations il JOIN location l ON l.id = il.location_id
                WHERE il.issue_id = $1
            ),
            story_arc = (
                SELECT NULLIF(string_agg(sa.name, ', ' ORDER BY sa.name), '')
                FROM issue_arcs ia JOIN story_arc sa ON sa.id = ia.arc_id
                WHERE ia.issue_id = $1
            ),
            genre = (
                SELECT NULLIF(string_agg(genre, ', ' ORDER BY genre), '')
                FROM issue_genres WHERE issue_id = $1
            ),
            tags = (
                SELECT NULLIF(string_agg(tag, ', ' ORDER BY tag), '')
                FROM issue_tags WHERE issue_id = $1
            )
        WHERE id = $1
        "#,
        [issue_id.into()],
    );
    db.execute(stmt).await?;
    Ok(())
}
