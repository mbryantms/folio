use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "series")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub library_id: Uuid,
    pub name: String,
    pub normalized_name: String,
    /// URL-safe identifier, globally unique across all series. Allocated at
    /// scanner-insert time via `crate::slug::allocate_slug` with the
    /// series' year/volume/publisher as fallback disambiguators. Distinct
    /// from `normalized_name`, which is for dedupe/CBL matching.
    pub slug: String,
    #[sea_orm(nullable)]
    pub year: Option<i32>,
    #[sea_orm(nullable)]
    pub volume: Option<i32>,
    #[sea_orm(nullable)]
    pub publisher: Option<String>,
    #[sea_orm(nullable)]
    pub imprint: Option<String>,
    pub status: String,
    #[sea_orm(nullable)]
    pub total_issues: Option<i32>,
    #[sea_orm(nullable)]
    pub age_rating: Option<String>,
    #[sea_orm(nullable)]
    pub summary: Option<String>,
    pub language_code: String,
    #[sea_orm(nullable)]
    pub comicvine_id: Option<i64>,
    #[sea_orm(nullable)]
    pub metron_id: Option<i64>,
    #[sea_orm(nullable)]
    pub gtin: Option<String>,
    #[sea_orm(nullable)]
    pub series_group: Option<String>,
    pub alternate_names: Json,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    /// Disk path of the series folder. Used as the fast-path identity match
    /// (spec §7.1.1). Nullable for legacy rows that predate scanner v1; the
    /// next scan backfills it.
    #[sea_orm(nullable)]
    pub folder_path: Option<String>,
    /// Last successful scan of *this series* (spec §4.4). Used to skip
    /// folders whose recursive mtime hasn't moved.
    #[sea_orm(nullable)]
    pub last_scanned_at: Option<DateTimeWithTimeZone>,
    /// Admin-set sticky match key (spec §7.4). When non-null, identity
    /// resolution returns `ExistingByMatchKey` and never overwrites.
    #[sea_orm(nullable)]
    pub match_key: Option<String>,
    /// Set by reconciliation when all issues in this series are removed
    /// (spec §4.7).
    #[sea_orm(nullable)]
    pub removed_at: Option<DateTimeWithTimeZone>,
    /// Set by the reconcile sweep job after `library.soft_delete_days`
    /// have elapsed since `removed_at` (spec §4.7).
    #[sea_orm(nullable)]
    pub removal_confirmed_at: Option<DateTimeWithTimeZone>,
    /// Sticky-override timestamp for `status`. Set by `PATCH /series/{slug}`
    /// whenever the body includes a `status` field. The post-scan
    /// `reconcile_series_status` helper checks this and skips the
    /// status write when non-null, so user-set values like `"hiatus"`
    /// or `"cancelled"` survive re-scans. The `total_issues` refresh
    /// is independent — manual override only freezes the status, not
    /// the count, so the Complete/Incomplete UI stays accurate.
    #[sea_orm(nullable)]
    pub status_user_set_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::library::Entity",
        from = "Column::LibraryId",
        to = "super::library::Column::Id"
    )]
    Library,
    #[sea_orm(has_many = "super::issue::Entity")]
    Issue,
}

impl Related<super::library::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Library.def()
    }
}
impl Related<super::issue::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Issue.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

/// Punctuation-stripped, lowercased, whitespace-collapsed name used for
/// dedupe and CBL matching (§5.8). Stable across rescans.
pub fn normalize_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_space = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            for c in ch.to_lowercase() {
                out.push(c);
            }
            last_was_space = false;
        } else if (ch.is_whitespace() || ch == '-' || ch == '_' || ch == '.')
            && !last_was_space
            && !out.is_empty()
        {
            out.push(' ');
            last_was_space = true;
        }
        // Other punctuation is stripped.
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_punct_and_lowercases() {
        assert_eq!(normalize_name("Spider-Man (2018)"), "spider man 2018");
        assert_eq!(normalize_name("X-Men: Blue"), "x men blue");
        assert_eq!(normalize_name("  Saga  "), "saga");
        assert_eq!(normalize_name("Pokémon Adventures"), "pokémon adventures");
    }
}
