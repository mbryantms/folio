use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One marker — bookmark, note, favorite, or highlight — anchored on a
/// `(user, issue, page)` triple. Region is normalized to 0–100% of the
/// page's natural pixel dims so the reader overlay survives resize and
/// fit-mode changes without recomputation. Per-kind invariants
/// (`body` required for notes, `region` required for highlights) are
/// enforced by the `markers_*_chk` CHECK constraints in the schema.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "markers")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub user_id: Uuid,
    pub series_id: Uuid,
    /// `issues.id` is BLAKE3 hex (TEXT), matching the rest of the
    /// schema.
    pub issue_id: String,
    pub page_index: i32,
    /// `'bookmark' | 'note' | 'highlight'`. Favorite is no longer a
    /// kind — see `is_favorite` flag below.
    pub kind: String,
    /// Star flag — any marker (any kind) can be favorited. Replaces the
    /// pre-2026-05-12 `kind='favorite'` arrangement.
    pub is_favorite: bool,
    /// Per-user freeform tag list. Empty array (not NULL) when unset,
    /// so `tags @> ARRAY[...]` filter queries don't need null guards.
    pub tags: Vec<String>,
    /// `{ x, y, w, h, shape }` — rect dims as 0–100 percent floats
    /// against the page's natural pixel dims; `shape ∈ rect | text |
    /// image`. NULL for whole-page markers.
    #[sea_orm(nullable)]
    pub region: Option<Json>,
    /// `{ text?, image_hash?, ocr_confidence? }` — populated by M5's
    /// client-side OCR / SHA-256 hashing for `shape='text'|'image'`
    /// highlights.
    #[sea_orm(nullable)]
    pub selection: Option<Json>,
    /// Markdown body. Required when `kind='note'`. Capped at 10 KB by
    /// the `markers_body_size_chk` constraint.
    #[sea_orm(nullable)]
    pub body: Option<String>,
    /// Palette token (e.g. `yellow | green | blue | red | violet`).
    /// Free-form text — the client maps it to its own swatch registry.
    #[sea_orm(nullable)]
    pub color: Option<String>,
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
    #[sea_orm(
        belongs_to = "super::series::Entity",
        from = "Column::SeriesId",
        to = "super::series::Column::Id",
        on_delete = "Cascade"
    )]
    Series,
    #[sea_orm(
        belongs_to = "super::issue::Entity",
        from = "Column::IssueId",
        to = "super::issue::Column::Id",
        on_delete = "Cascade"
    )]
    Issue,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}
impl Related<super::series::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Series.def()
    }
}
impl Related<super::issue::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Issue.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
