use sea_orm::Iterable;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

// Storage contract (AR-7): `saved_views.kind` stays `TEXT`, pinned by
// `saved_views_kind_chk` to exactly these strings; `string_value` and the
// serde name are the same string (tests below). An unknown stored string
// fails to load with a `DbErr`.

/// What a saved view is: a filter over series or issues, a CBL reading
/// list, a built-in system rail, or a user collection.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    EnumIter,
    DeriveActiveEnum,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
#[serde(rename_all = "snake_case")]
pub enum SavedViewKind {
    /// Inline filter DSL over series (M3).
    #[sea_orm(string_value = "filter_series")]
    FilterSeries,
    /// The same DSL compiled over issues (WP-5.4).
    #[sea_orm(string_value = "filter_issues")]
    FilterIssues,
    /// Wrapper around a `cbl_lists` row (M4).
    #[sea_orm(string_value = "cbl")]
    Cbl,
    /// Global built-in rail; `system_key` set, `user_id IS NULL`.
    #[sea_orm(string_value = "system")]
    System,
    /// User-owned manual list backed by `collection_entries`.
    #[sea_orm(string_value = "collection")]
    Collection,
}

impl SavedViewKind {
    /// The stored / wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FilterSeries => "filter_series",
            Self::FilterIssues => "filter_issues",
            Self::Cbl => "cbl",
            Self::System => "system",
            Self::Collection => "collection",
        }
    }

    /// The variant whose stored string is `s`, if any.
    pub fn parse(s: &str) -> Option<Self> {
        Self::iter().find(|v| v.as_str() == s)
    }
}

/// Polymorphic saved view (saved-views M3). One row per view; `kind`
/// discriminates between:
///
///   - `'filter_series'` — inline filter DSL in `conditions`, over series,
///   - `'filter_issues'` — the same DSL compiled over issues (WP-5.4),
///   - `'cbl'` — pointer to a `cbl_lists` row via `cbl_list_id`,
///   - `'system'` — global built-in rail (`system_key` populated, `user_id IS NULL`),
///   - `'collection'` — user-owned ordered list of mixed series + issue
///     refs, backed by `collection_entries`. The Want to Read list is a
///     per-user collection with `system_key = 'want_to_read'`.
///
/// A CHECK constraint enforces per-kind column shape.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "saved_views")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// `None` for system views (admin-curated, visible to every user).
    #[sea_orm(nullable)]
    pub user_id: Option<Uuid>,
    /// `'filter_series'` (M3), `'filter_issues'` (WP-5.4), `'cbl'` (M4), `'system'` (built-in rail),
    /// or `'collection'` (user-owned manual list). See [`SavedViewKind`].
    pub kind: SavedViewKind,
    /// Identifies the built-in rail when `kind = 'system'`
    /// (`'continue_reading'`, `'on_deck'`) or the per-user system
    /// collection when `kind = 'collection'` (`'want_to_read'`). NULL
    /// for user-authored filter views and normal user collections.
    #[sea_orm(nullable)]
    pub system_key: Option<String>,
    pub name: String,
    #[sea_orm(nullable)]
    pub description: Option<String>,
    /// Optional metadata overlay — surfaced in the UI alongside the view's
    /// own filter year clauses for organization. v1 is display-only.
    #[sea_orm(nullable)]
    pub custom_year_start: Option<i32>,
    #[sea_orm(nullable)]
    pub custom_year_end: Option<i32>,
    /// User-defined tags (chips in the UI). Stored as Postgres `text[]`.
    pub custom_tags: Vec<String>,
    // ───── filter_series fields (NULL for kind='cbl') ─────
    /// `'all'` or `'any'`.
    #[sea_orm(nullable)]
    pub match_mode: Option<String>,
    /// JSON array of `{group_id, field, op, value}` objects. See
    /// `server::views::dsl::FilterDsl`.
    #[sea_orm(nullable)]
    pub conditions: Option<Json>,
    #[sea_orm(nullable)]
    pub sort_field: Option<String>,
    /// `'asc'` or `'desc'`.
    #[sea_orm(nullable)]
    pub sort_order: Option<String>,
    /// 1..200, view-scoped page size.
    #[sea_orm(nullable)]
    pub result_limit: Option<i32>,
    // ───── cbl fields (NULL for kind='filter_series') ─────
    /// FK to `cbl_lists(id)` once M4 ships. Schema column lands here so the
    /// discriminator is settled in one migration.
    #[sea_orm(nullable)]
    pub cbl_list_id: Option<Uuid>,
    /// M9: marks system views that should auto-pin to a fresh user's home
    /// rail on first touch. Always `false` for user-owned views.
    pub auto_pin: bool,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    /// When true, the matching OPDS collection feed
    /// (`/opds/v1/collections/{id}` + v2) renders in strict
    /// position order regardless of the caller's progress. Default
    /// false — feeds auto-reorder the up-next issue to position 0.
    /// Only meaningful for kind='collection' rows. M2 of
    /// `opds-sync-cleanup-1.0`.
    pub preserve_canonical_order: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id",
        on_delete = "Cascade"
    )]
    User,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enum_contract::assert_exact_strings;

    #[test]
    fn kind_round_trips_exact_db_strings() {
        assert_exact_strings(
            &[
                (SavedViewKind::FilterSeries, "filter_series"),
                (SavedViewKind::FilterIssues, "filter_issues"),
                (SavedViewKind::Cbl, "cbl"),
                (SavedViewKind::System, "system"),
                (SavedViewKind::Collection, "collection"),
            ],
            SavedViewKind::as_str,
            SavedViewKind::parse,
            &["filter", "CBL", "page", ""],
        );
    }
}
