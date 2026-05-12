//! Series identity resolution (spec §7).
//!
//! Library Scanner v1, Milestone 6 — focused MVP.
//!
//! Resolves a series-folder + sample ComicInfo to the canonical series row.
//! Resolution order matches §7.1, with manual override pinned first:
//!
//!   1. **`match_key` sticky override** — admin set this via `PATCH /series/{id}`,
//!      it's never overwritten by the scanner.
//!   2. **`folder_path`** — fast path; once a folder→series mapping exists we
//!      reuse it without re-running name normalization.
//!   3. **`normalized_name + year`** — fallback for folders we haven't seen,
//!      including renamed folders that bring their old ComicInfo Series with them.
//!   4. **None matched** → create a new series row stamped with `folder_path`.
//!
//! The full §7.1.2 LocalizedSeries match + mixed-series-in-folder warning
//! (§7.2) lands in Milestone 8 with the rest of the parser integration; the
//! current code is robust against the common Mylar/CBL layout.
//!
//! Move detection (§7.3): the per-file `ingest_one` already updates the issue
//! row's `series_id` when a content hash matches an existing row. Folder-rename
//! detection works implicitly: if a folder is renamed, no folder-path match
//! exists; we fall through to normalized-name+year and pick up the existing
//! series, then [`stamp_folder_path`] backfills `folder_path` so the next scan
//! takes the fast path.

use chrono::Utc;
use entity::series::{self, ActiveModel as SeriesAM, Entity as SeriesEntity, normalize_name};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use std::path::Path;
use uuid::Uuid;

/// What identity resolution produced. The caller cares about the final
/// `series_id` and whether we created the row (for stat tracking).
#[derive(Debug, Clone, Copy)]
pub enum SeriesMatch {
    ByMatchKey { id: Uuid },
    ByFolderPath { id: Uuid },
    ByNormalizedNameYear { id: Uuid },
    Created { id: Uuid },
}

impl SeriesMatch {
    pub fn id(self) -> Uuid {
        match self {
            Self::ByMatchKey { id }
            | Self::ByFolderPath { id }
            | Self::ByNormalizedNameYear { id }
            | Self::Created { id } => id,
        }
    }
    pub fn was_created(self) -> bool {
        matches!(self, Self::Created { .. })
    }
}

/// Sample ComicInfo + filename inference per folder. The caller assembles
/// this from the first archive in the folder (Milestone 8 will widen it to
/// "most common Series across the folder" per spec §7.2).
#[derive(Debug, Clone, Default)]
pub struct SeriesIdentityHint {
    pub series_name: String,
    pub year: Option<i32>,
    pub volume: Option<i32>,
    pub publisher: Option<String>,
    pub imprint: Option<String>,
    pub language: Option<String>,
    pub age_rating: Option<String>,
    pub series_group: Option<String>,
    pub total_issues: Option<i32>,
    /// External-database IDs for the series (volume on ComicVine). Set when
    /// ComicInfo carries `<ComicVineSeriesID>` / `<ComicVineVolumeID>` /
    /// `<MetronSeriesID>`, or when the `<Web>` URL points at a series page.
    /// Optional — populated as a hint, never required for matching.
    pub comicvine_id: Option<i64>,
    pub metron_id: Option<i64>,
    /// Optional: the spec's `match_key` candidate (e.g. ComicInfo's
    /// `Web` URL or a stable external id). If set, identity-by-match-key takes
    /// priority. Currently unused by the scanner — admins set match_key via
    /// the API.
    pub explicit_match_key: Option<String>,
}

/// Resolve or create the series for `folder`. Returns the canonical id and
/// the resolution path so the caller can update its stats.
pub async fn resolve_or_create(
    db: &sea_orm::DatabaseConnection,
    library_id: Uuid,
    folder: &Path,
    hint: &SeriesIdentityHint,
    default_language: &str,
) -> anyhow::Result<SeriesMatch> {
    let folder_str = folder.to_string_lossy().into_owned();

    // (1) Sticky admin override.
    if let Some(key) = &hint.explicit_match_key
        && let Some(row) = SeriesEntity::find()
            .filter(series::Column::LibraryId.eq(library_id))
            .filter(series::Column::MatchKey.eq(key.clone()))
            .one(db)
            .await?
    {
        // Backfill folder_path if it doesn't match — the user moved the
        // canonical series here.
        if row.folder_path.as_deref() != Some(folder_str.as_str()) {
            let id = row.id;
            let mut am: SeriesAM = row.into();
            am.folder_path = Set(Some(folder_str.clone()));
            am.update(db).await?;
            return Ok(SeriesMatch::ByMatchKey { id });
        }
        return Ok(SeriesMatch::ByMatchKey { id: row.id });
    }

    // (2) Folder-path fast path.
    if let Some(row) = SeriesEntity::find()
        .filter(series::Column::LibraryId.eq(library_id))
        .filter(series::Column::FolderPath.eq(folder_str.clone()))
        .one(db)
        .await?
    {
        let id = row.id;
        backfill_external_ids(db, row, hint).await?;
        return Ok(SeriesMatch::ByFolderPath { id });
    }

    // (3) Normalized name + year.
    let normalized = normalize_name(&hint.series_name);
    let mut q = SeriesEntity::find()
        .filter(series::Column::LibraryId.eq(library_id))
        .filter(series::Column::NormalizedName.eq(normalized.clone()));
    q = match hint.year {
        Some(y) => q.filter(series::Column::Year.eq(y)),
        None => q.filter(series::Column::Year.is_null()),
    };
    if let Some(row) = q.one(db).await? {
        // Backfill folder_path so future scans take the fast path; also
        // pick up any external IDs the previous scan didn't have.
        let id = row.id;
        let needs_folder_backfill = row.folder_path.as_deref() != Some(folder_str.as_str());
        let needs_id_backfill = (hint.comicvine_id.is_some() && row.comicvine_id.is_none())
            || (hint.metron_id.is_some() && row.metron_id.is_none());
        if needs_folder_backfill || needs_id_backfill {
            let mut am: SeriesAM = row.into();
            if needs_folder_backfill {
                am.folder_path = Set(Some(folder_str));
            }
            if hint.comicvine_id.is_some() {
                am.comicvine_id = Set(hint.comicvine_id);
            }
            if hint.metron_id.is_some() {
                am.metron_id = Set(hint.metron_id);
            }
            am.updated_at = Set(Utc::now().fixed_offset());
            am.update(db).await?;
        }
        return Ok(SeriesMatch::ByNormalizedNameYear { id });
    }

    // (4) Create.
    let id = Uuid::now_v7();
    let now = Utc::now().fixed_offset();
    let slug = crate::slug::allocate_series_slug(
        db,
        &hint.series_name,
        hint.year,
        hint.volume,
        hint.publisher.as_deref(),
    )
    .await?;
    let am = SeriesAM {
        id: Set(id),
        library_id: Set(library_id),
        name: Set(hint.series_name.clone()),
        normalized_name: Set(normalized),
        slug: Set(slug),
        year: Set(hint.year),
        volume: Set(hint.volume),
        publisher: Set(hint.publisher.clone()),
        imprint: Set(hint.imprint.clone()),
        status: Set("continuing".into()),
        total_issues: Set(hint.total_issues),
        age_rating: Set(hint.age_rating.clone()),
        summary: Set(None),
        language_code: Set(hint
            .language
            .clone()
            .unwrap_or_else(|| default_language.to_string())),
        comicvine_id: Set(hint.comicvine_id),
        metron_id: Set(hint.metron_id),
        gtin: Set(None),
        series_group: Set(hint.series_group.clone()),
        alternate_names: Set(serde_json::json!([])),
        created_at: Set(now),
        updated_at: Set(now),
        folder_path: Set(Some(folder_str)),
        last_scanned_at: Set(None),
        match_key: Set(None),
        removed_at: Set(None),
        removal_confirmed_at: Set(None),
        // Scanner-created series have no manual status override.
        // PATCH /series/{slug} stamps this when a user pins a status.
        status_user_set_at: Set(None),
    };
    am.insert(db).await?;
    Ok(SeriesMatch::Created { id })
}

/// Fill in ComicVine / Metron series IDs on an existing series row when the
/// scanned hint provides values that the row currently lacks. Never
/// overwrites set values — admin edits and prior scans win.
async fn backfill_external_ids(
    db: &sea_orm::DatabaseConnection,
    row: series::Model,
    hint: &SeriesIdentityHint,
) -> anyhow::Result<()> {
    let needs_cv = hint.comicvine_id.is_some() && row.comicvine_id.is_none();
    let needs_mt = hint.metron_id.is_some() && row.metron_id.is_none();
    if !needs_cv && !needs_mt {
        return Ok(());
    }
    let mut am: SeriesAM = row.into();
    if needs_cv {
        am.comicvine_id = Set(hint.comicvine_id);
    }
    if needs_mt {
        am.metron_id = Set(hint.metron_id);
    }
    am.updated_at = Set(Utc::now().fixed_offset());
    am.update(db).await?;
    Ok(())
}
