//! Provider ids for creators (`external_ids`, `entity_type = 'person'`),
//! roadmap WP-9.8.
//!
//! A provider credit carries the provider's id for the creator. The
//! DB-direct apply links it (`writers::upsert_person` with the credit's
//! identifiers); on a writeback library the id travels through the file
//! instead: the sidecar composer writes it as MetronInfo's `<Creator id>`
//! (the **primary** source's id, as the schema defines the attribute), the
//! parser keeps it in `MetronInfo::credit_ids`, the scanner stores the
//! parsed file in `issues.metron_info_raw`, and
//! [`link_series_creator_ids_from_files`] links it once the series rollup
//! has minted the `person` rows.
//!
//! [`resolve_from_cache`] is the catch-up for creators credited before any
//! of that existed: it reads the provider issue details already in
//! `metadata_cache` and takes the id of the credit whose name matches. It
//! makes no provider request.
//!
//! Both paths only ever **add** a link. `person` is unique by normalized
//! name, so two real creators who share a name share a row; whichever id
//! arrives first stays, and neither path replaces an id a creator already
//! holds for that source (a profile fetch or an admin may).

use crate::library::scanner::metadata_rollup::split_csv;
use crate::metadata::identifier::{Identifier, Source};
use crate::metadata::writers::{self, SetBy, SetExternalIdOutcome};
use sea_orm::sea_query::{ArrayType, Value};
use sea_orm::{ConnectionTrait, DbErr, FromQueryResult, Statement};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// The sources a creator id is kept for: the providers Folio reads creator
/// records from. A file whose primary source is anything else (`Marvel`,
/// `League of Comic Geeks`, …) carries creator ids Folio has no use for.
pub const CREATOR_ID_SOURCES: [Source; 3] = [Source::Metron, Source::ComicVine, Source::Gcd];

fn is_creator_id_source(source: Source) -> bool {
    CREATOR_ID_SOURCES.contains(&source)
}

/// The name the scanner files a `<Creator>` under: [`split_csv`]'s
/// reading of it (a generational suffix re-attached, a tagger id
/// dropped), trimmed and lowercased like `person.normalized_name`. `None`
/// when the text does not read as exactly one name.
fn scanner_person_key(creator: &str) -> Option<String> {
    let pieces = split_csv(creator);
    match pieces.as_slice() {
        [one] => Some(one.trim().to_lowercase()).filter(|k| !k.is_empty()),
        _ => None,
    }
}

fn text_array(values: impl IntoIterator<Item = String>) -> Value {
    Value::Array(
        ArrayType::String,
        Some(Box::new(
            values.into_iter().map(|v| Value::String(Some(v))).collect(),
        )),
    )
}

/// `normalized_name → (person id, the sources it already holds an id for)`
/// for the given normalized names.
async fn persons_by_name<C: ConnectionTrait>(
    db: &C,
    names: HashSet<String>,
) -> Result<HashMap<String, (Uuid, HashSet<String>)>, DbErr> {
    #[derive(FromQueryResult)]
    struct Row {
        id: Uuid,
        normalized_name: String,
        source: Option<String>,
    }
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        "SELECT p.id AS id, p.normalized_name AS normalized_name, e.source AS source \
           FROM person p \
           LEFT JOIN external_ids e \
             ON e.entity_type = 'person' AND e.entity_id = p.id::text \
          WHERE p.normalized_name = ANY($1)",
        [text_array(names)],
    ))
    .all(db)
    .await?;
    let mut out: HashMap<String, (Uuid, HashSet<String>)> = HashMap::new();
    for r in rows {
        let entry = out
            .entry(r.normalized_name)
            .or_insert_with(|| (r.id, HashSet::new()));
        if let Some(s) = r.source {
            entry.1.insert(s);
        }
    }
    Ok(out)
}

/// Link the creator ids the series' MetronInfo files carry to the
/// `person` rows of the same name. A step of the series rollup, after the
/// persons are minted. Returns how many links were written.
///
/// In the settled state (every id in the files already belongs to a
/// creator) this is one statement that returns no rows. An id that cannot
/// be linked (its creator has no `person` row, or already holds another id
/// for that source) costs one more lookup per rollup.
pub async fn link_series_creator_ids_from_files<C: ConnectionTrait>(
    db: &C,
    series_id: Uuid,
) -> Result<u64, DbErr> {
    #[derive(FromQueryResult)]
    struct Row {
        source: String,
        name: String,
        external_id: String,
    }
    // Every `(primary source, creator name, id)` the active issues' files
    // carry whose id no creator holds yet, the id most files name first. `metron_info_raw` is the parsed
    // `MetronInfo` as the scanner serialized it, so `credit_ids` is an
    // object or absent; the CASE keeps a hand-edited row from failing the
    // set-returning call.
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        db.get_database_backend(),
        "SELECT i.metron_info_raw->>'primary_id_source' AS source, \
                kv.key AS name, btrim(kv.value) AS external_id \
           FROM issues i \
          CROSS JOIN LATERAL jsonb_each_text( \
                CASE WHEN jsonb_typeof(i.metron_info_raw->'credit_ids') = 'object' \
                     THEN i.metron_info_raw->'credit_ids' END) kv \
          WHERE i.series_id = $1 \
            AND i.state = 'active' \
            AND i.removed_at IS NULL \
            AND i.metron_info_raw->>'primary_id_source' = ANY($2) \
            AND btrim(kv.value) <> '' \
            AND NOT EXISTS ( \
                SELECT 1 FROM external_ids e \
                 WHERE e.entity_type = 'person' \
                   AND e.source = i.metron_info_raw->>'primary_id_source' \
                   AND e.external_id = btrim(kv.value)) \
          GROUP BY 1, 2, 3 \
          ORDER BY 1, 2, count(*) DESC, 3",
        [
            series_id.into(),
            text_array(CREATOR_ID_SOURCES.iter().map(|s| s.as_str().to_owned())),
        ],
    ))
    .all(db)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }

    let wanted: Vec<(Source, String, String)> = rows
        .into_iter()
        .filter_map(|r| {
            let source = r.source.parse::<Source>().ok()?;
            let key = scanner_person_key(&r.name)?;
            Some((source, key, r.external_id))
        })
        .collect();
    let mut persons =
        persons_by_name(db, wanted.iter().map(|(_, key, _)| key.clone()).collect()).await?;

    let mut linked = 0u64;
    for (source, key, external_id) in wanted {
        // No `person` row: the creator's only role is one ComicInfo has no
        // column for, so the scanner made no credit of it.
        let Some((person_id, held)) = persons.get_mut(&key) else {
            continue;
        };
        // First id wins (see the module docs); of two ids for one name in
        // this series, that is the one more files name.
        if !held.insert(source.as_str().to_owned()) {
            continue;
        }
        let identifier = Identifier::with_canonical_url(source, external_id, "person");
        match writers::set_external_id(
            db,
            "person",
            &person_id.to_string(),
            &identifier,
            SetBy::MetronInfo,
        )
        .await?
        {
            SetExternalIdOutcome::Set | SetExternalIdOutcome::Reclaimed { .. } => linked += 1,
            outcome => tracing::debug!(
                series_id = %series_id,
                person_id = %person_id,
                source = source.as_str(),
                ?outcome,
                "creator id from MetronInfo not linked"
            ),
        }
    }
    Ok(linked)
}

/// Result of one [`resolve_from_cache`] pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResolveOutcome {
    /// Links written.
    pub linked: u64,
    /// Ids that already belong to a different creator (an alias row, or a
    /// second spelling of the same name) and were left alone.
    pub conflicts: u64,
    /// Credited creators that still hold no Metron or ComicVine id.
    pub unlinked: u64,
}

/// The catch-up for creators credited before WP-9.8: for each creator with
/// no id from a provider, read the cached detail of the issues that
/// provider matched (`metadata_cache`, whatever its age: who drew an issue
/// does not expire) and take the id of the credit whose name is the
/// creator's. Where several cached issues disagree, the id most of them
/// name wins. No provider request is made, so a creator none of whose
/// issues is in the cache stays unlinked; the profile fetch resolves
/// those by name.
pub async fn resolve_from_cache<C: ConnectionTrait>(db: &C) -> Result<ResolveOutcome, DbErr> {
    #[derive(FromQueryResult)]
    struct Row {
        person_id: Uuid,
        source: String,
        external_id: String,
    }
    let backend = db.get_database_backend();
    // The cached payload is a serialized `GenericMetadata`; only its
    // `credits[].name` and `credits[].identifiers[]` are read, straight
    // from the JSON, so a row written under an older cache schema version
    // still counts (the credit shape has not changed since v0).
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        backend,
        "SELECT p.id AS person_id, mc.provider AS source, \
                btrim(ident->>'id') AS external_id \
           FROM metadata_cache mc \
           JOIN external_ids ie \
             ON ie.entity_type = 'issue' AND ie.source = mc.provider \
            AND ie.external_id = mc.external_id \
           JOIN issue_credits ic ON ic.issue_id = ie.entity_id \
           JOIN person p ON p.id = ic.person_id \
          CROSS JOIN LATERAL jsonb_array_elements( \
                CASE WHEN jsonb_typeof(mc.payload->'credits') = 'array' \
                     THEN mc.payload->'credits' END) c \
          CROSS JOIN LATERAL jsonb_array_elements( \
                CASE WHEN jsonb_typeof(c->'identifiers') = 'array' \
                     THEN c->'identifiers' END) ident \
          WHERE mc.entity = 'issue' \
            AND mc.provider = ANY($1) \
            AND btrim(lower(c->>'name')) = p.normalized_name \
            AND ident->>'source' = mc.provider \
            AND btrim(COALESCE(ident->>'id', '')) <> '' \
            AND NOT EXISTS ( \
                SELECT 1 FROM external_ids pe \
                 WHERE pe.entity_type = 'person' \
                   AND pe.entity_id = p.id::text \
                   AND pe.source = mc.provider) \
          GROUP BY 1, 2, 3 \
          ORDER BY p.id, mc.provider, count(DISTINCT mc.external_id) DESC, 3",
        [text_array(
            [Source::Metron, Source::ComicVine]
                .iter()
                .map(|s| s.as_str().to_owned()),
        )],
    ))
    .all(db)
    .await?;

    let mut outcome = ResolveOutcome::default();
    let mut seen: HashSet<(Uuid, String)> = HashSet::new();
    for r in rows {
        // Rows arrive best-supported id first per (creator, source).
        if !seen.insert((r.person_id, r.source.clone())) {
            continue;
        }
        let Ok(source) = r.source.parse::<Source>() else {
            continue;
        };
        if !is_creator_id_source(source) {
            continue;
        }
        let identifier = Identifier::with_canonical_url(source, r.external_id, "person");
        match writers::set_external_id(
            db,
            "person",
            &r.person_id.to_string(),
            &identifier,
            SetBy::Provider(source),
        )
        .await?
        {
            SetExternalIdOutcome::Set | SetExternalIdOutcome::Reclaimed { .. } => {
                outcome.linked += 1;
            }
            SetExternalIdOutcome::SkippedConflict { .. } => outcome.conflicts += 1,
            SetExternalIdOutcome::KeptUserValue { .. }
            | SetExternalIdOutcome::KeptProviderValue { .. } => {}
        }
    }

    #[derive(FromQueryResult)]
    struct CountRow {
        n: i64,
    }
    outcome.unlinked = CountRow::find_by_statement(Statement::from_string(
        backend,
        "SELECT count(*) AS n FROM person p \
          WHERE EXISTS (SELECT 1 FROM issue_credits ic WHERE ic.person_id = p.id) \
            AND NOT EXISTS ( \
                SELECT 1 FROM external_ids e \
                 WHERE e.entity_type = 'person' AND e.entity_id = p.id::text \
                   AND e.source IN ('metron', 'comicvine'))",
    ))
    .one(db)
    .await?
    .map_or(0, |r| r.n.max(0) as u64);
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::scanner_person_key;

    #[test]
    fn person_key_follows_the_scanners_reading_of_a_name() {
        assert_eq!(
            scanner_person_key("  Fiona Staples "),
            Some("fiona staples".into())
        );
        // The scanner re-attaches a comma'd generational suffix.
        assert_eq!(
            scanner_person_key("Mike Deodato, Jr."),
            Some("mike deodato jr.".into())
        );
        // …and drops a tagger's bracketed id.
        assert_eq!(
            scanner_person_key("John Doe [15487]"),
            Some("john doe".into())
        );
        // Two names in one `<Creator>` are not one creator.
        assert_eq!(scanner_person_key("Ann Writer, Bob Artist"), None);
        assert_eq!(scanner_person_key("   "), None);
    }
}
