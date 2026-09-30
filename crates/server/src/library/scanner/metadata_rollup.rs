//! Saved smart views — M1 scanner rollup.
//!
//! Replaces the CSV-shaped per-issue metadata fields (`genre`, `tags`, plus
//! the eight credit roles) into the normalized junction tables added by
//! migration `m20261203_000001_metadata_junctions`. Two write paths:
//!
//!   - `replace_issue_metadata` — called from the per-issue upsert in
//!     `process::ingest_one_with_fingerprint`. Wipes and re-writes a single
//!     issue's `issue_genres / issue_tags / issue_credits` rows from the
//!     ComicInfo CSVs (or the user-edited values if the column is sticky).
//!   - `rollup_series_metadata` — called once per series after a folder scan
//!     completes. Recomputes `series_genres / series_tags / series_credits`
//!     as the distinct union of the series's active issues' junctions.
//!
//! Series-level rows are pure aggregations: there is no admin override path.
//! Editing a series's surfaced genres means editing the underlying issues.

use entity::{
    issue, issue_character, issue_credit, issue_genre, issue_location, issue_tag, issue_team,
    series_character, series_credit, series_genre, series_location, series_tag, series_team,
};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult, QueryFilter,
    Statement, Value, sea_query::OnConflict,
};
use std::collections::HashSet;
use uuid::Uuid;

use crate::slug::allocate_person_slug;

/// The eight ComicInfo credit roles, in display order. Keep this list in
/// lockstep with the values used by the saved-views filter registry — the
/// strings are the public role identifiers.
pub const CREDIT_ROLES: &[CreditRole] = &[
    CreditRole::Writer,
    CreditRole::Penciller,
    CreditRole::Inker,
    CreditRole::Colorist,
    CreditRole::Letterer,
    CreditRole::CoverArtist,
    CreditRole::Editor,
    CreditRole::Translator,
];

#[derive(Debug, Clone, Copy)]
pub enum CreditRole {
    Writer,
    Penciller,
    Inker,
    Colorist,
    Letterer,
    CoverArtist,
    Editor,
    Translator,
}

impl CreditRole {
    pub fn as_str(self) -> &'static str {
        match self {
            CreditRole::Writer => "writer",
            CreditRole::Penciller => "penciller",
            CreditRole::Inker => "inker",
            CreditRole::Colorist => "colorist",
            CreditRole::Letterer => "letterer",
            CreditRole::CoverArtist => "cover_artist",
            CreditRole::Editor => "editor",
            CreditRole::Translator => "translator",
        }
    }
}

/// Resolved per-issue metadata values, post-user-edited check. `genre` /
/// `tags` carry the raw CSV strings as written to the issue row; each credit
/// role likewise holds its raw CSV. Splitting + dedup + write happens here.
#[derive(Debug, Default, Clone)]
pub struct IssueMetadataInputs<'a> {
    pub genre: Option<&'a str>,
    pub tags: Option<&'a str>,
    pub writer: Option<&'a str>,
    pub penciller: Option<&'a str>,
    pub inker: Option<&'a str>,
    pub colorist: Option<&'a str>,
    pub letterer: Option<&'a str>,
    pub cover_artist: Option<&'a str>,
    pub editor: Option<&'a str>,
    pub translator: Option<&'a str>,
    /// `<Characters>` from ComicInfo — CSV. Written to `issue_characters`
    /// and rolled up to `series_characters` so saved-view filters can
    /// match against character names.
    pub characters: Option<&'a str>,
    /// `<Teams>` from ComicInfo — CSV. Written to `issue_teams` and
    /// rolled up to `series_teams`.
    pub teams: Option<&'a str>,
    /// `<Locations>` from ComicInfo — CSV. Written to `issue_locations`
    /// and rolled up to `series_locations`.
    pub locations: Option<&'a str>,
}

impl<'a> IssueMetadataInputs<'a> {
    fn credit_csv(&self, role: CreditRole) -> Option<&'a str> {
        match role {
            CreditRole::Writer => self.writer,
            CreditRole::Penciller => self.penciller,
            CreditRole::Inker => self.inker,
            CreditRole::Colorist => self.colorist,
            CreditRole::Letterer => self.letterer,
            CreditRole::CoverArtist => self.cover_artist,
            CreditRole::Editor => self.editor,
            CreditRole::Translator => self.translator,
        }
    }
}

/// Split a CSV-shaped ComicInfo field into trimmed, deduped pieces.
///
/// ComicInfo's `<Teams>` / `<Characters>` / `<Genre>` / etc. are flat
/// strings; a name like `"Capes, Inc."` cannot survive a naive
/// `,`-split. We pick the separator per-value:
///
///   - if the input contains `;`, split on `;` only (each piece may
///     contain commas — `"Capes, Inc.; Comet Twins"` → 2 pieces)
///   - otherwise split on `,` (the conventional ComicInfo case)
///
/// The sidecar composer and scanner both **write** with the matching
/// rule (`; `-join when any name contains a comma, `, ` otherwise), so
/// round-trip is lossless. Dedupe is case-insensitive, first casing
/// wins. Empty pieces dropped.
pub fn split_csv(value: &str) -> Vec<String> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    let sep: char = if value.contains(';') { ';' } else { ',' };
    for piece in value.split(sep) {
        let trimmed = piece.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_lowercase();
        if seen.insert(key) {
            out.push(trimmed.to_string());
        }
    }
    out
}

/// Replace this issue's rows in `issue_genres / issue_tags / issue_credits`
/// to match the given parsed values. Idempotent: re-running with identical
/// inputs leaves the database byte-equal.
///
/// **F-10 short-circuit**: each junction first fetches the existing set and
/// compares to the desired set. When equal, no DELETE/INSERT fires. This is
/// the common case on rescans where ComicInfo hasn't changed and saves
/// ~3 redundant DELETEs + 3 redundant INSERTs per unchanged archive.
/// Cost on cold scans: 3 cheap SELECTs per archive (existing set is always
/// empty for new rows, so we still fall through to INSERT).
pub async fn replace_issue_metadata<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    inputs: &IssueMetadataInputs<'_>,
) -> Result<(), sea_orm::DbErr> {
    replace_issue_metadata_skipping(db, issue_id, inputs, &std::collections::HashSet::new()).await
}

/// [`replace_issue_metadata`] that leaves the junctions named in `skip`
/// untouched. The scanner passes the junction fields whose
/// `field_provenance` is user- or provider-owned (roadmap WP-2.5): those
/// rows were written by `writers::set_issue_*` with person ids and
/// ordinals, and rebuilding them from the CSV read-cache would both drop
/// that detail and, because the two shapes never compare equal, churn
/// on every rescan.
pub async fn replace_issue_metadata_skipping<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
    inputs: &IssueMetadataInputs<'_>,
    skip: &std::collections::HashSet<crate::metadata::MetadataField>,
) -> Result<(), sea_orm::DbErr> {
    use crate::metadata::MetadataField as F;
    use std::collections::HashSet;

    // ───── genres ─────
    if !skip.contains(&F::Genres) {
        let desired_genres: Vec<String> = inputs.genre.map(split_csv).unwrap_or_default();
        let existing_genres: HashSet<String> = issue_genre::Entity::find()
            .filter(issue_genre::Column::IssueId.eq(issue_id))
            .all(db)
            .await?
            .into_iter()
            .map(|r| r.genre)
            .collect();
        let desired_genres_set: HashSet<String> = desired_genres.iter().cloned().collect();
        if desired_genres_set != existing_genres {
            issue_genre::Entity::delete_many()
                .filter(issue_genre::Column::IssueId.eq(issue_id))
                .exec(db)
                .await?;
            if !desired_genres.is_empty() {
                let rows: Vec<issue_genre::ActiveModel> = desired_genres
                    .into_iter()
                    .map(|g| issue_genre::ActiveModel {
                        issue_id: Set(issue_id.to_string()),
                        genre: Set(g),
                    })
                    .collect();
                issue_genre::Entity::insert_many(rows)
                    .on_conflict(
                        OnConflict::columns([
                            issue_genre::Column::IssueId,
                            issue_genre::Column::Genre,
                        ])
                        .do_nothing()
                        .to_owned(),
                    )
                    .try_insert()
                    .exec(db)
                    .await?;
            }
        }
    }

    // ───── tags ─────
    if !skip.contains(&F::Tags) {
        let desired_tags: Vec<String> = inputs.tags.map(split_csv).unwrap_or_default();
        let existing_tags: HashSet<String> = issue_tag::Entity::find()
            .filter(issue_tag::Column::IssueId.eq(issue_id))
            .all(db)
            .await?
            .into_iter()
            .map(|r| r.tag)
            .collect();
        let desired_tags_set: HashSet<String> = desired_tags.iter().cloned().collect();
        if desired_tags_set != existing_tags {
            issue_tag::Entity::delete_many()
                .filter(issue_tag::Column::IssueId.eq(issue_id))
                .exec(db)
                .await?;
            if !desired_tags.is_empty() {
                let rows: Vec<issue_tag::ActiveModel> = desired_tags
                    .into_iter()
                    .map(|t| issue_tag::ActiveModel {
                        issue_id: Set(issue_id.to_string()),
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
        }
    }

    // ───── credits ─────
    if !skip.contains(&F::Credits) {
        let mut desired_credits: Vec<(String, String)> = Vec::new();
        for role in CREDIT_ROLES {
            let Some(csv) = inputs.credit_csv(*role) else {
                continue;
            };
            for person in split_csv(csv) {
                desired_credits.push((role.as_str().to_string(), person));
            }
        }
        let existing_credits: HashSet<(String, String)> = issue_credit::Entity::find()
            .filter(issue_credit::Column::IssueId.eq(issue_id))
            .all(db)
            .await?
            .into_iter()
            .map(|r| (r.role, r.person))
            .collect();
        let desired_credits_set: HashSet<(String, String)> =
            desired_credits.iter().cloned().collect();
        if desired_credits_set != existing_credits {
            issue_credit::Entity::delete_many()
                .filter(issue_credit::Column::IssueId.eq(issue_id))
                .exec(db)
                .await?;
            if !desired_credits.is_empty() {
                let rows: Vec<issue_credit::ActiveModel> = desired_credits
                    .into_iter()
                    .map(|(role, person)| issue_credit::ActiveModel {
                        issue_id: Set(issue_id.to_string()),
                        role: Set(role),
                        person: Set(person),
                        // person_id is populated during the series-level
                        // rollup (see `ensure_persons_for_series`), which
                        // runs after this per-issue write. Leaving it
                        // NULL here keeps this hot path off the slug
                        // allocator.
                        person_id: Set(None),
                        // Scanner has no per-credit ordering signal
                        // (ComicInfo lists writers in a single CSV);
                        // default 0 is correct. M4 Apply jobs populate
                        // the real ordinal from provider responses
                        // (Metron credits expose stable ordering).
                        ordinal: Set(0),
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
                    .try_insert()
                    .exec(db)
                    .await?;
            }
        }
    }

    // ───── characters ─────
    if !skip.contains(&F::Characters) {
        let desired_characters: Vec<String> = inputs.characters.map(split_csv).unwrap_or_default();
        let existing_characters: HashSet<String> = issue_character::Entity::find()
            .filter(issue_character::Column::IssueId.eq(issue_id))
            .all(db)
            .await?
            .into_iter()
            .map(|r| r.character)
            .collect();
        let desired_characters_set: HashSet<String> = desired_characters.iter().cloned().collect();
        if desired_characters_set != existing_characters {
            issue_character::Entity::delete_many()
                .filter(issue_character::Column::IssueId.eq(issue_id))
                .exec(db)
                .await?;
            if !desired_characters.is_empty() {
                let rows: Vec<issue_character::ActiveModel> = desired_characters
                    .into_iter()
                    .map(|c| issue_character::ActiveModel {
                        issue_id: Set(issue_id.to_string()),
                        character: Set(c),
                        // M4 Apply jobs are the first writer to
                        // populate character_id (via writers::upsert_character)
                        // and the first-appearance / died-in-issue
                        // flags (from provider responses). ComicInfo has
                        // no such signal; scanner writes NULL/false.
                        character_id: Set(None),
                        is_first_appearance: Set(false),
                        died_in_issue: Set(false),
                    })
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
        }
    }

    // ───── teams ─────
    if !skip.contains(&F::Teams) {
        let desired_teams: Vec<String> = inputs.teams.map(split_csv).unwrap_or_default();
        let existing_teams: HashSet<String> = issue_team::Entity::find()
            .filter(issue_team::Column::IssueId.eq(issue_id))
            .all(db)
            .await?
            .into_iter()
            .map(|r| r.team)
            .collect();
        let desired_teams_set: HashSet<String> = desired_teams.iter().cloned().collect();
        if desired_teams_set != existing_teams {
            issue_team::Entity::delete_many()
                .filter(issue_team::Column::IssueId.eq(issue_id))
                .exec(db)
                .await?;
            if !desired_teams.is_empty() {
                let rows: Vec<issue_team::ActiveModel> = desired_teams
                    .into_iter()
                    .map(|t| issue_team::ActiveModel {
                        issue_id: Set(issue_id.to_string()),
                        team: Set(t),
                        // See issue_character.rs above: M4 Apply jobs
                        // populate team_id + flags; scanner writes
                        // NULL/false because ComicInfo has no signal.
                        team_id: Set(None),
                        is_first_appearance: Set(false),
                        disbanded_in_issue: Set(false),
                    })
                    .collect();
                issue_team::Entity::insert_many(rows)
                    .on_conflict(
                        OnConflict::columns([
                            issue_team::Column::IssueId,
                            issue_team::Column::Team,
                        ])
                        .do_nothing()
                        .to_owned(),
                    )
                    .try_insert()
                    .exec(db)
                    .await?;
            }
        }
    }

    // ───── locations ─────
    if !skip.contains(&F::Locations) {
        let desired_locations: Vec<String> = inputs.locations.map(split_csv).unwrap_or_default();
        let existing_locations: HashSet<String> = issue_location::Entity::find()
            .filter(issue_location::Column::IssueId.eq(issue_id))
            .all(db)
            .await?
            .into_iter()
            .map(|r| r.location)
            .collect();
        let desired_locations_set: HashSet<String> = desired_locations.iter().cloned().collect();
        if desired_locations_set != existing_locations {
            issue_location::Entity::delete_many()
                .filter(issue_location::Column::IssueId.eq(issue_id))
                .exec(db)
                .await?;
            if !desired_locations.is_empty() {
                let rows: Vec<issue_location::ActiveModel> = desired_locations
                    .into_iter()
                    .map(|l| issue_location::ActiveModel {
                        issue_id: Set(issue_id.to_string()),
                        location: Set(l),
                        // See issue_character.rs above: M4 Apply jobs
                        // populate location_id + flag.
                        location_id: Set(None),
                        is_first_appearance: Set(false),
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
                    .try_insert()
                    .exec(db)
                    .await?;
            }
        }
    }

    Ok(())
}

/// Recompute `series_genres / series_tags / series_credits` as the distinct
/// union of the series's active (non-removed) issues' junctions.
///
/// Idempotent. Best-effort: the rollup is cosmetic for the GET /series/{slug}
/// view (which reads from the issue-level junctions) and a denormalization
/// for filter views (which read from `series_*` to avoid joining through
/// every issue). A failure here doesn't fail the scan — log and continue.
pub async fn rollup_series_metadata<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
) -> Result<(), sea_orm::DbErr> {
    series_genre::Entity::delete_many()
        .filter(series_genre::Column::SeriesId.eq(series_id))
        .exec(db)
        .await?;
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        r"INSERT INTO series_genres (series_id, genre)
            SELECT DISTINCT $1, ig.genre
            FROM issue_genres ig
            JOIN issues i ON i.id = ig.issue_id
            WHERE i.series_id = $1 AND i.state = 'active' AND i.removed_at IS NULL
            ON CONFLICT DO NOTHING",
        [series_id.into()],
    ))
    .await?;

    series_tag::Entity::delete_many()
        .filter(series_tag::Column::SeriesId.eq(series_id))
        .exec(db)
        .await?;
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        r"INSERT INTO series_tags (series_id, tag)
            SELECT DISTINCT $1, it.tag
            FROM issue_tags it
            JOIN issues i ON i.id = it.issue_id
            WHERE i.series_id = $1 AND i.state = 'active' AND i.removed_at IS NULL
            ON CONFLICT DO NOTHING",
        [series_id.into()],
    ))
    .await?;

    // Person upsert: ensure a `person` row exists for every distinct
    // creator name in this series's credits, then propagate
    // `person_id` onto the `issue_credits` rows so the about-to-rebuild
    // `series_credits` carries the FK along. The cost is bounded by
    // distinct-creators-in-this-series (typically <50); freshly-scanned
    // issues are the only path that introduces unknown names, so most
    // calls do zero inserts and just refresh the issue_credits join.
    ensure_persons_for_series(db, series_id).await?;
    // WP-5.5: same for the character / team / story-arc / publisher
    // names, so every chip on the series + issue pages has a slug for
    // its entity landing page. Entity rows only — no junction writes.
    crate::metadata::writers::ensure_series_entity_rows(db, series_id).await?;
    link_series_entity_ids(db, series_id).await?;
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        "UPDATE issue_credits ic \
         SET person_id = p.id \
         FROM person p, issues i \
         WHERE ic.issue_id = i.id \
           AND i.series_id = $1 \
           AND i.state = 'active' \
           AND i.removed_at IS NULL \
           AND ic.person_id IS DISTINCT FROM p.id \
           AND p.normalized_name = btrim(lower(ic.person))",
        [series_id.into()],
    ))
    .await?;

    series_credit::Entity::delete_many()
        .filter(series_credit::Column::SeriesId.eq(series_id))
        .exec(db)
        .await?;
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        r"INSERT INTO series_credits (series_id, role, person, person_id)
            SELECT DISTINCT $1, ic.role, ic.person, ic.person_id
            FROM issue_credits ic
            JOIN issues i ON i.id = ic.issue_id
            WHERE i.series_id = $1 AND i.state = 'active' AND i.removed_at IS NULL
            ON CONFLICT DO NOTHING",
        [series_id.into()],
    ))
    .await?;

    series_character::Entity::delete_many()
        .filter(series_character::Column::SeriesId.eq(series_id))
        .exec(db)
        .await?;
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        r#"INSERT INTO series_characters (series_id, "character", character_id)
            SELECT DISTINCT $1, ic."character", ic.character_id
            FROM issue_characters ic
            JOIN issues i ON i.id = ic.issue_id
            WHERE i.series_id = $1 AND i.state = 'active' AND i.removed_at IS NULL
            ON CONFLICT DO NOTHING"#,
        [series_id.into()],
    ))
    .await?;

    series_team::Entity::delete_many()
        .filter(series_team::Column::SeriesId.eq(series_id))
        .exec(db)
        .await?;
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        r"INSERT INTO series_teams (series_id, team, team_id)
            SELECT DISTINCT $1, it.team, it.team_id
            FROM issue_teams it
            JOIN issues i ON i.id = it.issue_id
            WHERE i.series_id = $1 AND i.state = 'active' AND i.removed_at IS NULL
            ON CONFLICT DO NOTHING",
        [series_id.into()],
    ))
    .await?;

    series_location::Entity::delete_many()
        .filter(series_location::Column::SeriesId.eq(series_id))
        .exec(db)
        .await?;
    db.execute_raw(Statement::from_sql_and_values(
        db.get_database_backend(),
        r#"INSERT INTO series_locations (series_id, "location")
            SELECT DISTINCT $1, il."location"
            FROM issue_locations il
            JOIN issues i ON i.id = il.issue_id
            WHERE i.series_id = $1 AND i.state = 'active' AND i.removed_at IS NULL
            ON CONFLICT DO NOTHING"#,
        [series_id.into()],
    ))
    .await?;

    auto_set_reading_direction(db, series_id).await?;

    Ok(())
}

/// `manga-and-bulk-metadata-1.0` M3 — when ≥80% of a series's active
/// issues carry `manga IN ('Yes', 'YesAndRightToLeft')` AND the
/// series row currently has no override, pin `reading_direction =
/// "rtl"`. Sticky: never overwrites an admin-set value (we only
/// touch rows where the column is currently NULL).
///
/// Threshold is intentionally generous (80%, not 100%) — series with
/// occasional non-manga inserts (Free Comic Book Day specials,
/// localized covers without the Manga flag) still get auto-flipped.
/// One pure-manga issue isn't enough; the ≥3-issue minimum prevents
/// tiny series from flipping on a single mis-tagged file.
async fn auto_set_reading_direction<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
) -> Result<(), sea_orm::DbErr> {
    use entity::{issue, series};
    use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};

    // Skip if the series already has an override — admin / user
    // values are sticky and never overwritten.
    let row = match series::Entity::find_by_id(series_id).one(db).await? {
        Some(r) if r.reading_direction.is_none() => r,
        _ => return Ok(()),
    };

    let total = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(series_id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .count(db)
        .await?;
    if total < 3 {
        return Ok(());
    }

    let manga_count = issue::Entity::find()
        .filter(issue::Column::SeriesId.eq(series_id))
        .filter(issue::Column::State.eq("active"))
        .filter(issue::Column::RemovedAt.is_null())
        .filter(issue::Column::Manga.is_in(["Yes", "YesAndRightToLeft"]))
        .count(db)
        .await?;
    // 80% threshold; integer math to avoid float drift on small totals.
    if manga_count * 5 < total * 4 {
        return Ok(());
    }

    let mut am: series::ActiveModel = row.into();
    am.reading_direction = Set(Some("rtl".to_owned()));
    am.updated_at = Set(chrono::Utc::now().fixed_offset());
    am.update(db).await?;
    tracing::info!(
        series_id = %series_id,
        manga_count,
        total,
        "scanner heuristic: auto-set series.reading_direction = rtl",
    );
    Ok(())
}

/// Ensure a `person` row exists for every distinct creator name in
/// this series's issue credits. New names get a slug allocated via
/// [`allocate_person_slug`] (same scheme the M8 backfill migration
/// used) and inserted with `ON CONFLICT (normalized_name) DO NOTHING`
/// so concurrent rollups racing on the same name don't error.
///
/// Cheap when every name is already present (one SELECT, zero
/// INSERTs); pays the slug-allocation cost only for fresh creators.
async fn ensure_persons_for_series<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
) -> Result<(), sea_orm::DbErr> {
    #[derive(FromQueryResult)]
    struct NameRow {
        person: String,
    }
    // Distinct names in this series's active-issue credits that
    // aren't yet represented in `person`. Normalisation matches what
    // `m20261223_000001_person` uses (btrim + lower).
    let rows = NameRow::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        "SELECT DISTINCT ic.person AS person \
         FROM issue_credits ic \
         JOIN issues i ON i.id = ic.issue_id \
         WHERE i.series_id = $1 \
           AND i.state = 'active' \
           AND i.removed_at IS NULL \
           AND ic.person IS NOT NULL \
           AND ic.person <> '' \
           AND NOT EXISTS ( \
               SELECT 1 FROM person p \
               WHERE p.normalized_name = btrim(lower(ic.person)) \
           )",
        [Value::Uuid(Some(series_id))],
    ))
    .all(db)
    .await?;

    // Dedupe by normalized form before allocating — protects against
    // two issues in the same series spelling the same creator
    // slightly differently (whitespace / case). The first variant
    // wins as the display_name; both will resolve to the same person
    // via the JOIN above.
    let mut seen = HashSet::<String>::new();
    for row in rows {
        let display_name = row.person.trim().to_owned();
        if display_name.is_empty() {
            continue;
        }
        let normalized = display_name.to_lowercase();
        if !seen.insert(normalized.clone()) {
            continue;
        }
        let slug = allocate_person_slug(db, &display_name).await?;
        db.execute_raw(Statement::from_sql_and_values(
            db.get_database_backend(),
            "INSERT INTO person (slug, name, normalized_name) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (normalized_name) DO NOTHING",
            [slug.into(), display_name.into(), normalized.into()],
        ))
        .await?;
    }
    Ok(())
}

/// Protection predicate on `issues i`: true when the issue's story arcs
/// are *not* owned by a provider apply or a user edit (no
/// `field_provenance` row for `story_arcs` outside the file tiers). Same
/// rule as `scanner::process`'s `protected()` for the other junctions.
fn arcs_file_owned() -> String {
    let tiers = crate::metadata::writers::FILE_SOURCED_SET_BY
        .iter()
        .map(|t| format!("'{t}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "NOT EXISTS (SELECT 1 FROM field_provenance fp \
          WHERE fp.entity_type = 'issue' AND fp.entity_id = i.id \
            AND fp.field = 'story_arcs' AND fp.set_by NOT IN ({tiers}))"
    )
}

/// WP-5.5: link this series' scanner-minted rows to their entity rows,
/// mirroring the `issue_credits.person_id` fill above. Runs after
/// [`crate::metadata::writers::ensure_series_entity_rows`] so every name
/// has a row to link to.
///
/// - `issue_characters.character_id` / `issue_teams.team_id` and
///   `series.publisher_id` are filled **only while NULL**. Provider
///   applies write these FKs by identifier match, where the provider's
///   entity can legitimately carry a different normalized name than the
///   junction text — a name match must never re-point those. (The
///   junction's name column is part of its PK, so a renamed value is a
///   new row with a NULL FK and gets linked here.)
/// - Story arcs live only in the `issues.story_arc` CSV for scanned
///   files; their link is an `issue_arcs` row. For issues whose arcs are
///   file-owned ([`ARCS_FILE_OWNED`]) `issue_arcs` is reconciled to the
///   CSV: missing rows inserted (position from a numeric
///   `story_arc_number` when the issue names a single arc), rows for arcs
///   no longer in the CSV removed. Provider- / user-owned issues are
///   untouched (their `issue_arcs` are the source the CSV was built from).
async fn link_series_entity_ids<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
) -> Result<(), sea_orm::DbErr> {
    let backend = db.get_database_backend();
    let owned = arcs_file_owned();
    let active = "i.series_id = $1 AND i.state = 'active' AND i.removed_at IS NULL";
    let arc_split = "regexp_split_to_table(i.story_arc, \
         CASE WHEN i.story_arc LIKE '%;%' THEN ';' ELSE ',' END)";
    let statements = [
        format!(
            "UPDATE issue_characters j SET character_id = e.id \
               FROM character e, issues i \
              WHERE j.issue_id = i.id AND {active} AND j.character_id IS NULL \
                AND e.normalized_name = btrim(lower(j.character))"
        ),
        format!(
            "UPDATE issue_teams j SET team_id = e.id \
               FROM team e, issues i \
              WHERE j.issue_id = i.id AND {active} AND j.team_id IS NULL \
                AND e.normalized_name = btrim(lower(j.team))"
        ),
        "UPDATE series s SET publisher_id = e.id \
           FROM publisher e \
          WHERE s.id = $1 AND s.publisher_id IS NULL \
            AND e.normalized_name = btrim(lower(s.publisher))"
            .to_owned(),
        format!(
            "INSERT INTO issue_arcs (issue_id, arc_id, position_in_arc) \
             SELECT DISTINCT ON (i.id, e.id) i.id, e.id, \
                    CASE WHEN i.story_arc NOT LIKE '%;%' AND i.story_arc NOT LIKE '%,%' \
                          AND i.story_arc_number ~ '^\\s*[0-9]{{1,9}}\\s*$' \
                         THEN btrim(i.story_arc_number)::int4 END \
               FROM issues i \
               CROSS JOIN LATERAL {arc_split} AS x(nm) \
               JOIN story_arc e ON e.normalized_name = btrim(lower(x.nm)) \
              WHERE {active} AND i.story_arc IS NOT NULL AND i.story_arc <> '' \
                AND {owned} \
             ON CONFLICT DO NOTHING"
        ),
        format!(
            "DELETE FROM issue_arcs ia USING issues i, story_arc e \
              WHERE ia.issue_id = i.id AND e.id = ia.arc_id AND {active} \
                AND {owned} \
                AND NOT EXISTS ( \
                    SELECT 1 FROM {arc_split} AS x(nm) \
                     WHERE btrim(lower(x.nm)) = e.normalized_name)"
        ),
    ];
    for sql in statements {
        db.execute_raw(Statement::from_sql_and_values(
            backend,
            sql,
            [series_id.into()],
        ))
        .await?;
    }
    Ok(())
}

/// Best-effort wrapper used by scan callers. Logs and swallows errors so a
/// stale rollup never fails a scan run; the next series scan retries.
pub async fn rollup_series_metadata_best_effort<C: ConnectionTrait>(db: &C, series_id: Uuid) {
    if let Err(e) = rollup_series_metadata(db, series_id).await {
        tracing::warn!(
            series_id = %series_id,
            error = %e,
            "metadata_rollup: series rollup failed; will retry on next scan",
        );
    }
}

/// Helper used by scanners that just upserted an issue: pull the current
/// row's resolved CSVs and write the junctions. Centralizing this here keeps
/// the per-issue path on `process::ingest_one_with_fingerprint` short.
///
/// **Prefer [`replace_issue_metadata_from_model`]** when the caller already
/// has the model in hand (the scanner fast path always does — it just
/// inserted/updated the row). This variant is kept for callers that only
/// have an issue id; it adds a `find_by_id` round-trip per call.
pub async fn replace_issue_metadata_from_row<C: ConnectionTrait>(
    db: &C,
    issue_id: &str,
) -> Result<(), sea_orm::DbErr> {
    // We re-read the row inside the txn so we always reflect the values that
    // just got written (post user-edited stickiness). Cheap PK lookup.
    let row = issue::Entity::find_by_id(issue_id.to_owned())
        .one(db)
        .await?;
    let Some(row) = row else {
        return Ok(());
    };
    replace_issue_metadata_from_model(db, &row).await
}

/// Same as [`replace_issue_metadata_from_row`], but skips the `find_by_id`
/// round-trip. Use when the caller already has the just-written
/// `issue::Model` in hand. Saves ~1 SELECT per scanned archive — see
/// `docs/dev/scanner-perf.md` finding F-1.
pub async fn replace_issue_metadata_from_model<C: ConnectionTrait>(
    db: &C,
    row: &issue::Model,
) -> Result<(), sea_orm::DbErr> {
    replace_issue_metadata_from_model_skipping(db, row, &std::collections::HashSet::new()).await
}

/// [`replace_issue_metadata_from_model`] with the WP-2.5 junction skips.
pub async fn replace_issue_metadata_from_model_skipping<C: ConnectionTrait>(
    db: &C,
    row: &issue::Model,
    skip: &std::collections::HashSet<crate::metadata::MetadataField>,
) -> Result<(), sea_orm::DbErr> {
    let inputs = IssueMetadataInputs {
        genre: row.genre.as_deref(),
        tags: row.tags.as_deref(),
        writer: row.writer.as_deref(),
        penciller: row.penciller.as_deref(),
        inker: row.inker.as_deref(),
        colorist: row.colorist.as_deref(),
        letterer: row.letterer.as_deref(),
        cover_artist: row.cover_artist.as_deref(),
        editor: row.editor.as_deref(),
        translator: row.translator.as_deref(),
        characters: row.characters.as_deref(),
        teams: row.teams.as_deref(),
        locations: row.locations.as_deref(),
    };
    replace_issue_metadata_skipping(db, &row.id, &inputs, skip).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_csv_trims_dedupes_and_keeps_first_casing() {
        assert_eq!(
            split_csv("Action, Adventure, sci-fi"),
            vec!["Action", "Adventure", "sci-fi"],
        );
        assert_eq!(
            split_csv("Brian K. Vaughan, brian k. vaughan"),
            vec!["Brian K. Vaughan"],
        );
        assert!(split_csv("").is_empty());
        assert!(split_csv(" ,  ,").is_empty());
    }

    #[test]
    fn split_csv_uses_semicolon_alone_when_present_so_names_can_contain_commas() {
        // `"Capes, Inc."` → comma is part of the name; the joiner that
        // produced this string used `; ` precisely so we'd recover the
        // boundaries here.
        assert_eq!(
            split_csv("Capes, Inc.; Comet Twins"),
            vec!["Capes, Inc.", "Comet Twins"],
        );
        // Same: a single comma-containing name with no separator at all.
        assert_eq!(split_csv("Capes, Inc."), vec!["Capes", "Inc."]);
    }

    #[test]
    fn credit_csv_dispatches_per_role() {
        let inputs = IssueMetadataInputs {
            writer: Some("Alice"),
            penciller: Some("Bob"),
            inker: Some("Carol"),
            colorist: Some("Dan"),
            letterer: Some("Eve"),
            cover_artist: Some("Frank"),
            editor: Some("Grace"),
            translator: Some("Heidi"),
            ..Default::default()
        };
        assert_eq!(inputs.credit_csv(CreditRole::Writer), Some("Alice"));
        assert_eq!(inputs.credit_csv(CreditRole::Penciller), Some("Bob"));
        assert_eq!(inputs.credit_csv(CreditRole::Inker), Some("Carol"));
        assert_eq!(inputs.credit_csv(CreditRole::Colorist), Some("Dan"));
        assert_eq!(inputs.credit_csv(CreditRole::Letterer), Some("Eve"));
        assert_eq!(inputs.credit_csv(CreditRole::CoverArtist), Some("Frank"));
        assert_eq!(inputs.credit_csv(CreditRole::Editor), Some("Grace"));
        assert_eq!(inputs.credit_csv(CreditRole::Translator), Some("Heidi"));
    }
}
