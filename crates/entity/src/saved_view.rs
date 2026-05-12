use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Polymorphic saved view (saved-views M3). One row per view; `kind`
/// discriminates between:
///
///   - `'filter_series'` — inline filter DSL in `conditions`,
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
    /// `'filter_series'` (M3), `'cbl'` (M4), `'system'` (built-in rail),
    /// or `'collection'` (user-owned manual list).
    pub kind: String,
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
