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
    issue, issue_credit, issue_genre, issue_tag, series_credit, series_genre, series_tag,
};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Statement,
    sea_query::OnConflict,
};
use uuid::Uuid;

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

/// Split a CSV-shaped ComicInfo field on `,` and `;`, trim each piece, and
/// dedupe case-insensitively while keeping the first casing seen. Empty
/// pieces are dropped. Used by both the per-issue write and any read-side
/// callers that want to surface the raw split.
pub fn split_csv(value: &str) -> Vec<String> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for piece in value.split([',', ';']) {
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
    use std::collections::HashSet;

    // ───── genres ─────
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
                    OnConflict::columns([issue_genre::Column::IssueId, issue_genre::Column::Genre])
                        .do_nothing()
                        .to_owned(),
                )
                .do_nothing()
                .exec(db)
                .await?;
        }
    }

    // ───── tags ─────
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
                .do_nothing()
                .exec(db)
                .await?;
        }
    }

    // ───── credits ─────
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
    let desired_credits_set: HashSet<(String, String)> = desired_credits.iter().cloned().collect();
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
    db.execute(Statement::from_sql_and_values(
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
    db.execute(Statement::from_sql_and_values(
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

    series_credit::Entity::delete_many()
        .filter(series_credit::Column::SeriesId.eq(series_id))
        .exec(db)
        .await?;
    db.execute(Statement::from_sql_and_values(
        db.get_database_backend(),
        r"INSERT INTO series_credits (series_id, role, person)
            SELECT DISTINCT $1, ic.role, ic.person
            FROM issue_credits ic
            JOIN issues i ON i.id = ic.issue_id
            WHERE i.series_id = $1 AND i.state = 'active' AND i.removed_at IS NULL
            ON CONFLICT DO NOTHING",
        [series_id.into()],
    ))
    .await?;

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
    };
    replace_issue_metadata(db, &row.id, &inputs).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_csv_trims_dedupes_and_keeps_first_casing() {
        assert_eq!(
            split_csv("Action, Adventure ; sci-fi"),
            vec!["Action", "Adventure", "sci-fi"],
        );
        assert_eq!(
            split_csv("Brian K. Vaughan, brian k. vaughan"),
            vec!["Brian K. Vaughan"],
        );
        assert!(split_csv("").is_empty());
        assert!(split_csv(" , ;  ;").is_empty());
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
